//! A session the daemon hosts, run from this process as if it were local.
//!
//! The terminal builds its usual [`AgentSession`] (tools, settings, skills,
//! everything an in-process run has) and hands its engine to a
//! [`RemoteSession`] attached to the daemon's session. From then on:
//!
//! - prompts, steers, follow-ups, abort, model and thinking changes and
//!   compaction go over the socket ([`SessionBackend`]), in the order they
//!   were made;
//! - the session's events come back decoded into the harness events the
//!   terminal renders for its own runs, through the sink it gave;
//! - the session's manager is a mirror of the daemon's file: rebuilt from
//!   `get_entries` on attach and extended with what the daemon appends, so
//!   context usage, cost, `/session` and `/export` read what the apps read;
//! - what has no harness form (approvals, commands other clients ran, the
//!   connection itself) arrives as [`AttachedNotice`]s.
//!
//! When the connection drops the session reconnects on its own and attaches
//! to the same session again (a restarted daemon loads it from its file).
//! When no daemon comes back the front end can take the engine back with
//! [`RemoteSession::release`] and continue in its own process.

use crate::client::{Attached, ClientError, ClientEvent, ClientInfo, DaemonClient, NewSession};
use serde_json::Value;
use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use super_agent::AgentEvent;
use super_coding::session_runner::{
    AgentSession, PromptMode, SessionBackend, SessionEvent, SessionEventSink,
};
use super_coding::{SessionEntry, SessionManager};
use super_sdk::approvals::ApprovalRequest;
use super_sdk::daemon::{DaemonEvent, EndReason, SessionSummary};
use super_sdk::events::{announced_command, decode_session_event};
use super_sdk::protocol::{Command, ImageInput, StreamingBehavior};
use super_sdk::{AgentMessage, Model, ThinkingLevel};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// How the session reaches the daemon again after the connection dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connector {
    /// The socket [`DaemonClient::connect_or_start`] finds, starting a
    /// registered daemon when it is down.
    Discover,
    /// This socket and nothing else.
    Socket(PathBuf),
}

impl Connector {
    async fn connect(&self, info: &ClientInfo) -> Result<DaemonClient, ClientError> {
        match self {
            Connector::Discover => DaemonClient::connect_or_start(info).await,
            Connector::Socket(socket) => DaemonClient::connect_to(socket, info).await,
        }
    }
}

/// How long to wait before reconnect attempt `attempt` (zero-based), or
/// `None` once the daemon counts as gone. About eight seconds in all: long
/// enough for launchd or systemd to bring a crashed daemon back, short
/// enough that a terminal whose daemon is gone soon offers to go on alone.
pub fn reconnect_delay(attempt: usize) -> Option<Duration> {
    const DELAYS_MS: [u64; 5] = [250, 500, 1_000, 2_000, 4_000];
    DELAYS_MS.get(attempt).copied().map(Duration::from_millis)
}

/// A command a client ran on the session, from its `command_applied` event.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedCommand {
    /// The client's `hello.name`, or the device a platform command came from.
    pub origin: String,
    /// The command as sent, with `imageCount` in place of images.
    pub command: Value,
    pub success: bool,
    pub error: Option<String>,
    /// The response's data.
    pub data: Option<Value>,
    /// This client sent it.
    pub own: bool,
}

impl AppliedCommand {
    /// The command, typed. `None` for a command this build does not know.
    pub fn parsed(&self) -> Option<Command> {
        serde_json::from_value(self.command.clone()).ok()
    }
}

/// What a front end learns besides the session's own events.
#[derive(Debug, Clone, PartialEq)]
pub enum AttachedNotice {
    /// The daemon asks a question; answer with
    /// [`RemoteSession::answer_approval`].
    ApprovalRequested {
        approval_id: String,
        request: ApprovalRequest,
    },
    /// Some client answered it; `by` is that client's name.
    ApprovalResolved {
        approval_id: String,
        choice: String,
        by: String,
    },
    /// A client ran a command on the session, this one included.
    CommandApplied(AppliedCommand),
    /// A command this client sent was refused. Model and thinking level
    /// were read again from the daemon.
    CommandFailed { command: String, error: String },
    /// Output of a shell command this client runs through the daemon.
    ShellOutput(String),
    /// Name, model, thinking level, state or attached clients changed.
    SessionUpdated(SessionSummary),
    /// The daemon unloaded the session; the next command loads it again.
    SessionEnded(EndReason),
    /// The next command loaded the session again after it ended.
    Reloaded,
    /// The daemon said it is about to exit.
    DaemonShutdown,
    /// The connection dropped; reconnecting.
    Disconnected(String),
    /// Attached again on a new connection; the mirror was read again.
    Reconnected,
    /// No daemon came back. [`RemoteSession::release`] lets the session go
    /// on in this process.
    Lost(String),
    /// Events were skipped (`event_lag`); the mirror was read again.
    Resynced,
}

/// What [`RemoteSession::attach`] needs: a connection that said hello and
/// is attached, and the event stream taken from it before `attach`.
pub struct Attachment {
    pub client: DaemonClient,
    pub events: mpsc::UnboundedReceiver<ClientEvent>,
    pub attached: Attached,
    /// What this client said in `hello`; reconnects say it again.
    pub info: ClientInfo,
    pub connector: Connector,
}

/// Counts that move when a prompt's wait is over: a run settled, or the
/// connection changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Progress {
    settled: u64,
    link: u64,
}

#[derive(Default)]
struct Queues {
    /// What the daemon reports queued (previews, as `queue_update` has them).
    steering: Vec<String>,
    follow_up: Vec<String>,
    /// The full text of what this client queued and is still queued.
    own: Vec<String>,
}

struct LaneItem {
    command: Command,
    reply: Option<oneshot::Sender<Result<Option<Value>, ClientError>>>,
}

/// The daemon's session, driven from this process. See the module docs.
pub struct RemoteSession {
    me: Weak<RemoteSession>,
    info: ClientInfo,
    connector: Connector,
    view: Weak<AgentSession>,
    sink: SessionEventSink,
    notices: mpsc::UnboundedSender<AttachedNotice>,
    session: Mutex<SessionSummary>,
    daemon_version: Mutex<String>,
    client: Mutex<Option<Arc<DaemonClient>>>,
    progress: watch::Sender<Progress>,
    running: AtomicBool,
    /// The daemon unloaded the session; the next command attaches again.
    ended: AtomicBool,
    queues: Mutex<Queues>,
    /// The newest daemon entry the mirror has, the `get_entries` cursor.
    cursor: Mutex<Option<String>>,
    /// Commands this client sent that the daemon announces, oldest first.
    own_commands: Mutex<VecDeque<Value>>,
    shell_running: AtomicBool,
    turn: Mutex<TurnTracker>,
    lane: mpsc::UnboundedSender<LaneItem>,
    stopped: CancellationToken,
}

impl RemoteSession {
    /// Make `view` a view of the attached session: its manager becomes a
    /// mirror of the daemon's entries, its model and thinking level the
    /// daemon's, and its engine this session. Events go to `sink` from now
    /// on; everything else arrives on the returned notices.
    pub async fn attach(
        attachment: Attachment,
        view: &Arc<AgentSession>,
        sink: SessionEventSink,
    ) -> Result<(Arc<RemoteSession>, mpsc::UnboundedReceiver<AttachedNotice>), ClientError> {
        let Attachment {
            client,
            events,
            attached,
            info,
            connector,
        } = attachment;
        let (notices, notices_rx) = mpsc::unbounded_channel();
        let (lane, lane_rx) = mpsc::unbounded_channel();
        let daemon_version = client.hello().daemon_version.clone();
        let remote = Arc::new_cyclic(|me| RemoteSession {
            me: me.clone(),
            info,
            connector,
            view: Arc::downgrade(view),
            sink,
            notices,
            session: Mutex::new(attached.session.clone()),
            daemon_version: Mutex::new(daemon_version),
            client: Mutex::new(None),
            progress: watch::channel(Progress::default()).0,
            running: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            queues: Mutex::new(Queues::default()),
            cursor: Mutex::new(None),
            own_commands: Mutex::new(VecDeque::new()),
            shell_running: AtomicBool::new(false),
            turn: Mutex::new(TurnTracker::default()),
            lane,
            stopped: CancellationToken::new(),
        });
        let client = Arc::new(client);
        remote.load(&client, true).await?;
        *remote.client.lock().unwrap() = Some(client);
        view.set_backend(Some(remote.clone()));
        tokio::spawn(remote.clone().pump(events));
        tokio::spawn(remote.clone().run_lane(lane_rx));
        Ok((remote, notices_rx))
    }

    /// The attached session as the daemon last described it.
    pub fn session(&self) -> SessionSummary {
        self.session.lock().unwrap().clone()
    }

    pub fn session_id(&self) -> String {
        self.session.lock().unwrap().session_id.clone()
    }

    /// The name other clients see for this one.
    pub fn client_name(&self) -> &str {
        &self.info.name
    }

    pub fn daemon_version(&self) -> String {
        self.daemon_version.lock().unwrap().clone()
    }

    /// Whether a connection to the daemon is up right now.
    pub fn is_connected(&self) -> bool {
        self.client.lock().unwrap().is_some()
    }

    /// Answer an approval the daemon asked for; the daemon names this
    /// client's `hello.name` as the one who answered.
    pub async fn answer_approval(
        &self,
        approval_id: &str,
        choice: &str,
    ) -> Result<(), ClientError> {
        self.call(Command::AnswerApproval {
            approval_id: approval_id.to_string(),
            choice: choice.to_string(),
        })
        .await
        .map(|_| ())
    }

    pub async fn set_session_name(&self, name: &str) -> Result<(), ClientError> {
        self.call(Command::SetSessionName {
            name: name.to_string(),
            auto: false,
        })
        .await
        .map(|_| ())
    }

    /// Name the session with a title generated from its first prompt. Other
    /// clients take it as the title without showing a rename, as a title
    /// generated in-process is never shown.
    pub async fn set_generated_title(&self, title: &str) -> Result<(), ClientError> {
        self.call(Command::SetSessionName {
            name: title.to_string(),
            auto: true,
        })
        .await
        .map(|_| ())
    }

    /// Run a shell command in the session's directory through the daemon,
    /// which records it for the next model request. Output streams as
    /// [`AttachedNotice::ShellOutput`]; the result is the `bash` response.
    pub async fn bash(&self, command: &str) -> Result<Value, ClientError> {
        let client = self.ready_client().await?;
        let command = Command::Bash {
            command: command.to_string(),
        };
        self.note_own(&command);
        self.shell_running.store(true, Ordering::SeqCst);
        let result = client.execute_ok(command).await;
        self.shell_running.store(false, Ordering::SeqCst);
        result.map(Option::unwrap_or_default)
    }

    pub fn abort_bash(&self) {
        self.spawn_direct(Command::AbortBash {});
    }

    /// Start a new session in `cwd` on the daemon and switch to it, the
    /// way `/new` starts one in-process.
    pub async fn new_session(&self, new: NewSession) -> Result<SessionSummary, ClientError> {
        let client = self.current_client()?;
        let created = client.create_session(new).await?;
        self.switch_to(&created.session_id).await
    }

    /// Attach to another session of the daemon on this connection and make
    /// the view a mirror of it.
    pub async fn switch_to(&self, session_id: &str) -> Result<SessionSummary, ClientError> {
        let client = self.current_client()?;
        let attached = client.attach(session_id).await?;
        *self.session.lock().unwrap() = attached.session.clone();
        *self.cursor.lock().unwrap() = None;
        *self.queues.lock().unwrap() = Queues::default();
        self.ended.store(false, Ordering::SeqCst);
        self.load(&client, true).await?;
        Ok(attached.session)
    }

    /// Stop the session in the daemon (abort the turn, unload it).
    pub async fn stop_session(&self) -> Result<(), ClientError> {
        let client = self.current_client()?;
        client.stop_session(&self.session_id()).await
    }

    /// Leave the session running in the daemon and close the connection.
    pub async fn detach(&self) {
        self.stopped.cancel();
        let client = self.client.lock().unwrap().take();
        if let Some(client) = client {
            let _ = tokio::time::timeout(Duration::from_secs(2), client.detach()).await;
        }
    }

    /// Give the engine back to the view: it runs nothing remote any more.
    /// Returns the session's file, which the caller opens in-process once
    /// the daemon no longer holds it.
    pub fn release(&self) -> PathBuf {
        self.stopped.cancel();
        self.client.lock().unwrap().take();
        if let Some(view) = self.view.upgrade() {
            view.set_backend(None);
        }
        PathBuf::from(self.session.lock().unwrap().session_file.clone())
    }

    // --- internals ---

    fn notify(&self, notice: AttachedNotice) {
        let _ = self.notices.send(notice);
    }

    fn view(&self) -> Option<Arc<AgentSession>> {
        self.view.upgrade()
    }

    fn current_client(&self) -> Result<Arc<DaemonClient>, ClientError> {
        self.client
            .lock()
            .unwrap()
            .clone()
            .ok_or(ClientError::Closed)
    }

    /// The connection, after loading the session again when the daemon
    /// unloaded it.
    async fn ready_client(&self) -> Result<Arc<DaemonClient>, ClientError> {
        let client = self.current_client()?;
        if self.ended.load(Ordering::SeqCst) {
            let attached = client.attach(&self.session_id()).await?;
            *self.session.lock().unwrap() = attached.session;
            self.load(&client, false).await?;
            self.ended.store(false, Ordering::SeqCst);
            self.notify(AttachedNotice::Reloaded);
        }
        Ok(client)
    }

    /// Read the daemon's state and entries into the view. `fresh` rebuilds
    /// the mirror from every entry; otherwise only what is new is added.
    async fn load(&self, client: &DaemonClient, fresh: bool) -> Result<(), ClientError> {
        let state = client
            .execute_ok(Command::GetState {})
            .await?
            .unwrap_or_default();
        if fresh {
            let data = client
                .execute_ok(Command::GetEntries { since: None })
                .await?
                .unwrap_or_default();
            let (entries, leaf) = entries_of(&data);
            *self.cursor.lock().unwrap() = entries.last().map(|entry| entry.id().to_string());
            if let Some(view) = self.view() {
                let summary = self.session();
                view.replace_manager(SessionManager::mirror(
                    &summary.session_id,
                    Path::new(&summary.cwd),
                    entries,
                    leaf,
                ));
            }
        } else {
            self.sync_entries(client).await;
        }
        self.adopt_state(&state);
        // A turn in flight is picked up from its next event on.
        *self.turn.lock().unwrap() = TurnTracker::default();
        // A question asked before this view came is asked again here.
        if let Ok(Some(data)) = client.execute_ok(Command::GetApprovals {}).await {
            for approval in data["approvals"].as_array().into_iter().flatten() {
                if let (Some(approval_id), Ok(request)) = (
                    approval["approvalId"].as_str(),
                    serde_json::from_value::<ApprovalRequest>(approval.clone()),
                ) {
                    self.notify(AttachedNotice::ApprovalRequested {
                        approval_id: approval_id.to_string(),
                        request,
                    });
                }
            }
        }
        Ok(())
    }

    fn adopt_state(&self, state: &Value) {
        let model = state
            .get("model")
            .and_then(|model| serde_json::from_value::<Model>(model.clone()).ok());
        let thinking = state
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .and_then(ThinkingLevel::parse);
        if let Some(view) = self.view() {
            view.adopt_remote_state(model, thinking);
        }
        self.running.store(
            state["isStreaming"].as_bool().unwrap_or(false),
            Ordering::SeqCst,
        );
    }

    /// Add the entries the daemon appended since the cursor to the mirror.
    async fn sync_entries(&self, client: &DaemonClient) {
        let since = self.cursor.lock().unwrap().clone();
        let data = match client
            .execute_ok(Command::GetEntries {
                since: since.clone(),
            })
            .await
        {
            Ok(data) => data.unwrap_or_default(),
            // The cursor is unknown (another file under the same id?):
            // read everything; the mirror skips what it has.
            Err(ClientError::Command { .. }) if since.is_some() => {
                match client.execute_ok(Command::GetEntries { since: None }).await {
                    Ok(data) => data.unwrap_or_default(),
                    Err(_) => return,
                }
            }
            Err(_) => return,
        };
        let (entries, leaf) = entries_of(&data);
        if let Some(last) = entries.last() {
            *self.cursor.lock().unwrap() = Some(last.id().to_string());
        }
        if let Some(view) = self.view() {
            view.absorb_entries(entries, leaf);
        }
    }

    async fn refresh_state(&self, client: &DaemonClient) {
        if let Ok(Some(state)) = client.execute_ok(Command::GetState {}).await {
            self.adopt_state(&state);
        }
    }

    fn bump(&self, settled: bool) {
        self.progress.send_modify(|progress| {
            if settled {
                progress.settled += 1;
            } else {
                progress.link += 1;
            }
        });
    }

    /// Remember a command the daemon will announce, so its
    /// `command_applied` is known to be this client's.
    fn note_own(&self, command: &Command) {
        if let Some(announced) = announced_command(command) {
            let mut own = self.own_commands.lock().unwrap();
            own.push_back(announced);
            // Commands whose announcement never came (a lost connection)
            // must not pile up.
            while own.len() > 64 {
                own.pop_front();
            }
        }
    }

    fn take_own(&self, origin: &str, command: &Value) -> bool {
        if origin != self.info.name {
            return false;
        }
        let mut own = self.own_commands.lock().unwrap();
        match own.iter().position(|sent| sent == command) {
            Some(position) => {
                own.remove(position);
                true
            }
            None => false,
        }
    }

    /// Run `command` in order with every other command of this client and
    /// return its response's data.
    async fn call(&self, command: Command) -> Result<Option<Value>, ClientError> {
        let (reply, answer) = oneshot::channel();
        self.lane
            .send(LaneItem {
                command,
                reply: Some(reply),
            })
            .map_err(|_| ClientError::Closed)?;
        answer.await.unwrap_or(Err(ClientError::Closed))
    }

    /// [`call`](Self::call) without waiting; a refusal becomes a notice.
    fn send(&self, command: Command) {
        let _ = self.lane.send(LaneItem {
            command,
            reply: None,
        });
    }

    /// Run `command` now, beside the ordered lane: `abort` must not wait
    /// behind a compaction.
    fn spawn_direct(&self, command: Command) {
        let Some(me) = self.me.upgrade() else { return };
        tokio::spawn(async move {
            let Ok(client) = me.current_client() else {
                return;
            };
            me.note_own(&command);
            let name = command.name();
            if let Err(error) = client.execute_ok(command).await {
                me.notify(AttachedNotice::CommandFailed {
                    command: name.to_string(),
                    error: error.to_string(),
                });
            }
        });
    }

    async fn run_lane(self: Arc<Self>, mut items: mpsc::UnboundedReceiver<LaneItem>) {
        loop {
            let item = tokio::select! {
                _ = self.stopped.cancelled() => break,
                item = items.recv() => match item {
                    Some(item) => item,
                    None => break,
                },
            };
            let name = item.command.name();
            let result = match self.ready_client().await {
                Ok(client) => {
                    self.note_own(&item.command);
                    let result = client.execute_ok(item.command).await;
                    if result.is_err() && matches!(name, "set_model" | "set_thinking_level") {
                        // The view showed the change already; show the truth.
                        self.refresh_state(&client).await;
                    }
                    result
                }
                Err(error) => Err(error),
            };
            match item.reply {
                Some(reply) => {
                    let _ = reply.send(result);
                }
                None => {
                    if let Err(error) = result {
                        self.notify(AttachedNotice::CommandFailed {
                            command: name.to_string(),
                            error: describe(&error),
                        });
                    }
                }
            }
        }
    }

    /// Read the connection until it closes, then reconnect; ends when the
    /// session is detached or released, or no daemon comes back.
    async fn pump(self: Arc<Self>, mut events: mpsc::UnboundedReceiver<ClientEvent>) {
        loop {
            let event = tokio::select! {
                _ = self.stopped.cancelled() => return,
                event = events.recv() => event,
            };
            match event {
                Some(ClientEvent::Session(event)) => self.on_session_event(event.0).await,
                Some(ClientEvent::Daemon(event)) => self.on_daemon_event(event),
                None => match self.reconnect().await {
                    Some(next) => events = next,
                    None => return,
                },
            }
        }
    }

    async fn on_session_event(&self, event: Value) {
        let kind = event["type"].as_str().unwrap_or_default().to_string();
        match kind.as_str() {
            "agent_start" => self.running.store(true, Ordering::SeqCst),
            "agent_settled" => {
                self.running.store(false, Ordering::SeqCst);
                self.bump(true);
            }
            "queue_update" => {
                let previews = |key: &str| -> Vec<String> {
                    serde_json::from_value(event[key].clone()).unwrap_or_default()
                };
                let mut queues = self.queues.lock().unwrap();
                queues.steering = previews("steering");
                queues.follow_up = previews("followUp");
                let mut waiting: Vec<String> = queues
                    .steering
                    .iter()
                    .chain(queues.follow_up.iter())
                    .cloned()
                    .collect();
                queues.own.retain(|text| {
                    match waiting.iter().position(|shown| *shown == preview(text)) {
                        Some(position) => {
                            waiting.remove(position);
                            true
                        }
                        None => false,
                    }
                });
            }
            "approval_request" => {
                if let (Some(approval_id), Ok(request)) = (
                    event["approvalId"].as_str(),
                    serde_json::from_value::<ApprovalRequest>(event.clone()),
                ) {
                    self.notify(AttachedNotice::ApprovalRequested {
                        approval_id: approval_id.to_string(),
                        request,
                    });
                }
            }
            "approval_resolved" => self.notify(AttachedNotice::ApprovalResolved {
                approval_id: text(&event, "approvalId"),
                choice: text(&event, "choice"),
                by: text(&event, "by"),
            }),
            "command_applied" => {
                let origin = text(&event, "origin");
                let command = event["command"].clone();
                let own = self.take_own(&origin, &command);
                let success = event["success"].as_bool().unwrap_or(false);
                if success && let Ok(client) = self.current_client() {
                    self.sync_entries(&client).await;
                }
                self.notify(AttachedNotice::CommandApplied(AppliedCommand {
                    origin,
                    command,
                    success,
                    error: event["error"].as_str().map(str::to_string),
                    data: event.get("data").cloned(),
                    own,
                }));
            }
            "bash_execution_update" if self.shell_running.load(Ordering::SeqCst) => {
                self.notify(AttachedNotice::ShellOutput(text(&event, "delta")));
            }
            "event_lag" => {
                if let Ok(client) = self.current_client() {
                    self.sync_entries(&client).await;
                    self.refresh_state(&client).await;
                }
                self.notify(AttachedNotice::Resynced);
            }
            _ => {}
        }
        if let Some(decoded) = decode_session_event(&event) {
            let missing = self.turn.lock().unwrap().missing_starts(&decoded);
            for missing in missing {
                (self.sink)(missing);
            }
            (self.sink)(decoded);
        }
        // What the daemon appended to its file with this event.
        if matches!(
            kind.as_str(),
            "message_end" | "agent_end" | "compaction_end" | "model_changed" | "agent_settled"
        ) && let Ok(client) = self.current_client()
        {
            self.sync_entries(&client).await;
        }
    }

    fn on_daemon_event(&self, event: DaemonEvent) {
        let ours = self.session_id();
        match event {
            DaemonEvent::SessionUpdated { session } if session.session_id == ours => {
                *self.session.lock().unwrap() = session.clone();
                self.notify(AttachedNotice::SessionUpdated(session));
            }
            DaemonEvent::SessionEnded { session_id, reason } if session_id == ours => {
                self.ended.store(true, Ordering::SeqCst);
                self.running.store(false, Ordering::SeqCst);
                self.bump(false);
                self.notify(AttachedNotice::SessionEnded(reason));
            }
            DaemonEvent::DaemonShutdown { .. } => self.notify(AttachedNotice::DaemonShutdown),
            _ => {}
        }
    }

    /// The connection closed: attach again on a new one, or give up.
    async fn reconnect(&self) -> Option<mpsc::UnboundedReceiver<ClientEvent>> {
        self.client.lock().unwrap().take();
        self.running.store(false, Ordering::SeqCst);
        self.bump(false);
        if self.stopped.is_cancelled() {
            return None;
        }
        self.notify(AttachedNotice::Disconnected(
            "the SUPER daemon closed the connection".into(),
        ));
        let mut last_error = String::from("no SUPER daemon came back");
        let mut attempt = 0;
        while let Some(delay) = reconnect_delay(attempt) {
            attempt += 1;
            tokio::select! {
                _ = self.stopped.cancelled() => return None,
                _ = tokio::time::sleep(delay) => {}
            }
            match self.reattach().await {
                Ok(events) => {
                    self.notify(AttachedNotice::Reconnected);
                    return Some(events);
                }
                Err(ClientError::NotRunning { .. } | ClientError::Unavailable(_)) => {
                    last_error = "no SUPER daemon came back".into();
                }
                Err(error) => last_error = describe(&error),
            }
        }
        self.notify(AttachedNotice::Lost(last_error));
        None
    }

    async fn reattach(&self) -> Result<mpsc::UnboundedReceiver<ClientEvent>, ClientError> {
        let mut client = self.connector.connect(&self.info).await?;
        let events = client.take_events().ok_or(ClientError::Closed)?;
        let attached = client.attach(&self.session_id()).await?;
        *self.session.lock().unwrap() = attached.session;
        *self.daemon_version.lock().unwrap() = client.hello().daemon_version.clone();
        let client = Arc::new(client);
        self.load(&client, false).await?;
        self.ended.store(false, Ordering::SeqCst);
        if self.stopped.is_cancelled() {
            return Err(ClientError::Closed);
        }
        *self.client.lock().unwrap() = Some(client);
        Ok(events)
    }
}

/// What of the current turn a view has seen begin. A view that attached in
/// the middle of a reply or a tool call is handed the start it missed, so
/// the reply streams into a message and the tool call gets its line.
#[derive(Debug, Default)]
struct TurnTracker {
    in_reply: bool,
    open_tools: std::collections::HashSet<String>,
}

impl TurnTracker {
    /// The starts a view that attached mid-turn did not see, for `event`.
    fn missing_starts(&mut self, event: &SessionEvent) -> Vec<SessionEvent> {
        let SessionEvent::Agent(agent) = event else {
            return Vec::new();
        };
        let start = |message: AgentMessage| {
            SessionEvent::Agent(Box::new(AgentEvent::MessageStart { message }))
        };
        match agent.as_ref() {
            AgentEvent::MessageStart {
                message: AgentMessage::Assistant(_),
            } => {
                self.in_reply = true;
                Vec::new()
            }
            AgentEvent::MessageUpdate { .. } => {
                if std::mem::replace(&mut self.in_reply, true) {
                    Vec::new()
                } else {
                    vec![start(AgentMessage::Assistant(
                        super_ai::AssistantMessage::empty("", "", ""),
                    ))]
                }
            }
            AgentEvent::MessageEnd {
                message: message @ AgentMessage::Assistant(_),
            } => {
                if std::mem::replace(&mut self.in_reply, false) {
                    Vec::new()
                } else {
                    vec![start(message.clone())]
                }
            }
            AgentEvent::ToolExecutionStart { tool_call_id, .. } => {
                self.open_tools.insert(tool_call_id.clone());
                Vec::new()
            }
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                tool_name,
                args,
                ..
            } => {
                if self.open_tools.insert(tool_call_id.clone()) {
                    vec![tool_start(tool_call_id, tool_name, args.clone())]
                } else {
                    Vec::new()
                }
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                ..
            } if !self.open_tools.remove(tool_call_id) => {
                vec![tool_start(tool_call_id, tool_name, Value::Null)]
            }
            AgentEvent::ToolExecutionEnd { tool_call_id, .. } => {
                self.open_tools.remove(tool_call_id);
                Vec::new()
            }
            AgentEvent::AgentEnd { .. } => {
                self.in_reply = false;
                self.open_tools.clear();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

impl SessionBackend for RemoteSession {
    fn prompt(&self, prompts: Vec<AgentMessage>, mode: PromptMode) -> BoxFuture<()> {
        let me = self.me.clone();
        Box::pin(async move {
            let Some(me) = me.upgrade() else { return };
            let mut progress = me.progress.subscribe();
            let start = *progress.borrow_and_update();
            for message in prompts {
                let command = prompt_command(&message, mode, StreamingBehavior::Steer);
                if let Err(error) = me.call(command).await {
                    me.notify(AttachedNotice::CommandFailed {
                        command: "prompt".into(),
                        error: describe(&error),
                    });
                    return;
                }
            }
            // Until the run this prompt started or joined has settled, or
            // the connection changed under it.
            loop {
                let now = *progress.borrow_and_update();
                if now.settled > start.settled || now.link != start.link {
                    return;
                }
                if progress.changed().await.is_err() {
                    return;
                }
            }
        })
    }

    fn queue(&self, message: AgentMessage, mode: PromptMode, follow_up: bool) {
        let behavior = if follow_up {
            StreamingBehavior::FollowUp
        } else {
            StreamingBehavior::Steer
        };
        if let AgentMessage::User(user) = &message {
            self.queues.lock().unwrap().own.push(user.content.as_text());
        }
        self.send(prompt_command(&message, mode, behavior));
    }

    fn reclaim_queued(&self) -> Vec<AgentMessage> {
        let mut queues = self.queues.lock().unwrap();
        let waiting = !queues.steering.is_empty() || !queues.follow_up.is_empty();
        let own = std::mem::take(&mut queues.own);
        drop(queues);
        if waiting {
            self.send(Command::ClearQueue {});
        }
        own.into_iter().map(AgentMessage::user).collect()
    }

    fn abort(&self) {
        self.spawn_direct(Command::Abort {});
    }

    fn set_model(&self, model: &Model) {
        self.send(Command::SetModel {
            provider: model.provider.clone(),
            model_id: model.id.clone(),
        });
    }

    fn set_thinking_level(&self, level: ThinkingLevel) {
        self.send(Command::SetThinkingLevel {
            level: level.as_str().to_string(),
        });
    }

    fn compact(&self, custom_instructions: Option<String>) -> BoxFuture<()> {
        let me = self.me.clone();
        Box::pin(async move {
            let Some(me) = me.upgrade() else { return };
            if let Err(error) = me
                .call(Command::Compact {
                    custom_instructions,
                })
                .await
            {
                me.notify(AttachedNotice::CommandFailed {
                    command: "compact".into(),
                    error: describe(&error),
                });
            }
        })
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// The `prompt` command for one message. A workflow turn the daemon cannot
/// tell from the text is asked for in words, the way a user asks for one.
pub fn prompt_command(
    message: &AgentMessage,
    mode: PromptMode,
    behavior: StreamingBehavior,
) -> Command {
    let (mut text, images) = match message {
        AgentMessage::User(user) => match &user.content {
            super_ai::UserContent::Text(text) => (text.clone(), Vec::new()),
            super_ai::UserContent::Blocks(blocks) => {
                let mut text = Vec::new();
                let mut images = Vec::new();
                for block in blocks {
                    match block {
                        super_ai::ContentBlock::Text { text: part, .. } => text.push(part.clone()),
                        super_ai::ContentBlock::Image { data, mime_type } => {
                            images.push(ImageInput {
                                kind: "image".into(),
                                data: data.clone(),
                                mime_type: mime_type.clone(),
                            })
                        }
                        _ => {}
                    }
                }
                (text.join("\n"), images)
            }
        },
        other => (other.role().to_string(), Vec::new()),
    };
    if mode == PromptMode::Workflow && super_coding::workflows::workflow_trigger(&text).is_none() {
        text.push_str("\n\nRun this as a dynamic workflow.");
    }
    Command::Prompt {
        message: text,
        images,
        streaming_behavior: Some(behavior),
    }
}

fn tool_start(tool_call_id: &str, tool_name: &str, args: Value) -> SessionEvent {
    SessionEvent::Agent(Box::new(AgentEvent::ToolExecutionStart {
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        args,
    }))
}

/// `queue_update` shows the first 80 characters of a queued message.
fn preview(text: &str) -> String {
    text.chars().take(80).collect()
}

fn text(event: &Value, key: &str) -> String {
    event[key].as_str().unwrap_or_default().to_string()
}

fn entries_of(data: &Value) -> (Vec<SessionEntry>, Option<String>) {
    let entries = data
        .get("entries")
        .and_then(|entries| serde_json::from_value(entries.clone()).ok())
        .unwrap_or_default();
    let leaf = data
        .get("leafId")
        .and_then(Value::as_str)
        .map(str::to_string);
    (entries, leaf)
}

/// A client error as one line for the terminal.
fn describe(error: &ClientError) -> String {
    match error {
        ClientError::Closed => "not connected to the SUPER daemon".into(),
        ClientError::Command { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnecting_backs_off_and_gives_up_after_about_eight_seconds() {
        let delays: Vec<Duration> = (0..).map_while(reconnect_delay).collect();
        assert_eq!(delays.len(), 5);
        assert!(delays.windows(2).all(|pair| pair[0] < pair[1]));
        let total: Duration = delays.iter().sum();
        assert!(total >= Duration::from_secs(5) && total <= Duration::from_secs(10));
        assert_eq!(reconnect_delay(5), None);
    }

    #[test]
    fn prompts_carry_text_images_and_the_queue_behaviour() {
        let message = AgentMessage::User(super_ai::UserMessage {
            content: super_ai::UserContent::Blocks(vec![
                super_ai::ContentBlock::text("what is this"),
                super_ai::ContentBlock::Image {
                    data: "AAAA".into(),
                    mime_type: "image/png".into(),
                },
            ]),
            timestamp: 0,
        });
        let Command::Prompt {
            message: text,
            images,
            streaming_behavior,
        } = prompt_command(&message, PromptMode::Ordinary, StreamingBehavior::FollowUp)
        else {
            panic!("a prompt");
        };
        assert_eq!(text, "what is this");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime_type, "image/png");
        assert_eq!(streaming_behavior, Some(StreamingBehavior::FollowUp));
    }

    #[test]
    fn a_workflow_turn_is_asked_for_in_words_unless_the_text_does() {
        let workflow = |text: &str| match prompt_command(
            &AgentMessage::user(text),
            PromptMode::Workflow,
            StreamingBehavior::Steer,
        ) {
            Command::Prompt { message, .. } => message,
            _ => unreachable!(),
        };
        let asked = workflow("audit the tools");
        assert!(
            super_coding::workflows::workflow_trigger(&asked).is_some(),
            "{asked}"
        );
        assert_eq!(
            workflow("use a workflow to audit"),
            "use a workflow to audit"
        );
        match prompt_command(
            &AgentMessage::user("audit the tools"),
            PromptMode::Ordinary,
            StreamingBehavior::Steer,
        ) {
            Command::Prompt { message, .. } => assert_eq!(message, "audit the tools"),
            _ => unreachable!(),
        }
    }

    fn kinds(events: &[SessionEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(|event| match event {
                SessionEvent::Agent(agent) => match agent.as_ref() {
                    AgentEvent::MessageStart { .. } => "message_start",
                    AgentEvent::ToolExecutionStart { .. } => "tool_execution_start",
                    _ => "other",
                },
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn a_view_attached_mid_turn_gets_the_starts_it_missed() {
        let agent = |event| SessionEvent::Agent(Box::new(event));
        let assistant =
            AgentMessage::Assistant(super_ai::AssistantMessage::empty("api", "mock", "mock-1"));
        let delta = || {
            agent(AgentEvent::MessageUpdate {
                assistant_event: Box::new(super_ai::AssistantEvent::TextDelta {
                    content_index: 0,
                    delta: "lo".into(),
                }),
            })
        };
        let end = || {
            agent(AgentEvent::MessageEnd {
                message: assistant.clone(),
            })
        };
        let tool_end = |id: &str| {
            agent(AgentEvent::ToolExecutionEnd {
                tool_call_id: id.into(),
                tool_name: "bash".into(),
                result: Default::default(),
                is_error: false,
            })
        };

        // Attached in the middle of a reply: one start before the first delta.
        let mut turn = TurnTracker::default();
        assert_eq!(kinds(&turn.missing_starts(&delta())), ["message_start"]);
        assert!(turn.missing_starts(&delta()).is_empty());
        assert!(turn.missing_starts(&end()).is_empty());
        // Attached just before the reply ended: its start comes with it.
        let mut turn = TurnTracker::default();
        assert_eq!(kinds(&turn.missing_starts(&end())), ["message_start"]);
        // Attached during a tool call: it gets its line, once.
        assert_eq!(
            kinds(&turn.missing_starts(&tool_end("call_1"))),
            ["tool_execution_start"]
        );
        assert!(
            turn.missing_starts(&agent(AgentEvent::ToolExecutionStart {
                tool_call_id: "call_2".into(),
                tool_name: "bash".into(),
                args: Value::Null,
            }))
            .is_empty()
        );
        assert!(turn.missing_starts(&tool_end("call_2")).is_empty());
        // A whole turn seen from its start needs nothing.
        let mut turn = TurnTracker::default();
        for event in [
            agent(AgentEvent::MessageStart {
                message: assistant.clone(),
            }),
            delta(),
            end(),
            agent(AgentEvent::AgentEnd {
                messages: Vec::new(),
            }),
        ] {
            assert!(turn.missing_starts(&event).is_empty());
        }
    }

    #[test]
    fn queue_previews_match_what_queue_update_shows() {
        let long = "x".repeat(200);
        assert_eq!(preview(&long).chars().count(), 80);
        assert_eq!(preview("short"), "short");
    }
}
