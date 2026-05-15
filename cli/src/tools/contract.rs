use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

pub struct ToolCallContext {
    pub cwd: std::path::PathBuf,
    pub permission_mode: crate::state::store::PermissionMode,
    pub abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
    /// When the tool is invoked from inside a subagent, this carries the
    /// parent's invoking tool_use_id so emitted events can be demuxed by
    /// consumers (TUI, server forwarder, sidechain transcript writer).
    /// Always `None` in v1 — subagent execution is out of scope.
    pub parent_tool_use_id: Option<String>,
    /// Handle to the session bus, available to tools that need to emit
    /// events (currently only AgentTool when subagents are wired up).
    /// `None` v1.
    pub bus: Option<std::sync::Arc<crate::conversation::session_bus::SessionBus>>,
    /// When true, tools that would otherwise prompt the user (AskUserQuestion,
    /// permission prompts, etc.) must auto-deny and return an error result.
    /// Set by AgentTool for async subagents.
    pub auto_deny_prompts: bool,
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    async fn call(&self, input: serde_json::Value, context: &ToolCallContext) -> ToolResult;

    fn name(&self) -> &str;

    fn description(&self) -> &str;

    fn input_schema(&self) -> serde_json::Value;

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_destructive(&self) -> bool {
        false
    }

    fn check_permission(&self, _input: &serde_json::Value) -> crate::tools::permission::Decision {
        crate::tools::permission::Decision::Ask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_context_carries_auto_deny_flag() {
        let ctx = ToolCallContext {
            cwd: std::path::PathBuf::from("/tmp"),
            permission_mode: crate::state::store::PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: true,
        };
        assert!(ctx.auto_deny_prompts);
    }
}
