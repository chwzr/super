use async_trait::async_trait;
use serde_json::json;
use super::contract::{DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent};

pub struct SleepTool;

#[async_trait]
impl Tool for SleepTool {
    fn name(&self) -> &str { "Sleep" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Sleep for a specified duration in milliseconds. Max 300000ms (5 minutes).".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/sleep.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "duration_ms": {"type": "integer", "description": "Duration to sleep in milliseconds", "maximum": 300000}
            },
            "required": ["duration_ms"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let ms = input["duration_ms"].as_u64().unwrap_or(1000).min(300000);
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        ToolResult { content: format!("Slept for {ms}ms"), is_error: false, inject_messages: Vec::new(), metadata: None, mcp_meta: None, new_messages: Vec::new() }
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
