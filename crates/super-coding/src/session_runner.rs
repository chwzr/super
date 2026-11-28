//! AgentSession: the high-level facade every mode drives. Owns the session
//! manager, tool set, queues, retry, auto-compaction, and cost accounting.

use crate::agents::registry::{AgentLink, AgentRegistry, MAX_DEPTH, StoredAgent};
use crate::agents::{AgentObserver, AgentResources, AgentType, AgentTypeSummary};
use crate::child_turn::{ForkTurns, fork_messages};
use crate::compaction::{
    self, estimate_context_tokens, extract_file_ops, file_ops_details, plan_compaction,
    should_compact,
};
use crate::iterative::IterativeRuntime;
use crate::notifications::Priority;
use crate::session::manager::{SUBAGENT_ENTRY, SessionManager};
use crate::settings::{
    CacheWarmingMode, CompactionMode, Mode, QueueMode, ReasoningEffortMode, Settings,
};
use crate::tasks::{BackgroundTasks, TaskInfo, TaskStopError};
use crate::workflows::{WorkflowApprover, WorkflowRuntime};
use anyhow::Context as _;
use futures::future::BoxFuture;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use super_agent::{
    AgentContext, AgentEvent, AgentLoopConfig, AgentMessage, DynTool, EventSink, FileStates,
    ShellDirectory, StreamFn, ToolSession, TurnUpdate,
};
use super_ai::{Model, Registry, StopReason, ThinkingLevel, Usage};
use tokio_util::sync::CancellationToken;

/// Harness-level events layered over the loop's AgentEvents.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Agent(Box<AgentEvent>),
    QueueUpdate {
        steering: Vec<String>,
        follow_up: Vec<String>,
    },
    CompactionStart {
        auto: bool,
    },
    CompactionEnd {
        summary: String,
        tokens_before: u64,
        error: Option<String>,
    },
    Retry {
        attempt: u32,
        max: u32,
        delay_ms: u64,
        error: String,
    },
    ModelChanged {
        provider: String,
        model_id: String,
    },
    ReasoningEffortChanged {
        level: ThinkingLevel,
        generations: u8,
    },
    ReasoningEffortFallback {
        level: ThinkingLevel,
        reason: String,
    },
    /// A dynamic workflow changed. The terminal redraws from the shared
    /// snapshot. The event carries only the cheap version marker.
    Workflow {
        run: crate::workflows::RunId,
        version: u64,
    },
    /// The verified result of a workflow tool call. Frontends show this after
    /// the assistant answer so model text cannot replace the real run state.
    WorkflowOutcome {
        run: Option<crate::workflows::RunId>,
        name: String,
        status: WorkflowTurnStatus,
    },
    /// A loop or autoresearch job changed. The terminal reads its snapshot.
    Iterative {
        job: crate::iterative::JobId,
        version: u64,
    },
    /// A pooled account hit its rate limit and the request moved to the
    /// next account. `index` is zero-based; `total` is the pool size.
    AccountSwitched(super_ai::auth::accounts::AccountSwitch),
    /// A background task was registered; see [`crate::tasks`].
    TaskStarted(TaskInfo),
    /// A running background task changed its progress line.
    TaskUpdated(TaskInfo),
    /// A background task ended: completed, failed or stopped.
    TaskEnded(TaskInfo),
}

pub type SessionEventSink = Arc<dyn Fn(SessionEvent) + Send + Sync>;

/// Where a child session's events go: nowhere until a host forwards them
/// ([`AgentSession::forward_events`]).
type EventRelay = Arc<Mutex<Option<SessionEventSink>>>;

/// Runs a turn the session starts on its own, because an entry was queued
/// while it was idle ([`AgentSession::queue_entry`]). The future resolves
/// to whether a turn ran: another run may have taken the entries first.
/// A host that tracks its runs (the SDK, which announces `agent_settled`)
/// installs one with [`AgentSession::set_turn_starter`]; without one the
/// session spawns the future on the current Tokio runtime.
pub type TurnStarter = Arc<dyn Fn(BoxFuture<'static, bool>) + Send + Sync>;

/// Runs the engine of a session somewhere else, such as the SUPER daemon.
///
/// An [`AgentSession`] with a backend is a view of a session another process
/// runs: prompts, queued messages, abort, model and thinking changes and
/// compaction go to the backend, and the backend feeds the events of the
/// session back through the view's sink, so a front end renders them as if
/// they happened here. The view's [`SessionManager`] is a mirror the backend
/// keeps up to date with [`AgentSession::absorb_entries`].
pub trait SessionBackend: Send + Sync {
    /// Start a run with `prompts`, or queue them when one is running there.
    /// Resolves once the run they started or joined has settled.
    fn prompt(&self, prompts: Vec<AgentMessage>, mode: PromptMode) -> BoxFuture<'static, ()>;
    /// Queue one message as a steer, or as a follow-up after the run; starts
    /// a run when none is in flight.
    fn queue(&self, message: AgentMessage, mode: PromptMode, follow_up: bool);
    /// Take back the queued messages that were not delivered yet.
    fn reclaim_queued(&self) -> Vec<AgentMessage>;
    fn abort(&self);
    fn set_model(&self, model: &Model);
    fn set_thinking_level(&self, level: ThinkingLevel);
    /// Compact there; resolves when the compaction finished.
    fn compact(&self, custom_instructions: Option<String>) -> BoxFuture<'static, ()>;
    /// Whether a run is in flight there.
    fn is_running(&self) -> bool;
    /// Stop one background task there, by id or by name.
    fn stop_task(&self, task_id: &str);
}

/// Why an operation that only works on a session this process runs refused.
pub const REMOTE_SESSION_REFUSAL: &str =
    "not available while this session runs in the SUPER daemon";

/// What one submitted user turn asked for.
///
/// Workflow mode is explicit and local to the prompt invocation: the user
/// asked for the task to run as a dynamic workflow, and the turn's requests
/// say so (the placeholder `Workflow.opt_in`). The `Workflow` tool itself is
/// offered with every request while dynamic workflows are on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PromptMode {
    #[default]
    Ordinary,
    Workflow,
}

/// The result that the workflow runtime, rather than the model, observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowTurnStatus {
    Cancelled,
    Completed,
    Failed,
    Stopped,
}

#[derive(Clone)]
struct QueuedPrompt {
    message: AgentMessage,
    mode: PromptMode,
    /// Queued with [`AgentSession::queue_entry`]: it starts a turn while the
    /// session is idle, keeps the active prompt mode, is not shown as queued
    /// input and is not handed back to the editor.
    starts_turn: bool,
}

/// How [`AgentSession::create_child`] builds a child session from its
/// session manager.
struct ChildSpec {
    /// Start with a copy of what the parent read.
    copy_file_state: bool,
    model: Model,
    thinking: ThinkingLevel,
    system_prompt: String,
    /// The child's base tools, before they are bound to it.
    tools: Vec<DynTool>,
    settings: Settings,
}

/// What a workflow run or a job asks of
/// [`AgentSession::create_subagent_session`].
pub(crate) struct SubagentSpec<'a> {
    /// The name clients see the child by.
    pub name: &'a str,
    /// What the child's system prompt calls it.
    pub canonical_path: &'a str,
    pub fork_turns: ForkTurns,
    /// A model of the registry; `None` is this session's.
    pub model: Option<&'a str>,
    /// A thinking level; `None` is the model's or this session's.
    pub effort: Option<&'a str>,
    /// The workflow run or job it belongs to.
    pub origin: crate::agents::AgentOrigin,
}

impl<'a> SubagentSpec<'a> {
    /// A child named `name`, called `canonical_path`, that starts with
    /// `fork_turns` of the conversation on this session's model.
    pub(crate) fn new(
        name: &'a str,
        canonical_path: &'a str,
        fork_turns: ForkTurns,
        origin: crate::agents::AgentOrigin,
    ) -> Self {
        SubagentSpec {
            name,
            canonical_path,
            fork_turns,
            model: None,
            effort: None,
            origin,
        }
    }
}

/// An agent [`AgentSession::create_agent_session`] makes.
pub(crate) struct AgentChild<'a> {
    pub id: &'a str,
    pub agent_type: &'a AgentType,
    pub description: &'a str,
    pub model: Model,
    pub thinking: ThinkingLevel,
    /// 1 for an agent of the main session.
    pub depth: usize,
    pub tool_use_id: &'a str,
    /// The worktree it works in, for `isolation: "worktree"`.
    pub worktree: Option<&'a crate::worktree::Worktree>,
}

/// What an agent's session is built from, whether the agent starts now or
/// its stored session is loaded.
struct AgentSetup<'a> {
    id: &'a str,
    agent_type: &'a AgentType,
    model: Model,
    thinking: ThinkingLevel,
    depth: usize,
    /// The folder it works in, when it is not this session's: its worktree.
    folder: Option<&'a Path>,
    /// Start from what this session read: a fork that starts now.
    copy_file_state: bool,
}

pub struct TreeNavigationOutcome {
    pub editor_text: Option<String>,
    pub summarized: bool,
    pub cancelled: bool,
}

#[derive(Debug, Clone)]
pub struct EphemeralResponse {
    pub text: String,
    pub usage: Usage,
}

/// One line on why a helper request came back without text.
fn empty_answer_reason(assistant: &super_ai::AssistantMessage, budget: u64) -> String {
    if assistant.stop_reason != StopReason::Length {
        return "the provider returned an empty answer".into();
    }
    match assistant.usage.reasoning {
        Some(reasoning) if reasoning > 0 => format!(
            "the model spent its {budget}-token budget on reasoning ({reasoning} tokens) and returned no text"
        ),
        _ => format!("the model reached its {budget}-token budget before answering"),
    }
}

/// `prompt` for a session that works in `folder`: its last working
/// directory line names `folder` instead. A prompt without one is returned
/// as it is.
fn in_folder(prompt: &str, folder: &Path) -> String {
    const LINE: &str = "Current working directory: ";
    let Some(start) = prompt.rfind(LINE) else {
        return prompt.to_string();
    };
    let value = start + LINE.len();
    let end = prompt[value..]
        .find('\n')
        .map_or(prompt.len(), |offset| value + offset);
    format!(
        "{}{}{}",
        &prompt[..value],
        folder.display().to_string().replace('\\', "/"),
        &prompt[end..]
    )
}

/// `tools` as they work in `folder` ([`super_agent::AgentTool::in_directory`]);
/// a tool that does not depend on a folder stays as it is.
fn in_directory(tools: Vec<DynTool>, folder: &Path) -> Vec<DynTool> {
    tools
        .into_iter()
        .map(|tool| tool.in_directory(folder).unwrap_or(tool))
        .collect()
}

const SESSION_TITLE_MAX_CHARS: usize = 36;
/// Output budget for the one retry of a helper request whose first, tight
/// budget was spent on reasoning the model would not switch off.
const EPHEMERAL_REASONING_BUDGET: u64 = 4096;
const SESSION_TITLE_PROMPT_MAX_BYTES: usize = 960;

fn bounded_session_title_prompt(prompt: &str) -> &str {
    let prompt = prompt.trim();
    let end = prompt.floor_char_boundary(SESSION_TITLE_PROMPT_MAX_BYTES);
    &prompt[..end]
}

fn normalize_session_title(text: &str) -> Option<String> {
    let line = text.lines().find(|line| !line.trim().is_empty())?;
    let cleaned: String = line
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let normalized = cleaned
        .trim()
        .trim_matches(|character| matches!(character, '"' | '\'' | '`' | '“' | '”' | '‘' | '’'))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let title: String = normalized.chars().take(SESSION_TITLE_MAX_CHARS).collect();
    let title = title
        .trim_end_matches(['.', '?', '!', ',', ':', ';'])
        .trim();
    (!title.is_empty()).then(|| title.to_string())
}

/// Which of the tools a session adds itself the host's tool list lets
/// through, by name: the agent tools, `Monitor`, `TaskStop`, `Workflow`,
/// `Skill`, `ScheduleWakeup`, `EnterWorktree` and `ExitWorktree`. They are
/// offered as the mode and the switches say, unless an allowlist (`--tools`)
/// leaves them out or they are named to be left out (`--exclude-tools`, a
/// `-Name` entry of `defaultTools`). The default lets every one through.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolSelection {
    /// Only these, when given.
    pub allow: Option<Vec<String>>,
    /// Never these.
    pub exclude: Vec<String>,
}

impl ToolSelection {
    /// Whether the tool `name` may be offered.
    pub fn admits(&self, name: &str) -> bool {
        self.allow
            .as_ref()
            .is_none_or(|allow| allow.iter().any(|allowed| allowed == name))
            && !self.exclude.iter().any(|excluded| excluded == name)
    }

    /// This selection for an agent of a type that gets `choice` of the
    /// tools: a type that names its tools gets those and nothing else, one
    /// that leaves tools out goes without them.
    pub fn narrowed(&self, choice: &crate::agents::ToolChoice) -> ToolSelection {
        use crate::agents::ToolChoice;
        let mut narrowed = self.clone();
        match choice {
            ToolChoice::All => {}
            ToolChoice::AllExcept(names) => narrowed.exclude.extend(names.iter().cloned()),
            ToolChoice::Only(names) => {
                narrowed.allow = Some(
                    names
                        .iter()
                        .filter(|name| self.admits(name))
                        .cloned()
                        .collect(),
                );
            }
        }
        narrowed
    }
}

pub struct AgentSession {
    pub manager: Mutex<SessionManager>,
    /// Read with [`AgentSession::registry`]; a host replaces it when the
    /// catalog changed under a session that keeps running.
    registry: RwLock<Arc<Registry>>,
    base_tools: Mutex<Vec<DynTool>>,
    session_tools: Mutex<Vec<DynTool>>,
    tools: Mutex<Vec<DynTool>>,
    settings: Mutex<Settings>,
    system_prompt: Mutex<String>,
    model: Mutex<Model>,
    thinking: Mutex<ThinkingLevel>,
    fast_mode: Mutex<bool>,
    steering: Arc<Mutex<VecDeque<QueuedPrompt>>>,
    follow_up: Arc<Mutex<VecDeque<QueuedPrompt>>>,
    cancel: Mutex<CancellationToken>,
    cache_warm_cancel: Mutex<CancellationToken>,
    running: Mutex<bool>,
    totals: Mutex<Usage>,
    context_usage_cache: Mutex<Option<(u64, u64)>>,
    context_file: Mutex<Option<crate::context_file::ContextFile>>,
    api_key_override: Option<(String, String)>,
    sink: SessionEventSink,
    /// The agents this session started; set when it may start agents (see
    /// [`AgentSession::agent_registry`]).
    agents: OnceLock<Arc<AgentRegistry>>,
    /// Set for an agent's own session: who started it.
    agent_link: OnceLock<AgentLink>,
    /// What this session's agents start from besides the session.
    agent_resources: Mutex<AgentResources>,
    /// `Agent`, `SendMessage` and `ListAgents`, bound to this session.
    agent_tools: OnceLock<Vec<DynTool>>,
    /// Hears of this session's agent sessions; see [`AgentObserver`].
    agent_observer: Mutex<Option<Arc<dyn AgentObserver>>>,
    /// Set for a child session: where its events go.
    event_relay: OnceLock<EventRelay>,
    workflows: OnceLock<Arc<WorkflowRuntime>>,
    iterative: OnceLock<Arc<IterativeRuntime>>,
    workflow_approver: Mutex<Option<WorkflowApprover>>,
    /// Optional replacement for the provider streaming function. Embedders and
    /// tests install one to run the whole loop against a scripted fake model.
    stream_fn: Mutex<Option<StreamFn>>,
    /// Set while the engine runs elsewhere; see [`SessionBackend`].
    backend: Mutex<Option<Arc<dyn SessionBackend>>>,
    /// This session's background tasks; set once at construction.
    tasks: OnceLock<Arc<BackgroundTasks>>,
    /// What this session's tools keep per session (what was read of each
    /// file); set once at construction, and every tool is bound to it.
    tool_session: OnceLock<ToolSession>,
    /// `Monitor` and `TaskStop`, which work on this session's background
    /// tasks; offered alongside `Bash`.
    task_tools: OnceLock<Vec<DynTool>>,
    /// `Skill`, which loads this session's skills; offered in the advanced
    /// mode alongside `Read` or `Bash`, or with the `Workflow` tool.
    skill_tool: OnceLock<DynTool>,
    /// What the session's process last reported about its tasks, while the
    /// engine runs elsewhere.
    remote_tasks: Mutex<Vec<TaskInfo>>,
    turn_starter: Mutex<Option<TurnStarter>>,
    /// Bumped when a run ends, an entry is queued or a task ends, so
    /// [`AgentSession::wait_for_background_work`] looks again.
    activity: tokio::sync::watch::Sender<u64>,
    /// The worktree the session moved into with `EnterWorktree`; its tools,
    /// its agents and its prompt's working-directory line use it while set.
    worktree: Mutex<Option<crate::worktree::EnteredWorktree>>,
    /// `ScheduleWakeup`, `EnterWorktree` and `ExitWorktree`, bound to this
    /// session; offered in the advanced mode to a session that is no child.
    session_folder_tools: OnceLock<Vec<DynTool>>,
    /// This session's pending wake-up and its self-paced loop.
    wakeups: OnceLock<Arc<crate::wakeup::Wakeups>>,
    /// Which of the tools the session adds itself the host's tool list lets
    /// through; see [`ToolSelection`].
    tool_selection: Mutex<ToolSelection>,
    /// Built as the session of a child: an agent, a workflow agent or a
    /// loop iteration.
    child: bool,
    /// The tools or the prompt changed during a run (the session moved into
    /// or out of a worktree): the run builds its context again before its
    /// next model request.
    context_stale: std::sync::atomic::AtomicBool,
}

struct ReasoningRunState {
    lease: crate::jev::ReasoningLease,
    provider: String,
    model_id: String,
    saved_effort: ThinkingLevel,
}

impl ReasoningRunState {
    fn checkpoint(
        &mut self,
        model: &Model,
        saved_effort: ThinkingLevel,
        current_effort: ThinkingLevel,
        dynamic_enabled: bool,
    ) -> (bool, Option<ThinkingLevel>) {
        let model_changed = self.provider != model.provider || self.model_id != model.id;
        if model_changed || self.saved_effort != saved_effort || !dynamic_enabled {
            self.lease.clear();
        }
        self.provider.clone_from(&model.provider);
        self.model_id.clone_from(&model.id);
        self.saved_effort = saved_effort;
        if self
            .lease
            .current()
            .is_some_and(|level| level != current_effort)
        {
            self.lease.clear();
        }
        (model_changed, self.lease.current())
    }
}

impl AgentSession {
    pub(crate) fn emit_iterative(&self, job: crate::iterative::JobId, version: u64) {
        (self.sink)(SessionEvent::Iterative { job, version });
    }

    pub(crate) fn emit_workflow(&self, run: crate::workflows::RunId, version: u64) {
        (self.sink)(SessionEvent::Workflow { run, version });
    }

    pub(crate) fn emit_workflow_outcome(
        &self,
        run: Option<crate::workflows::RunId>,
        name: String,
        status: WorkflowTurnStatus,
    ) {
        (self.sink)(SessionEvent::WorkflowOutcome { run, name, status });
    }

    pub(crate) fn emit_event(&self, event: SessionEvent) {
        (self.sink)(event);
    }

    pub(crate) fn note_activity(&self) {
        self.activity
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        manager: SessionManager,
        tools: Vec<DynTool>,
        registry: impl Into<Arc<Registry>>,
        settings: Settings,
        system_prompt: String,
        model: Model,
        thinking: ThinkingLevel,
        api_key_override: Option<(String, String)>,
        sink: SessionEventSink,
    ) -> Arc<Self> {
        Self::new_with_subagents_allowed(
            manager,
            tools,
            registry,
            settings,
            system_prompt,
            model,
            thinking,
            api_key_override,
            sink,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_subagents_allowed(
        manager: SessionManager,
        tools: Vec<DynTool>,
        registry: impl Into<Arc<Registry>>,
        settings: Settings,
        system_prompt: String,
        model: Model,
        thinking: ThinkingLevel,
        api_key_override: Option<(String, String)>,
        sink: SessionEventSink,
        subagents_allowed: bool,
    ) -> Arc<Self> {
        Self::construct(
            manager,
            tools,
            registry.into(),
            settings,
            system_prompt,
            model,
            thinking,
            api_key_override,
            sink,
            subagents_allowed,
            false,
        )
    }

    /// A session; `child` for a child agent's, whose shell starts every
    /// command in the session folder instead of where a `cd` left it.
    #[allow(clippy::too_many_arguments)]
    fn construct(
        manager: SessionManager,
        tools: Vec<DynTool>,
        registry: Arc<Registry>,
        settings: Settings,
        system_prompt: String,
        model: Model,
        thinking: ThinkingLevel,
        api_key_override: Option<(String, String)>,
        sink: SessionEventSink,
        subagents_allowed: bool,
        child: bool,
    ) -> Arc<Self> {
        let totals = manager.usage_totals();
        let session = Arc::new(AgentSession {
            manager: Mutex::new(manager),
            registry: RwLock::new(registry),
            base_tools: Mutex::new(tools.clone()),
            session_tools: Mutex::new(Vec::new()),
            tools: Mutex::new(tools),
            settings: Mutex::new(settings),
            system_prompt: Mutex::new(system_prompt),
            model: Mutex::new(model),
            thinking: Mutex::new(thinking),
            fast_mode: Mutex::new(false),
            steering: Default::default(),
            follow_up: Default::default(),
            cancel: Mutex::new(CancellationToken::new()),
            cache_warm_cancel: Mutex::new(CancellationToken::new()),
            running: Mutex::new(false),
            totals: Mutex::new(totals),
            context_usage_cache: Default::default(),
            context_file: Mutex::new(None),
            api_key_override,
            sink,
            agents: OnceLock::new(),
            agent_link: OnceLock::new(),
            agent_resources: Mutex::new(AgentResources::default()),
            agent_tools: OnceLock::new(),
            agent_observer: Mutex::new(None),
            event_relay: OnceLock::new(),
            workflows: OnceLock::new(),
            iterative: OnceLock::new(),
            workflow_approver: Mutex::new(None),
            stream_fn: Mutex::new(None),
            backend: Mutex::new(None),
            tasks: OnceLock::new(),
            tool_session: OnceLock::new(),
            task_tools: OnceLock::new(),
            skill_tool: OnceLock::new(),
            remote_tasks: Mutex::new(Vec::new()),
            turn_starter: Mutex::new(None),
            activity: tokio::sync::watch::channel(0).0,
            worktree: Mutex::new(None),
            session_folder_tools: OnceLock::new(),
            wakeups: OnceLock::new(),
            tool_selection: Mutex::new(ToolSelection::default()),
            child,
            context_stale: std::sync::atomic::AtomicBool::new(false),
        });
        assert!(
            session
                .tasks
                .set(BackgroundTasks::new(Arc::downgrade(&session)))
                .is_ok()
        );
        assert!(
            session
                .wakeups
                .set(crate::wakeup::Wakeups::new(Arc::downgrade(&session)))
                .is_ok()
        );
        let room_of = Arc::downgrade(&session);
        let tool_session = ToolSession {
            files: Arc::new(FileStates::new()),
            context_room: Some(Arc::new(move || {
                let session = room_of.upgrade()?;
                let (used, window) = session.context_usage();
                Some(window.saturating_sub(used))
            })),
            // A `cd` carries over in the main session only.
            shell: (!child).then(|| Arc::new(ShellDirectory::new())),
            tasks: Some(session.background_tasks().host()),
        };
        assert!(session.tool_session.set(tool_session).is_ok());
        let this = Arc::downgrade(&session);
        let task_tools: Vec<DynTool> = vec![
            Arc::new(crate::tools::monitor::MonitorTool::new(this.clone())),
            Arc::new(crate::tools::task_stop::TaskStopTool::new(this)),
        ];
        assert!(session.task_tools.set(task_tools).is_ok());
        let skill_tool: DynTool = Arc::new(crate::tools::skill::SkillTool::new(Arc::downgrade(
            &session,
        )));
        assert!(session.skill_tool.set(skill_tool).is_ok());
        let agent_tools = crate::agents::tools::agent_tools(Arc::downgrade(&session));
        assert!(session.agent_tools.set(agent_tools).is_ok());
        let this = Arc::downgrade(&session);
        let session_folder_tools: Vec<DynTool> = vec![
            Arc::new(crate::tools::schedule_wakeup::ScheduleWakeupTool::new(
                this.clone(),
            )),
            Arc::new(crate::tools::worktree::EnterWorktreeTool::new(this.clone())),
            Arc::new(crate::tools::worktree::ExitWorktreeTool::new(this)),
        ];
        assert!(
            session
                .session_folder_tools
                .set(session_folder_tools)
                .is_ok()
        );
        if subagents_allowed {
            let agents = AgentRegistry::new(Arc::downgrade(&session), 0);
            // A session loaded again knows the agents it had.
            agents.restore();
            assert!(session.agents.set(agents).is_ok());
            let workflows = WorkflowRuntime::new(Arc::downgrade(&session));
            assert!(session.workflows.set(workflows).is_ok());
            let iterative = IterativeRuntime::new(Arc::downgrade(&session));
            assert!(session.iterative.set(iterative).is_ok());
        }
        session.rebuild_tools();
        session
    }

    /// The model catalog this session resolves models and credentials with.
    pub fn registry(&self) -> Arc<Registry> {
        self.registry.read().unwrap().clone()
    }

    /// Replace the model catalog. A session outlives changes to it (a
    /// sign-in, a provider the user adds, models an account gains), and
    /// whoever hosts the session hands it the catalog as it is now. The
    /// selected model stays; a subagent that is running keeps the catalog it
    /// started with.
    pub fn set_registry(&self, registry: impl Into<Arc<Registry>>) {
        *self.registry.write().unwrap() = registry.into();
    }

    /// Replace the provider streaming function used by every later request.
    ///
    /// Pass `None` to restore the real provider. Child sessions created after
    /// this call inherit the override so a scripted fake model also serves
    /// subagents and compaction.
    pub fn set_stream_fn(&self, stream_fn: Option<StreamFn>) {
        *self.stream_fn.lock().unwrap() = stream_fn;
    }

    /// Hand the engine to `backend`, or take it back with `None`. While a
    /// backend is set this session runs nothing itself: see
    /// [`SessionBackend`].
    pub fn set_backend(&self, backend: Option<Arc<dyn SessionBackend>>) {
        *self.backend.lock().unwrap() = backend;
    }

    /// The backend running this session's engine, when another process does.
    pub fn backend(&self) -> Option<Arc<dyn SessionBackend>> {
        self.backend.lock().unwrap().clone()
    }

    /// Whether the engine runs elsewhere.
    pub fn is_remote(&self) -> bool {
        self.backend.lock().unwrap().is_some()
    }

    /// Add entries the process running this session appended, and follow
    /// what they change: model, thinking level, usage totals.
    pub fn absorb_entries(&self, entries: Vec<crate::SessionEntry>, leaf_id: Option<String>) {
        let (totals, context) = {
            let mut manager = self.manager.lock().unwrap();
            manager.absorb(entries, leaf_id);
            (manager.usage_totals(), manager.build_session_context())
        };
        if let Some((provider, model_id)) = context.model
            && let Some((model, _)) = self.registry().resolve(&model_id, Some(&provider))
        {
            let mut current = self.model.lock().unwrap();
            if current.provider != model.provider || current.id != model.id {
                *current = model;
            }
        }
        if let Some(thinking) = context.thinking_level {
            *self.thinking.lock().unwrap() = thinking;
        }
        *self.context_usage_cache.lock().unwrap() = None;
        *self.totals.lock().unwrap() = totals;
    }

    /// Take the model and thinking level the process running this session
    /// reports, without recording a change: it recorded its own.
    pub fn adopt_remote_state(&self, model: Option<Model>, thinking: Option<ThinkingLevel>) {
        if let Some(model) = model {
            *self.model.lock().unwrap() = model;
        }
        if let Some(thinking) = thinking {
            *self.thinking.lock().unwrap() = thinking;
        }
        *self.context_usage_cache.lock().unwrap() = None;
    }

    pub fn model(&self) -> Model {
        self.model.lock().unwrap().clone()
    }

    pub fn thinking_level(&self) -> ThinkingLevel {
        *self.thinking.lock().unwrap()
    }

    pub fn fast_mode(&self) -> bool {
        *self.fast_mode.lock().unwrap()
    }

    pub fn set_fast_mode(&self, enabled: bool) {
        *self.fast_mode.lock().unwrap() = enabled;
    }

    pub fn totals(&self) -> Usage {
        *self.totals.lock().unwrap()
    }

    pub(crate) fn record_subagent_usage(&self, usage: Usage) {
        self.totals.lock().unwrap().add(&usage);
    }

    pub fn is_running(&self) -> bool {
        *self.running.lock().unwrap() || self.backend().is_some_and(|backend| backend.is_running())
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// The mode this session runs in, from the `mode` setting. It follows
    /// [`AgentSession::update_settings`], and everything that differs between
    /// the basic and the advanced mode asks here.
    pub fn mode(&self) -> Mode {
        self.settings.lock().unwrap().mode
    }

    pub fn update_settings(&self, settings: Settings) {
        *self.context_usage_cache.lock().unwrap() = None;
        let was_enabled = self.subagents_enabled();
        let workflows_were_enabled = self.workflows_enabled();
        let cache_warming = settings.cache_warming;
        *self.settings.lock().unwrap() = settings;
        if cache_warming == CacheWarmingMode::Off
            || (cache_warming == CacheWarmingMode::Streaming && !self.is_running())
        {
            self.cache_warm_cancel.lock().unwrap().cancel();
        }
        let is_enabled = self.subagents_enabled();
        if was_enabled && !is_enabled {
            self.stop_child_work();
        } else if workflows_were_enabled && !self.workflows_enabled() {
            self.stop_workflows();
        }
        self.rebuild_tools();
    }

    /// The system prompt the next model request starts from, before the
    /// parts the session adds itself for the tools it offers.
    pub fn system_prompt(&self) -> String {
        self.system_prompt.lock().unwrap().clone()
    }

    /// Replace the system prompt the next model request starts from, for
    /// example after the mode changed.
    pub fn set_system_prompt(&self, system_prompt: String) {
        *self.context_usage_cache.lock().unwrap() = None;
        *self.system_prompt.lock().unwrap() = system_prompt;
    }

    /// Replace resources used by the next model request.
    pub fn reload_runtime(&self, settings: Settings, system_prompt: String, tools: Vec<DynTool>) {
        *self.context_usage_cache.lock().unwrap() = None;
        let was_enabled = self.subagents_enabled();
        let workflows_were_enabled = self.workflows_enabled();
        let cache_warming = settings.cache_warming;
        *self.settings.lock().unwrap() = settings;
        *self.system_prompt.lock().unwrap() = system_prompt;
        *self.base_tools.lock().unwrap() = tools;
        if cache_warming == CacheWarmingMode::Off
            || (cache_warming == CacheWarmingMode::Streaming && !self.is_running())
        {
            self.cache_warm_cancel.lock().unwrap().cancel();
        }
        let is_enabled = self.subagents_enabled();
        if was_enabled && !is_enabled {
            self.stop_child_work();
        } else if workflows_were_enabled && !self.workflows_enabled() {
            self.stop_workflows();
        }
        self.rebuild_tools();
    }

    /// Add or replace one tool for this process without changing saved tool configuration.
    pub fn install_session_tool(&self, tool: DynTool) {
        let mut tools = self.session_tools.lock().unwrap();
        if let Some(existing) = tools
            .iter_mut()
            .find(|existing| existing.name() == tool.name())
        {
            *existing = tool;
        } else {
            tools.push(tool);
        }
        drop(tools);
        self.rebuild_tools();
    }

    /// Remove one process-only tool. Return true when a tool was removed.
    pub fn remove_session_tool(&self, name: &str) -> bool {
        let mut tools = self.session_tools.lock().unwrap();
        let previous_len = tools.len();
        tools.retain(|tool| tool.name() != name);
        let removed = tools.len() != previous_len;
        drop(tools);
        if removed {
            self.rebuild_tools();
        }
        removed
    }

    /// Stop every child agent and workflow run this session started.
    fn stop_child_work(&self) {
        if let Some(agents) = self.agents.get() {
            agents.stop_all();
        }
        if let Some(runtime) = self.workflows.get() {
            runtime.stop_all();
        }
        if let Some(runtime) = self.iterative.get() {
            runtime.stop_all();
        }
    }

    fn stop_workflows(&self) {
        if let Some(runtime) = self.workflows.get() {
            runtime.stop_all();
        }
    }

    /// Whether this session offers the agent tools: it may start agents,
    /// runs in the advanced mode and has the Subagents switch on. The basic
    /// mode has no child agents, whatever the switch says.
    pub(crate) fn subagents_enabled(&self) -> bool {
        self.agents.get().is_some()
            && self.mode().is_advanced()
            && self.settings.lock().unwrap().subagents.enabled
    }

    /// Dynamic workflows are built on child agents, so they need subagents on
    /// as well as their own setting.
    pub fn workflows_enabled(&self) -> bool {
        self.subagents_enabled() && self.settings.lock().unwrap().workflows.enabled
    }

    /// Select workflow mode from one submitted user prompt.
    pub fn prompt_mode_for(&self, text: &str) -> PromptMode {
        if self.workflows_enabled()
            && self.settings.lock().unwrap().workflows.keyword_trigger
            && crate::workflows::workflow_trigger(text).is_some()
        {
            PromptMode::Workflow
        } else {
            PromptMode::Ordinary
        }
    }

    pub fn workflows(&self) -> Option<Arc<WorkflowRuntime>> {
        self.workflows.get().cloned()
    }

    pub fn iterative_jobs(&self) -> Option<Arc<IterativeRuntime>> {
        self.iterative.get().cloned()
    }

    /// Install the callback that asks the user to approve a run.
    pub fn set_workflow_approver(&self, approver: WorkflowApprover) {
        *self.workflow_approver.lock().unwrap() = Some(approver);
    }

    pub(crate) fn workflow_approver(&self) -> Option<WorkflowApprover> {
        self.workflow_approver.lock().unwrap().clone()
    }

    fn rebuild_tools(&self) {
        let mut tools = self.base_tools.lock().unwrap().clone();
        for session_tool in self.session_tools.lock().unwrap().iter().cloned() {
            if let Some(existing) = tools
                .iter_mut()
                .find(|existing| existing.name() == session_tool.name())
            {
                *existing = session_tool;
            } else {
                tools.push(session_tool);
            }
        }
        // In a worktree the tools work there.
        if let Some(folder) = self.worktree_folder() {
            tools = in_directory(tools, &folder);
        }
        // The host chose these; what follows the session adds itself.
        let chosen = tools.len();
        let agents_offered = self.subagents_enabled();
        if let Some(agent_tools) = self.agent_tools.get() {
            if agents_offered {
                tools.extend(agent_tools.iter().cloned());
            } else if self.agent_link.get().is_some() {
                // An agent that starts no agents still writes to main.
                tools.extend(
                    agent_tools
                        .iter()
                        .filter(|tool| tool.name() == "SendMessage")
                        .cloned(),
                );
            }
        }
        // Monitor runs commands as Bash does, and TaskStop stops what they
        // or the agents leave running.
        let has_bash = tools.iter().any(|tool| tool.name() == "Bash");
        if let Some(task_tools) = self.task_tools.get() {
            tools.extend(
                task_tools
                    .iter()
                    .filter(|tool| match tool.name() {
                        "Monitor" => has_bash,
                        _ => has_bash || agents_offered,
                    })
                    .cloned(),
            );
        }
        // A workflow runs in the background of the main session; its tool is
        // offered with every request while dynamic workflows are on.
        let workflows_offered = self.workflows_enabled();
        if workflows_offered && let Some(runtime) = self.workflows.get() {
            tools.push(runtime.tool());
        }
        // Skills tell the model how to work with files and commands, as the
        // basic mode's list does for `Read` or `Bash`; the guide to writing
        // a workflow is a skill too.
        let works_with_files = tools
            .iter()
            .any(|tool| matches!(tool.name(), "Read" | "Bash"));
        if (works_with_files || workflows_offered)
            && let Some(skill_tool) = self.skill_tool.get()
        {
            tools.push(skill_tool.clone());
        }
        // Wake-ups and worktrees belong to the session the user works in, not
        // to a child; like the skills they come with the file tools.
        if works_with_files
            && !self.child
            && let Some(session_folder_tools) = self.session_folder_tools.get()
        {
            tools.extend(session_folder_tools.iter().cloned());
        }
        let selection = self.tool_selection();
        let mut position = 0;
        tools.retain(|tool| {
            position += 1;
            position <= chosen || selection.admits(tool.name())
        });
        let tools = self.bound_to_session(tools);
        *self.tools.lock().unwrap() = self.offered_in_mode(tools);
    }

    /// Which of the tools this session adds itself its host lets through.
    pub fn tool_selection(&self) -> ToolSelection {
        self.tool_selection.lock().unwrap().clone()
    }

    /// Let through only what `selection` admits of the tools this session
    /// adds itself, from the next request on. The host's own tools are the
    /// ones it built; the agents this session starts follow the selection
    /// too.
    pub fn set_tool_selection(&self, selection: ToolSelection) {
        *self.tool_selection.lock().unwrap() = selection;
        self.rebuild_tools();
    }

    /// The folder this session works in: the worktree it entered with
    /// `EnterWorktree`, else its session folder.
    pub fn working_folder(&self) -> std::path::PathBuf {
        self.worktree_folder()
            .unwrap_or_else(|| self.manager.lock().unwrap().cwd().to_path_buf())
    }

    /// The worktree this session works in, while it is in one.
    pub fn worktree(&self) -> Option<crate::worktree::EnteredWorktree> {
        self.worktree.lock().unwrap().clone()
    }

    fn worktree_folder(&self) -> Option<std::path::PathBuf> {
        self.worktree
            .lock()
            .unwrap()
            .as_ref()
            .map(|entered| entered.worktree.path.clone())
    }

    /// Move the session into `entered`: from the next request on, its tools
    /// work there and its prompt's working-directory line names it, and the
    /// shell starts there. `EnterWorktree` does this after it made or found
    /// the worktree.
    pub fn enter_worktree(&self, entered: crate::worktree::EnteredWorktree) {
        *self.worktree.lock().unwrap() = Some(entered);
        self.moved();
    }

    /// Move the session back to its own folder; returns the worktree it was
    /// in. The worktree itself stays as it is.
    pub fn leave_worktree(&self) -> Option<crate::worktree::EnteredWorktree> {
        let left = self.worktree.lock().unwrap().take();
        if left.is_some() {
            self.moved();
        }
        left
    }

    /// The session's folder changed: the shell starts in the new one, and
    /// the tools are built for it.
    fn moved(&self) {
        if let Some(shell) = &self.tool_session().shell {
            shell.reset();
        }
        *self.context_usage_cache.lock().unwrap() = None;
        self.rebuild_tools();
        self.context_stale
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// This session's base tools as they work in its folder, for a child it
    /// starts: in the worktree while the session is in one.
    fn base_tools_here(&self) -> Vec<DynTool> {
        let tools = self.base_tools.lock().unwrap().clone();
        match self.worktree_folder() {
            Some(folder) => in_directory(tools, &folder),
            None => tools,
        }
    }

    /// This session's pending wake-up and self-paced loop.
    pub fn wakeups(&self) -> Arc<crate::wakeup::Wakeups> {
        self.wakeups
            .get()
            .expect("every session has its wake-ups")
            .clone()
    }

    /// What this session's tools keep per session.
    pub(crate) fn tool_session(&self) -> &ToolSession {
        self.tool_session
            .get()
            .expect("the tool session is set at construction")
    }

    /// `tools` bound to this session's own state with
    /// [`super_agent::AgentTool::for_session`], so a tool list shared with a
    /// parent or child session keeps each session's reads apart.
    fn bound_to_session(&self, tools: Vec<DynTool>) -> Vec<DynTool> {
        let session = self.tool_session();
        tools
            .into_iter()
            .map(|tool| tool.for_session(session).unwrap_or(tool))
            .collect()
    }

    /// What this session has read of each file: `Read` records into it,
    /// `Edit` and `Write` check it. A child agent has its own; a fork starts
    /// with a copy of its parent's.
    pub fn file_state(&self) -> Arc<FileStates> {
        self.tool_session().files.clone()
    }

    /// The tools of `tools` this session's mode offers: the basic mode leaves
    /// out every tool that declares itself advanced-only with
    /// [`super_agent::AgentTool::advanced_only`].
    fn offered_in_mode(&self, mut tools: Vec<DynTool>) -> Vec<DynTool> {
        if !self.mode().is_advanced() {
            tools.retain(|tool| !tool.advanced_only());
        }
        tools
    }

    pub fn available_tool_names(&self) -> Vec<String> {
        self.tools
            .lock()
            .unwrap()
            .iter()
            .map(|tool| tool.name().to_string())
            .collect()
    }

    /// The tools the session offers the model now, in order.
    #[cfg(test)]
    pub(crate) fn offered_tools(&self) -> Vec<DynTool> {
        self.tools.lock().unwrap().clone()
    }

    /// Switch the active session without appending synthetic history.
    pub fn replace_manager(&self, manager: SessionManager) {
        self.cache_warm_cancel.lock().unwrap().cancel();
        *self.context_file.lock().unwrap() = None;
        if let Some(agents) = self.agents.get() {
            agents.reset();
        }
        if let Some(runtime) = self.workflows.get() {
            runtime.stop_all();
        }
        if let Some(runtime) = self.iterative.get() {
            runtime.stop_all();
        }
        // Background work and what it queued belong to the conversation
        // that is left, and so does what it read, a pending wake-up and the
        // worktree it moved into; the worktree stays on disk.
        self.file_state().clear();
        if let Some(shell) = &self.tool_session().shell {
            shell.reset();
        }
        self.wakeups().reset();
        self.leave_worktree();
        self.background_tasks().reset();
        self.remote_tasks.lock().unwrap().clear();
        for queue in [&self.steering, &self.follow_up] {
            queue.lock().unwrap().retain(|prompt| !prompt.starts_turn);
        }
        let totals = manager.usage_totals();
        let context = manager.build_session_context();
        if let Some((provider, model_id)) = context.model
            && let Some((model, _)) = self.registry().resolve(&model_id, Some(&provider))
        {
            *self.model.lock().unwrap() = model;
        }
        if let Some(thinking) = context.thinking_level {
            *self.thinking.lock().unwrap() = thinking;
        }
        *self.manager.lock().unwrap() = manager;
        *self.context_usage_cache.lock().unwrap() = None;
        *self.totals.lock().unwrap() = totals;
        // The conversation it moved to may have agents of its own.
        if let Some(agents) = self.agents.get() {
            agents.restore();
        }
    }

    pub fn set_model(&self, model: Model) {
        self.cache_warm_cancel.lock().unwrap().cancel();
        if let Some(backend) = self.backend() {
            // Shown at once; the backend records the change and announces it.
            backend.set_model(&model);
            *self.model.lock().unwrap() = model;
            return;
        }
        {
            let mut m = self.manager.lock().unwrap();
            let _ = m.append_model_change(&model.provider, &model.id);
        }
        (self.sink)(SessionEvent::ModelChanged {
            provider: model.provider.clone(),
            model_id: model.id.clone(),
        });
        *self.model.lock().unwrap() = model;
    }

    pub fn set_thinking_level(&self, level: ThinkingLevel) {
        if let Some(backend) = self.backend() {
            backend.set_thinking_level(level);
            *self.thinking.lock().unwrap() = level;
            return;
        }
        let _ = self
            .manager
            .lock()
            .unwrap()
            .append_thinking_level_change(level);
        *self.thinking.lock().unwrap() = level;
    }

    /// Move the active leaf in the current session tree, with an optional
    /// summary of the branch that is no longer active.
    pub async fn navigate_tree(
        self: &Arc<Self>,
        target_id: &str,
        summarize: bool,
        custom_instructions: Option<String>,
        cancel: CancellationToken,
    ) -> anyhow::Result<TreeNavigationOutcome> {
        if self.is_remote() {
            anyhow::bail!("moving in the session tree is {REMOTE_SESSION_REFUSAL}");
        }
        let (old_leaf, new_leaf, editor_text, abandoned_messages) = {
            let manager = self.manager.lock().unwrap();
            let target = manager
                .get_entry(target_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("unknown tree entry {target_id}"))?;
            let old_leaf = manager.leaf_id().map(str::to_string);
            if old_leaf.as_deref() == Some(target_id) {
                return Ok(TreeNavigationOutcome {
                    editor_text: None,
                    summarized: false,
                    cancelled: false,
                });
            }

            let (new_leaf, editor_text) = match &target {
                crate::session::entry::SessionEntry::Message {
                    message: AgentMessage::User(user),
                    ..
                } => (
                    target.parent_id().map(str::to_string),
                    Some(user.content.as_text()),
                ),
                _ => (Some(target_id.to_string()), None),
            };

            let target_ancestors: std::collections::HashSet<String> = new_leaf
                .as_deref()
                .map(|leaf| {
                    manager
                        .branch_entries(Some(leaf))
                        .into_iter()
                        .map(|entry| entry.id().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let common_ancestor = old_leaf.as_deref().and_then(|leaf| {
                manager
                    .branch_entries(Some(leaf))
                    .into_iter()
                    .rev()
                    .find(|entry| target_ancestors.contains(entry.id()))
                    .map(|entry| entry.id().to_string())
            });
            let abandoned_messages = old_leaf
                .as_deref()
                .map(|leaf| manager.branch_messages_after(leaf, common_ancestor.as_deref()))
                .unwrap_or_default();
            (old_leaf, new_leaf, editor_text, abandoned_messages)
        };

        let summary = if summarize && !abandoned_messages.is_empty() {
            let model = self.model();
            let credential = self.resolve_credential(&model.provider).await;
            let serialized = compaction::serialize_agent_messages(&abandoned_messages);
            let result = compaction::generate_summary(
                &model,
                credential,
                &serialized,
                None,
                custom_instructions.as_deref(),
                false,
                cancel.clone(),
            )
            .await?;
            if cancel.is_cancelled() {
                return Ok(TreeNavigationOutcome {
                    editor_text: None,
                    summarized: false,
                    cancelled: true,
                });
            }
            Some(result)
        } else {
            None
        };

        let mut manager = self.manager.lock().unwrap();
        if manager.leaf_id() != old_leaf.as_deref() {
            anyhow::bail!("the session tree changed during navigation");
        }
        let summarized = if let Some(summary) = summary {
            let (read, modified) = extract_file_ops(&abandoned_messages);
            if let Some(usage) = &summary.usage {
                self.totals.lock().unwrap().add(usage);
            }
            manager.branch_with_summary(
                new_leaf.as_deref(),
                old_leaf.as_deref().unwrap_or(target_id),
                summary.summary,
                summary.usage,
                Some(file_ops_details(&read, &modified)),
            )?;
            true
        } else {
            if let Some(new_leaf) = new_leaf.as_deref() {
                manager.branch(new_leaf)?;
            } else {
                manager.reset_leaf();
            }
            false
        };

        Ok(TreeNavigationOutcome {
            editor_text,
            summarized,
            cancelled: false,
        })
    }

    pub fn queue_steering(&self, message: AgentMessage) {
        self.queue_steering_with_mode(message, PromptMode::Ordinary);
    }

    pub fn queue_steering_with_mode(&self, message: AgentMessage, mode: PromptMode) {
        if let Some(backend) = self.backend() {
            backend.queue(message, mode, false);
            return;
        }
        self.steering.lock().unwrap().push_back(QueuedPrompt {
            message,
            mode,
            starts_turn: false,
        });
        self.emit_queues();
    }

    pub fn queue_follow_up(&self, message: AgentMessage) {
        self.queue_follow_up_with_mode(message, PromptMode::Ordinary);
    }

    pub fn queue_follow_up_with_mode(&self, message: AgentMessage, mode: PromptMode) {
        if let Some(backend) = self.backend() {
            backend.queue(message, mode, true);
            return;
        }
        self.follow_up.lock().unwrap().push_back(QueuedPrompt {
            message,
            mode,
            starts_turn: false,
        });
        self.emit_queues();
    }

    /// Queue something that reaches the agent from outside a tool call: the
    /// notice of a background task, a wake-up. While the session is idle the
    /// entry starts a new turn (see [`TurnStarter`]); while it is busy a
    /// [`Priority::Next`] entry is delivered at its next step, between two
    /// tool rounds, a [`Priority::Now`] entry the same way but ahead of
    /// everything else, and a [`Priority::Later`] entry once the turn is
    /// over. The entry enters the session history when it is delivered, so
    /// a reloaded session still has it. A custom message
    /// ([`crate::notifications::task_notification`]) is shown by clients as
    /// a notice, not as something the user typed.
    ///
    /// A session whose engine runs elsewhere queues nothing: its entries
    /// come from the process that runs it.
    pub fn queue_entry(self: &Arc<Self>, message: AgentMessage, priority: Priority) {
        if self.is_remote() {
            return;
        }
        let entry = QueuedPrompt {
            message,
            mode: PromptMode::Ordinary,
            starts_turn: true,
        };
        match priority {
            Priority::Now => self.steering.lock().unwrap().push_front(entry),
            Priority::Next => self.steering.lock().unwrap().push_back(entry),
            Priority::Later => self.follow_up.lock().unwrap().push_back(entry),
        }
        self.note_activity();
        self.wake();
    }

    /// Install how the session runs the turns it starts on its own; `None`
    /// spawns them on the current Tokio runtime.
    pub fn set_turn_starter(&self, starter: Option<TurnStarter>) {
        *self.turn_starter.lock().unwrap() = starter;
    }

    /// Whether an entry from [`AgentSession::queue_entry`] waits for a turn.
    pub fn has_queued_entries(&self) -> bool {
        self.steering
            .lock()
            .unwrap()
            .iter()
            .any(|prompt| prompt.starts_turn)
            || self
                .follow_up
                .lock()
                .unwrap()
                .iter()
                .any(|prompt| prompt.starts_turn)
    }

    #[cfg(test)]
    pub(crate) fn running_flag_for_test(&self) -> std::sync::MutexGuard<'_, bool> {
        self.running.lock().unwrap()
    }

    /// Everything queued, in the order it would be delivered.
    #[cfg(test)]
    pub(crate) fn take_queued_for_test(&self) -> Vec<AgentMessage> {
        let mut out: Vec<AgentMessage> = self
            .steering
            .lock()
            .unwrap()
            .drain(..)
            .map(|prompt| prompt.message)
            .collect();
        out.extend(
            self.follow_up
                .lock()
                .unwrap()
                .drain(..)
                .map(|prompt| prompt.message),
        );
        out
    }

    /// Start a turn for queued entries when nothing runs.
    fn wake(self: &Arc<Self>) {
        if self.is_remote() || *self.running.lock().unwrap() {
            // The run in flight delivers the entries.
            return;
        }
        let turn: BoxFuture<'static, bool> = Box::pin(self.clone().run_queued());
        let starter = self.turn_starter.lock().unwrap().clone();
        match starter {
            Some(starter) => starter(turn),
            None => {
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(turn);
                }
            }
        }
    }

    /// The turn [`AgentSession::wake`] starts: the queued entries, as the
    /// queue modes release them, become the turn's prompts.
    async fn run_queued(self: Arc<Self>) -> bool {
        let settings = self.settings();
        let (prompts, mode) = {
            let mut running = self.running.lock().unwrap();
            if *running || self.is_remote() {
                return false;
            }
            let mut entries = drain_entries(&self.steering, settings.steering_mode);
            if entries.is_empty() {
                entries = drain_entries(&self.follow_up, settings.follow_up_mode);
            }
            if entries.is_empty() {
                return false;
            }
            *running = true;
            let mode = if entries
                .iter()
                .any(|entry| !entry.starts_turn && entry.mode == PromptMode::Workflow)
            {
                PromptMode::Workflow
            } else {
                PromptMode::Ordinary
            };
            (
                entries.into_iter().map(|entry| entry.message).collect(),
                mode,
            )
        };
        self.emit_queues();
        self.cache_warm_cancel.lock().unwrap().cancel();
        self.run_claimed(prompts, mode, true).await;
        true
    }

    /// This session's background tasks.
    pub fn background_tasks(&self) -> Arc<BackgroundTasks> {
        self.tasks
            .get()
            .expect("every session has a task registry")
            .clone()
    }

    /// The session's tasks, oldest first: those of the process running its
    /// engine when that is another one.
    pub fn tasks(&self) -> Vec<TaskInfo> {
        if self.is_remote() {
            return self.remote_tasks.lock().unwrap().clone();
        }
        self.background_tasks().list()
    }

    /// Stop one background task by id or by name. A session whose engine
    /// runs elsewhere asks that process and answers with what it last
    /// reported about the task.
    pub fn stop_task(&self, id_or_name: &str) -> Result<TaskInfo, TaskStopError> {
        if let Some(backend) = self.backend() {
            let task = self
                .remote_tasks
                .lock()
                .unwrap()
                .iter()
                .find(|task| task.id == id_or_name || task.name.as_deref() == Some(id_or_name))
                .cloned()
                .ok_or_else(|| TaskStopError::Unknown(id_or_name.to_string()))?;
            if !task.status.is_running() {
                return Err(TaskStopError::NotRunning {
                    id: task.id,
                    status: task.status,
                });
            }
            backend.stop_task(&task.id);
            return Ok(task);
        }
        self.background_tasks().stop(id_or_name)
    }

    /// Stop every background task this process runs; returns how many.
    pub fn stop_background_tasks(&self) -> usize {
        self.background_tasks().stop_all().len()
    }

    /// Follow a task the process running this session's engine reported.
    pub fn absorb_task(&self, task: TaskInfo) {
        let mut tasks = self.remote_tasks.lock().unwrap();
        match tasks.iter_mut().find(|known| known.id == task.id) {
            Some(known) => *known = task,
            None => tasks.push(task),
        }
    }

    /// Replace what is known of the tasks of the process running this
    /// session's engine, after attaching to it.
    pub fn replace_remote_tasks(&self, tasks: Vec<TaskInfo>) {
        *self.remote_tasks.lock().unwrap() = tasks;
    }

    /// Whether the session has work that will reach the agent later: a
    /// running task, a held wake-up, an entry waiting for a turn. A host
    /// keeps such a session loaded.
    pub fn has_background_work(&self) -> bool {
        self.background_tasks().is_busy() || self.has_queued_entries()
    }

    /// Resolve once no run is in flight, no background task runs and no
    /// queued entry waits: the turns that task notices start are part of
    /// the wait. A print-mode run waits here before it exits.
    pub async fn wait_for_background_work(&self) {
        let mut activity = self.activity.subscribe();
        loop {
            activity.borrow_and_update();
            if self.has_nothing_to_do() {
                return;
            }
            if activity.changed().await.is_err() {
                return;
            }
        }
    }

    /// Whether no run is in flight, no background task runs and no queued
    /// entry waits, as [`AgentSession::wait_for_background_work`] waits for.
    pub(crate) fn has_nothing_to_do(&self) -> bool {
        // In the order work moves through: a task counts as running until
        // its notice is queued, and a queued entry until the run that takes
        // it holds the running flag.
        !self.background_tasks().has_running()
            && !self.has_queued_entries()
            && !*self.running.lock().unwrap()
    }

    /// Drain both queues back to the caller (Escape restores to editor).
    /// Entries from [`AgentSession::queue_entry`] stay: nobody typed them.
    pub fn reclaim_queued(&self) -> Vec<AgentMessage> {
        if let Some(backend) = self.backend() {
            return backend.reclaim_queued();
        }
        let take_typed = |queue: &Mutex<VecDeque<QueuedPrompt>>| {
            let mut queue = queue.lock().unwrap();
            let (typed, kept): (VecDeque<QueuedPrompt>, VecDeque<QueuedPrompt>) =
                queue.drain(..).partition(|prompt| !prompt.starts_turn);
            *queue = kept;
            typed.into_iter().map(|prompt| prompt.message)
        };
        let mut out: Vec<AgentMessage> = take_typed(&self.steering).collect();
        out.extend(take_typed(&self.follow_up));
        self.emit_queues();
        out
    }

    /// Cancel the current turn. Background tasks keep running; a client
    /// without a stop control per task also calls
    /// [`AgentSession::stop_background_tasks`].
    pub fn abort(&self) {
        if let Some(backend) = self.backend() {
            backend.abort();
        }
        self.cancel.lock().unwrap().cancel();
    }

    fn emit_queues(&self) {
        let preview = |q: &VecDeque<QueuedPrompt>| {
            q.iter()
                .filter(|prompt| !prompt.starts_turn)
                .map(|prompt| match &prompt.message {
                    AgentMessage::User(u) => u.content.as_text().chars().take(80).collect(),
                    other => other.role().to_string(),
                })
                .collect::<Vec<String>>()
        };
        (self.sink)(SessionEvent::QueueUpdate {
            steering: preview(&self.steering.lock().unwrap()),
            follow_up: preview(&self.follow_up.lock().unwrap()),
        });
    }

    async fn resolve_credential(&self, provider: &str) -> Option<super_ai::ResolvedCredential> {
        if let Some((override_provider, key)) = &self.api_key_override
            && override_provider == provider
        {
            return Some(super_ai::ResolvedCredential::api_key(key));
        }
        let registry = self.registry();
        let credential_provider = registry.credential_provider(provider);
        super_ai::auth::resolve_credential_async(credential_provider, &registry.declared_keys)
            .await
            .ok()
            .flatten()
    }

    fn loop_config(
        &self,
        session_arc: &Arc<Self>,
        active_prompt_mode: Arc<Mutex<PromptMode>>,
    ) -> AgentLoopConfig {
        let mut config = AgentLoopConfig::new(self.model());
        config.thinking_level = self.thinking_level();
        config.fast_mode = self.fast_mode();
        config.session_id = Some(self.manager.lock().unwrap().session_id().to_string());
        let settings = self.settings();
        config.transport = settings.transport;
        let registry = self.registry();
        let declared = registry.declared_keys.clone();
        let api_key_override = self.api_key_override.clone();
        config.get_credential = Some(Arc::new(move |provider| {
            let registry = registry.clone();
            let declared = declared.clone();
            let api_key_override = api_key_override.clone();
            Box::pin(async move {
                if let Some((override_provider, key)) = api_key_override
                    && override_provider == provider
                {
                    return Some(super_ai::ResolvedCredential::api_key(key));
                }
                let credential_provider = registry.credential_provider(&provider).to_string();
                super_ai::auth::resolve_credential_async(&credential_provider, &declared)
                    .await
                    .ok()
                    .flatten()
            })
        }));

        let steering = self.steering.clone();
        let steering_for_mode = self.steering.clone();
        let steering_mode = settings.steering_mode;
        let session_for_queues = session_arc.clone();
        config.get_steering_messages = Some(Arc::new(move || {
            let drained = drain_queue(&steering, steering_mode);
            session_for_queues.emit_queues();
            Box::pin(async move { drained })
        }));
        let follow_up = self.follow_up.clone();
        let follow_up_for_mode = self.follow_up.clone();
        let follow_up_mode = settings.follow_up_mode;
        let session_for_queues = session_arc.clone();
        config.get_follow_up_messages = Some(Arc::new(move || {
            let drained = drain_queue(&follow_up, follow_up_mode);
            session_for_queues.emit_queues();
            Box::pin(async move { drained })
        }));
        let reasoning_run = Arc::new(Mutex::new(ReasoningRunState {
            lease: crate::jev::ReasoningLease::default(),
            provider: config.model.provider.clone(),
            model_id: config.model.id.clone(),
            saved_effort: config.thinking_level,
        }));
        let session_for_reasoning = session_arc.clone();
        let reasoning_for_generation = reasoning_run.clone();
        let prompt_mode_for_reasoning = active_prompt_mode.clone();
        config.prepare_generation = Some(Arc::new(move |current_effort| {
            let session = session_for_reasoning.clone();
            let reasoning_run = reasoning_for_generation.clone();
            let active_prompt_mode = prompt_mode_for_reasoning.clone();
            Box::pin(async move {
                let settings = session.settings();
                let context_file_changed = session.sync_context_file();
                let model = session.model();
                let saved_effort = session.thinking_level();
                let dynamic_enabled =
                    settings.reasoning_effort.mode == ReasoningEffortMode::Jev && model.reasoning;
                let (model_changed, leased) = {
                    let mut run = reasoning_run.lock().unwrap();
                    run.checkpoint(&model, saved_effort, current_effort, dynamic_enabled)
                };
                let mut update = TurnUpdate {
                    context: (model_changed
                        || context_file_changed
                        || settings.experimental_context_file)
                        .then(|| session.build_context_for(*active_prompt_mode.lock().unwrap())),
                    model: model_changed.then(|| model.clone()),
                    thinking_level: Some(saved_effort),
                    fast_mode: Some(session.fast_mode()),
                    ..Default::default()
                };
                if settings.experimental_context_file
                    && let Some(context) = &mut update.context
                {
                    // A retry must not put the failed answer back in the
                    // provider request when the file refreshes the context.
                    while matches!(context.messages.last(), Some(AgentMessage::Assistant(assistant)) if assistant.stop_reason == StopReason::Error)
                    {
                        context.messages.pop();
                    }
                }
                if !dynamic_enabled {
                    return Some(update);
                }
                if let Some(level) = leased {
                    update.thinking_level = Some(level);
                    return Some(update);
                }

                let selected = match super_ai::auth::resolve_api_key_async(
                    "typesafe",
                    &session.registry().declared_keys,
                )
                .await
                {
                    Ok(Some(api_key)) => {
                        let messages = session
                            .manager
                            .lock()
                            .unwrap()
                            .build_session_context()
                            .messages;
                        let cancel = session.cancel.lock().unwrap().clone();
                        crate::jev::select_reasoning(&messages, &[], &model, &api_key, cancel)
                            .await
                            .map_err(|error| error.to_string())
                    }
                    Ok(None) => Err(
                        "TypeSafe credentials are unavailable; run /login typesafe or set TYPESAFE_API_KEY"
                            .into(),
                    ),
                    Err(error) => Err(format!(
                        "TypeSafe credentials could not be read: {error}; run /login typesafe or set TYPESAFE_API_KEY"
                    )),
                };
                let latest_model = session.model();
                let latest_effort = session.thinking_level();
                if latest_model.provider != model.provider
                    || latest_model.id != model.id
                    || latest_effort != saved_effort
                    || session.settings().reasoning_effort.mode != ReasoningEffortMode::Jev
                {
                    reasoning_run.lock().unwrap().lease.clear();
                    update.context =
                        Some(session.build_context_for(*active_prompt_mode.lock().unwrap()));
                    update.model = Some(latest_model);
                    update.thinking_level = Some(latest_effort);
                    return Some(update);
                }
                match selected {
                    Ok(selection) => {
                        update.thinking_level = Some(selection.level);
                        if selection.level != current_effort {
                            let sink = session.sink.clone();
                            let level = selection.level;
                            let generations = selection.generations;
                            update.on_applied = Some(Box::new(move |_, applied| {
                                if applied == level {
                                    sink(SessionEvent::ReasoningEffortChanged {
                                        level,
                                        generations,
                                    });
                                }
                            }));
                        }
                        reasoning_run.lock().unwrap().lease.install(&selection);
                    }
                    Err(reason) => {
                        let sink = session.sink.clone();
                        let reason = reason.chars().take(300).collect();
                        update.on_applied = Some(Box::new(move |_, applied| {
                            sink(SessionEvent::ReasoningEffortFallback {
                                level: applied,
                                reason,
                            });
                        }));
                    }
                }
                Some(update)
            })
        }));
        let session_for_compaction = session_arc.clone();
        config.prepare_next_turn = Some(Arc::new(move |turn| {
            let has_tool_results = !turn.tool_results.is_empty();
            let tool_failed = turn.tool_results.iter().any(|result| result.is_error);
            let will_continue = turn.will_continue;
            let session = session_for_compaction.clone();
            let active_prompt_mode = active_prompt_mode.clone();
            let steering_for_mode = steering_for_mode.clone();
            let follow_up_for_mode = follow_up_for_mode.clone();
            let reasoning_run = reasoning_run.clone();
            Box::pin(async move {
                let has_queued_user_input = !steering_for_mode.lock().unwrap().is_empty()
                    || (!will_continue && !follow_up_for_mode.lock().unwrap().is_empty());
                let queued_mode = queued_mode(&steering_for_mode, steering_mode)
                    .or_else(|| queued_mode(&follow_up_for_mode, follow_up_mode));
                let mode_changed = {
                    let mut active = active_prompt_mode.lock().unwrap();
                    let before = *active;
                    if let Some(queued_mode) = queued_mode {
                        *active = queued_mode;
                    } else if *active == PromptMode::Workflow && !has_tool_results {
                        // A workflow turn remains active across its tool call.
                        // Its final assistant response has no tool results, so
                        // the next queued or follow-up turn is ordinary again.
                        *active = PromptMode::Ordinary;
                    }
                    before != *active
                };

                let context_file_changed = session.sync_context_file();
                let moved = session
                    .context_stale
                    .swap(false, std::sync::atomic::Ordering::SeqCst);
                let mut context_changed = mode_changed || context_file_changed || moved;
                if has_tool_results {
                    let settings = session.settings();
                    let cancel = session.cancel.lock().unwrap().clone();
                    let model = session.model();
                    let revision_before = {
                        let manager = session.manager.lock().unwrap();
                        let context = manager.build_session_context();
                        if !auto_compaction_needed(
                            &settings,
                            &context.messages,
                            &model,
                            cancel.is_cancelled(),
                        ) {
                            None
                        } else {
                            Some(manager.context_revision())
                        }
                    };

                    if let Some(revision_before) = revision_before {
                        session.compact(None, true).await;
                        let revision_after = session.manager.lock().unwrap().context_revision();
                        context_changed |= revision_after != revision_before;
                    }
                }

                {
                    let mut run = reasoning_run.lock().unwrap();
                    run.lease.consume_generation();
                    if tool_failed || has_queued_user_input {
                        run.lease.clear();
                    }
                }

                Some(TurnUpdate {
                    context: context_changed
                        .then(|| session.build_context_for(*active_prompt_mode.lock().unwrap())),
                    fast_mode: Some(session.fast_mode()),
                    ..Default::default()
                })
            })
        }));
        if let Some(stream_fn) = self.stream_fn.lock().unwrap().clone() {
            config.stream_fn = stream_fn;
        }
        let sink = self.sink.clone();
        config.stream_fn = crate::account_failover::wrap(
            config.stream_fn,
            self.registry(),
            Arc::new(move |switch| sink(SessionEvent::AccountSwitched(switch))),
        );
        config
    }

    fn sync_context_file(&self) -> bool {
        let mut file = self.context_file.lock().unwrap();
        if !self.settings().experimental_context_file {
            return file.take().is_some();
        }
        let mut manager = self.manager.lock().unwrap();
        let result = (|| {
            if file.is_none() {
                *file = Some(crate::context_file::ContextFile::new().context(
                    "Cannot create a context file. A writable temporary directory is required. Set TMPDIR to a writable directory and restart SUPER",
                )?);
            }
            file.as_mut().unwrap().sync(&mut manager)
        })();
        match result {
            Ok(changed) => {
                if changed {
                    *self.context_usage_cache.lock().unwrap() = None;
                    self.cache_warm_cancel.lock().unwrap().cancel();
                }
                changed
            }
            Err(error) => {
                let message = AgentMessage::Custom(super_agent::CustomMessage {
                    custom_type: "context_file_error".into(),
                    content: super_ai::UserContent::Text(format!("{error:#}")),
                    display: true,
                    details: None,
                    timestamp: super_ai::now_ms(),
                });
                if manager.append_message(message.clone()).is_err() {
                    return false;
                }
                drop(manager);
                drop(file);
                (self.sink)(SessionEvent::Agent(Box::new(AgentEvent::MessageEnd {
                    message,
                })));
                *self.context_usage_cache.lock().unwrap() = None;
                true
            }
        }
    }

    fn build_context(&self) -> AgentContext {
        self.build_context_for(PromptMode::Ordinary)
    }

    fn build_context_for(&self, prompt_mode: PromptMode) -> AgentContext {
        let model = self.model();
        let manager = self.manager.lock().unwrap();
        let (openai_responses_input, messages) = if !self.settings().experimental_context_file
            && super_ai::api::openai_compaction::supports_remote_compaction(&model)
            && let Some(remote) = manager.build_openai_compaction_context(&model)
        {
            (Some(remote.replacement_history), remote.messages)
        } else {
            (None, manager.build_session_context().messages)
        };
        drop(manager);
        let mut system_prompt = self.system_prompt.lock().unwrap().clone();
        if let Some(folder) = self.worktree_folder() {
            system_prompt = in_folder(&system_prompt, &folder);
        }
        let tools = self.tools.lock().unwrap().clone();
        // Cursor also advertises its native Shell tool; it must use the registered shell's syntax.
        if model.provider == "cursor"
            && let Some(shell) = tools.iter().find(|tool| tool.name() == "Bash")
        {
            system_prompt.push_str("\n\nSUPER shell tool (Bash): ");
            system_prompt.push_str(&shell.description());
        }
        if self.settings().experimental_context_file
            && let Some(file) = self.context_file.lock().unwrap().as_ref()
        {
            system_prompt.push_str(&format!(
                "\n\nExperimental context file: {}\n\
                 This JSON array contains your live SUPER conversation, without system instructions. \
                 Use your normal file tools to edit it. Changes apply after the tool batch and before the next model request. \
                 New user messages, your current answer, and tool results are added automatically. \
                 Keep important task instructions, exact facts, progress, and next steps. Replace stale output with useful notes. \
                 Keep complete assistant tool-call/result groups, or replace the whole group with a user note. \
                 A user note has the form {{\"role\":\"user\",\"content\":\"Notes here\",\"timestamp\":0}}. \
                 Preserve fields of messages you keep, including image and reasoning data. \
                 The file must be a valid JSON array at most 16 MiB. Invalid edits leave the live conversation unchanged; repair the file after an error. \
                 Read only the parts you need, because the conversation is already in your context. \
                 Your model context window is {} tokens. Manage the file before it fills; automatic compaction remains an emergency fallback. \
                 Batch edits: changing early text can require the provider to process all later text again. \
                 The full session record remains separate from this editable file.",
                file.path().display(), model.context_window
            ));
        }
        if self.subagents_enabled() && tools.iter().any(|tool| tool.name() == "Agent") {
            system_prompt.push_str("\n\n");
            system_prompt.push_str(&super_agent::placeholder::prompt_todo(
                "Agent.system_section",
            ));
            system_prompt.push_str("\n\n");
            system_prompt.push_str(&crate::agents::type_list_section(&self.agent_type_list()));
        }
        // The advanced mode lists the skills for its `Skill` tool here; the
        // basic mode's prompt lists them with their files.
        if tools.iter().any(|tool| tool.name() == "Skill") {
            let list = crate::skills::skill_list_section(&crate::tools::skill::summaries(self));
            if !list.is_empty() {
                system_prompt.push_str("\n\n");
                system_prompt.push_str(&list);
            }
        }
        // The user asked for this turn to run as a dynamic workflow.
        if prompt_mode == PromptMode::Workflow
            && self.workflows_enabled()
            && tools.iter().any(|tool| tool.name() == "Workflow")
        {
            system_prompt.push_str("\n\n");
            system_prompt.push_str(&super_agent::placeholder::prompt_todo("Workflow.opt_in"));
        }
        AgentContext {
            system_prompt,
            openai_responses_input,
            messages,
            tools,
        }
    }

    /// A child session for a workflow agent or a loop: the parent's system
    /// prompt with one sentence for the child, the parent's tools, and as
    /// much of the parent's text conversation as `fork_turns` asks for. It
    /// gets an agent id, is stored in this session's folder of agents as
    /// `<id>.jsonl`, and has a record among this session's agents (when the
    /// session keeps them), so clients list, read and mirror it like an
    /// agent.
    pub(crate) fn create_subagent_session(
        self: &Arc<Self>,
        spec: SubagentSpec<'_>,
    ) -> anyhow::Result<crate::child_turn::ChildAgent> {
        let (model, suggested_thinking) = match spec.model {
            Some(pattern) => self
                .registry()
                .resolve(pattern, None)
                .with_context(|| format!("no model matches subagent model '{pattern}'"))?,
            None => (self.model(), None),
        };
        let thinking = match spec.effort {
            Some(level) => ThinkingLevel::parse(level)
                .with_context(|| format!("unknown subagent reasoning_effort '{level}'"))?,
            None => suggested_thinking.unwrap_or_else(|| self.thinking_level()),
        };
        let history = if spec.fork_turns == ForkTurns::None {
            Vec::new()
        } else {
            let messages = self
                .manager
                .lock()
                .unwrap()
                .build_session_context()
                .messages;
            fork_messages(&messages, spec.fork_turns)
        };
        let mut settings = self.settings();
        settings.subagents.enabled = false;
        let mut system_prompt = self.system_prompt.lock().unwrap().clone();
        // A child of a session in a worktree works there too.
        let folder = self.worktree_folder();
        if let Some(folder) = &folder {
            system_prompt = in_folder(&system_prompt, folder);
        }
        let canonical_path = spec.canonical_path;
        system_prompt.push_str(&format!(
            "\n\nYou are child agent {canonical_path}. Complete only the assigned task. Return a concise result to the parent agent."
        ));
        let id = self
            .background_tasks()
            .new_id(crate::tasks::TaskKind::Agent);
        let depth = self
            .agent_link
            .get()
            .map_or(0, |link| link.depth)
            .saturating_add(1);
        let manager = self.child_manager(
            Some(&id),
            folder.as_deref(),
            history,
            serde_json::json!({
                "taskName": spec.name,
                "canonicalPath": canonical_path,
                "agentId": id,
                "agentType": spec.origin.child_type(),
                "origin": spec.origin,
                "depth": depth,
                "model": {"provider": model.provider, "id": model.id},
                "thinkingLevel": thinking,
            }),
        )?;
        let child = self.create_child(
            manager,
            ChildSpec {
                // A fork carries on the parent's conversation and starts
                // from what the parent read; any other child reads for
                // itself.
                copy_file_state: spec.fork_turns != ForkTurns::None,
                model,
                thinking,
                system_prompt,
                tools: self.base_tools_here(),
                settings,
            },
        );
        // Its `Skill` tool loads the parent's skills.
        child.set_agent_resources(self.agent_resources());
        let record = self.agents.get().map(|registry| {
            (
                Arc::downgrade(registry),
                registry.adopt(&id, spec.name, spec.origin, &child),
            )
        });
        Ok(crate::child_turn::ChildAgent::new(child, record))
    }

    /// A workflow's child named `name`, for tests.
    #[cfg(test)]
    pub(crate) fn test_child(self: &Arc<Self>, name: &str, fork_turns: ForkTurns) -> Arc<Self> {
        let path = format!("/root/{name}");
        self.create_subagent_session(SubagentSpec::new(
            name,
            &path,
            fork_turns,
            crate::agents::AgentOrigin::Workflow {
                run_id: "wf_test0000".into(),
            },
        ))
        .unwrap()
        .session
    }

    /// The session of an agent the `Agent` tool starts. A fork starts with
    /// an exact copy of this conversation, this system prompt and what this
    /// session read; any other type with its instructions followed by the
    /// context of this session's prompt ([`AgentResources::prompt_context`])
    /// and nothing read. Its tools are this session's that its type allows;
    /// it gets the agent tools when its type may start agents and it is not
    /// as deep as agents nest, else `SendMessage` only, to write to main. It
    /// is stored in this session's folder of agents as `<id>.jsonl`, and an
    /// agent with a worktree works there.
    pub(crate) fn create_agent_session(
        self: &Arc<Self>,
        child: AgentChild<'_>,
    ) -> anyhow::Result<Arc<Self>> {
        let fork = child.agent_type.is_fork();
        let history = if fork {
            let messages = self
                .manager
                .lock()
                .unwrap()
                .build_session_context()
                .messages;
            crate::child_turn::exact_copy(messages)
        } else {
            Vec::new()
        };
        // Its own worktree, else the one this session works in.
        let folder = child
            .worktree
            .map(|worktree| worktree.path.clone())
            .or_else(|| self.worktree_folder());
        let folder = folder.as_deref();
        let model = &child.model;
        let manager = self.child_manager(
            Some(child.id),
            folder,
            history,
            serde_json::json!({
                "taskName": child.description,
                "canonicalPath": self.agent_path(child.id),
                "agentId": child.id,
                "agentType": child.agent_type.name,
                "depth": child.depth,
                "toolUseId": child.tool_use_id,
                "model": {"provider": model.provider, "id": model.id},
                "thinkingLevel": child.thinking,
                "worktree": child.worktree,
            }),
        )?;
        Ok(self.agent_session_from(
            manager,
            AgentSetup {
                id: child.id,
                agent_type: child.agent_type,
                model: child.model,
                thinking: child.thinking,
                depth: child.depth,
                folder,
                copy_file_state: fork,
            },
        ))
    }

    /// The session of an agent this session started before it was loaded
    /// again, built from its stored session the way
    /// [`create_agent_session`](Self::create_agent_session) builds a new one,
    /// with nothing read. A type that is gone is taken as `general-purpose`,
    /// a model that is gone as this session's. An agent whose folder is gone
    /// (the worktree this session worked in was removed) works in this
    /// session's folder; one with a worktree of its own gets it back before
    /// its next run.
    pub(crate) fn load_agent_session(
        self: &Arc<Self>,
        stored: &StoredAgent,
    ) -> anyhow::Result<Arc<Self>> {
        let mut manager = SessionManager::open(&stored.file)?;
        if stored.worktree.is_none() && !manager.cwd().exists() {
            manager.set_cwd(&self.working_folder());
        }
        let types = self.agent_types();
        let agent_type = types
            .iter()
            .find(|known| known.name == stored.agent_type)
            .or_else(|| {
                types
                    .iter()
                    .find(|known| known.name == crate::agents::types::GENERAL_PURPOSE)
            })
            .context("no agent type to load the agent with")?;
        let model = stored
            .model
            .as_ref()
            .and_then(|(provider, id)| self.registry().resolve(id, Some(provider)))
            .map(|(model, _)| model)
            .unwrap_or_else(|| self.model());
        let thinking = stored
            .thinking
            .filter(|level| model.supported_thinking_levels().contains(level))
            .unwrap_or_else(|| self.thinking_level());
        let own_folder = self.manager.lock().unwrap().cwd().to_path_buf();
        let folder = manager.cwd().to_path_buf();
        let depth = self
            .agent_link
            .get()
            .map_or(0, |link| link.depth)
            .saturating_add(1);
        Ok(self.agent_session_from(
            manager,
            AgentSetup {
                id: &stored.id,
                agent_type,
                model,
                thinking,
                depth,
                folder: (folder != own_folder).then_some(folder.as_path()),
                copy_file_state: false,
            },
        ))
    }

    /// `/root/<id>`, with the ids of the agents above this session between.
    fn agent_path(&self, id: &str) -> String {
        format!(
            "{}/{}",
            self.agent_link
                .get()
                .map_or("/root", |link| link.path.as_str()),
            id
        )
    }

    /// Build an agent's session on `manager`: see
    /// [`create_agent_session`](Self::create_agent_session).
    fn agent_session_from(
        self: &Arc<Self>,
        manager: SessionManager,
        setup: AgentSetup<'_>,
    ) -> Arc<Self> {
        let mut system_prompt = if setup.agent_type.is_fork() {
            self.system_prompt()
        } else {
            format!(
                "{}{}",
                setup.agent_type.instructions,
                self.child_prompt_context()
            )
        };
        let mut tools = self.base_tools.lock().unwrap().clone();
        if let Some(folder) = setup.folder {
            system_prompt = in_folder(&system_prompt, folder);
            tools = tools
                .into_iter()
                .map(|tool| tool.in_directory(folder).unwrap_or(tool))
                .collect();
        }
        tools.retain(|tool| setup.agent_type.tools.allows(tool.name()));
        let mut settings = self.settings();
        // Workflows belong to the main session.
        settings.workflows.enabled = false;
        let session = self.create_child(
            manager,
            ChildSpec {
                copy_file_state: setup.copy_file_state,
                model: setup.model,
                thinking: setup.thinking,
                system_prompt,
                tools,
                settings,
            },
        );
        // The type's list of tools holds for the tools the session adds
        // itself as it does for the base tools.
        *session.tool_selection.lock().unwrap() =
            self.tool_selection().narrowed(&setup.agent_type.tools);
        let link = AgentLink {
            parent: Arc::downgrade(self),
            id: setup.id.to_string(),
            depth: setup.depth,
            path: self.agent_path(setup.id),
        };
        assert!(session.agent_link.set(link).is_ok());
        if setup.agent_type.can_start_agents() && setup.depth < MAX_DEPTH {
            let agents = AgentRegistry::new(Arc::downgrade(&session), setup.depth);
            assert!(session.agents.set(agents.clone()).is_ok());
            agents.restore();
        }
        session.set_agent_resources(self.agent_resources());
        session.rebuild_tools();
        session
    }

    /// A new child's session manager, stored in this session's folder of
    /// agents as `<name>.jsonl` (its own id when `name` is `None`) and
    /// working in `folder` (this session's when `None`): `history` first,
    /// then the `subagent` entry that names the parent.
    fn child_manager(
        &self,
        name: Option<&str>,
        folder: Option<&Path>,
        history: Vec<AgentMessage>,
        entry: serde_json::Value,
    ) -> anyhow::Result<SessionManager> {
        let (mut manager, parent_id) = {
            let parent = self.manager.lock().unwrap();
            let folder = folder.unwrap_or_else(|| parent.cwd()).to_path_buf();
            (
                parent.create_child_named(name, &folder)?,
                parent.session_id().to_string(),
            )
        };
        for message in history {
            manager.append_message(message)?;
        }
        let mut entry = entry;
        entry["parentSessionId"] = serde_json::Value::String(parent_id);
        manager.append_custom(SUBAGENT_ENTRY, Some(entry))?;
        Ok(manager)
    }

    /// Build a child session on `manager`. Its events go nowhere until a
    /// host forwards them ([`forward_events`](Self::forward_events)).
    fn create_child(self: &Arc<Self>, manager: SessionManager, spec: ChildSpec) -> Arc<Self> {
        let relay: EventRelay = Arc::default();
        let sink: SessionEventSink = {
            let relay = relay.clone();
            Arc::new(move |event| {
                let forward = relay.lock().unwrap().clone();
                if let Some(forward) = forward {
                    forward(event);
                }
            })
        };
        let child = Self::construct(
            manager,
            spec.tools,
            self.registry(),
            spec.settings,
            spec.system_prompt,
            spec.model,
            spec.thinking,
            self.api_key_override.clone(),
            sink,
            false,
            true,
        );
        assert!(child.event_relay.set(relay).is_ok());
        child.set_tool_selection(self.tool_selection());
        child.set_stream_fn(self.stream_fn.lock().unwrap().clone());
        if spec.copy_file_state {
            child.file_state().copy_from(&self.file_state());
        }
        child
    }

    /// Send the events of this child session to `sink` from now on, or
    /// nowhere with `None`: what a host that shows or mirrors an agent's
    /// session does when its [`AgentObserver`] hears of it. Returns whether
    /// it took: a session that is no child keeps the sink it was built with.
    pub fn forward_events(&self, sink: Option<SessionEventSink>) -> bool {
        match self.event_relay.get() {
            Some(relay) => {
                *relay.lock().unwrap() = sink;
                true
            }
            None => false,
        }
    }

    /// Install what hears of this session's agent sessions, and tell it of
    /// those that exist already. A session's agents are told to their own
    /// observer, which the host installs on them in turn.
    pub fn set_agent_observer(&self, observer: Option<Arc<dyn AgentObserver>>) {
        *self.agent_observer.lock().unwrap() = observer.clone();
        if let (Some(observer), Some(agents)) = (observer, self.agents.get()) {
            for (id, session) in agents.built_sessions() {
                observer.agent_session(&id, &session);
            }
        }
    }

    pub(crate) fn agent_observer(&self) -> Option<Arc<dyn AgentObserver>> {
        self.agent_observer.lock().unwrap().clone()
    }

    /// The session of the agent at `path`: an agent id of this session, or
    /// ids separated by `/` for an agent of an agent (`a1b2c3d4e/a5f6g7h8i`).
    /// A stored agent's session is built if it was not yet.
    pub fn agent_at(&self, path: &str) -> anyhow::Result<Arc<Self>> {
        let mut ids = path.split('/').map(str::trim).filter(|id| !id.is_empty());
        let first = ids
            .next()
            .context("Give the id of an agent of this session.")?;
        let mut session = self
            .agent_registry()
            .context("This session has no agents.")?
            .session_of(first)?;
        for id in ids {
            let next = session
                .agent_registry()
                .with_context(|| format!("Agent {} has no agents.", session.agent_path_label()))?
                .session_of(id)?;
            session = next;
        }
        Ok(session)
    }

    /// This agent's id, for a message about it.
    fn agent_path_label(&self) -> String {
        self.agent_link
            .get()
            .map_or_else(|| "main".to_string(), |link| link.id.clone())
    }

    /// The agents this session started, when it may start agents: the main
    /// session that may have subagents, and an agent of a type that may
    /// start agents while it is not as deep as agents nest. The registry
    /// exists in the basic mode and with the Subagents switch off too; the
    /// tools are not offered then.
    pub fn agent_registry(&self) -> Option<Arc<AgentRegistry>> {
        self.agents.get().cloned()
    }

    /// Who started this session, when it is an agent's.
    pub fn agent_link(&self) -> Option<&AgentLink> {
        self.agent_link.get()
    }

    /// Install what this session's agents start from: the types of the
    /// definition files and the context of the system prompt. A host sets
    /// it where it builds the system prompt, and again when it builds it
    /// anew.
    pub fn set_agent_resources(&self, resources: AgentResources) {
        *self.agent_resources.lock().unwrap() = resources;
        *self.context_usage_cache.lock().unwrap() = None;
    }

    pub fn agent_resources(&self) -> AgentResources {
        self.agent_resources.lock().unwrap().clone()
    }

    /// Every agent type this session's agents can have: the built-in ones
    /// with the definition files' types.
    pub fn agent_types(&self) -> Vec<AgentType> {
        crate::agents::with_definitions(&self.agent_resources.lock().unwrap().definitions)
    }

    /// The name and description of every agent type, as the model is told
    /// of them.
    pub fn agent_type_list(&self) -> Vec<AgentTypeSummary> {
        crate::agents::type_list(&self.agent_types())
    }

    /// What an agent's system prompt has after its instructions: this
    /// session's prompt context, else the working directory alone.
    fn child_prompt_context(&self) -> String {
        if let Some(context) = &self.agent_resources.lock().unwrap().prompt_context {
            return context.clone();
        }
        let cwd = self.manager.lock().unwrap().cwd().display().to_string();
        crate::system_prompt::prompt_context(&crate::system_prompt::SystemPromptOptions {
            mode: self.mode(),
            custom_prompt: None,
            append: None,
            selected_tools: &[],
            cwd: &cwd,
            context_files: &[],
            skills: &[],
        })
    }

    async fn run_ephemeral(
        self: &Arc<Self>,
        system_prompt: String,
        prompt: String,
        tools: Vec<DynTool>,
        max_tokens: u64,
        cancel: CancellationToken,
    ) -> anyhow::Result<EphemeralResponse> {
        let mut usage = Usage::default();
        let mut budget = max_tokens;
        let assistant = loop {
            let mut config = self.loop_config(self, Arc::new(Mutex::new(PromptMode::Ordinary)));
            config.thinking_level = ThinkingLevel::Off;
            config.max_tokens = Some(budget);
            config.session_id = Some(format!("ephemeral-{}", uuid::Uuid::new_v4()));
            config.get_steering_messages = None;
            config.get_follow_up_messages = None;
            config.prepare_next_turn = None;
            config.prepare_generation = None;

            let context = AgentContext {
                system_prompt: system_prompt.clone(),
                openai_responses_input: None,
                messages: Vec::new(),
                tools: tools.clone(),
            };
            let sink: EventSink = Arc::new(|_| {});
            let messages = super_agent::run_agent_loop(
                vec![AgentMessage::user(prompt.clone())],
                context,
                config,
                cancel.clone(),
                sink,
            )
            .await;
            if cancel.is_cancelled() {
                anyhow::bail!("request cancelled");
            }
            for message in &messages {
                if let AgentMessage::Assistant(assistant) = message {
                    usage.add(&assistant.usage);
                }
            }
            let assistant = messages
                .into_iter()
                .rev()
                .find_map(|message| match message {
                    AgentMessage::Assistant(assistant) => Some(assistant),
                    _ => None,
                });
            let Some(assistant) = assistant else {
                anyhow::bail!("the provider returned no answer");
            };
            // A reasoning model that ignores "thinking off" spends a tight
            // budget on reasoning alone and stops at the limit with no text.
            // Ask once more with room for the reasoning and the answer.
            if assistant.stop_reason == StopReason::Length
                && assistant.text().trim().is_empty()
                && budget < EPHEMERAL_REASONING_BUDGET
            {
                budget = EPHEMERAL_REASONING_BUDGET;
                continue;
            }
            break assistant;
        };
        self.totals.lock().unwrap().add(&usage);
        if assistant.stop_reason == StopReason::Error {
            anyhow::bail!(
                "{}",
                assistant
                    .error_message
                    .as_deref()
                    .unwrap_or("the provider request failed")
            );
        }
        let text = assistant.text();
        if text.trim().is_empty() {
            anyhow::bail!("{}", empty_answer_reason(&assistant, budget));
        }
        Ok(EphemeralResponse { text, usage })
    }

    /// Answer a short side question without changing the active session.
    pub async fn answer_btw(
        self: &Arc<Self>,
        question: &str,
        cancel: CancellationToken,
    ) -> anyhow::Result<EphemeralResponse> {
        let messages = self
            .manager
            .lock()
            .unwrap()
            .build_session_context()
            .messages;
        let transcript = transcript_excerpt(&messages, 4, 4_000);
        let prompt = if transcript.is_empty() {
            format!("Side question:\n{question}")
        } else {
            format!("Recent session context:\n{transcript}\n\nSide question:\n{question}")
        };
        // The side question reads with a record of its own: the main
        // conversation never sees those reads.
        let side = ToolSession::new();
        let read_tools = self
            .tools
            .lock()
            .unwrap()
            .iter()
            .filter(|tool| tool.name() == "Read")
            .map(|tool| tool.for_session(&side).unwrap_or_else(|| tool.clone()))
            .collect();
        self.run_ephemeral(
            "Answer the side question from the supplied session context. This is a read-only request. Use the Read tool only when a file is needed. Do not propose or perform edits. Give no more than 150 words or 600 characters. Use no more than five bullets. Return only the answer.".into(),
            prompt,
            read_tools,
            500,
            cancel,
        )
        .await
    }

    /// Generate a short session title without changing conversation history.
    pub async fn generate_session_title(
        self: &Arc<Self>,
        prompt: &str,
        cancel: CancellationToken,
    ) -> anyhow::Result<String> {
        let prompt = bounded_session_title_prompt(prompt);
        if prompt.is_empty() {
            anyhow::bail!("the session prompt is empty");
        }
        let response = self
            .run_ephemeral(
                format!(
                    "Write a one-line title for this task, no more than {SESSION_TITLE_MAX_CHARS} characters. Aim for fewer than five words and start with an imperative verb. Keep ticket IDs and code terms unchanged. Match the user's language. Return the title in sentence case without quotes, Markdown, or ending punctuation."
                ),
                format!("User prompt:\n{prompt}"),
                Vec::new(),
                64,
                cancel,
            )
            .await?;
        normalize_session_title(&response.text)
            .context("the provider returned an invalid session title")
    }

    /// Create a one-line recap without changing the active session.
    pub async fn generate_recap(
        self: &Arc<Self>,
        previous_recap: Option<&str>,
        cancel: CancellationToken,
    ) -> anyhow::Result<EphemeralResponse> {
        let messages = self
            .manager
            .lock()
            .unwrap()
            .build_session_context()
            .messages;
        let transcript = transcript_excerpt(&messages, 12, 12_000);
        if transcript.is_empty() {
            anyhow::bail!("the session has no conversation to recap");
        }
        let previous = previous_recap
            .filter(|recap| !recap.trim().is_empty())
            .map(|recap| format!("\n\nPrevious recap:\n{recap}"))
            .unwrap_or_default();
        self.run_ephemeral(
            "Summarize the supplied coding session in one plain-text line of at most 120 characters. State what was done and the next action when one is clear. Do not use a prefix, Markdown, or a newline. Return only the recap.".into(),
            format!("Session transcript:\n{transcript}{previous}"),
            Vec::new(),
            160,
            cancel,
        )
        .await
    }

    /// Run one prompt to completion, including retry and auto-compaction.
    pub async fn prompt(self: &Arc<Self>, prompts: Vec<AgentMessage>) {
        self.prompt_with_mode(prompts, PromptMode::Ordinary).await;
    }

    /// Run one prompt with tools and instructions selected for this turn.
    pub async fn prompt_with_mode(
        self: &Arc<Self>,
        prompts: Vec<AgentMessage>,
        prompt_mode: PromptMode,
    ) {
        if let Some(backend) = self.backend() {
            return backend.prompt(prompts, prompt_mode).await;
        }
        self.cache_warm_cancel.lock().unwrap().cancel();
        {
            let mut running = self.running.lock().unwrap();
            if *running {
                // Already running: enqueue as steering instead.
                drop(running);
                for p in prompts {
                    self.queue_steering_with_mode(p, prompt_mode);
                }
                return;
            }
            *running = true;
        }
        self.run_claimed(prompts, prompt_mode, false).await;
    }

    /// Run `prompts` once this caller holds the `running` flag. `announce`
    /// delivers them through the loop's message events, which records them
    /// and shows them to clients: entries the session queued itself. A
    /// prompt the caller submitted is recorded here and shown by the caller.
    async fn run_claimed(
        self: &Arc<Self>,
        prompts: Vec<AgentMessage>,
        prompt_mode: PromptMode,
        announce: bool,
    ) {
        let cancel = {
            let mut guard = self.cancel.lock().unwrap();
            *guard = CancellationToken::new();
            guard.clone()
        };

        // Persist prompts and run.
        if !announce {
            let mut manager = self.manager.lock().unwrap();
            for p in &prompts {
                let _ = manager.append_message(p.clone());
            }
        }

        let session = self.clone();
        let sink: EventSink = Arc::new(move |event: AgentEvent| {
            session.on_agent_event(&event);
            (session.sink)(SessionEvent::Agent(Box::new(event)));
        });

        let active_prompt_mode = Arc::new(Mutex::new(prompt_mode));
        let mut config = self.loop_config(self, active_prompt_mode.clone());
        let mut context = self.build_context_for(prompt_mode);
        // Persisted prompts are in the context already, so the run is a
        // continuation without a second prompt list; announced ones are
        // handed to the loop, which records them as it delivers them.
        let mut announced = announce.then_some(prompts);

        let mut attempt: u32 = 0;
        loop {
            let messages = match announced.take() {
                Some(prompts) => {
                    super_agent::run_agent_loop(
                        prompts,
                        context,
                        config.clone(),
                        cancel.clone(),
                        sink.clone(),
                    )
                    .await
                }
                None => {
                    super_agent::run_agent_loop_continue(
                        context,
                        config.clone(),
                        cancel.clone(),
                        sink.clone(),
                    )
                    .await
                }
            };

            // Retry on transient error stops.
            let last_error = messages.iter().rev().find_map(|m| match m {
                AgentMessage::Assistant(a) if a.stop_reason == StopReason::Error => {
                    Some(a.error_message.clone().unwrap_or_default())
                }
                _ => None,
            });
            let settings = self.settings();
            let retry = &settings.retry;
            if let Some(error) = last_error
                && retry.enabled
                && attempt < retry.max_retries
                && is_transient(&error)
                && !cancel.is_cancelled()
            {
                attempt += 1;
                let delay = retry
                    .base_delay_ms
                    .saturating_mul(1u64.checked_shl(attempt - 1).unwrap_or(u64::MAX))
                    .min(retry.max_agent_delay_ms);
                (self.sink)(SessionEvent::Retry {
                    attempt,
                    max: retry.max_retries,
                    delay_ms: delay,
                    error,
                });
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(delay)) => {}
                    _ = cancel.cancelled() => break,
                }
                context = self.build_context_for(*active_prompt_mode.lock().unwrap());
                // Drop the trailing error assistant message from context.
                while matches!(
                    context.messages.last(),
                    Some(AgentMessage::Assistant(a)) if a.stop_reason == StopReason::Error
                ) {
                    context.messages.pop();
                }
                config.model = self.model();
                config.thinking_level = self.thinking_level();
                config.fast_mode = self.fast_mode();
                continue;
            }

            // Auto-compaction check after a completed run.
            let ctx = self.manager.lock().unwrap().build_session_context();
            if auto_compaction_needed(
                &settings,
                &ctx.messages,
                &self.model(),
                cancel.is_cancelled(),
            ) {
                self.compact(None, true).await;
            }
            break;
        }

        *self.running.lock().unwrap() = false;
        if self.settings.lock().unwrap().cache_warming == CacheWarmingMode::Streaming {
            self.cache_warm_cancel.lock().unwrap().cancel();
        }
        self.note_activity();
        // An entry that came after the loop last looked starts the next
        // turn. After an interrupt queued entries wait for the next turn,
        // so a stopped turn is not followed by one the user did not ask for.
        if !cancel.is_cancelled() && self.has_queued_entries() {
            self.wake();
        }
    }

    fn on_agent_event(self: &Arc<Self>, event: &AgentEvent) {
        match event {
            AgentEvent::MessageEnd { message } => {
                // Persist assistant + tool results (prompts persisted earlier;
                // steering/follow-up user messages arrive here too).
                let persist = match message {
                    AgentMessage::Assistant(a) => {
                        self.schedule_cache_warming(a);
                        let mut totals = self.totals.lock().unwrap();
                        totals.add(&a.usage);
                        true
                    }
                    AgentMessage::ToolResult(_)
                    | AgentMessage::User(_)
                    | AgentMessage::Custom(_) => true,
                    _ => false,
                };
                if persist {
                    // User prompts were persisted in prompt(). Avoid double
                    // writes by checking the current leaf message identity.
                    let mut manager = self.manager.lock().unwrap();
                    let duplicate = matches!(
                        (manager.entries().last(), message),
                        (Some(crate::session::entry::SessionEntry::Message { message: last, .. }), m) if last == m
                    );
                    if !duplicate {
                        let _ = manager.append_message(message.clone());
                    }
                }
            }
            AgentEvent::AgentEnd { .. } => {}
            _ => {}
        }
    }

    fn schedule_cache_warming(self: &Arc<Self>, assistant: &super_ai::AssistantMessage) {
        let settings = self.settings();
        let model = self.model();
        let Some(cache) = model.prompt_cache else {
            return;
        };
        let Some(short_ttl) = cache.short else {
            return;
        };
        let reasoning = self.thinking_level();
        let fast_mode = self.fast_mode();
        if settings.cache_warming == CacheWarmingMode::Off
            || matches!(
                assistant.stop_reason,
                StopReason::Error | StopReason::Aborted
            )
            || (assistant.usage.input + assistant.usage.cache_read + assistant.usage.cache_write
                == 0)
            || (reasoning != ThinkingLevel::Off
                && model.api == "anthropic-messages"
                && !model
                    .compat
                    .as_ref()
                    .and_then(|compat| compat.force_adaptive_thinking)
                    .unwrap_or(false))
        {
            return;
        }
        let ttl = std::time::Duration::from_secs(short_ttl);
        let Some(delay) = cache_warming_delay(ttl) else {
            return;
        };
        let prompt_tokens =
            assistant.usage.input + assistant.usage.cache_read + assistant.usage.cache_write;
        let agent_context = self.build_context();
        let context = super_ai::Context {
            system_prompt: Some(agent_context.system_prompt),
            openai_responses_input: agent_context.openai_responses_input,
            messages: super_agent::convert_to_llm(&agent_context.messages),
            tools: agent_context
                .tools
                .iter()
                .map(|tool| tool.to_def())
                .collect(),
        };
        let cancel = CancellationToken::new();
        {
            let mut current = self.cache_warm_cancel.lock().unwrap();
            current.cancel();
            *current = cancel.clone();
        }
        let session = self.clone();
        tokio::spawn(async move {
            let started = tokio::time::Instant::now();
            loop {
                let scheduled = tokio::time::Instant::now();
                if tokio::select! {
                    _ = tokio::time::sleep(delay) => false,
                    _ = cancel.cancelled() => true,
                } {
                    return;
                }
                if cache_refresh_deadline_missed(scheduled.elapsed(), ttl, delay) {
                    return;
                }
                let idle = !session.is_running();
                let max_age = if idle {
                    std::time::Duration::from_secs(30 * 60)
                } else {
                    std::time::Duration::from_secs(60 * 60)
                };
                if started.elapsed() > max_age {
                    return;
                }
                let priced = |input: u64, output: u64, cache_read: u64, cache_write: u64| {
                    let mut usage = Usage {
                        input,
                        output,
                        cache_read,
                        cache_write,
                        ..Default::default()
                    };
                    super_ai::api::finalize_cost(&mut usage, &model);
                    usage.cost.total
                };
                let hit_cost = priced(0, 0, prompt_tokens, 0);
                let miss_cost = if model.cost.cache_write > 0.0 {
                    priced(0, 0, 0, prompt_tokens)
                } else {
                    priced(prompt_tokens, 0, 0, 0)
                };
                let warm_cost = priced(0, 1, prompt_tokens, 0);
                let probability = if idle { 0.15 } else { 1.0 };
                if probability * (miss_cost - hit_cost).max(0.0) - warm_cost < 0.05 {
                    return;
                }
                let Some(credential) = session.resolve_credential(&model.provider).await else {
                    return;
                };
                let options = super_ai::StreamOptions {
                    credential: Some(credential),
                    max_tokens: Some(1),
                    reasoning,
                    fast_mode,
                    session_id: Some(session.manager.lock().unwrap().session_id().to_string()),
                    transport: settings.transport,
                    cancel: cancel.clone(),
                    ..Default::default()
                };
                let stream_fn = session
                    .stream_fn
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| Arc::new(super_ai::stream_simple));
                let warmed = stream_fn(&model, &context, &options).result().await;
                if matches!(warmed.stop_reason, StopReason::Error | StopReason::Aborted) {
                    return;
                }
                session.totals.lock().unwrap().add(&warmed.usage);
                let _ = session.manager.lock().unwrap().append_usage(
                    "cache_warm",
                    &warmed.provider,
                    warmed.response_model.as_deref().unwrap_or(&warmed.model),
                    warmed.usage,
                    None,
                );
            }
        });
    }

    /// Manual or automatic compaction.
    pub async fn compact(self: &Arc<Self>, custom_instructions: Option<String>, auto: bool) {
        if let Some(backend) = self.backend() {
            return backend.compact(custom_instructions).await;
        }
        (self.sink)(SessionEvent::CompactionStart { auto });
        let ctx = self.manager.lock().unwrap().build_session_context();
        let previous_summary = ctx.messages.iter().rev().find_map(|m| match m {
            AgentMessage::CompactionSummary(c) => Some(c.summary.clone()),
            _ => None,
        });
        let settings = self.settings();
        let model = self.model();
        let override_settings = settings
            .compaction
            .model_overrides
            .get(&format!("{}/{}", model.provider, model.id));
        let keep_recent_tokens = override_settings
            .and_then(|value| value.keep_recent_tokens)
            .unwrap_or(settings.compaction.keep_recent_tokens);
        let reserve_tokens = override_settings
            .and_then(|value| value.reserve_tokens)
            .unwrap_or(settings.compaction.reserve_tokens);
        let plan = plan_compaction(&ctx.messages, keep_recent_tokens);
        if plan.to_summarize.is_empty() && plan.turn_prefix.is_empty() {
            (self.sink)(SessionEvent::CompactionEnd {
                summary: String::new(),
                tokens_before: plan.tokens_before,
                error: Some("Nothing to compact".into()),
            });
            return;
        }

        if settings.compaction.mode == CompactionMode::Jev
            && let Ok(Some(api_key)) =
                super_ai::auth::resolve_api_key_async("typesafe", &self.registry().declared_keys)
                    .await
        {
            let cancel = self.cancel.lock().unwrap().clone();
            let pinned_start = ctx.messages.len().saturating_sub(plan.kept.len());
            if let Ok(result) =
                crate::jev::compact(&ctx.messages, pinned_start, &api_key, cancel).await
            {
                let estimated_before: u64 = ctx
                    .messages
                    .iter()
                    .map(compaction::estimate_message_tokens)
                    .sum();
                let estimated_after: u64 = result
                    .messages
                    .iter()
                    .map(compaction::estimate_message_tokens)
                    .sum();
                let removed = estimated_before.saturating_sub(estimated_after);
                let tokens_after = plan.tokens_before.saturating_sub(removed);
                let useful =
                    estimated_after.saturating_mul(4) <= estimated_before.saturating_mul(3);
                let resolved_auto_threshold =
                    !auto || !should_compact(tokens_after, model.context_window, reserve_tokens);
                if useful && resolved_auto_threshold {
                    let summary = format!(
                        "Jev kept {}, truncated {}, and removed {} of {} older tool interactions",
                        result.stats.kept,
                        result.stats.truncated,
                        result.stats.dropped,
                        result.stats.eligible,
                    );
                    let details = serde_json::json!({
                        "mode": "jev",
                        "stats": result.stats,
                        "estimatedTokensBefore": plan.tokens_before,
                        "estimatedTokensAfter": tokens_after,
                    });
                    let mut manager = self.manager.lock().unwrap();
                    let append = manager.append_compaction(
                        String::new(),
                        plan.tokens_before,
                        result.messages,
                        None,
                        Some(details),
                    );
                    drop(manager);
                    let error = append.err().map(|error| format!("{error:#}"));
                    (self.sink)(SessionEvent::CompactionEnd {
                        summary: if error.is_none() {
                            summary
                        } else {
                            String::new()
                        },
                        tokens_before: plan.tokens_before,
                        error,
                    });
                    return;
                }
            }
        }

        let credential = self.resolve_credential(&model.provider).await;

        let mut serialized = compaction::serialize_agent_messages(&plan.to_summarize);
        if plan.is_split_turn {
            serialized.push_str(
                "\n\n[The following is the earlier part of the still-active task turn:]\n\n",
            );
            serialized.push_str(&compaction::serialize_agent_messages(&plan.turn_prefix));
        }

        let summary_cancel = self.cancel.lock().unwrap().clone();
        let remote_request = if super_ai::api::openai_compaction::supports_remote_compaction(&model)
        {
            let context = self.build_context();
            Some((
                super_ai::Context {
                    system_prompt: Some(context.system_prompt),
                    openai_responses_input: context.openai_responses_input,
                    messages: super_agent::convert_to_llm(&context.messages),
                    tools: context.tools.iter().map(|tool| tool.to_def()).collect(),
                },
                super_ai::StreamOptions {
                    credential: credential.clone(),
                    reasoning: self.thinking_level(),
                    fast_mode: self.fast_mode(),
                    session_id: Some(self.manager.lock().unwrap().session_id().to_string()),
                    cancel: summary_cancel.clone(),
                    ..Default::default()
                },
            ))
        } else {
            None
        };
        let local_future = compaction::generate_summary(
            &model,
            credential.clone(),
            &serialized,
            previous_summary.as_deref(),
            custom_instructions.as_deref(),
            plan.is_split_turn,
            summary_cancel.clone(),
        );
        let remote_model = model.clone();
        let remote_future = async move {
            match remote_request {
                Some((context, options)) => Some(
                    super_ai::api::openai_compaction::compact(&remote_model, &context, &options)
                        .await,
                ),
                None => None,
            }
        };
        let (local_outcome, remote_outcome) = tokio::join!(local_future, remote_future);

        match select_compaction_outcome(&model, local_outcome, remote_outcome) {
            Ok(result) => {
                let mut summarized_all = plan.to_summarize.clone();
                summarized_all.extend(plan.turn_prefix.clone());
                let (read, modified) = extract_file_ops(&summarized_all);
                {
                    let mut totals = self.totals.lock().unwrap();
                    if let Some(u) = &result.local_usage {
                        totals.add(u);
                    }
                    if let Some(u) = &result.remote_usage {
                        totals.add(u);
                    }
                }
                let details = merge_compaction_details(
                    file_ops_details(&read, &modified),
                    result.remote_details,
                );
                let mut manager = self.manager.lock().unwrap();
                let _ = manager.append_compaction(
                    result.summary.clone(),
                    plan.tokens_before,
                    plan.kept.clone(),
                    result.local_usage,
                    Some(details),
                );
                (self.sink)(SessionEvent::CompactionEnd {
                    summary: result.summary,
                    tokens_before: plan.tokens_before,
                    error: None,
                });
            }
            Err(error) => {
                (self.sink)(SessionEvent::CompactionEnd {
                    summary: String::new(),
                    tokens_before: plan.tokens_before,
                    error: Some(format!("{error:#}")),
                });
            }
        }
    }

    /// Current estimated context tokens and window fraction.
    pub fn context_usage(&self) -> (u64, u64) {
        let manager = self.manager.lock().unwrap();
        let revision = manager.context_revision();
        let used = if let Some((cached_revision, tokens)) =
            *self.context_usage_cache.lock().unwrap()
            && cached_revision == revision
        {
            tokens
        } else {
            let messages = manager.build_session_context().messages;
            let tokens = if self.settings().experimental_context_file {
                messages
                    .iter()
                    .map(compaction::estimate_message_tokens)
                    .sum()
            } else {
                estimate_context_tokens(&messages)
            };
            *self.context_usage_cache.lock().unwrap() = Some((revision, tokens));
            tokens
        };
        drop(manager);
        (used, self.model().context_window)
    }
}

struct SelectedCompaction {
    summary: String,
    local_usage: Option<Usage>,
    remote_usage: Option<Usage>,
    remote_details: Option<serde_json::Value>,
}

fn select_compaction_outcome(
    model: &Model,
    local: anyhow::Result<compaction::SummaryOutcome>,
    remote: Option<anyhow::Result<super_ai::api::openai_compaction::RemoteCompactionResult>>,
) -> anyhow::Result<SelectedCompaction> {
    match (local, remote) {
        (Ok(local), Some(Ok(remote))) => Ok(SelectedCompaction {
            summary: local.summary,
            local_usage: local.usage,
            remote_usage: remote.usage,
            remote_details: Some(
                super_ai::api::openai_compaction::build_remote_compaction_details(model, &remote),
            ),
        }),
        (Ok(local), Some(Err(_)) | None) => Ok(SelectedCompaction {
            summary: local.summary,
            local_usage: local.usage,
            remote_usage: None,
            remote_details: None,
        }),
        (Err(_), Some(Ok(remote))) => Ok(SelectedCompaction {
            summary: format!(
                "OpenAI server-side compaction was applied for {}/{}. The provider-native context is stored in this session, and this notice keeps the compaction boundary readable for other providers.",
                model.provider, model.id
            ),
            local_usage: None,
            remote_usage: remote.usage,
            remote_details: Some(
                super_ai::api::openai_compaction::build_remote_compaction_details(model, &remote),
            ),
        }),
        (Err(local), Some(Err(remote))) => anyhow::bail!(
            "local compaction failed: {local:#}. OpenAI remote compaction failed: {remote:#}"
        ),
        (Err(error), None) => Err(error),
    }
}

fn merge_compaction_details(
    mut local: serde_json::Value,
    remote: Option<serde_json::Value>,
) -> serde_json::Value {
    let Some(remote) = remote else {
        return local;
    };
    let Some(local_object) = local.as_object_mut() else {
        return remote;
    };
    if let Some(remote_object) = remote.as_object() {
        for (key, value) in remote_object {
            local_object.insert(key.clone(), value.clone());
        }
    }
    local
}

fn transcript_excerpt(messages: &[AgentMessage], max_messages: usize, max_chars: usize) -> String {
    let mut entries = messages
        .iter()
        .rev()
        .filter_map(|message| match message {
            AgentMessage::User(user) => Some(("User", user.content.as_text())),
            AgentMessage::Assistant(assistant) => Some(("Assistant", assistant.text())),
            _ => None,
        })
        .filter(|(_, text)| !text.trim().is_empty())
        .take(max_messages)
        .collect::<Vec<_>>();
    entries.reverse();
    let transcript = entries
        .into_iter()
        .map(|(role, text)| format!("{role}: {}", text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n");
    let count = transcript.chars().count();
    if count <= max_chars {
        return transcript;
    }
    let omitted = count - max_chars;
    let tail = transcript.chars().skip(omitted).collect::<String>();
    format!("[earlier text omitted]\n{tail}")
}

fn drain_queue(queue: &Arc<Mutex<VecDeque<QueuedPrompt>>>, mode: QueueMode) -> Vec<AgentMessage> {
    drain_entries(queue, mode)
        .into_iter()
        .map(|prompt| prompt.message)
        .collect()
}

fn drain_entries(queue: &Arc<Mutex<VecDeque<QueuedPrompt>>>, mode: QueueMode) -> Vec<QueuedPrompt> {
    let mut q = queue.lock().unwrap();
    match mode {
        QueueMode::All => q.drain(..).collect(),
        QueueMode::OneAtATime => q.pop_front().into_iter().collect(),
    }
}

/// The prompt mode the next queued input asks for. Entries the session
/// queued itself keep the mode that is active.
fn queued_mode(queue: &Arc<Mutex<VecDeque<QueuedPrompt>>>, mode: QueueMode) -> Option<PromptMode> {
    let queue = queue.lock().unwrap();
    match mode {
        QueueMode::All => {
            let typed: Vec<&QueuedPrompt> =
                queue.iter().filter(|prompt| !prompt.starts_turn).collect();
            typed
                .iter()
                .any(|prompt| prompt.mode == PromptMode::Workflow)
                .then_some(PromptMode::Workflow)
                .or_else(|| (!typed.is_empty()).then_some(PromptMode::Ordinary))
        }
        QueueMode::OneAtATime => queue
            .front()
            .filter(|prompt| !prompt.starts_turn)
            .map(|prompt| prompt.mode),
    }
}

fn auto_compaction_needed(
    settings: &Settings,
    messages: &[AgentMessage],
    model: &Model,
    cancelled: bool,
) -> bool {
    let reserve_tokens = settings
        .compaction
        .model_overrides
        .get(&format!("{}/{}", model.provider, model.id))
        .and_then(|value| value.reserve_tokens)
        .unwrap_or(settings.compaction.reserve_tokens);
    settings.compaction.enabled
        && !cancelled
        && model.context_window > 0
        && should_compact(
            if settings.experimental_context_file {
                // Usage in retained assistant messages describes the old
                // request, not the conversation after a model-owned edit.
                messages
                    .iter()
                    .map(compaction::estimate_message_tokens)
                    .sum()
            } else {
                estimate_context_tokens(messages)
            },
            model.context_window,
            reserve_tokens,
        )
}

fn cache_warming_delay(ttl: std::time::Duration) -> Option<std::time::Duration> {
    (ttl > std::time::Duration::from_secs(10))
        .then(|| std::cmp::min(ttl.mul_f64(0.9), ttl - std::time::Duration::from_secs(10)))
}

fn cache_refresh_deadline_missed(
    elapsed: std::time::Duration,
    ttl: std::time::Duration,
    delay: std::time::Duration,
) -> bool {
    elapsed > delay + ttl.saturating_sub(delay) / 2
}

fn is_transient(error: &str) -> bool {
    let e = error.to_lowercase();
    if let Some(value) = error
        .find('{')
        .and_then(|start| serde_json::from_str::<serde_json::Value>(&error[start..]).ok())
    {
        let detail = value.get("error").unwrap_or(&value);
        if detail
            .get("isRetryable")
            .and_then(serde_json::Value::as_bool)
            == Some(false)
            || detail
                .get("details")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|details| {
                    details.iter().any(|detail| {
                        detail
                            .pointer("/debug/details/isRetryable")
                            .and_then(serde_json::Value::as_bool)
                            == Some(false)
                    })
                })
        {
            return false;
        }
    }
    if e.contains("subscription_sharing_usage_limit_exceeded") {
        return false;
    }
    let transient_status = [429, 500, 502, 503, 504, 520].iter().any(|status| {
        [
            format!("http {status}"),
            format!("status {status}"),
            format!("status: {status}"),
            format!("status code {status}"),
        ]
        .iter()
        .any(|marker| e.contains(marker))
    });
    transient_status
        || [
            "overloaded",
            "currently experiencing high demand",
            "rate limit",
            "timeout",
            "timed out",
            "connection reset",
            "connection refused",
            "connection closed",
            "connection aborted",
            "connection error",
            "failed to connect",
            "network error",
            "stream error",
            "subscription_sharing_usage_unavailable",
            "subscription_sharing_user_unavailable",
        ]
        .iter()
        .any(|needle| e.contains(needle))
}

#[cfg(test)]
mod ephemeral_tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn openai_model() -> Model {
        Model {
            id: "gpt-test".into(),
            name: "GPT test".into(),
            api: "openai-responses".into(),
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            reasoning: true,
            input: vec!["text".into()],
            cost: Default::default(),
            prompt_cache: None,
            context_window: 100_000,
            max_tokens: 1_000,
            compat: None,
            thinking_level_map: BTreeMap::new(),
            headers: BTreeMap::new(),
            sampling_params: Default::default(),
        }
    }

    /// A session whose provider is the given scripted stream function. The
    /// directory holds the session file for as long as the test keeps it.
    fn scripted_session(stream_fn: StreamFn) -> (Arc<AgentSession>, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let manager =
            SessionManager::create(directory.path(), Some(directory.path().join("sessions")))
                .unwrap();
        let settings = Settings {
            retry: crate::settings::RetrySettings {
                base_delay_ms: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let session = AgentSession::new(
            manager,
            Vec::new(),
            Registry::load(None),
            settings,
            "test".into(),
            openai_model(),
            ThinkingLevel::Off,
            None,
            Arc::new(|_| {}),
        );
        session.set_stream_fn(Some(stream_fn));
        (session, directory)
    }

    /// What a reasoning model answers when thinking stays on and the whole
    /// output budget goes to reasoning: no text, stopped at the limit.
    fn reasoning_only_answer(budget: u64) -> super_ai::EventStream {
        let mut message =
            super_ai::AssistantMessage::empty("openai-responses", "openai", "gpt-test");
        message.stop_reason = StopReason::Length;
        message.usage.output = budget;
        message.usage.reasoning = Some(budget);
        let (sink, stream) = super_ai::EventStream::channel();
        sink.send(super_ai::AssistantEvent::Start {
            partial: message.clone(),
        });
        sink.done(message);
        stream
    }

    #[tokio::test]
    async fn helper_requests_retry_with_room_for_reasoning() {
        let budgets = Arc::new(Mutex::new(Vec::new()));
        let seen = budgets.clone();
        let (session, _directory) = scripted_session(Arc::new(move |_, _, options| {
            let mut seen = seen.lock().unwrap();
            seen.push(options.max_tokens);
            assert_eq!(options.reasoning, ThinkingLevel::Off);
            if seen.len() == 1 {
                return reasoning_only_answer(options.max_tokens.unwrap());
            }
            let mut message =
                super_ai::AssistantMessage::empty("openai-responses", "openai", "gpt-test");
            message
                .content
                .push(super_ai::ContentBlock::text("Fix login redirect"));
            message.stop_reason = StopReason::Stop;
            message.usage.output = 5;
            let (sink, stream) = super_ai::EventStream::channel();
            sink.send(super_ai::AssistantEvent::Start {
                partial: message.clone(),
            });
            sink.done(message);
            stream
        }));
        let title = session
            .generate_session_title("fix the login redirect bug", CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(title, "Fix login redirect");
        assert_eq!(
            *budgets.lock().unwrap(),
            vec![Some(64), Some(EPHEMERAL_REASONING_BUDGET)]
        );
        // Both attempts count towards the session totals.
        assert_eq!(session.totals.lock().unwrap().output, 64 + 5);
    }

    #[tokio::test]
    async fn helper_requests_say_when_reasoning_spent_the_budget() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        let (session, _directory) = scripted_session(Arc::new(move |_, _, options| {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            reasoning_only_answer(options.max_tokens.unwrap())
        }));
        let error = session
            .generate_recap(None, CancellationToken::new())
            .await
            .map(|_| ())
            .unwrap_err()
            .to_string();
        // No conversation yet: the recap has nothing to summarise.
        assert_eq!(error, "the session has no conversation to recap");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

        let error = session
            .generate_session_title("fix the login redirect bug", CancellationToken::new())
            .await
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "the model spent its 4096-token budget on reasoning (4096 tokens) and returned no text"
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn context_file_bash_edits_reach_next_request_and_survive_resume() {
        let directory = tempfile::tempdir().unwrap();
        let mut manager =
            SessionManager::create(directory.path(), Some(directory.path().join("sessions")))
                .unwrap();
        manager
            .append_message(AgentMessage::user("obsolete output"))
            .unwrap();
        manager
            .append_compaction(
                "portable summary".into(),
                100,
                vec![AgentMessage::user("obsolete output")],
                None,
                Some(serde_json::json!({"remoteCompaction": {
                    "version": 2,
                    "provider": "openai-responses-compaction",
                    "modelKey": "openai:openai-responses:gpt-test",
                    "replacementHistory": [{"type": "compaction", "encrypted_content": "opaque"}]
                }})),
            )
            .unwrap();
        assert!(
            manager
                .build_openai_compaction_context(&openai_model())
                .is_some()
        );
        let session_path = manager.session_file().unwrap().to_path_buf();
        let settings = Settings {
            experimental_context_file: true,
            compaction: crate::settings::CompactionSettings {
                enabled: false,
                ..Default::default()
            },
            retry: crate::settings::RetrySettings {
                base_delay_ms: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let events = Arc::new(Mutex::new(Vec::new()));
        let saved_events = events.clone();
        let session = AgentSession::new(
            manager,
            vec![Arc::new(super_agent::tools::bash::BashTool::new(
                directory.path().to_path_buf(),
            ))],
            Registry::load(None),
            settings,
            "test".into(),
            openai_model(),
            ThinkingLevel::Off,
            None,
            Arc::new(move |event| saved_events.lock().unwrap().push(event)),
        );
        let weak = Arc::downgrade(&session);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_calls = calls.clone();
        session.set_stream_fn(Some(Arc::new(move |_, context, _| {
            let step = observed_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let prompt = context.system_prompt.as_ref().unwrap();
            let path = prompt.lines().find_map(|line| line.strip_prefix("Experimental context file: ")).unwrap();
            assert!(Path::new(path).is_file());
            assert!(context.openai_responses_input.is_none());
            let mut message = super_ai::AssistantMessage::empty("openai-responses", "openai", "gpt-test");
            let replacement = match step {
                0 => {
                    weak.upgrade().unwrap().queue_steering(AgentMessage::user("new instruction"));
                    Some(r#"[{"role":"user","content":"saved notes","timestamp":0}]"#)
                }
                1 => {
                    let text = serde_json::to_string(&context.messages).unwrap();
                    assert!(text.contains("saved notes"));
                    assert!(!text.contains("obsolete output"));
                    assert!(text.contains("new instruction"));
                    assert!(context.messages.iter().any(|message| matches!(message, super_ai::Message::ToolResult(result) if result.tool_call_id == "edit_0" && !result.is_error)));
                    Some("invalid JSON")
                }
                2 => {
                    let text = serde_json::to_string(&context.messages).unwrap();
                    assert!(text.contains("saved notes"));
                    assert!(text.contains("Repair the file"));
                    assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid JSON");
                    Some(r#"[{"role":"user","content":"repaired notes","timestamp":0}]"#)
                }
                3 => {
                    let text = serde_json::to_string(&context.messages).unwrap();
                    assert!(text.contains("repaired notes"));
                    assert!(context.messages.iter().any(|message| matches!(message, super_ai::Message::User(user) if user.content.as_text() == "repaired notes")));
                    assert!(context.messages.iter().any(|message| matches!(message, super_ai::Message::ToolResult(result) if result.tool_call_id == "edit_2" && !result.is_error)));
                    None
                }
                4 => {
                    assert!(!matches!(context.messages.last(), Some(super_ai::Message::Assistant(assistant)) if assistant.stop_reason == StopReason::Error));
                    assert!(context.messages.iter().any(|message| matches!(message, super_ai::Message::User(user) if user.content.as_text() == "repaired notes")));
                    None
                }
                _ => panic!("unexpected model request"),
            };
            if let Some(replacement) = replacement {
                let quoted_path = path.replace('\'', "'\\''");
                message.content.push(super_ai::ContentBlock::ToolCall(super_ai::ToolCall {
                    id: format!("edit_{step}"),
                    name: "Bash".into(),
                    arguments: serde_json::json!({"command": format!("printf '%s' '{replacement}' > '{quoted_path}'")}),
                    thought_signature: None,
                }));
                message.stop_reason = StopReason::ToolUse;
            } else if step == 3 {
                message.stop_reason = StopReason::Error;
                message.error_message = Some("HTTP 503".into());
            } else {
                message.content.push(super_ai::ContentBlock::text("done"));
                message.stop_reason = StopReason::Stop;
            }
            let (sink, stream) = super_ai::EventStream::channel();
            sink.send(super_ai::AssistantEvent::Start { partial: message.clone() });
            if message.stop_reason == StopReason::Error {
                sink.error(message);
            } else {
                sink.done(message);
            }
            stream
        })));
        session
            .prompt(vec![AgentMessage::user("manage context")])
            .await;
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 5);
        let context = session
            .manager
            .lock()
            .unwrap()
            .build_session_context()
            .messages;
        assert_eq!(
            SessionManager::open(&session_path)
                .unwrap()
                .build_session_context()
                .messages,
            context
        );
        assert!(
            std::fs::read_to_string(session_path)
                .unwrap()
                .contains("obsolete output")
        );
        let errors = events.lock().unwrap().iter().filter(|event| matches!(event, SessionEvent::Agent(event) if matches!(event.as_ref(), AgentEvent::MessageEnd { message: AgentMessage::Custom(custom) } if custom.custom_type == "context_file_error"))).count();
        assert_eq!(errors, 1);
    }

    #[test]
    fn context_file_lifecycle_is_opt_in_and_session_local() {
        let session = AgentSession::new(
            SessionManager::in_memory(Path::new("/test")),
            Vec::new(),
            Registry::load(None),
            Settings::default(),
            "test".into(),
            openai_model(),
            ThinkingLevel::Off,
            None,
            Arc::new(|_| {}),
        );
        assert!(!session.sync_context_file());
        assert!(session.context_file.lock().unwrap().is_none());
        assert_eq!(session.build_context().system_prompt, "test");
        let mut settings = session.settings();
        settings.experimental_context_file = true;
        session.update_settings(settings);
        session.sync_context_file();
        let path = session
            .context_file
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .path()
            .to_path_buf();
        let child = session.test_child("task", ForkTurns::None);
        child.sync_context_file();
        let child_path = child
            .context_file
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .path()
            .to_path_buf();
        assert_ne!(path, child_path);
        assert!(child_path.is_file());
        session.replace_manager(SessionManager::in_memory(Path::new("/other")));
        assert!(!path.exists());
        assert!(session.context_file.lock().unwrap().is_none());
        session.sync_context_file();
        let new_path = session
            .context_file
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .path()
            .to_path_buf();
        assert_ne!(new_path, child_path);
        let mut settings = session.settings();
        settings.experimental_context_file = false;
        session.update_settings(settings);
        assert!(session.sync_context_file());
        assert!(!new_path.exists());
        assert_eq!(session.build_context().system_prompt, "test");
    }

    #[test]
    fn reasoning_lease_ends_when_model_saved_effort_or_applied_effort_changes() {
        let model = openai_model();
        let mut run = ReasoningRunState {
            lease: crate::jev::ReasoningLease::default(),
            provider: model.provider.clone(),
            model_id: model.id.clone(),
            saved_effort: ThinkingLevel::Medium,
        };
        let selection = crate::jev::ReasoningSelection {
            level: ThinkingLevel::High,
            generations: 5,
        };
        run.lease.install(&selection);
        assert_eq!(
            run.checkpoint(&model, ThinkingLevel::Medium, ThinkingLevel::High, true),
            (false, Some(ThinkingLevel::High))
        );

        let mut other_model = model.clone();
        other_model.provider = "other".into();
        assert_eq!(
            run.checkpoint(
                &other_model,
                ThinkingLevel::Medium,
                ThinkingLevel::High,
                true
            ),
            (true, None)
        );
        run.lease.install(&selection);
        assert_eq!(
            run.checkpoint(&other_model, ThinkingLevel::Low, ThinkingLevel::High, true),
            (false, None)
        );
        run.lease.install(&selection);
        assert_eq!(
            run.checkpoint(&other_model, ThinkingLevel::Low, ThinkingLevel::Low, true),
            (false, None)
        );
        run.lease.install(&selection);
        assert_eq!(
            run.checkpoint(&other_model, ThinkingLevel::Low, ThinkingLevel::High, false),
            (false, None)
        );
    }

    #[test]
    fn late_cache_refreshes_are_skipped_before_the_cache_expires() {
        let ttl = std::time::Duration::from_secs(300);
        let delay = cache_warming_delay(ttl).unwrap();
        assert!(!cache_refresh_deadline_missed(
            std::time::Duration::from_secs(284),
            ttl,
            delay,
        ));
        assert!(cache_refresh_deadline_missed(
            std::time::Duration::from_secs(286),
            ttl,
            delay,
        ));
    }

    #[test]
    fn transient_errors_require_a_status_or_specific_network_failure() {
        assert!(is_transient("request failed with HTTP 503"));
        assert!(is_transient("Cloudflare returned HTTP 520"));
        assert!(is_transient("Azure is currently experiencing high demand"));
        assert!(is_transient("connection reset by peer"));
        assert!(is_transient("rate limit exceeded"));
        assert!(!is_transient("model has a 500 token limit"));
        assert!(!is_transient("connection settings are invalid"));
        assert!(!is_transient(
            "HTTP 429: subscription_sharing_usage_limit_exceeded"
        ));
        assert!(is_transient("subscription_sharing_usage_unavailable"));
        assert!(is_transient("subscription_sharing_user_unavailable"));
        assert!(!is_transient(
            r#"Cursor request failed: stream ended: {"error":{"code":"not_found","message":"Model name is not valid: auto"}}"#
        ));
        assert!(!is_transient(
            r#"Cursor request failed: stream ended: {"error":{"code":"internal","message":"SUPER does not run Cursor-native tools"}}"#
        ));
        assert!(!is_transient(
            r#"Cursor request failed: stream ended: {"error":{"details":[{"debug":{"details":{"isRetryable":false,"detail":"rate limit exceeded"}}}]}}"#
        ));
    }

    #[test]
    fn session_tools_install_replace_and_remove_without_changing_base_tools() {
        let registry = Registry::load(None);
        let session = AgentSession::new(
            SessionManager::in_memory(std::path::Path::new("/test")),
            Vec::new(),
            registry,
            Settings::default(),
            "test".into(),
            openai_model(),
            ThinkingLevel::Off,
            None,
            Arc::new(|_| {}),
        );
        let tool = || {
            Arc::new(crate::tools::grep::GrepTool {
                cwd: std::path::PathBuf::from("/test"),
            }) as DynTool
        };

        assert!(!session.available_tool_names().contains(&"grep".into()));
        session.install_session_tool(tool());
        session.install_session_tool(tool());
        assert_eq!(
            session
                .available_tool_names()
                .iter()
                .filter(|name| name.as_str() == "grep")
                .count(),
            1
        );
        assert!(session.remove_session_tool("grep"));
        assert!(!session.remove_session_tool("grep"));
        assert!(!session.available_tool_names().contains(&"grep".into()));
    }

    fn remote_result() -> super_ai::api::openai_compaction::RemoteCompactionResult {
        super_ai::api::openai_compaction::RemoteCompactionResult {
            replacement_history: vec![serde_json::json!({
                "type": "compaction",
                "encrypted_content": "opaque"
            })],
            usage: Some(Usage {
                input: 10,
                output: 2,
                total_tokens: 12,
                ..Default::default()
            }),
        }
    }

    fn settings_test_session(settings: Settings, subagents_allowed: bool) -> Arc<AgentSession> {
        let registry = Registry::from_builtin();
        let model = registry.all().first().expect("built-in model").clone();
        AgentSession::new_with_subagents_allowed(
            SessionManager::in_memory(std::path::Path::new("/test")),
            Vec::new(),
            registry,
            settings,
            "root prompt".into(),
            model,
            ThinkingLevel::Off,
            None,
            Arc::new(|_| {}),
            subagents_allowed,
        )
    }

    #[test]
    fn cursor_native_shell_receives_registered_shell_instructions() {
        let session = settings_test_session(Settings::default(), false);
        let mut model = session.model();
        model.provider = "cursor".into();
        session.set_model(model);
        // The description of Bash is the placeholder of its shell family.
        for (path, key) in [
            ("pwsh.exe", "Bash.description.pwsh"),
            ("powershell.exe", "Bash.description.powershell"),
            ("cmd.exe", "Bash.description.cmd"),
            ("bash", "Bash.description"),
        ] {
            let mut tool = super_agent::tools::bash::BashTool::new("/test".into());
            tool.shell_path = Some(path.into());
            session.install_session_tool(Arc::new(tool));
            let context = session.build_context();
            let placeholder = super_agent::placeholder::prompt_todo(key);
            assert!(
                context
                    .system_prompt
                    .ends_with(&format!("SUPER shell tool (Bash): {placeholder}")),
                "{}",
                context.system_prompt
            );
            assert_eq!(context.tools[0].to_def().description, placeholder);
        }
        assert!(session.remove_session_tool("Bash"));
        assert_eq!(session.build_context().system_prompt, "root prompt");
    }

    fn benchmark_tools() -> Vec<DynTool> {
        let cwd = std::path::PathBuf::from("/synthetic");
        vec![
            Arc::new(super_agent::tools::read::ReadTool::new(cwd.clone())),
            Arc::new(super_agent::tools::write::WriteTool::new(cwd.clone())),
            Arc::new(super_agent::tools::edit::EditTool::new(cwd.clone())),
            Arc::new(super_agent::tools::bash::BashTool::new(cwd)),
        ]
    }

    #[test]
    fn subagent_tools_follow_settings_and_command_line_authority() {
        let section = super_agent::placeholder::prompt_todo("Agent.system_section");
        let type_list = super_agent::placeholder::prompt_todo("Agent.type_list");
        let session = settings_test_session(Settings::default(), true);
        assert!(session.available_tool_names().is_empty());
        assert_eq!(session.build_context().system_prompt, "root prompt");

        let mut enabled = session.settings();
        enabled.subagents.enabled = true;
        session.update_settings(enabled.clone());
        // TaskStop stops the agents a session leaves running; the workflow
        // tool and the skill with its guide come with dynamic workflows,
        // which are on by default.
        assert_eq!(
            session.available_tool_names(),
            [
                "Agent",
                "SendMessage",
                "ListAgents",
                "TaskStop",
                "Workflow",
                "Skill"
            ]
        );
        let prompt = session.build_context().system_prompt;
        assert!(prompt.starts_with(&format!("root prompt\n\n{section}\n\n{type_list}\n")));

        enabled.workflows.enabled = false;
        session.update_settings(enabled.clone());
        assert_eq!(
            session.available_tool_names(),
            ["Agent", "SendMessage", "ListAgents", "TaskStop"]
        );

        enabled.subagents.enabled = false;
        session.update_settings(enabled);
        assert!(session.available_tool_names().is_empty());

        let mut blocked_settings = Settings::default();
        blocked_settings.subagents.enabled = true;
        let blocked = settings_test_session(blocked_settings, false);
        assert!(blocked.available_tool_names().is_empty());
        assert_eq!(blocked.build_context().system_prompt, "root prompt");
    }

    #[test]
    fn the_basic_mode_ignores_the_subagent_and_workflow_switches() {
        let mut settings = Settings {
            mode: Mode::Basic,
            ..Settings::default()
        };
        settings.subagents.enabled = true;
        settings.workflows.enabled = true;
        let session = settings_test_session(settings, true);
        assert_eq!(session.mode(), Mode::Basic);
        assert!(session.available_tool_names().is_empty());
        assert!(!session.workflows_enabled());
        assert_eq!(
            session.prompt_mode_for("run a dynamic workflow for this task"),
            PromptMode::Ordinary
        );
        let prompt = session
            .build_context_for(PromptMode::Workflow)
            .system_prompt;
        assert!(!prompt.contains(&super_agent::placeholder::prompt_todo(
            "Agent.system_section"
        )));
        assert!(!prompt.contains(&super_agent::placeholder::prompt_todo("Workflow.opt_in")));
        assert!(!prompt.contains(&super_agent::placeholder::prompt_todo("Skill.list")));

        // The same switches count again in the advanced mode.
        let mut advanced = session.settings();
        advanced.mode = Mode::Advanced;
        session.update_settings(advanced.clone());
        assert_eq!(session.mode(), Mode::Advanced);
        assert!(session.available_tool_names().contains(&"Agent".into()));
        assert!(session.workflows_enabled());
        assert!(session.available_tool_names().contains(&"Workflow".into()));
        assert!(session.available_tool_names().contains(&"Skill".into()));

        advanced.mode = Mode::Basic;
        session.update_settings(advanced);
        assert!(session.available_tool_names().is_empty());
    }

    struct AdvancedOnlyTool;

    #[async_trait::async_trait]
    impl super_agent::AgentTool for AdvancedOnlyTool {
        fn name(&self) -> &str {
            "AdvancedOnly"
        }

        fn advanced_only(&self) -> bool {
            true
        }

        fn description(&self) -> String {
            String::new()
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _args: serde_json::Value,
            _cancel: CancellationToken,
            _on_update: Option<super_agent::ToolUpdateSink>,
        ) -> anyhow::Result<super_agent::ToolResult> {
            Ok(super_agent::ToolResult::text("done"))
        }
    }

    #[test]
    fn an_advanced_only_tool_is_left_out_in_the_basic_mode() {
        let settings = Settings {
            mode: Mode::Basic,
            ..Settings::default()
        };
        let registry = Registry::from_builtin();
        let model = registry.all().first().expect("built-in model").clone();
        let session = AgentSession::new(
            SessionManager::in_memory(std::path::Path::new("/test")),
            vec![
                Arc::new(super_agent::tools::read::ReadTool::new("/test".into())),
                Arc::new(AdvancedOnlyTool),
            ],
            registry,
            settings,
            "root prompt".into(),
            model,
            ThinkingLevel::Off,
            None,
            Arc::new(|_| {}),
        );
        let names = |session: &AgentSession| -> Vec<String> {
            session
                .build_context()
                .tools
                .iter()
                .map(|tool| tool.name().to_string())
                .collect()
        };
        assert_eq!(session.available_tool_names(), ["Read"]);
        assert_eq!(names(&session), ["Read"]);
        session.remove_session_tool("AdvancedOnly");
        session.install_session_tool(Arc::new(AdvancedOnlyTool));
        assert_eq!(names(&session), ["Read"]);

        let mut advanced = session.settings();
        advanced.mode = Mode::Advanced;
        session.update_settings(advanced);
        // `Skill`, `ScheduleWakeup` and the worktree tools come with `Read`
        // in the advanced mode.
        let advanced = [
            "Read",
            "AdvancedOnly",
            "Skill",
            "ScheduleWakeup",
            "EnterWorktree",
            "ExitWorktree",
        ];
        assert_eq!(session.available_tool_names(), advanced);
        assert_eq!(names(&session), advanced);
    }

    #[test]
    fn the_system_prompt_can_be_replaced_for_the_next_request() {
        let session = settings_test_session(Settings::default(), false);
        assert_eq!(session.system_prompt(), "root prompt");
        session.set_system_prompt("new prompt".into());
        assert_eq!(session.system_prompt(), "new prompt");
        assert_eq!(session.build_context().system_prompt, "new prompt");
    }

    #[test]
    fn the_workflow_tool_is_offered_with_every_request_and_a_workflow_turn_says_so() {
        let mut settings = Settings::default();
        settings.subagents.enabled = true;
        let session = settings_test_session(settings, true);
        let opt_in = super_agent::placeholder::prompt_todo("Workflow.opt_in");

        // An ordinary turn has the tool too; the guide is a skill the model
        // loads, not a part of the prompt.
        assert!(session.available_tool_names().contains(&"Workflow".into()));
        let ordinary = session.build_context();
        assert!(ordinary.tools.iter().any(|tool| tool.name() == "Workflow"));
        assert!(
            !ordinary
                .system_prompt
                .contains("Writing a dynamic workflow")
        );
        assert!(!ordinary.system_prompt.contains(&opt_in));
        assert!(ordinary.system_prompt.contains(&format!(
            "{}\n<available_skills>\n  <skill>\n    <name>workflow-authoring</name>",
            super_agent::placeholder::prompt_todo("Skill.list")
        )));

        assert_eq!(
            session.prompt_mode_for("run a dynamic workflow for this task"),
            PromptMode::Workflow
        );
        assert_eq!(
            session.prompt_mode_for("fix this small function"),
            PromptMode::Ordinary
        );
        // A turn the user asked to run as a workflow says so, and offers the
        // same tools.
        let asked = session.build_context_for(PromptMode::Workflow);
        assert!(asked.system_prompt.ends_with(&format!("\n\n{opt_in}")));
        assert!(!asked.system_prompt.contains("Writing a dynamic workflow"));
        let names = |context: &AgentContext| -> Vec<String> {
            context
                .tools
                .iter()
                .map(|tool| tool.name().to_string())
                .collect()
        };
        assert_eq!(names(&asked), names(&ordinary));
    }

    #[test]
    fn the_workflow_tool_needs_subagents_and_dynamic_workflows() {
        // Workflows are built on child agents, so the subagent setting is the
        // authority for both.
        let session = settings_test_session(Settings::default(), true);
        assert!(!session.workflows_enabled());
        assert!(!session.available_tool_names().contains(&"Workflow".into()));
        assert!(
            !session
                .build_context_for(PromptMode::Workflow)
                .system_prompt
                .contains(&super_agent::placeholder::prompt_todo("Workflow.opt_in"))
        );

        let mut settings = session.settings();
        settings.subagents.enabled = true;
        settings.workflows.enabled = false;
        session.update_settings(settings.clone());
        assert!(!session.workflows_enabled());
        assert!(!session.available_tool_names().contains(&"Workflow".into()));

        settings.workflows.enabled = true;
        session.update_settings(settings);
        assert!(session.workflows_enabled());
        assert!(session.available_tool_names().contains(&"Workflow".into()));
    }

    #[test]
    fn one_at_a_time_queues_keep_each_prompts_mode() {
        let queue = Arc::new(Mutex::new(VecDeque::from([
            QueuedPrompt {
                message: AgentMessage::user("ordinary"),
                mode: PromptMode::Ordinary,
                starts_turn: false,
            },
            QueuedPrompt {
                message: AgentMessage::user("workflow"),
                mode: PromptMode::Workflow,
                starts_turn: false,
            },
        ])));

        assert_eq!(
            queued_mode(&queue, QueueMode::OneAtATime),
            Some(PromptMode::Ordinary)
        );
        assert_eq!(drain_queue(&queue, QueueMode::OneAtATime).len(), 1);
        assert_eq!(
            queued_mode(&queue, QueueMode::OneAtATime),
            Some(PromptMode::Workflow)
        );
    }

    #[test]
    fn a_session_without_subagent_authority_has_no_workflow_runtime() {
        let mut settings = Settings::default();
        settings.subagents.enabled = true;
        let child = settings_test_session(settings, false);
        assert!(child.workflows().is_none());
        assert!(!child.workflows_enabled());
    }

    #[test]
    fn child_session_has_safe_forked_context_without_control_tools() {
        let mut settings = Settings::default();
        settings.subagents.enabled = true;
        let parent = settings_test_session(settings, true);
        parent
            .manager
            .lock()
            .unwrap()
            .append_message(AgentMessage::user("parent context"))
            .unwrap();

        let child = parent.test_child("inspect", ForkTurns::All);
        assert!(Arc::ptr_eq(&parent.registry(), &child.registry()));
        assert!(child.available_tool_names().is_empty());
        let context = child.manager.lock().unwrap().build_session_context();
        assert!(matches!(
            context.messages.as_slice(),
            [AgentMessage::User(user)] if user.content.as_text() == "parent context"
        ));
    }

    #[test]
    fn child_without_forked_turns_does_not_copy_parent_context() {
        let parent = settings_test_session(Settings::default(), true);
        parent
            .manager
            .lock()
            .unwrap()
            .append_message(AgentMessage::user("parent context"))
            .unwrap();

        let child = parent.test_child("inspect", ForkTurns::None);
        assert!(
            child
                .manager
                .lock()
                .unwrap()
                .build_session_context()
                .messages
                .is_empty()
        );
    }

    #[test]
    #[ignore = "release-mode performance benchmark"]
    fn benchmark_performance_subagent_overhead() {
        let registry = Registry::from_builtin();
        let model = registry.all().first().expect("built-in model").clone();
        let tools = benchmark_tools();
        let make_session = |enabled: bool| {
            let mut settings = Settings::default();
            settings.subagents.enabled = enabled;
            AgentSession::new_with_subagents_allowed(
                SessionManager::in_memory(std::path::Path::new("/synthetic")),
                tools.clone(),
                registry.clone(),
                settings,
                "benchmark root prompt".into(),
                model.clone(),
                ThinkingLevel::Off,
                None,
                Arc::new(|_| {}),
                true,
            )
        };

        super_bench::measure_pair(
            (
                "agent_session_create_subagents_off",
                "agent_session_create_subagents_on",
            ),
            21,
            500,
            (
                "new_root_session_4_base_tools_0_agent_tools",
                "new_root_session_4_base_tools_3_agent_tools",
            ),
            || make_session(false),
            || make_session(true),
        );

        let off = make_session(false);
        let on = make_session(true);
        super_bench::measure_pair(
            (
                "agent_context_build_subagents_off",
                "agent_context_build_subagents_on",
            ),
            21,
            10_000,
            (
                "empty_session_4_base_tools_0_agent_tools",
                "empty_session_4_base_tools_3_agent_tools",
            ),
            || off.build_context(),
            || on.build_context(),
        );
    }

    #[test]
    #[ignore = "release-mode performance benchmark"]
    fn benchmark_performance_workflow_tool_exposure() {
        // The `Workflow` tool and the skill list are part of every request
        // while dynamic workflows are on. The disarmed session is the
        // baseline; the armed one adds the section that says the user asked
        // for a workflow.
        let registry = Registry::from_builtin();
        let model = registry.all().first().expect("built-in model").clone();
        let tools = benchmark_tools();
        let make_session = || {
            let mut settings = Settings::default();
            settings.subagents.enabled = true;
            AgentSession::new_with_subagents_allowed(
                SessionManager::in_memory(std::path::Path::new("/synthetic")),
                tools.clone(),
                registry.clone(),
                settings,
                "benchmark root prompt".into(),
                model.clone(),
                ThinkingLevel::Off,
                None,
                Arc::new(|_| {}),
                true,
            )
        };

        let ordinary = make_session();
        let workflow = make_session();
        super_bench::measure_pair(
            (
                "agent_context_build_workflow_disarmed",
                "agent_context_build_workflow_armed",
            ),
            21,
            10_000,
            (
                "empty_session_subagents_on_workflow_disarmed",
                "empty_session_subagents_on_workflow_armed",
            ),
            || ordinary.build_context_for(PromptMode::Ordinary),
            || workflow.build_context_for(PromptMode::Workflow),
        );
    }

    #[test]
    fn transcript_excerpt_keeps_only_recent_user_and_assistant_text() {
        let messages = vec![
            AgentMessage::user("old"),
            AgentMessage::BashExecution(super_agent::BashExecutionMessage {
                command: "pwd".into(),
                output: "ignored".into(),
                exit_code: Some(0),
                cancelled: false,
                truncated: false,
                full_output_path: None,
                exclude_from_context: false,
                timestamp: 1,
            }),
            AgentMessage::user("new"),
        ];
        let excerpt = transcript_excerpt(&messages, 1, 100);
        assert_eq!(excerpt, "User: new");
    }

    #[test]
    fn transcript_excerpt_enforces_character_budget_from_the_tail() {
        let excerpt = transcript_excerpt(&[AgentMessage::user("abcdefghij")], 4, 5);
        assert!(excerpt.ends_with("fghij"));
        assert!(excerpt.starts_with("[earlier text omitted]"));
    }

    #[test]
    fn hybrid_compaction_keeps_local_summary_and_remote_details() {
        let selected = select_compaction_outcome(
            &openai_model(),
            Ok(compaction::SummaryOutcome {
                summary: "portable".into(),
                usage: None,
            }),
            Some(Ok(remote_result())),
        )
        .unwrap();
        assert_eq!(selected.summary, "portable");
        assert_eq!(selected.remote_usage.unwrap().input, 10);
        assert_eq!(
            selected.remote_details.unwrap()["remoteCompaction"]["version"],
            2
        );
    }

    #[test]
    fn remote_failure_falls_back_to_local_compaction() {
        let selected = select_compaction_outcome(
            &openai_model(),
            Ok(compaction::SummaryOutcome {
                summary: "portable".into(),
                usage: None,
            }),
            Some(Err(anyhow::anyhow!("remote unavailable"))),
        )
        .unwrap();
        assert_eq!(selected.summary, "portable");
        assert!(selected.remote_details.is_none());
    }

    #[test]
    fn remote_success_survives_local_summary_failure() {
        let selected = select_compaction_outcome(
            &openai_model(),
            Err(anyhow::anyhow!("summary unavailable")),
            Some(Ok(remote_result())),
        )
        .unwrap();
        assert!(
            selected
                .summary
                .contains("server-side compaction was applied")
        );
        assert!(selected.remote_details.is_some());
    }

    #[test]
    fn details_merge_keeps_file_operations_and_remote_artifact() {
        let merged = merge_compaction_details(
            serde_json::json!({"readFiles": ["a.rs"], "modifiedFiles": []}),
            Some(serde_json::json!({"remoteCompaction": {"version": 2}})),
        );
        assert_eq!(merged["readFiles"][0], "a.rs");
        assert_eq!(merged["remoteCompaction"]["version"], 2);
    }

    #[test]
    fn auto_compaction_guard_checks_settings_threshold_and_cancel() {
        let mut settings = Settings::default();
        settings.compaction.reserve_tokens = 20;
        let messages = vec![AgentMessage::user("x".repeat(360))];
        let mut model = openai_model();
        model.context_window = 100;
        assert!(auto_compaction_needed(&settings, &messages, &model, false));
        assert!(!auto_compaction_needed(&settings, &messages, &model, true));
        settings.compaction.enabled = false;
        assert!(!auto_compaction_needed(&settings, &messages, &model, false));
        settings.compaction.enabled = true;
        let mut assistant = super_ai::AssistantMessage::empty("test", "test", "test");
        assistant.usage.input = 1000;
        assistant
            .content
            .push(super_ai::ContentBlock::text("short note"));
        let edited = vec![
            AgentMessage::user("task"),
            AgentMessage::Assistant(assistant),
        ];
        assert!(auto_compaction_needed(&settings, &edited, &model, false));
        settings.experimental_context_file = true;
        assert!(!auto_compaction_needed(&settings, &edited, &model, false));
    }

    #[test]
    fn session_title_normalization_is_safe_and_bounded() {
        assert_eq!(
            normalize_session_title("  `Fix AUTH-123 login flow!`  \nignored").as_deref(),
            Some("Fix AUTH-123 login flow")
        );
        assert_eq!(normalize_session_title("\n\t"), None);
        assert_eq!(
            normalize_session_title("🚀".repeat(50).as_str())
                .unwrap()
                .chars()
                .count(),
            SESSION_TITLE_MAX_CHARS
        );
    }

    #[test]
    fn session_title_prompt_is_utf8_safe_and_bounded() {
        let prompt = "🚀".repeat(SESSION_TITLE_PROMPT_MAX_BYTES);
        let bounded = bounded_session_title_prompt(&prompt);
        assert!(bounded.len() <= SESSION_TITLE_PROMPT_MAX_BYTES);
        assert!(std::str::from_utf8(bounded.as_bytes()).is_ok());
    }

    #[test]
    fn cache_warming_uses_ninety_percent_with_ten_second_margin() {
        assert_eq!(
            cache_warming_delay(std::time::Duration::from_secs(300)),
            Some(std::time::Duration::from_secs(270))
        );
        assert_eq!(
            cache_warming_delay(std::time::Duration::from_secs(60)),
            Some(std::time::Duration::from_secs(50))
        );
        assert_eq!(
            cache_warming_delay(std::time::Duration::from_secs(10)),
            None
        );
    }
}
