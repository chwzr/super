use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
    /// Additional text blocks to inject into the conversation alongside
    /// the tool_result. Never serialized — in-process only.
    #[serde(skip)]
    pub inject_messages: Vec<String>,
}

impl Default for ToolResult {
    fn default() -> Self {
        ToolResult {
            content: String::new(),
            is_error: false,
            metadata: None,
            inject_messages: Vec::new(),
        }
    }
}

pub struct ToolCallContext {
    pub cwd: std::path::PathBuf,
    pub permission_mode: crate::state::store::PermissionMode,
    pub abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
    /// When the tool is invoked from inside a subagent, this carries the
    /// parent's invoking tool_use_id so emitted events can be demuxed by
    /// consumers (TUI, server forwarder, sidechain transcript writer).
    pub parent_tool_use_id: Option<String>,
    /// Handle to the session bus. Available to tools that need to emit
    /// events (subagent spawn, progress tickers).
    pub bus: Option<std::sync::Arc<crate::conversation::session_bus::SessionBus>>,
    /// When true, tools that would otherwise prompt the user (AskUserQuestion,
    /// permission prompts, etc.) must auto-deny and return an error result.
    /// Set by AgentTool for async subagents.
    pub auto_deny_prompts: bool,
    /// The `tool_use_id` of the in-flight tool_use block that invoked this
    /// tool. Empty string is acceptable when constructed outside the tool
    /// loop (e.g. unit tests that aren't testing this field).
    pub tool_use_id: String,
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
    fn tool_result_default_has_empty_inject_messages() {
        let r = ToolResult::default();
        assert!(r.inject_messages.is_empty());
        assert!(!r.is_error);
    }

    #[test]
    fn tool_result_inject_messages_not_in_json() {
        let r = ToolResult {
            content: "hello".into(),
            inject_messages: vec!["injected".into()],
            ..Default::default()
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("inject_messages"), "inject_messages must not appear in JSON: {json}");
        assert!(!json.contains("injected"));
    }

    #[test]
    fn tool_call_context_carries_auto_deny_flag() {
        let ctx = ToolCallContext {
            cwd: std::path::PathBuf::from("/tmp"),
            permission_mode: crate::state::store::PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: true,
            tool_use_id: "tu_test".into(),
        };
        assert!(ctx.auto_deny_prompts);
        assert_eq!(ctx.tool_use_id, "tu_test");
    }
}
