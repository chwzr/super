use async_trait::async_trait;
use serde_json::json;
use super::contract::{DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent};

pub struct ExitWorktreeTool;

#[async_trait]
impl Tool for ExitWorktreeTool {
    fn name(&self) -> &str { "ExitWorktree" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Exit a worktree session and return to the original working directory. \
         Optionally keep or remove the worktree on disk.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/exit_worktree.txt").into()
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

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        ToolResult {
            content: "ExitWorktree — worktree exit is not yet implemented in this CLI. \
                      This feature will allow exiting isolated worktree sessions."
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
