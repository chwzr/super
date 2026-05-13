use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct ExitWorktreeTool;

#[async_trait]
impl Tool for ExitWorktreeTool {
    fn name(&self) -> &str { "ExitWorktree" }
    fn description(&self) -> &str {
        "Exit a worktree session and return to the original working directory. \
         Optionally keep or remove the worktree on disk."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["keep", "remove"], "description": "Whether to keep or remove the worktree on exit"},
                "discard_changes": {"type": "boolean", "description": "If true, discard uncommitted changes when removing", "default": false}
            },
            "required": ["action"]
        })
    }

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        ToolResult {
            content: "ExitWorktree — worktree exit is not yet implemented in this CLI. \
                      This feature will allow exiting isolated worktree sessions."
                .into(),
            is_error: false,
            metadata: None,
        }
    }
}