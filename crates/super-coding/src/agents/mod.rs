//! The agent system of the advanced mode: a session starts child agents
//! with `Agent`, writes to them with `SendMessage`, lists them with
//! `ListAgents` and stops a background one with `TaskStop`.
//!
//! - [`types`]: the built-in agent types and the definition files that add
//!   more.
//! - [`registry`]: [`AgentRegistry`], the agents one session started; a
//!   background run is a background task with the agent's id, announced
//!   with a notice when it ends.
//! - [`tools`]: the three tools.
//!
//! The tools need the advanced mode and the Subagents switch. Workflows and
//! loops run their children on the same engine
//! (`crate::child_turn`), without these tools; the registry keeps a record
//! of those children too, so that clients see them as agents.

pub mod registry;
pub mod tools;
pub mod types;

pub use registry::{
    AGENT_RUN_ENTRY, AgentInfo, AgentLink, AgentOrigin, AgentRegistry, AgentRequest, Isolation,
    MAX_DEPTH, MAX_RUNNING,
};
pub use types::{
    AgentType, AgentTypeSource, AgentTypeSummary, ToolChoice, built_in_types, discover, type_list,
    type_list_section, with_definitions,
};

/// What a session and its agents start from, besides the session itself:
/// the agent types of the definition files, the part of the session's
/// system prompt after its body, which an agent's system prompt repeats
/// after the instructions of its type, and the skills the `Skill` tool
/// loads. The host that builds the session's prompt sets it with
/// [`AgentSession::set_agent_resources`](crate::AgentSession::set_agent_resources).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentResources {
    pub definitions: Vec<AgentType>,
    /// `None` when the host did not say; an agent's prompt then ends with
    /// the working directory only. It holds no list of skills: in the
    /// advanced mode, where agents run, the session lists them on each
    /// request.
    pub prompt_context: Option<String>,
    /// The skills of the session's folder and of the user, as the host
    /// discovered them; the advanced mode's `Skill` tool loads them.
    pub skills: Vec<crate::skills::Skill>,
}

/// Hears of the agent sessions of one session, so that the host of that
/// session can show and send them like the session itself: the SDK wraps
/// each one so that its events and entries reach the clients, and a mirror
/// sends them to its streams nested under the session's. A host installs it
/// with
/// [`AgentSession::set_agent_observer`](crate::AgentSession::set_agent_observer).
///
/// The calls come from the session's own work (a tool call, the end of a
/// run); an observer must not block and must not call back into the
/// session's agent registry.
pub trait AgentObserver: Send + Sync {
    /// The session of the agent `agent_id` exists now: the agent was
    /// started, or its stored session was loaded to resume it or to be read.
    /// Its events go nowhere until the observer forwards them
    /// ([`AgentSession::forward_events`](crate::AgentSession::forward_events)).
    fn agent_session(&self, agent_id: &str, session: &std::sync::Arc<crate::AgentSession>);
    /// A run of the agent `agent_id` ended.
    fn agent_settled(&self, _agent_id: &str) {}
    /// The session moved to another conversation and forgot its agents.
    fn agents_reset(&self) {}
}
