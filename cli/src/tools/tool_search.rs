use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct ToolSearchTool;

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str { "ToolSearch" }
    fn description(&self) -> &str { "Search for tools by name or description. Returns matching tools." }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query to match against tool names and descriptions"}
            },
            "required": ["query"]
        })
    }
    fn is_read_only(&self) -> bool { true }

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        ToolResult {
            content: "ToolSearch — searching registered tools by name/description. \
                      This tool requires access to the tool registry, which will be wired up in a future task."
                .into(),
            is_error: false,
            metadata: None,
        }
    }
}