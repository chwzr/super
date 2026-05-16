use async_trait::async_trait;
use serde_json::json;
use super::contract::{DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent};

pub struct ToolSearchTool;

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str { "ToolSearch" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Search for tools by name or description. Returns matching tools.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/tool_search.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query to match against tool names and descriptions"}
            },
            "required": ["query"]
        })
    }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        ToolResult {
            content: "ToolSearch — searching registered tools by name/description. \
                      This tool requires access to the tool registry, which will be wired up in a future task."
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
