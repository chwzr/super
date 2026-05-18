use std::collections::HashMap;
use std::path::Path;

use super::built_in::built_in_agents;
use super::definition::{AgentDefinition, AgentSource};
use super::loader::load_agents_from_dir;

pub struct AgentRegistry {
    by_type: HashMap<String, AgentDefinition>,
}

impl AgentRegistry {
    /// Load with built-ins only. Used in tests and as a degraded fallback.
    pub fn built_in_only() -> Self {
        Self::from_layers(built_in_agents(), Vec::new(), Vec::new())
    }

    /// Load with all three layers from disk. User layer scans
    /// `~/.claude/agents/*.md`; project layer scans `<cwd>/.claude/agents/*.md`.
    pub fn load(cwd: &Path) -> Self {
        let user = dirs::home_dir()
            .map(|h| h.join(".claude").join("agents"))
            .map(|d| load_agents_from_dir(&d, AgentSource::User))
            .unwrap_or_default();
        let project = load_agents_from_dir(
            &cwd.join(".claude").join("agents"),
            AgentSource::Project,
        );
        Self::from_layers(built_in_agents(), user, project)
    }

    /// Merge three layers with later writes winning: built-in < user < project.
    pub fn from_layers(
        built_in: Vec<AgentDefinition>,
        user: Vec<AgentDefinition>,
        project: Vec<AgentDefinition>,
    ) -> Self {
        let mut by_type: HashMap<String, AgentDefinition> = HashMap::new();
        for layer in [built_in, user, project] {
            for def in layer {
                by_type.insert(def.agent_type.clone(), def);
            }
        }
        Self { by_type }
    }

    pub fn resolve(&self, subagent_type: &str) -> Option<&AgentDefinition> {
        self.by_type.get(subagent_type)
    }

    pub fn list(&self) -> Vec<&AgentDefinition> {
        self.by_type.values().collect()
    }
}
