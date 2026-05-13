use crate::tools::contract::{Tool, ToolCallContext, ToolResult};
use async_trait::async_trait;

#[derive(Default)]
pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "Edit"
    }

    fn description(&self) -> &str {
        "Performs exact string replacements in files."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to modify"
                },
                "old_string": {
                    "type": "string",
                    "description": "The text to replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The text to replace it with (must be different from old_string)"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences of old_string (default false)"
                }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    fn is_destructive(&self) -> bool {
        true
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let file_path = match input.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return ToolResult {
                    content: "Missing required parameter: file_path".to_string(),
                    is_error: true,
                    metadata: None,
                };
            }
        };

        let old_string = match input.get("old_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult {
                    content: "Missing required parameter: old_string".to_string(),
                    is_error: true,
                    metadata: None,
                };
            }
        };

        let new_string = match input.get("new_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult {
                    content: "Missing required parameter: new_string".to_string(),
                    is_error: true,
                    metadata: None,
                };
            }
        };

        let replace_all = input
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if old_string == new_string {
            return ToolResult {
                content: "old_string and new_string must be different".to_string(),
                is_error: true,
                metadata: None,
            };
        }

        let path = std::path::Path::new(file_path);
        if !path.exists() {
            return ToolResult {
                content: format!("File not found: {}", file_path),
                is_error: true,
                metadata: None,
            };
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Error reading file {}: {}", file_path, e),
                    is_error: true,
                    metadata: None,
                };
            }
        };

        let occurrences = content.matches(old_string).count();

        if occurrences == 0 {
            return ToolResult {
                content: format!(
                    "The specified text was not found in {}. The text to replace must appear at least once.",
                    file_path
                ),
                is_error: true,
                metadata: None,
            };
        }

        if occurrences > 1 && !replace_all {
            return ToolResult {
                content: format!(
                    "Found {} occurrences of the search text in {}. Use replace_all=true to replace all occurrences, or provide more surrounding context to make the match unique.",
                    occurrences, file_path
                ),
                is_error: true,
                metadata: None,
            };
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            // Replace only the first occurrence
            match content.find(old_string) {
                Some(pos) => {
                    let before = &content[..pos];
                    let after = &content[pos + old_string.len()..];
                    format!("{}{}{}", before, new_string, after)
                }
                None => {
                    // Should not happen since we checked occurrences > 0
                    content
                }
            }
        };

        match std::fs::write(path, &new_content) {
            Ok(()) => {
                let mut meta = std::collections::HashMap::new();
                meta.insert("replaced".to_string(), occurrences.to_string());
                meta.insert("replace_all".to_string(), replace_all.to_string());

                ToolResult {
                    content: format!(
                        "Successfully replaced {} occurrence(s) in {}",
                        occurrences, file_path
                    ),
                    is_error: false,
                    metadata: Some(meta),
                }
            }
            Err(e) => ToolResult {
                content: format!("Error writing file {}: {}", file_path, e),
                is_error: true,
                metadata: None,
            },
        }
    }
}
