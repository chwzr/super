use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspServerState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub extension_to_language: HashMap<String, String>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    #[serde(default, rename = "initialization_options")]
    pub initialization_options: Option<serde_json::Value>,
    #[serde(default, rename = "workspace_folder")]
    pub workspace_folder: Option<String>,
    #[serde(default = "default_startup_timeout_ms")]
    pub startup_timeout_ms: u64,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
}

fn default_startup_timeout_ms() -> u64 {
    30_000
}
fn default_max_restarts() -> u32 {
    3
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitState {
    NotStarted,
    Pending,
    Success,
    Failed,
}
