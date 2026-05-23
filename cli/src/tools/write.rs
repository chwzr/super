use crate::tools::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
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

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string" },
                "bytes_written": { "type": "integer" },
                "overwritten": { "type": "boolean" }
            }
        }))
    }

    fn get_path(&self, input: &serde_json::Value) -> Option<std::path::PathBuf> {
        input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from)
    }

    fn get_activity_description(&self, input: &serde_json::Value) -> Option<String> {
        input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|p| format!("Writing {}", p))
    }

    fn to_auto_classifier_input(&self, input: &serde_json::Value) -> serde_json::Value {
        let path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let preview = if content.len() > 200 {
            format!("{}...", &content[..200])
        } else {
            content.to_string()
        };
        serde_json::Value::String(format!("{}: {}", path, preview))
    }

    fn is_destructive(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
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
                        content: format!(
                            "Error creating parent directories for {}: {}",
                            file_path, e
                        ),
                        is_error: true,
                        ..Default::default()
                    };
                }
            }
        }

        let existed_before = path.exists();
        match std::fs::write(path, content) {
            Ok(()) => {
                // Notify LSP of file save (and open if new)
                if let Some(ref mut guard) = crate::lsp::get_lsp_manager() {
                    if let Some(ref mut manager) = guard.as_mut() {
                        let rt = tokio::runtime::Handle::current();
                        if !existed_before {
                            let _ = rt.block_on(manager.open_file(file_path, content));
                        }
                        let _ = rt.block_on(manager.save_file(file_path));
                    }
                }

                let mut meta = std::collections::HashMap::new();
                meta.insert("bytes_written".to_string(), content.len().to_string());
                meta.insert("overwritten".to_string(), existed_before.to_string());
                ToolResult {
                    content: format!(
                        "Successfully {} {} bytes to {}{}",
                        if existed_before { "overwrote" } else { "wrote" },
                        content.len(),
                        file_path,
                        if existed_before {
                            " (file was overwritten)"
                        } else {
                            ""
                        }
                    ),
                    is_error: false,
                    metadata: Some(meta),
                    ..Default::default()
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
                output
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}
