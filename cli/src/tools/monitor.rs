use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct MonitorTool;

#[async_trait]
impl Tool for MonitorTool {
    fn name(&self) -> &str { "Monitor" }
    fn description(&self) -> &str {
        "Start a background monitor that streams events from a long-running script. \
         Each stdout line is an event. Supports tailing logs, polling for changes, \
         and watching processes."
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

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        ToolResult {
            content: "Monitor tool — file/process watching is not yet implemented in this CLI. \
                      This feature will stream events from long-running scripts in a future release."
                .into(),
            is_error: false,
            metadata: None,
        }
    }
}