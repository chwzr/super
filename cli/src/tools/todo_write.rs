use async_trait::async_trait;
use serde_json::json;
use crate::tools::contract::{DescriptionCtx, PromptCtx, RenderOpts, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent, ProgressSink};

pub struct TodoWriteTool;

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str { "TodoWrite" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Writes a structured task list.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/todo_write.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {"type": "string"},
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"]
                            }
                        }
                    }
                }
            },
            "required": ["todos"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let todos = input["todos"].as_array().map(|a| a.len()).unwrap_or(0);
        ToolResult {
            content: format!("Todo list updated ({todos} items)"),
            is_error: false,
            ..Default::default()
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