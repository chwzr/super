use async_trait::async_trait;
use serde_json::json;
use super::contract::{DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent};

pub struct MonitorTool;

#[async_trait]
impl Tool for MonitorTool {
    fn name(&self) -> &str { "Monitor" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Start a background monitor that streams events from a long-running script. \
         Each stdout line is an event. Supports tailing logs, polling for changes, \
         and watching processes.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/monitor.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command or script to monitor"},
                "description": {"type": "string", "description": "Human-readable description"},
                "timeout_ms": {"type": "integer", "description": "Maximum time to run in milliseconds"},
                "persistent": {"type": "boolean", "description": "Run for the lifetime of the session", "default": false}
            },
            "required": ["command", "description"]
        })
    }

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        ToolResult {
            content: "Monitor tool — file/process watching is not yet implemented in this CLI. \
                      This feature will stream events from long-running scripts in a future release."
                .into(),
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
