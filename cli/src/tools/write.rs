use crate::tools::contract::{DescriptionCtx, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent, ProgressSink};
use async_trait::async_trait;

#[derive(Default)]
pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "Write"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Creates or overwrites a file with the given content.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/write.txt").into()
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["file_path", "content"]
        })
    }

    fn is_destructive(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let file_path = match input.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return ToolResult {
                    content: "Missing required parameter: file_path".to_string(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => {
                return ToolResult {
                    content: "Missing required parameter: content".to_string(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let path = std::path::Path::new(file_path);

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return ToolResult {
                        content: format!("Error creating parent directories for {}: {}", file_path, e),
                        is_error: true,
                        ..Default::default()
                    };
                }
            }
        }

        match std::fs::write(path, content) {
            Ok(()) => {
                let mut meta = std::collections::HashMap::new();
                meta.insert("bytes_written".to_string(), content.len().to_string());
                ToolResult {
                    content: format!(
                        "Successfully wrote {} bytes to {}",
                        content.len(),
                        file_path
                    ),
                    is_error: false,
                    metadata: Some(meta),
                    inject_messages: Vec::new(),
                    mcp_meta: None,
                    new_messages: Vec::new(),
                }
            }
            Err(e) => ToolResult {
                content: format!("Error writing to {}: {}", file_path, e),
                is_error: true,
                ..Default::default()
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
