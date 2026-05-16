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
    /// the tool_result. In-process only.
    #[serde(skip)]
    pub inject_messages: Vec<String>,
    /// MCP-shaped passthrough metadata (structuredContent, _meta) for
    /// SDK consumers. Empty for non-MCP tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_meta: Option<serde_json::Value>,
    /// New conversation messages to inject. Used by tools whose effect
    /// includes adding user/assistant/system messages (TodoWrite,
    /// SendMessage). In-process only.
    #[serde(skip)]
    pub new_messages: Vec<serde_json::Value>,
}

impl Default for ToolResult {
    fn default() -> Self {
        Self {
            content: String::new(),
            is_error: false,
            metadata: None,
            inject_messages: Vec::new(),
            mcp_meta: None,
            new_messages: Vec::new(),
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
    /// Optional per-call progress channel. When set, the tool can push
    /// `ProgressEvent`s and the executor relays them onto the session bus.
    pub progress_sink: Option<ProgressSink>,
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    // ── Identity ──────────────────────────────────────────────────────────
    fn name(&self) -> &str;
    fn aliases(&self) -> &[&'static str] { &[] }
    fn user_facing_name(&self, _input: Option<&serde_json::Value>) -> String {
        self.name().into()
    }
    fn user_facing_name_bg_color(&self, _input: Option<&serde_json::Value>) -> Option<ColorHint> {
        None
    }

    // ── Discovery / loading ───────────────────────────────────────────────
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String;
    fn prompt(&self, _ctx: &PromptCtx) -> String;
    fn search_hint(&self) -> Option<&'static str> { None }
    fn should_defer(&self) -> bool { false }
    fn always_load(&self) -> bool { false }
    fn is_enabled(&self) -> bool { true }

    // ── Schemas ───────────────────────────────────────────────────────────
    fn input_schema(&self) -> serde_json::Value;
    fn output_schema(&self) -> Option<serde_json::Value> { None }
    fn max_result_size_chars(&self) -> usize { 200_000 }
    fn strict(&self) -> bool { false }

    // ── Behavior flags (per-input) ────────────────────────────────────────
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { false }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool { false }
    fn is_destructive(&self, _input: &serde_json::Value) -> bool { false }
    fn is_open_world(&self, _input: &serde_json::Value) -> bool { false }
    fn is_mcp(&self) -> bool { false }
    fn is_lsp(&self) -> bool { false }
    fn requires_user_interaction(&self) -> bool { false }
    fn interrupt_behavior(&self) -> InterruptBehavior { InterruptBehavior::Block }
    fn inputs_equivalent(&self, a: &serde_json::Value, b: &serde_json::Value) -> bool {
        a == b
    }
    fn get_path(&self, _input: &serde_json::Value) -> Option<std::path::PathBuf> { None }
    fn is_search_or_read_command(&self, _input: &serde_json::Value) -> SearchReadKind {
        SearchReadKind::default()
    }
    fn is_transparent_wrapper(&self) -> bool { false }

    // ── Activity / spinner ────────────────────────────────────────────────
    fn get_activity_description(&self, _input: &serde_json::Value) -> Option<String> { None }
    fn get_tool_use_summary(&self, _input: &serde_json::Value) -> Option<String> { None }

    // ── Validation + permissions ──────────────────────────────────────────
    async fn validate_input(
        &self,
        _input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> ValidationResult {
        ValidationResult::Ok
    }
    async fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> crate::tools::permission::PermissionResult {
        crate::tools::permission::PermissionResult::Allow {
            updated_input: None,
            decision_reason: Some(
                crate::tools::permission::DecisionReason::ToolDefault,
            ),
        }
    }
    async fn prepare_permission_matcher(
        &self,
        _input: &serde_json::Value,
    ) -> Option<Box<dyn Fn(&str) -> bool + Send + Sync>> {
        None
    }
    fn to_auto_classifier_input(&self, _input: &serde_json::Value) -> serde_json::Value {
        serde_json::Value::String(String::new())
    }
    fn backfill_observable_input(&self, _input: &mut serde_json::Value) {}

    // ── Execution ─────────────────────────────────────────────────────────
    async fn call(
        &self,
        input: serde_json::Value,
        context: &ToolCallContext,
        on_progress: Option<ProgressSink>,
    ) -> ToolResult;

    // ── Render hooks ──────────────────────────────────────────────────────
    fn render_tool_use_message(
        &self,
        _input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> shared::RenderSpec {
        shared::RenderSpec::Nothing
    }
    fn render_tool_use_tag(&self, _input: &serde_json::Value) -> Option<shared::RenderSpec> {
        None
    }
    fn render_tool_use_progress_message(
        &self,
        _progress: &[ProgressEvent],
        _opts: &RenderOpts,
    ) -> Option<shared::RenderSpec> {
        None
    }
    fn render_tool_use_queued_message(&self) -> Option<shared::RenderSpec> { None }
    fn render_tool_result_message(
        &self,
        _output: &serde_json::Value,
        _progress: &[ProgressEvent],
        _opts: &RenderOpts,
    ) -> Option<shared::RenderSpec> {
        None
    }
    fn render_tool_use_rejected_message(
        &self,
        _input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> Option<shared::RenderSpec> {
        None
    }
    fn render_tool_use_error_message(
        &self,
        _err: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> Option<shared::RenderSpec> {
        None
    }
    fn render_grouped_tool_use(
        &self,
        _calls: &[GroupedCall],
        _opts: &RenderOpts,
    ) -> Option<shared::RenderSpec> {
        None
    }
    fn is_result_truncated(&self, _output: &serde_json::Value) -> bool { false }
    fn extract_search_text(&self, _output: &serde_json::Value) -> Option<String> { None }

    // ── Result mapping ────────────────────────────────────────────────────
    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(
                output.as_str().map(String::from).unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
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
            progress_sink: None,
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
