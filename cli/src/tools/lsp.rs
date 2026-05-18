use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
use async_trait::async_trait;
use serde_json::json;

pub struct LspTool;

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &str {
        "LSP"
    }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Interact with Language Server Protocol (LSP) servers to get code intelligence features."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/lsp.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {"type": "string", "enum": [
                    "goToDefinition", "findReferences", "hover",
                    "documentSymbol", "workspaceSymbol", "goToImplementation",
                    "prepareCallHierarchy", "incomingCalls", "outgoingCalls"
                ]},
                "filePath": {"type": "string"},
                "line": {"type": "integer"},
                "character": {"type": "integer"}
            },
            "required": ["operation", "filePath", "line", "character"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "result": {"description": "LSP operation result"}
            }
        }))
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let operation = input["operation"].as_str().unwrap_or("");
        let file_path = input["filePath"].as_str().unwrap_or("");
        ToolResult {
            content: format!(
                "LSP {operation} on {file_path} — LSP server integration not yet implemented"
            ),
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
                output
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}
