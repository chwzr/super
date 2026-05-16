use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use super::contract::{DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent};
use crate::state::store::PermissionMode;

pub struct EnterPlanModeTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn name(&self) -> &str { "EnterPlanMode" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Enters plan mode, restricting tools to read-only operations.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/enter_plan_mode.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        self.store.set_state(|s| {
            s.permission_mode = PermissionMode::Plan;
        });
        ToolResult {
            content: "Entered plan mode. Only read-only tools are available.".into(),
            is_error: false,
            inject_messages: Vec::new(),
            metadata: None,
            mcp_meta: None,
            new_messages: Vec::new(),
        }
    }

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