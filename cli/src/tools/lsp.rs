use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct LspTool;

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &str { "LSP" }
    fn description(&self) -> &str { "Language server protocol operations: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol, goToImplementation." }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {"type": "string", "enum": ["goToDefinition", "findReferences", "hover", "documentSymbol", "workspaceSymbol", "goToImplementation"]},
                "filePath": {"type": "string"},
                "line": {"type": "integer"},
                "character": {"type": "integer"}
            },
            "required": ["operation", "filePath", "line", "character"]
        })
    }
    fn is_read_only(&self) -> bool { true }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let operation = input["operation"].as_str().unwrap_or("");
        let file_path = input["filePath"].as_str().unwrap_or("");
        ToolResult { content: format!("LSP {operation} on {file_path} — LSP server integration not yet implemented"), is_error: false, ..Default::default() }
    }
}