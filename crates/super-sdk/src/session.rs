//! [`Session`]: one embeddable conversation with the SUPER coding agent.
//!
//! The most important function here is [`Session::execute`]. It implements
//! every [`Command`] in the protocol, and every other method on `Session` is a
//! thin, typed wrapper around it. The RPC server and the Python and TypeScript
//! bindings call `execute` too, which is why the four surfaces cannot drift.

use crate::approvals::{
    ApprovalAnswer, ApprovalRequest, Approvals, approval_request_event, approval_resolved_event,
};
use crate::events::{
    agent_settled, announced_command, bash_execution_update, command_applied, event_lag,
    session_event_json,
};
use crate::options::{SessionOptions, UnknownModel};
use crate::protocol::{Command, Event, ImageInput, QueueMode, Response, StreamingBehavior};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use super_agent::{AgentMessage, BashExecutionMessage};
use super_ai::auth::super_platform;
use super_ai::{ContentBlock, Model, Registry, ThinkingLevel, UserContent, UserMessage};
use super_coding::session_runner::{AgentSession, SessionEvent};
use super_coding::settings::QueueMode as SettingsQueueMode;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// Anything that can go wrong in a typed SDK call.
#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    /// The agent refused the request. The message is the same text the RPC
    /// protocol would have put in `response.error`.
    #[error("{0}")]
    Command(String),
    /// Session startup failed.
    #[error(transparent)]
    Startup(#[from] anyhow::Error),
}

/// A subscriber to the session's event stream.
///
/// Events are buffered per subscriber. If you consume them more slowly than the
/// agent produces them the oldest are dropped and you receive a single
/// `event_lag` event naming how many you missed. Re-read state rather than
/// assume you saw everything.
pub struct EventStream {
    receiver: broadcast::Receiver<Event>,
}

impl EventStream {
    /// Wait for the next event. Returns `None` once the session is closed.
    pub async fn recv(&mut self) -> Option<Event> {
        match self.receiver.recv().await {
            Ok(event) => Some(event),
            // Report the gap rather than hide it. The next `recv` resumes.
            Err(broadcast::error::RecvError::Lagged(skipped)) => Some(Event(event_lag(skipped))),
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }

    /// Take the next event if one is already buffered.
    pub fn try_recv(&mut self) -> Option<Event> {
        match self.receiver.try_recv() {
            Ok(event) => Some(event),
            Err(broadcast::error::TryRecvError::Lagged(skipped)) => Some(Event(event_lag(skipped))),
            Err(_) => None,
        }
    }
}

/// Everything a prompt can carry.
#[derive(Debug, Clone, Default)]
pub struct PromptArgs {
    pub message: String,
    pub images: Vec<ImageInput>,
    /// Required when the agent is already streaming.
    pub streaming_behavior: Option<StreamingBehavior>,
}

impl PromptArgs {
    pub fn new(message: impl Into<String>) -> Self {
        PromptArgs {
            message: message.into(),
            ..Default::default()
        }
    }
}

/// The result of a direct shell command (the `bash` command, not a model tool
/// call).
#[derive(Debug, Clone, PartialEq)]
pub struct BashResult {
    pub output: String,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    pub full_output_path: Option<String>,
}

impl BashResult {
    fn to_json(&self) -> Value {
        json!({
            "output": self.output,
            "exitCode": self.exit_code,
            "cancelled": self.cancelled,
            "truncated": self.truncated,
            "fullOutputPath": self.full_output_path,
        })
    }
}

/// A convenient snapshot of the session for user interfaces.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub model: Option<Model>,
    pub thinking_level: ThinkingLevel,
    pub is_streaming: bool,
    pub session_file: Option<PathBuf>,
    pub session_id: String,
    pub session_name: Option<String>,
    pub message_count: usize,
    pub tools: Vec<String>,
}

/// Something a surface embedding the session wants to know about commands that
/// arrived through another surface, for example a remote controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionNotice {
    /// A prompt, steer or follow-up was accepted from `origin`.
    RemoteInput {
        origin: String,
        /// `prompt`, `steer` or `follow_up`.
        kind: &'static str,
        text: String,
    },
    /// Any other command ran from `origin`: `set_model`, `abort`, ...
    RemoteCommand {
        origin: String,
        command: &'static str,
        success: bool,
    },
    /// A command from `origin` ran, as `command_applied` announces it to
    /// every client: the command as sent (`imageCount` in place of images),
    /// whether it succeeded, and its response's data. A prompt is announced
    /// before its run starts.
    Applied {
        origin: String,
        command: Value,
        success: bool,
        data: Option<Value>,
    },
    /// An approval was answered, by the terminal (`local`) or a remote device.
    ApprovalAnswered {
        approval_id: String,
        choice: String,
        by: String,
    },
}

/// Receives [`SessionNotice`]s. Must not block.
pub type SessionObserver = Arc<dyn Fn(SessionNotice) + Send + Sync>;

/// Asked before `switch_session` opens a file; `Err` refuses the switch with
/// that message. An embedder uses it to keep away from a session file that
/// another process writes.
pub type SessionFileGuard = Arc<dyn Fn(&std::path::Path) -> Result<(), String> + Send + Sync>;

/// The model catalog file of a session the SDK built.
///
/// Such a session lives as long as its host, in the daemon for days, and the
/// catalog changes under it: the user signs in or adds a provider, the
/// platform adds a model. The session therefore reads the file again when a
/// client asks for the models or for one it does not know, where it would
/// otherwise answer from the catalog of the day it started.
struct Catalog {
    /// [`SessionOptions::models_file`]; `None` is the user's own catalog.
    file: Option<PathBuf>,
}

impl Catalog {
    fn load(&self) -> Registry {
        Registry::load(self.file.as_deref())
    }

    /// Whether asking the platform changes what the session reads: the user
    /// is signed in, and the account's models are saved to this file.
    fn follows_platform(&self) -> bool {
        let users_own = match &self.file {
            None => true,
            Some(file) => super_ai::provider_config::catalog_path().is_ok_and(|path| path == *file),
        };
        users_own && super_platform::is_signed_in()
    }
}

/// How old the platform's answer may be when a client asks for the models.
const PLATFORM_MODELS_MAX_AGE: Duration = Duration::from_secs(60);
/// How long a client waits for the platform. After that it is answered from
/// the catalog saved last.
const PLATFORM_MODELS_TIMEOUT: Duration = Duration::from_secs(5);

/// When this process last asked the platform for the account's models. Held
/// while it asks, so sessions asked at the same moment share one request.
static PLATFORM_MODELS_ASKED: tokio::sync::Mutex<Option<Instant>> =
    tokio::sync::Mutex::const_new(None);

/// Ask the platform for the account's models and save them to the user's
/// catalog, unless this process asked within `max_age`. A failure is not
/// reported: the catalog saved last stays in use, as it does in the terminal.
async fn sync_platform_models(max_age: Duration) {
    let mut asked = PLATFORM_MODELS_ASKED.lock().await;
    if asked.is_some_and(|at| at.elapsed() < max_age) {
        return;
    }
    let config = super_platform::OAuthConfig::default();
    let _ = tokio::time::timeout(
        PLATFORM_MODELS_TIMEOUT,
        super_platform::sync_models(&config, &CancellationToken::new()),
    )
    .await;
    // An attempt counts, so an unreachable platform costs one wait, not one
    // for every time the list is asked for.
    *asked = Some(Instant::now());
}

/// One embeddable conversation with the agent.
pub struct Session {
    inner: Arc<AgentSession>,
    /// `None` for a session something else built, which keeps its catalog.
    catalog: Option<Catalog>,
    events: broadcast::Sender<Event>,
    running: Arc<AtomicBool>,
    /// Serializes prompt runs so `wait_idle` has something to wait on.
    run_lock: Arc<tokio::sync::Mutex<()>>,
    bash_cancel: Mutex<Option<CancellationToken>>,
    cwd: PathBuf,
    closed: AtomicBool,
    approvals: Approvals,
    observer: Mutex<Option<SessionObserver>>,
    file_guard: Mutex<Option<SessionFileGuard>>,
    /// What `initialize` said about stop controls, per origin; `None` is a
    /// direct client.
    task_controls: Mutex<HashMap<Option<String>, bool>>,
    /// The loaded sessions of this session's agents, by agent id, and who
    /// hears of changes to them.
    agents: Mutex<AgentViewList>,
    /// This is the view of an agent's session, which answers reading
    /// commands only.
    agent_view: bool,
}

#[derive(Default)]
struct AgentViewList {
    views: BTreeMap<String, Arc<Session>>,
    subscribers: Vec<tokio::sync::mpsc::UnboundedSender<AgentChange>>,
}

impl AgentViewList {
    fn tell(&mut self, change: impl Fn() -> AgentChange) {
        self.subscribers
            .retain(|subscriber| subscriber.send(change()).is_ok());
    }
}

/// A change to the agent sessions of a [`Session`], as
/// [`Session::subscribe_agents`] reports it.
pub enum AgentChange {
    /// The session of the agent `id` was loaded: when the agent started, or
    /// when its stored session was loaded to resume it or to be read.
    /// `events` is subscribed from that moment, before the agent's run
    /// starts, so it misses nothing of the run.
    Loaded {
        id: String,
        session: Arc<Session>,
        events: EventStream,
    },
    /// The session moved to another conversation and forgot its agents.
    Reset,
}

/// How many events a subscriber to an agent's session may fall behind.
const AGENT_EVENT_CAPACITY: usize = 1024;

/// Why an agent's session refused a command.
pub const AGENT_VIEW_REFUSAL: &str = "An agent's session takes reading commands only; send this command to the session that started the agent.";

/// Wraps each agent session of one [`Session`] as a [`Session`] of its own,
/// so that clients and mirrors reach it the way they reach the session.
struct AgentViews {
    session: Weak<Session>,
}

impl super_coding::agents::AgentObserver for AgentViews {
    fn agent_session(&self, agent_id: &str, child: &Arc<AgentSession>) {
        let Some(session) = self.session.upgrade() else {
            return;
        };
        let mut list = session.agents.lock().unwrap();
        if list
            .views
            .get(agent_id)
            .is_some_and(|view| Arc::ptr_eq(&view.inner, child))
        {
            return;
        }
        let (sink, events) = mirrored_sink(AGENT_EVENT_CAPACITY, None);
        child.forward_events(Some(sink));
        let cwd = child.manager.lock().unwrap().cwd().to_path_buf();
        let view = Session::wrap_agent(child.clone(), cwd, events);
        list.views.insert(agent_id.to_string(), view.clone());
        list.tell(|| AgentChange::Loaded {
            id: agent_id.to_string(),
            session: view.clone(),
            events: view.events(),
        });
    }

    fn agent_settled(&self, agent_id: &str) {
        let view = self
            .session
            .upgrade()
            .and_then(|session| session.agents.lock().unwrap().views.get(agent_id).cloned());
        if let Some(view) = view {
            view.emit(agent_settled());
        }
    }

    fn agents_reset(&self) {
        if let Some(session) = self.session.upgrade() {
            let mut list = session.agents.lock().unwrap();
            list.views.clear();
            list.tell(|| AgentChange::Reset);
        }
    }
}

/// Who sent a command, as far as the session needs to know.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandSender {
    /// The name the sending surface gave itself (a daemon client's
    /// `hello.name`, the device a platform command came from); `None` for a
    /// direct client.
    pub origin: Option<String>,
    /// The sender shows a stop control for each background task, so its
    /// `abort` cancels the turn only. A sender without one also stops the
    /// background tasks with `abort`: it has no other way to stop them.
    pub task_controls: bool,
}

/// Build the event sink a harness session needs so that every event is also
/// published to the SDK event stream returned alongside it.
///
/// `forward` receives the original event after it was encoded, so a terminal
/// can keep consuming typed events while remote clients see the JSON form.
/// Encoding is skipped while nobody subscribes, so an idle mirror costs one
/// atomic load per event.
pub fn mirrored_sink(
    capacity: usize,
    forward: Option<super_coding::SessionEventSink>,
) -> (super_coding::SessionEventSink, broadcast::Sender<Event>) {
    let (sender, _) = broadcast::channel(capacity.max(1));
    let publish = sender.clone();
    let sink: super_coding::SessionEventSink = Arc::new(move |event: SessionEvent| {
        if publish.receiver_count() > 0
            && let Some(value) = session_event_json(&event)
        {
            // A send error only means nobody is listening any more.
            let _ = publish.send(Event(value));
        }
        if let Some(forward) = &forward {
            forward(event);
        }
    });
    (sink, sender)
}

/// Route workflow approvals through [`Session::open_approval`], so a headless
/// session asks its clients (`approval_request`) instead of auto-approving.
/// `--mode rpc` and the daemon both install this.
pub fn install_workflow_approval_gate(session: &Arc<Session>) {
    let sdk = session.clone();
    session
        .agent_session()
        .set_workflow_approver(Arc::new(move |plan| {
            let sdk = sdk.clone();
            Box::pin(async move {
                let (_, answer) = sdk.open_approval(ApprovalRequest {
                    kind: crate::approvals::ApprovalKind::Workflow,
                    title: format!("Run workflow {}?", plan.name),
                    detail: Some(plan.agent_estimate()),
                    choices: vec![
                        crate::approvals::ApprovalChoice::new("approve", "Run it"),
                        crate::approvals::ApprovalChoice::new("cancel", "Cancel"),
                    ],
                });
                match answer.await {
                    Ok(answer) if answer.choice == "approve" => {
                        super_coding::workflows::ApprovalDecision::Approve
                    }
                    _ => super_coding::workflows::ApprovalDecision::Cancel,
                }
            })
        }));
}

/// Fluent construction of a [`Session`].
///
/// ```no_run
/// # async fn example() -> anyhow::Result<()> {
/// let session = super_sdk::Session::builder()
///     .cwd(".")
///     .tools(["Read", "Bash"])
///     .build()
///     .await?;
/// session.prompt("List the files here").await?;
/// # Ok(())
/// # }
/// ```
#[derive(Default)]
pub struct SessionBuilder {
    options: SessionOptions,
}

impl SessionBuilder {
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.options.cwd = cwd.into();
        self
    }

    pub fn model(mut self, pattern: impl Into<String>) -> Self {
        self.options.model = Some(pattern.into());
        self
    }

    pub fn provider(mut self, provider: impl Into<String>) -> Self {
        self.options.provider = Some(provider.into());
        self
    }

    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.options.api_key = Some(key.into());
        self
    }

    pub fn models_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.options.models_file = Some(path.into());
        self
    }

    pub fn thinking_level(mut self, level: ThinkingLevel) -> Self {
        self.options.thinking_level = Some(level);
        self
    }

    pub fn tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.options.tools = Some(tools.into_iter().map(Into::into).collect());
        self
    }

    pub fn exclude_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.options.exclude_tools = tools.into_iter().map(Into::into).collect();
        self
    }

    pub fn no_tools(mut self, no_tools: bool) -> Self {
        self.options.no_tools = no_tools;
        self
    }

    pub fn custom_tool(mut self, tool: super_agent::DynTool) -> Self {
        self.options.custom_tools.push(tool);
        self
    }

    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.options.system_prompt = Some(prompt.into());
        self
    }

    pub fn append_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.options.append_system_prompt = Some(prompt.into());
        self
    }

    pub fn session(mut self, source: crate::options::SessionSource) -> Self {
        self.options.session = source;
        self
    }

    /// Restore externally managed entries in a non-persistent session.
    pub fn restore_entries(mut self, entries: Vec<super_coding::SessionEntry>) -> Self {
        self.options.session = crate::options::SessionSource::InMemoryEntries(entries);
        self
    }

    pub fn session_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.options.session_dir = Some(dir.into());
        self
    }

    pub fn session_name(mut self, name: impl Into<String>) -> Self {
        self.options.session_name = Some(name.into());
        self
    }

    pub fn trust_project_files(mut self, trust: bool) -> Self {
        self.options.trust_project_files = trust;
        self
    }

    pub fn event_capacity(mut self, capacity: usize) -> Self {
        self.options.event_capacity = capacity.max(1);
        self
    }

    pub fn stream_fn(mut self, stream_fn: super_agent::StreamFn) -> Self {
        self.options.stream_fn = Some(stream_fn);
        self
    }

    pub fn options(mut self, options: SessionOptions) -> Self {
        self.options = options;
        self
    }

    pub async fn build(self) -> Result<Arc<Session>, SdkError> {
        Session::create(self.options).await
    }
}

impl Session {
    pub fn builder() -> SessionBuilder {
        SessionBuilder::default()
    }

    /// Start a session. This performs disk and (for some model catalogs)
    /// network work, so it is async even though most of the body is not.
    pub async fn create(options: SessionOptions) -> Result<Arc<Session>, SdkError> {
        let (sink, sender) = mirrored_sink(options.event_capacity, None);
        let catalog = Catalog {
            file: options.models_file.clone(),
        };
        let built = match options.build(sink.clone()) {
            // The model may be one the platform added after the catalog was
            // saved on this machine.
            Err(error) if error.is::<UnknownModel>() && catalog.follows_platform() => {
                sync_platform_models(Duration::ZERO).await;
                options.build(sink)?
            }
            built => built?,
        };
        Ok(Self::wrap(
            built.session,
            options.cwd.clone(),
            sender,
            Some(catalog),
        ))
    }

    /// Wrap a harness session that something else built and drives, such as
    /// the interactive terminal, so RPC and remote clients can observe and
    /// control it. `events` must be the sender half returned by
    /// [`mirrored_sink`] for that session's sink, otherwise clients see no
    /// events.
    pub fn attach(
        inner: Arc<AgentSession>,
        cwd: PathBuf,
        events: broadcast::Sender<Event>,
    ) -> Arc<Session> {
        Self::wrap(inner, cwd, events, None)
    }

    fn wrap(
        inner: Arc<AgentSession>,
        cwd: PathBuf,
        events: broadcast::Sender<Event>,
        catalog: Option<Catalog>,
    ) -> Arc<Session> {
        let session = Self::build(inner, cwd, events, catalog, false);
        session.install_turn_starter();
        session
    }

    /// The view of an agent's session: its events and entries reach clients
    /// as a session's do, its own agents are views in turn, and it answers
    /// reading commands only, because the agent runs as its parent says.
    fn wrap_agent(
        inner: Arc<AgentSession>,
        cwd: PathBuf,
        events: broadcast::Sender<Event>,
    ) -> Arc<Session> {
        Self::build(inner, cwd, events, None, true)
    }

    fn build(
        inner: Arc<AgentSession>,
        cwd: PathBuf,
        events: broadcast::Sender<Event>,
        catalog: Option<Catalog>,
        agent_view: bool,
    ) -> Arc<Session> {
        let session = Arc::new(Session {
            inner,
            catalog,
            events,
            running: Arc::new(AtomicBool::new(false)),
            run_lock: Arc::new(tokio::sync::Mutex::new(())),
            bash_cancel: Mutex::new(None),
            cwd,
            closed: AtomicBool::new(false),
            approvals: Approvals::default(),
            observer: Mutex::new(None),
            file_guard: Mutex::new(None),
            task_controls: Mutex::new(HashMap::new()),
            agents: Mutex::new(AgentViewList::default()),
            agent_view,
        });
        session.inner.set_agent_observer(Some(Arc::new(AgentViews {
            session: Arc::downgrade(&session),
        })));
        session
    }

    /// The loaded sessions of this session's agents, by agent id, oldest
    /// first: an agent's session is loaded when the agent starts, and a
    /// stored one when it is resumed or read. Each is a view that answers
    /// reading commands only ([`Session::is_agent_view`]); its events are
    /// the agent's, and its own agents are listed the same way.
    pub fn agent_sessions(&self) -> Vec<(String, Arc<Session>)> {
        let views = self.agents.lock().unwrap().views.clone();
        self.in_agent_order(views)
    }

    /// The loaded agent sessions as [`Session::agent_sessions`] lists them,
    /// and a receiver of every change after that: a session that is loaded
    /// (with its events from that moment) and the agents being forgotten.
    /// Nothing falls between the list and the first change.
    pub fn subscribe_agents(
        &self,
    ) -> (
        Vec<(String, Arc<Session>)>,
        tokio::sync::mpsc::UnboundedReceiver<AgentChange>,
    ) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let views = {
            let mut list = self.agents.lock().unwrap();
            list.subscribers.push(sender);
            list.views.clone()
        };
        (self.in_agent_order(views), receiver)
    }

    /// `views` in the order the agents were started.
    fn in_agent_order(&self, views: BTreeMap<String, Arc<Session>>) -> Vec<(String, Arc<Session>)> {
        let mut sessions: Vec<(String, Arc<Session>)> = views.into_iter().collect();
        let order: Vec<String> = self
            .inner
            .agent_registry()
            .map(|registry| registry.list().into_iter().map(|agent| agent.id).collect())
            .unwrap_or_default();
        sessions.sort_by_key(|(id, _)| order.iter().position(|known| known == id));
        sessions
    }

    /// Whether this is the view of an agent's session, which answers
    /// reading commands only.
    pub fn is_agent_view(&self) -> bool {
        self.agent_view
    }

    /// Run the turns the harness session starts on its own (a background
    /// task's notice arrived while it was idle) the way a prompt runs: one
    /// at a time with the prompts, waited for by `wait_idle`, and followed
    /// by `agent_settled`.
    fn install_turn_starter(self: &Arc<Self>) {
        let sdk = Arc::downgrade(self);
        self.inner.set_turn_starter(Some(Arc::new(move |turn| {
            let sdk = sdk.clone();
            tokio::spawn(async move {
                let Some(session) = sdk.upgrade() else {
                    turn.await;
                    return;
                };
                let guard = session.run_lock.clone().lock_owned().await;
                let claimed = session
                    .running
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok();
                let ran = turn.await;
                if claimed {
                    session.running.store(false, Ordering::SeqCst);
                }
                drop(guard);
                if ran {
                    session.emit(agent_settled());
                }
            });
        })));
    }

    /// Tell clients the agent is idle after a run that another surface drove.
    ///
    /// Runs started through the SDK emit `agent_settled` on their own. A
    /// terminal that calls the harness directly must call this when its own
    /// run finishes so remote clients learn the same thing.
    pub fn notify_settled(&self) {
        self.emit(agent_settled());
    }

    /// Install the observer that hears about remote commands and approvals.
    pub fn set_observer(&self, observer: Option<SessionObserver>) {
        *self.observer.lock().unwrap() = observer;
    }

    /// Install the check `switch_session` runs before opening a file.
    pub fn set_session_file_guard(&self, guard: Option<SessionFileGuard>) {
        *self.file_guard.lock().unwrap() = guard;
    }

    fn notify(&self, notice: SessionNotice) {
        let observer = self.observer.lock().unwrap().clone();
        if let Some(observer) = observer {
            observer(notice);
        }
    }

    /// Register a question for a human. Every subscriber receives an
    /// `approval_request` event; the returned receiver resolves with the first
    /// answer from any surface.
    pub fn open_approval(
        &self,
        request: ApprovalRequest,
    ) -> (String, tokio::sync::oneshot::Receiver<ApprovalAnswer>) {
        let (id, answer) = self.approvals.open(request.clone());
        self.emit(approval_request_event(&id, &request));
        (id, answer)
    }

    /// Answer an open approval on behalf of `by` (`local` for the terminal).
    pub fn answer_approval(&self, id: &str, choice: &str, by: &str) -> Result<(), SdkError> {
        self.approvals
            .answer(id, choice, by)
            .map_err(SdkError::Command)?;
        self.emit(approval_resolved_event(id, choice, by));
        self.notify(SessionNotice::ApprovalAnswered {
            approval_id: id.to_string(),
            choice: choice.to_string(),
            by: by.to_string(),
        });
        Ok(())
    }

    /// Drop an approval nobody can answer any more.
    pub fn close_approval(&self, id: &str) {
        self.approvals.close(id);
    }

    /// For a session that ends in a worktree it made with `EnterWorktree`:
    /// ask every surface whether to keep or remove it
    /// ([`crate::approvals::worktree_exit_request`]), wait at most `wait` for
    /// an answer, and do what was chosen. Without an answer the worktree is
    /// kept, and the question is answered `keep` so every surface closes it.
    /// `remove` stops the session's background tasks first and removes the
    /// worktree with its branch and its changes. Returns what was done, as
    /// `ExitWorktree` says it; `None` when the session is in no worktree it
    /// made.
    pub async fn settle_worktree(&self, wait: std::time::Duration) -> Option<String> {
        let entered = self.inner.worktree().filter(|entered| entered.created)?;
        let (id, answer) = self.open_approval(crate::approvals::worktree_exit_request(&entered));
        let choice = match tokio::time::timeout(wait, answer).await {
            Ok(Ok(answer)) => answer.choice,
            _ => {
                if self
                    .answer_approval(&id, crate::approvals::WORKTREE_KEEP, "local")
                    .is_err()
                {
                    self.close_approval(&id);
                }
                crate::approvals::WORKTREE_KEEP.to_string()
            }
        };
        let action = if choice == crate::approvals::WORKTREE_REMOVE {
            self.inner.stop_background_tasks();
            super_coding::tools::worktree::ExitAction::Remove
        } else {
            super_coding::tools::worktree::ExitAction::Keep
        };
        let session = self.inner.clone();
        let exited = tokio::task::spawn_blocking(move || {
            super_coding::tools::worktree::exit_worktree(&session, action, true)
        })
        .await
        .ok()?;
        Some(match exited {
            Ok(exited) => exited.message,
            Err(error) => error,
        })
    }

    /// Subscribe to the event stream. Each subscriber gets every event emitted
    /// after it subscribed.
    pub fn events(&self) -> EventStream {
        EventStream {
            receiver: self.events.subscribe(),
        }
    }

    /// The underlying harness session, for operations the SDK does not wrap.
    pub fn agent_session(&self) -> &Arc<AgentSession> {
        &self.inner
    }

    fn emit(&self, value: Value) {
        let _ = self.events.send(Event(value));
    }

    // ----------------------------------------------------------------
    // Typed helpers. Each one is a thin wrapper over `execute`.
    // ----------------------------------------------------------------

    /// Send a prompt and wait for the whole run, including tool calls, retry,
    /// and automatic compaction, to finish.
    pub async fn prompt(self: &Arc<Self>, message: impl Into<String>) -> Result<(), SdkError> {
        self.prompt_with(PromptArgs::new(message)).await
    }

    /// Send a prompt with images or an explicit queueing behavior, and wait for
    /// the run to finish. When the message was queued rather than started, this
    /// returns as soon as it was queued.
    pub async fn prompt_with(self: &Arc<Self>, args: PromptArgs) -> Result<(), SdkError> {
        let queued = self.accept_prompt(&args)?;
        if queued {
            return Ok(());
        }
        self.clone().run_prompt(args).await;
        Ok(())
    }

    /// Send a prompt and return as soon as it has been accepted or queued. The
    /// run continues in the background. Wait for the `agent_settled` event or
    /// call [`Session::wait_idle`].
    pub fn prompt_detached(self: &Arc<Self>, args: PromptArgs) -> Result<(), SdkError> {
        let queued = self.accept_prompt(&args)?;
        if queued {
            return Ok(());
        }
        let session = self.clone();
        tokio::spawn(async move { session.clone().run_prompt(args).await });
        Ok(())
    }

    /// Decide whether a prompt starts a run or joins a queue.
    ///
    /// Returns `Ok(true)` when the message was queued and no run should start.
    fn accept_prompt(self: &Arc<Self>, args: &PromptArgs) -> Result<bool, SdkError> {
        if args.message.trim().is_empty() && args.images.is_empty() {
            return Err(SdkError::Command("prompt message is empty".into()));
        }
        let busy = self.inner.is_running()
            || self
                .running
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err();
        if busy {
            return match args.streaming_behavior {
                Some(StreamingBehavior::Steer) => {
                    self.inner.queue_steering(user_message(args));
                    Ok(true)
                }
                Some(StreamingBehavior::FollowUp) => {
                    self.inner.queue_follow_up(user_message(args));
                    Ok(true)
                }
                None => Err(SdkError::Command(
                    "agent is streaming: pass streamingBehavior \"steer\" or \"followUp\"".into(),
                )),
            };
        }
        Ok(false)
    }

    async fn run_prompt(self: Arc<Self>, args: PromptArgs) {
        let guard = self.run_lock.clone().lock_owned().await;
        let message = user_message(&args);
        let mode = self.inner.prompt_mode_for(&args.message);
        self.inner.prompt_with_mode(vec![message], mode).await;
        self.running.store(false, Ordering::SeqCst);
        drop(guard);
        self.emit(agent_settled());
    }

    /// Queue a message for delivery after the current turn's tool calls.
    pub fn steer(&self, message: impl Into<String>) -> Result<(), SdkError> {
        let text = message.into();
        if text.trim().is_empty() {
            return Err(SdkError::Command("steering message is empty".into()));
        }
        self.inner.queue_steering(AgentMessage::user(text));
        Ok(())
    }

    /// Queue a message for delivery once the agent stops.
    pub fn follow_up(&self, message: impl Into<String>) -> Result<(), SdkError> {
        let text = message.into();
        if text.trim().is_empty() {
            return Err(SdkError::Command("follow-up message is empty".into()));
        }
        self.inner.queue_follow_up(AgentMessage::user(text));
        Ok(())
    }

    /// Cancel the current run and any running direct shell command. Unless
    /// this embedder said it shows a stop control for each background task
    /// ([`Session::set_task_controls`]), the background tasks stop too.
    pub fn abort(&self) {
        self.interrupt(!self.has_task_controls(None));
    }

    /// Say whether this embedder (a direct client) shows a stop control for
    /// each background task, as the `initialize` command does. With one,
    /// [`Session::abort`] cancels the turn only.
    pub fn set_task_controls(&self, task_controls: bool) {
        self.task_controls
            .lock()
            .unwrap()
            .insert(None, task_controls);
    }

    /// The session's background tasks, oldest first.
    pub fn tasks(&self) -> Vec<super_coding::TaskInfo> {
        self.inner.tasks()
    }

    /// Stop one running background task, by id or by name.
    pub fn stop_task(&self, id_or_name: &str) -> Result<super_coding::TaskInfo, SdkError> {
        self.inner
            .stop_task(id_or_name)
            .map_err(|error| SdkError::Command(error.to_string()))
    }

    fn has_task_controls(&self, origin: Option<&str>) -> bool {
        self.task_controls
            .lock()
            .unwrap()
            .get(&origin.map(str::to_string))
            .copied()
            .unwrap_or(false)
    }

    /// Cancel the turn and a direct shell command; with `stop_tasks` the
    /// background tasks as well.
    fn interrupt(&self, stop_tasks: bool) {
        self.inner.abort();
        if let Some(cancel) = self.bash_cancel.lock().unwrap().take() {
            cancel.cancel();
        }
        if stop_tasks {
            self.inner.stop_background_tasks();
        }
    }

    /// Resolve once no prompt run is in flight.
    pub async fn wait_idle(&self) {
        let _guard = self.run_lock.lock().await;
        while self.inner.is_running() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// A snapshot of model, thinking level, session identity, and tools.
    pub fn state(&self) -> SessionState {
        let manager = self.inner.manager.lock().unwrap();
        SessionState {
            model: Some(self.inner.model()),
            thinking_level: self.inner.thinking_level(),
            is_streaming: self.inner.is_running() || self.running.load(Ordering::SeqCst),
            session_file: manager.session_file().map(Path::to_path_buf),
            session_id: manager.session_id().to_string(),
            session_name: manager.session_name(),
            message_count: manager.build_session_context().messages.len(),
            tools: self.inner.available_tool_names(),
        }
    }

    /// The active conversation, after compaction and branching are applied.
    pub fn messages(&self) -> Vec<AgentMessage> {
        self.inner
            .manager
            .lock()
            .unwrap()
            .build_session_context()
            .messages
    }

    /// Replace old history with a model-written summary.
    pub async fn compact(
        self: &Arc<Self>,
        custom_instructions: Option<String>,
    ) -> Result<(), SdkError> {
        self.inner.compact(custom_instructions, false).await;
        Ok(())
    }

    /// Choose a model by provider and identifier.
    ///
    /// A model this session does not know is looked for in the catalog as it
    /// is now, and a SUPER model after that on the platform. A model added
    /// while the session runs can so be chosen without restarting its host.
    pub async fn set_model(&self, provider: &str, model_id: &str) -> Result<Model, SdkError> {
        if !self.knows_model(provider, model_id) {
            self.refresh_catalog(None).await;
            if provider == super_platform::PROVIDER && !self.knows_model(provider, model_id) {
                self.refresh_catalog(Some(Duration::ZERO)).await;
            }
        }
        let (model, _) = self
            .inner
            .registry()
            .resolve(model_id, Some(provider))
            .ok_or_else(|| SdkError::Command(format!("model not found: {provider}/{model_id}")))?;
        self.inner.set_model(model.clone());
        Ok(model)
    }

    /// Whether the catalog has exactly this model. `resolve` also accepts a
    /// part of a name, which in an old catalog can be another model's.
    fn knows_model(&self, provider: &str, model_id: &str) -> bool {
        self.inner
            .registry()
            .all()
            .iter()
            .any(|model| model.provider == provider && model.id == model_id)
    }

    /// Read the catalog file again. With `platform_max_age`, the platform is
    /// asked for the account's models first, when its last answer is older.
    async fn refresh_catalog(&self, platform_max_age: Option<Duration>) {
        let Some(catalog) = &self.catalog else {
            return;
        };
        if let Some(max_age) = platform_max_age
            && catalog.follows_platform()
        {
            sync_platform_models(max_age).await;
        }
        self.inner.set_registry(catalog.load());
    }

    /// Choose a reasoning effort. Levels the model does not support are
    /// rejected rather than silently downgraded.
    pub fn set_thinking_level(&self, level: ThinkingLevel) -> Result<(), SdkError> {
        let model = self.inner.model();
        if !model.supported_thinking_levels().contains(&level) {
            return Err(SdkError::Command(format!(
                "model {}/{} does not support thinking level {}",
                model.provider,
                model.id,
                level.as_str()
            )));
        }
        self.inner.set_thinking_level(level);
        Ok(())
    }

    /// Run a shell command directly and record it in the conversation.
    ///
    /// The output becomes part of the context sent with the *next* prompt, the
    /// same as `!` in the terminal interface.
    pub async fn bash(self: &Arc<Self>, command: &str) -> Result<BashResult, SdkError> {
        self.bash_with_id(command, None).await
    }

    async fn bash_with_id(
        self: &Arc<Self>,
        command: &str,
        request_id: Option<String>,
    ) -> Result<BashResult, SdkError> {
        let cancel = CancellationToken::new();
        *self.bash_cancel.lock().unwrap() = Some(cancel.clone());
        let settings = self.inner.settings();
        let session = self.clone();
        let id_for_updates = request_id.clone();
        let on_update = Arc::new(move |delta: String| {
            session.emit(bash_execution_update(id_for_updates.as_deref(), &delta));
        });
        let result = crate::shell::run(
            command,
            &self.cwd,
            settings.shell_path.as_deref(),
            settings.shell_command_prefix.as_deref(),
            cancel.clone(),
            on_update,
        )
        .await
        .map_err(|error| SdkError::Command(format!("{error:#}")))?;
        *self.bash_cancel.lock().unwrap() = None;

        let message = AgentMessage::BashExecution(BashExecutionMessage {
            command: command.to_string(),
            output: result.output.clone(),
            exit_code: result.exit_code,
            cancelled: result.cancelled,
            truncated: result.truncated,
            full_output_path: result.full_output_path.clone(),
            exclude_from_context: false,
            timestamp: super_ai::now_ms(),
        });
        let _ = self.inner.manager.lock().unwrap().append_message(message);
        Ok(result)
    }

    /// Stop a running direct shell command.
    pub fn abort_bash(&self) {
        if let Some(cancel) = self.bash_cancel.lock().unwrap().take() {
            cancel.cancel();
        }
    }

    /// Release the event channel. Subscribers see their streams end. The
    /// turn and every background task stop.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.interrupt(true);
    }

    // ----------------------------------------------------------------
    // The dispatcher.
    // ----------------------------------------------------------------

    /// Execute one protocol command.
    ///
    /// This never panics and never returns `Err`: every outcome, including
    /// invalid arguments, is encoded in the [`Response`]. A binding that let a
    /// panic escape would abort the host process, so all fallible work is
    /// converted here.
    pub async fn execute(self: &Arc<Self>, command: Command) -> Response {
        self.execute_with_id(command, None).await
    }

    /// Execute a command while preserving an RPC request identifier.
    ///
    /// In-process callers normally use [`Session::execute`]. RPC transports use
    /// this form so direct `bash_execution_update` events carry the same id as
    /// the command that produced them.
    pub async fn execute_with_id(
        self: &Arc<Self>,
        command: Command,
        id: Option<String>,
    ) -> Response {
        self.execute_from(command, id, None).await
    }

    /// Execute a command that arrived from another surface, naming it so the
    /// embedding terminal can attribute what happens (`♦ from iPhone`).
    pub async fn execute_from(
        self: &Arc<Self>,
        command: Command,
        id: Option<String>,
        origin: Option<String>,
    ) -> Response {
        self.execute_by(
            command,
            id,
            CommandSender {
                origin,
                task_controls: false,
            },
        )
        .await
    }

    /// Execute a command for `sender`. A sender shows a stop control for
    /// each background task when it says so here (a daemon client in its
    /// `hello`) or in an earlier `initialize`.
    pub async fn execute_by(
        self: &Arc<Self>,
        command: Command,
        id: Option<String>,
        sender: CommandSender,
    ) -> Response {
        let CommandSender {
            origin,
            task_controls,
        } = sender;
        let task_controls = task_controls || self.has_task_controls(origin.as_deref());
        let name = command.name();
        if self.agent_view && !command.only_reads() {
            return Response::err(name, AGENT_VIEW_REFUSAL).with_id(id);
        }
        if let Some(origin) = &origin {
            let input = match &command {
                Command::Prompt { message, .. } => Some(("prompt", message.clone())),
                Command::Steer { message, .. } => Some(("steer", message.clone())),
                Command::FollowUp { message, .. } => Some(("follow_up", message.clone())),
                _ => None,
            };
            if let Some((kind, text)) = input {
                self.notify(SessionNotice::RemoteInput {
                    origin: origin.clone(),
                    kind,
                    text,
                });
            }
        }
        let by = origin.clone().unwrap_or_else(|| "rpc".to_string());
        // What every other client is told once the command ran.
        let mut announced = origin
            .as_ref()
            .and_then(|origin| Some((origin.clone(), announced_command(&command)?)));
        let result = match command {
            // Announced before its run starts, so every client shows the
            // prompt before the reply.
            Command::Prompt {
                message,
                images,
                streaming_behavior,
            } if announced.is_some() => {
                let args = PromptArgs {
                    message,
                    images,
                    streaming_behavior,
                };
                match self.accept_prompt(&args) {
                    Ok(queued) => {
                        let data =
                            json!({"disposition": if queued { "queued" } else { "started" }});
                        if let Some((origin, command)) = announced.take() {
                            self.announce(&origin, command, &Ok(Some(data.clone())));
                        }
                        if !queued {
                            let session = self.clone();
                            tokio::spawn(async move { session.run_prompt(args).await });
                        }
                        Ok(Some(data))
                    }
                    Err(error) => Err(SdkErrorText::from(error)),
                }
            }
            Command::Bash { command } => self
                .bash_with_id(&command, id.clone())
                .await
                .map(|result| Some(result.to_json()))
                .map_err(SdkErrorText::from),
            Command::AnswerApproval {
                approval_id,
                choice,
            } => self
                .answer_approval(&approval_id, &choice, &by)
                .map(|()| None)
                .map_err(SdkErrorText::from),
            Command::Initialize { task_controls } => {
                self.task_controls
                    .lock()
                    .unwrap()
                    .insert(origin.clone(), task_controls);
                Ok(None)
            }
            Command::Abort {} => {
                self.interrupt(!task_controls);
                self.wait_idle().await;
                Ok(None)
            }
            other => self.execute_inner(other).await,
        };
        if let Some((origin, command)) = announced {
            self.announce(&origin, command, &result);
        }
        if let Some(origin) = origin
            && !matches!(name, "prompt" | "steer" | "follow_up" | "ping")
            && !name.starts_with("get_")
        {
            self.notify(SessionNotice::RemoteCommand {
                origin,
                command: name,
                success: result.is_ok(),
            });
        }
        match result {
            Ok(Some(data)) => Response::ok_data(name, data).with_id(id),
            Ok(None) => Response::ok(name).with_id(id),
            Err(error) => Response::err(name, error).with_id(id),
        }
    }

    /// Tell every client, and the embedding terminal, that `command` from
    /// `origin` ran.
    fn announce(&self, origin: &str, command: Value, result: &Result<Option<Value>, SdkErrorText>) {
        let (success, error, data) = match result {
            Ok(data) => (true, None, data.clone()),
            Err(error) => (false, Some(error.0.as_str()), None),
        };
        self.notify(SessionNotice::Applied {
            origin: origin.to_string(),
            command: command.clone(),
            success,
            data: data.clone(),
        });
        self.emit(command_applied(origin, command, success, error, data));
    }

    async fn execute_inner(
        self: &Arc<Self>,
        command: Command,
    ) -> Result<Option<Value>, SdkErrorText> {
        match command {
            Command::Prompt {
                message,
                images,
                streaming_behavior,
            } => {
                let args = PromptArgs {
                    message,
                    images,
                    streaming_behavior,
                };
                let queued = self.accept_prompt(&args)?;
                if !queued {
                    let session = self.clone();
                    tokio::spawn(async move { session.run_prompt(args).await });
                }
                Ok(Some(
                    json!({"disposition": if queued { "queued" } else { "started" }}),
                ))
            }
            Command::Steer { message, images } => {
                let args = PromptArgs {
                    message,
                    images,
                    streaming_behavior: None,
                };
                self.inner.queue_steering(user_message(&args));
                Ok(Some(json!({"disposition": "queued"})))
            }
            Command::FollowUp { message, images } => {
                let args = PromptArgs {
                    message,
                    images,
                    streaming_behavior: None,
                };
                self.inner.queue_follow_up(user_message(&args));
                Ok(Some(json!({"disposition": "queued"})))
            }
            // Handled in `execute_by`, where the sender is known.
            Command::Abort {} | Command::Initialize { .. } => {
                Err(SdkErrorText("unreachable".into()))
            }
            Command::ClearQueue {} => {
                let reclaimed: Vec<String> = self
                    .inner
                    .reclaim_queued()
                    .iter()
                    .map(message_text)
                    .collect();
                // The harness returns one flat list. Report it under both keys
                // so a client can restore the text without guessing.
                Ok(Some(json!({"messages": reclaimed})))
            }
            Command::NewSession {} => {
                let manager = super_coding::SessionManager::in_memory(&self.cwd);
                self.inner.replace_manager(manager);
                Ok(Some(json!({"cancelled": false})))
            }
            Command::GetState {} => {
                let state = self.state();
                let settings = self.inner.settings();
                Ok(Some(json!({
                    "model": state.model,
                    "thinkingLevel": state.thinking_level,
                    "isStreaming": state.is_streaming,
                    "sessionFile": state.session_file,
                    "sessionId": state.session_id,
                    "sessionName": state.session_name,
                    "messageCount": state.message_count,
                    "tools": state.tools,
                    "steeringMode": queue_mode_name(settings.steering_mode),
                    "followUpMode": queue_mode_name(settings.follow_up_mode),
                    "autoCompactionEnabled": settings.compaction.enabled,
                    "autoRetryEnabled": settings.retry.enabled,
                })))
            }
            Command::GetMessages {} => Ok(Some(json!({"messages": self.messages()}))),
            Command::GetEntries { since } => {
                let manager = self.inner.manager.lock().unwrap();
                entries_since(&manager, since.as_deref()).map(Some)
            }
            Command::GetAgents { agent } => {
                let (owner, prefix) = match &agent {
                    Some(path) => (
                        self.inner
                            .agent_at(path)
                            .map_err(|error| SdkErrorText(format!("{error:#}")))?,
                        format!("{}/", path.trim_matches('/')),
                    ),
                    None => (self.inner.clone(), String::new()),
                };
                let agents: Vec<Value> = owner
                    .agent_registry()
                    .map(|registry| registry.list())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|info| {
                        let mut value = serde_json::to_value(&info).unwrap_or(Value::Null);
                        value["path"] = Value::String(format!("{prefix}{}", info.id));
                        value
                    })
                    .collect();
                Ok(Some(json!({"agents": agents})))
            }
            Command::GetAgentEntries { agent, since } => {
                let child = self
                    .inner
                    .agent_at(&agent)
                    .map_err(|error| SdkErrorText(format!("{error:#}")))?;
                let manager = child.manager.lock().unwrap();
                let mut data = entries_since(&manager, since.as_deref())?;
                data["sessionId"] = Value::String(manager.session_id().to_string());
                data["sessionFile"] = json!(manager.session_file());
                Ok(Some(data))
            }
            Command::GetTree {} => {
                let manager = self.inner.manager.lock().unwrap();
                Ok(Some(json!({
                    "tree": build_tree(&manager, None),
                    "leafId": manager.leaf_id(),
                })))
            }
            Command::GetLastAssistantText {} => {
                let text = self
                    .messages()
                    .iter()
                    .rev()
                    .find_map(|message| match message {
                        AgentMessage::Assistant(assistant) => {
                            let text = assistant.text();
                            (!text.is_empty()).then_some(text)
                        }
                        _ => None,
                    });
                Ok(Some(json!({"text": text})))
            }
            Command::GetSessionStats {} => Ok(Some(self.session_stats())),
            Command::SetSessionName { name, .. } => {
                self.inner
                    .manager
                    .lock()
                    .unwrap()
                    .append_session_info(&name)
                    .map_err(|error| SdkErrorText(format!("{error:#}")))?;
                Ok(None)
            }
            Command::SetModel { provider, model_id } => {
                let model = self.set_model(&provider, &model_id).await?;
                Ok(Some(serde_json::to_value(model).unwrap_or(Value::Null)))
            }
            Command::GetAvailableModels { search } => {
                // The models a user can switch to: those of providers that are
                // configured in models.json or have a credential, the same set
                // the TUI's /model picker and `--list-models` offer, as they
                // are now and not as they were when the session started.
                self.refresh_catalog(Some(PLATFORM_MODELS_MAX_AGE)).await;
                let needle = search.unwrap_or_default().to_lowercase();
                let registry = self.inner.registry();
                let models: Vec<&Model> = registry
                    .available_models()
                    .into_iter()
                    .map(|(_, model)| model)
                    .filter(|model| {
                        needle.is_empty()
                            || format!("{}/{}", model.provider, model.id)
                                .to_lowercase()
                                .contains(&needle)
                    })
                    .collect();
                Ok(Some(json!({"models": models})))
            }
            Command::SetThinkingLevel { level } => {
                let parsed = ThinkingLevel::parse(&level)
                    .ok_or_else(|| SdkErrorText(format!("unknown thinking level '{level}'")))?;
                self.set_thinking_level(parsed)?;
                Ok(None)
            }
            Command::GetAvailableThinkingLevels {} => {
                let levels: Vec<&str> = self
                    .inner
                    .model()
                    .supported_thinking_levels()
                    .iter()
                    .map(|level| level.as_str())
                    .collect();
                Ok(Some(json!({"levels": levels})))
            }
            Command::SetSteeringMode { mode } => {
                let mut settings = self.inner.settings();
                settings.steering_mode = settings_queue_mode(mode);
                self.inner.update_settings(settings);
                Ok(None)
            }
            Command::SetFollowUpMode { mode } => {
                let mut settings = self.inner.settings();
                settings.follow_up_mode = settings_queue_mode(mode);
                self.inner.update_settings(settings);
                Ok(None)
            }
            Command::Compact {
                custom_instructions,
            } => {
                self.inner.compact(custom_instructions, false).await;
                let (used, window) = self.inner.context_usage();
                Ok(Some(json!({
                    "contextTokens": used,
                    "contextWindow": window,
                })))
            }
            Command::SetAutoCompaction { enabled } => {
                let mut settings = self.inner.settings();
                settings.compaction.enabled = enabled;
                self.inner.update_settings(settings);
                Ok(None)
            }
            Command::SetAutoRetry { enabled } => {
                let mut settings = self.inner.settings();
                settings.retry.enabled = enabled;
                self.inner.update_settings(settings);
                Ok(None)
            }
            // `execute_with_id` handles this before entering this match so
            // streamed output can carry the request correlation id.
            Command::Bash { .. } => unreachable!("bash is handled by execute_with_id"),
            Command::AbortBash {} => {
                self.abort_bash();
                Ok(None)
            }
            Command::GetTools {} => {
                let tools: Vec<Value> = self
                    .inner
                    .available_tool_names()
                    .into_iter()
                    .map(|name| json!({"name": name}))
                    .collect();
                Ok(Some(json!({"tools": tools})))
            }
            Command::GetTasks {} => Ok(Some(json!({"tasks": self.inner.tasks()}))),
            Command::StopTask { task_id } => {
                let task = self
                    .inner
                    .stop_task(&task_id)
                    .map_err(|error| SdkErrorText(error.to_string()))?;
                Ok(Some(json!({"task": task})))
            }
            Command::ExportHtml { output_path } => {
                let path = output_path.map(PathBuf::from).unwrap_or_else(|| {
                    self.inner
                        .manager
                        .lock()
                        .unwrap()
                        .session_file()
                        .map(|path| path.with_extension("html"))
                        .unwrap_or_else(|| self.cwd.join("super-session.html"))
                });
                self.export_html(&path)?;
                Ok(Some(json!({"path": path})))
            }
            Command::SwitchSession { session_path } => {
                let path = std::path::Path::new(&session_path);
                let guard = self.file_guard.lock().unwrap().clone();
                if let Some(guard) = guard {
                    guard(path).map_err(SdkErrorText)?;
                }
                let manager = super_coding::SessionManager::open(path)
                    .map_err(|error| SdkErrorText(format!("{error:#}")))?;
                self.inner.replace_manager(manager);
                Ok(Some(json!({"cancelled": false})))
            }
            Command::Fork { entry_id } => {
                let outcome = self
                    .inner
                    .navigate_tree(&entry_id, false, None, CancellationToken::new())
                    .await
                    .map_err(|error| SdkErrorText(format!("{error:#}")))?;
                Ok(Some(json!({
                    "text": outcome.editor_text,
                    "cancelled": outcome.cancelled,
                })))
            }
            Command::GetForkMessages {} => {
                let manager = self.inner.manager.lock().unwrap();
                let messages: Vec<Value> = manager
                    .branch_entries(manager.leaf_id())
                    .into_iter()
                    .filter_map(|entry| match entry {
                        super_coding::SessionEntry::Message {
                            base,
                            message: AgentMessage::User(user),
                            ..
                        } => Some(json!({
                            "entryId": base.id,
                            "text": user.content.as_text(),
                        })),
                        _ => None,
                    })
                    .collect();
                Ok(Some(json!({"messages": messages})))
            }
            Command::GetApprovals {} => {
                let approvals: Vec<Value> = self
                    .approvals
                    .list()
                    .iter()
                    .map(|(id, request)| {
                        let mut event = approval_request_event(id, request);
                        if let Value::Object(map) = &mut event {
                            map.remove("type");
                        }
                        event
                    })
                    .collect();
                Ok(Some(json!({"approvals": approvals})))
            }
            // Handled in `execute_from`, where the answering surface is known.
            Command::AnswerApproval { .. } => Err(SdkErrorText("unreachable".into())),
            Command::Ping {} => Ok(Some(json!({"pong": true}))),
        }
    }

    fn export_html(&self, path: &Path) -> Result<(), SdkErrorText> {
        let mut body = String::new();
        for message in self.messages() {
            let (role, text) = match message {
                AgentMessage::User(user) => ("user", user.content.as_text()),
                AgentMessage::Assistant(assistant) => ("assistant", assistant.text()),
                AgentMessage::ToolResult(result) => (
                    "tool",
                    result
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                other => (other.role(), message_text(&other)),
            };
            body.push_str(&format!(
                "<section><h2>{}</h2><pre>{}</pre></section>\n",
                escape_html(role),
                escape_html(&text)
            ));
        }
        let document = format!(
            "<!doctype html><meta charset=\"utf-8\"><title>SUPER session</title>\
             <style>body{{max-width:60rem;margin:2rem auto;font:16px system-ui}}\
             pre{{white-space:pre-wrap;background:#f5f5f5;padding:1rem}}</style>\
             <h1>SUPER session</h1>{body}"
        );
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| SdkErrorText(format!("create {}: {error}", parent.display())))?;
        }
        std::fs::write(path, document)
            .map_err(|error| SdkErrorText(format!("write {}: {error}", path.display())))
    }

    fn session_stats(&self) -> Value {
        let messages = self.messages();
        let mut user = 0usize;
        let mut assistant = 0usize;
        let mut tool_calls = 0usize;
        let mut tool_results = 0usize;
        for message in &messages {
            match message {
                AgentMessage::User(_) => user += 1,
                AgentMessage::Assistant(a) => {
                    assistant += 1;
                    tool_calls += a.tool_calls().count();
                }
                AgentMessage::ToolResult(_) => tool_results += 1,
                _ => {}
            }
        }
        let totals = self.inner.totals();
        let (used, window) = self.inner.context_usage();
        let manager = self.inner.manager.lock().unwrap();
        json!({
            "sessionFile": manager.session_file(),
            "sessionId": manager.session_id(),
            "userMessages": user,
            "assistantMessages": assistant,
            "toolCalls": tool_calls,
            "toolResults": tool_results,
            "totalMessages": messages.len(),
            "tokens": {
                "input": totals.input,
                "output": totals.output,
                "cacheRead": totals.cache_read,
                "cacheWrite": totals.cache_write,
                "cacheWrite1h": totals.cache_write_1h,
                "cacheReadAvailable": totals.cache_read_available,
                "total": totals.total_tokens,
            },
            "cost": totals.cost.total,
            "contextUsage": {
                "tokens": used,
                "contextWindow": window,
                "percent": used.saturating_mul(100).checked_div(window).unwrap_or(0),
            },
        })
    }
}

/// The entries of `manager` after the entry `since` (all without it), and
/// the leaf, as `get_entries` answers.
fn entries_since(
    manager: &super_coding::SessionManager,
    since: Option<&str>,
) -> Result<Value, SdkErrorText> {
    let entries = manager.entries();
    let selected: Vec<&super_coding::SessionEntry> = match since {
        None => entries.iter().collect(),
        Some(cursor) => {
            let position = entries
                .iter()
                .position(|entry| entry.id() == cursor)
                .ok_or_else(|| SdkErrorText(format!("no entry matches cursor '{cursor}'")))?;
            entries[position + 1..].iter().collect()
        }
    };
    Ok(json!({
        "entries": selected,
        "leafId": manager.leaf_id(),
    }))
}

/// A plain error message. `execute_inner` uses it so every failure path becomes
/// a `Response::err` rather than an escaping error type.
struct SdkErrorText(String);

impl std::fmt::Display for SdkErrorText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<SdkError> for SdkErrorText {
    fn from(error: SdkError) -> Self {
        SdkErrorText(error.to_string())
    }
}

fn queue_mode_name(mode: SettingsQueueMode) -> &'static str {
    match mode {
        SettingsQueueMode::All => "all",
        SettingsQueueMode::OneAtATime => "one-at-a-time",
    }
}

fn settings_queue_mode(mode: QueueMode) -> SettingsQueueMode {
    match mode {
        QueueMode::All => SettingsQueueMode::All,
        QueueMode::OneAtATime => SettingsQueueMode::OneAtATime,
    }
}

fn message_text(message: &AgentMessage) -> String {
    match message {
        AgentMessage::User(user) => user.content.as_text(),
        other => other.role().to_string(),
    }
}

/// Build the user message a prompt turns into, attaching any images.
fn user_message(args: &PromptArgs) -> AgentMessage {
    if args.images.is_empty() {
        return AgentMessage::user(args.message.clone());
    }
    let mut blocks: Vec<ContentBlock> = Vec::with_capacity(args.images.len() + 1);
    if !args.message.is_empty() {
        blocks.push(ContentBlock::text(args.message.clone()));
    }
    for image in &args.images {
        blocks.push(ContentBlock::Image {
            data: image.data.clone(),
            mime_type: image.mime_type.clone(),
        });
    }
    AgentMessage::User(UserMessage {
        content: UserContent::Blocks(blocks),
        timestamp: super_ai::now_ms(),
    })
}

/// Recursively build `{entry, children}` nodes for `get_tree`.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn build_tree(manager: &super_coding::SessionManager, parent: Option<&str>) -> Vec<Value> {
    manager
        .children(parent)
        .into_iter()
        .map(|entry| {
            let id = entry.id().to_string();
            json!({
                "entry": entry,
                "label": manager.label_of(&id),
                "children": build_tree(manager, Some(&id)),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_mode_names_match_the_settings_file() {
        assert_eq!(
            queue_mode_name(SettingsQueueMode::OneAtATime),
            "one-at-a-time"
        );
        assert_eq!(queue_mode_name(SettingsQueueMode::All), "all");
    }

    #[test]
    fn images_become_content_blocks_alongside_the_text() {
        let args = PromptArgs {
            message: "look".into(),
            images: vec![ImageInput {
                kind: "image".into(),
                data: "AA==".into(),
                mime_type: "image/png".into(),
            }],
            streaming_behavior: None,
        };
        match user_message(&args) {
            AgentMessage::User(UserMessage {
                content: UserContent::Blocks(blocks),
                ..
            }) => {
                assert_eq!(blocks.len(), 2);
                assert!(matches!(blocks[1], ContentBlock::Image { .. }));
            }
            other => panic!("expected block content, got {other:?}"),
        }
    }
}
