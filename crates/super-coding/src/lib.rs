//! super-coding harness: sessions, compaction, settings,
//! project context, skills, prompt templates, search tools, and the
//! AgentSession facade.

mod account_failover;
#[cfg(test)]
mod agent_tools_tests;
pub mod agents;
#[cfg(test)]
mod background_tests;
mod child_turn;
pub mod compaction;
mod context_file;
pub mod context_files;
#[cfg(test)]
mod file_state_tests;
pub mod iterative;
mod jev;
pub mod notifications;
pub mod prompts;
pub mod session;
pub mod session_runner;
pub mod settings;
#[cfg(test)]
mod shell_tasks_tests;
pub mod skills;
pub mod system_prompt;
pub mod tasks;
pub mod tools;
pub mod trust;
pub mod workflows;
pub mod worktree;

pub use agents::{AgentInfo, AgentRegistry, AgentResources, AgentType, AgentTypeSummary};
pub use notifications::{Priority, agent_message, monitor_event, task_notification};
pub use session::entry::{SessionEntry, SessionHeader};
pub use session::manager::{SessionListing, SessionManager, default_session_dir};
pub use session_runner::{
    AgentSession, EphemeralResponse, PromptMode, REMOTE_SESSION_REFUSAL, SessionBackend,
    SessionEvent, SessionEventSink, TreeNavigationOutcome, TurnStarter, WorkflowTurnStatus,
};
pub use settings::Settings;
pub use tasks::{
    BackgroundTasks, TaskHandle, TaskInfo, TaskKind, TaskOutcome, TaskSpec, TaskStatus,
    TaskStopError, TaskUsage,
};
