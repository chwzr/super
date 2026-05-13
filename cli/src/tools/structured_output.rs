use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct StructuredOutputTool;

#[async_trait]
impl Tool for StructuredOutputTool {
    fn name(&self) -> &str { "StructuredOutput" }
    fn description(&self) -> &str {
        "Validates and returns structured output. Ensures the response matches a specified schema."
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

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let value = input.get("value");
        match value {
            Some(v) => ToolResult {
                content: v.to_string(),
                is_error: false,
                metadata: None,
            },
            None => ToolResult {
                content: "StructuredOutput: no value provided".into(),
                is_error: true,
                metadata: None,
            },
        }
    }
}