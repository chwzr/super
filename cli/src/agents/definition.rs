use crate::state::store::PermissionMode;

#[derive(Debug, Clone)]
pub struct AgentDefinition {
    pub agent_type: String,
    pub description: String,
    pub system_prompt: String,
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Vec<String>,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub max_turns: Option<u32>,
    pub source: AgentSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSource {
    BuiltIn,
    User,
    Project,
}
