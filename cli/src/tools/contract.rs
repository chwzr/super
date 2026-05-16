use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

/// Outcome of `Tool::validate_input` — pre-flight validation that the model
/// sees as a failed-tool result so it can self-correct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    Ok,
    Err { message: String, error_code: u32 },
}

/// User-facing color hint for a tool name pill in the TUI / web frontend.
/// Renderer maps to its own palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorHint {
    Default,
    Permission,
    Subagent,
    Tool,
}

/// What should happen when the user sends a new message while this tool
/// is mid-flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterruptBehavior {
    /// Stop the tool and discard its result.
    Cancel,
    /// Keep running; the new message waits.
    #[default]
    Block,
}

/// UI collapse hints. Tools that are "search-like" or "read-like" get
/// folded together in compact transcript views.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchReadKind {
    pub is_search: bool,
    pub is_read: bool,
    pub is_list: bool,
}

/// Streamed progress event from a long-running tool. Tools push these
/// into the `ProgressSink` channel during `call()`; the executor relays
/// them onto the session bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressEvent {
    Started,
    Update { data: serde_json::Value },
    Finished,
}

pub type ProgressSink = tokio::sync::mpsc::Sender<ProgressEvent>;

/// Snapshot of a group of parallel tool calls for `render_grouped_tool_use`.
#[derive(Debug, Clone)]
pub struct GroupedCall {
    pub tool_use_id: String,
    pub input: serde_json::Value,
    pub is_resolved: bool,
    pub is_error: bool,
    pub is_in_progress: bool,
    pub result: Option<ToolResultBlock>,
    pub progress: Vec<ProgressEvent>,
}

/// Anthropic-shaped tool result block. Produced by `Tool::map_tool_result_to_block`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultBlock {
    pub tool_use_id: String,
    pub content: ToolResultContent,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Multi(Vec<ToolResultPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultPart {
    Text { text: String },
    Image { source: ImageSource },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub kind: String, // "base64"
    pub media_type: String,
    pub data: String,
}

/// Light context bags passed to the description / prompt / render hooks.
#[derive(Debug, Clone, Default)]
pub struct DescriptionCtx {
    pub is_non_interactive_session: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PromptCtx {
    pub is_non_interactive_session: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RenderOpts {
    pub verbose: bool,
    pub is_transcript_mode: bool,
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

#[cfg(test)]
mod new_types_tests {
    use super::*;

    #[test]
    fn validation_result_ok_is_default() {
        let v = ValidationResult::Ok;
        assert!(matches!(v, ValidationResult::Ok));
    }

    #[test]
    fn interrupt_behavior_defaults_to_block() {
        assert_eq!(InterruptBehavior::default(), InterruptBehavior::Block);
    }

    #[test]
    fn search_read_kind_default_is_all_false() {
        let k = SearchReadKind::default();
        assert!(!k.is_search && !k.is_read && !k.is_list);
    }

    #[test]
    fn tool_result_block_text_round_trip() {
        let b = ToolResultBlock {
            tool_use_id: "tu_1".into(),
            content: ToolResultContent::Text("ok".into()),
            is_error: false,
        };
        let json = serde_json::to_string(&b).unwrap();
        let back: ToolResultBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_use_id, "tu_1");
    }
}
