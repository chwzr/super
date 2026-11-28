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
//! (`crate::child_turn`), without these tools.

pub mod registry;
pub mod tools;
pub mod types;

pub use registry::{
    AgentInfo, AgentLink, AgentRegistry, AgentRequest, Isolation, MAX_DEPTH, MAX_RUNNING,
};
pub use types::{
    AgentType, AgentTypeSource, AgentTypeSummary, ToolChoice, built_in_types, discover, type_list,
    type_list_section, with_definitions,
};

/// What a session's agents start from, besides the session itself: the
/// agent types of the definition files, and the part of the session's
/// system prompt after its body, which an agent's system prompt repeats
/// after the instructions of its type. The host that builds the session's
/// prompt sets it with
/// [`AgentSession::set_agent_resources`](crate::AgentSession::set_agent_resources).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentResources {
    pub definitions: Vec<AgentType>,
    /// `None` when the host did not say; an agent's prompt then ends with
    /// the working directory only.
    pub prompt_context: Option<String>,
}
