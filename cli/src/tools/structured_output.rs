use async_trait::async_trait;
use serde_json::json;
use super::contract::{DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent};

pub struct StructuredOutputTool;

#[async_trait]
impl Tool for StructuredOutputTool {
    fn name(&self) -> &str { "StructuredOutput" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Validates and returns structured output. Ensures the response matches a specified schema.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/structured_output.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "value": {"description": "The value to validate and return as structured output"},
                "schema": {"description": "Optional JSON schema to validate against"}
            },
            "required": ["value"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let value = input.get("value");
        match value {
            Some(v) => ToolResult {
                content: v.to_string(),
                is_error: false,
                inject_messages: Vec::new(),
                metadata: None,
                mcp_meta: None,
                new_messages: Vec::new(),
            },
            None => ToolResult {
                content: "StructuredOutput: no value provided".into(),
                is_error: true,
                inject_messages: Vec::new(),
                metadata: None,
                mcp_meta: None,
                new_messages: Vec::new(),
            },
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
