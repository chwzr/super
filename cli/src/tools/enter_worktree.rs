use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct EnterWorktreeTool;

#[async_trait]
impl Tool for EnterWorktreeTool {
    fn name(&self) -> &str { "EnterWorktree" }
    fn description(&self) -> &str {
        "Create an isolated git worktree and switch the session into it. \
         Used for working on features in isolation."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Optional name for the worktree"},
                "path": {"type": "string", "description": "Optional path to an existing worktree"}
            }
        })
    }

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        ToolResult {
            content: "EnterWorktree — git worktree isolation is not yet implemented in this CLI. \
                      This feature will create isolated worktrees for feature development."
                .into(),
            is_error: false,
            metadata: None,
        }
    }
}