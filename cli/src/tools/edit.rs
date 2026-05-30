use crate::tools::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
use async_trait::async_trait;
use shared;

#[derive(Default)]
pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "Edit"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Performs exact string replacements in files.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/edit.txt").into()
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

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string" },
                "hunks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_start": { "type": "integer" },
                            "new_start": { "type": "integer" },
                            "lines": {
                                "type": "array",
                                "items": {
                                    "anyOf": [
                                        { "type": "object", "properties": { "kind": { "const": "context" }, "line": { "type": "string" } } },
                                        { "type": "object", "properties": { "kind": { "const": "add" }, "line": { "type": "string" } } },
                                        { "type": "object", "properties": { "kind": { "const": "remove" }, "line": { "type": "string" } } }
                                    ]
                                }
                            }
                        }
                    }
                },
                "replaced": { "type": "integer" },
                "replace_all": { "type": "boolean" }
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
            .map(|p| format!("Editing {}", p))
    }

    fn to_auto_classifier_input(&self, input: &serde_json::Value) -> serde_json::Value {
        let path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let new_str = input
            .get("new_string")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        serde_json::Value::String(format!("{}: {}", path, new_str))
    }

    fn backfill_observable_input(&self, input: &mut serde_json::Value) {
        if let Some(path_str) = input.get("file_path").and_then(|v| v.as_str()) {
            let p = std::path::Path::new(path_str);
            if p.is_relative() {
                if let Ok(canon) = std::env::current_dir().map(|cwd| cwd.join(p)) {
                    if let Some(s) = canon.to_str() {
                        input["file_path"] = serde_json::Value::String(s.to_string());
                    }
                }
            }
        }
    }

    fn is_destructive(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn render_tool_result_message(
        &self,
        output: &serde_json::Value,
        _progress: &[crate::tools::contract::ProgressEvent],
        _opts: &crate::tools::contract::RenderOpts,
    ) -> Option<shared::RenderSpec> {
        let file_path = output["file_path"].as_str().unwrap_or("").to_string();
        let hunks_json = output["hunks"].as_array().cloned().unwrap_or_default();

        let hunks: Vec<shared::DiffHunk> = hunks_json
            .iter()
            .filter_map(|h| {
                let lines: Vec<shared::DiffLine> = h["lines"]
                    .as_array()?
                    .iter()
                    .filter_map(|l| match l["kind"].as_str()? {
                        "context" => Some(shared::DiffLine::Context {
                            line: l["line"].as_str()?.to_string(),
                        }),
                        "add" => Some(shared::DiffLine::Add {
                            line: l["line"].as_str()?.to_string(),
                        }),
                        "remove" => Some(shared::DiffLine::Remove {
                            line: l["line"].as_str()?.to_string(),
                        }),
                        _ => None,
                    })
                    .collect();
                Some(shared::DiffHunk {
                    old_start: h["old_start"].as_u64()? as u32,
                    new_start: h["new_start"].as_u64()? as u32,
                    lines,
                })
            })
            .collect();

        Some(shared::RenderSpec::Group {
            children: vec![
                shared::RenderSpec::Status {
                    state: shared::StatusState::Success,
                    message: Some(format!("Updated {file_path}")),
                },
                shared::RenderSpec::Diff { file_path, hunks },
            ],
        })
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

        let old_string = match input.get("old_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult {
                    content: "Missing required parameter: old_string".to_string(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let new_string = match input.get("new_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult {
                    content: "Missing required parameter: new_string".to_string(),
                    is_error: true,
                    ..Default::default()
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
                ..Default::default()
            };
        }

        let path = std::path::Path::new(file_path);
        if !path.exists() {
            return ToolResult {
                content: format!("File not found: {}", file_path),
                is_error: true,
                ..Default::default()
            };
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Error reading file {}: {}", file_path, e),
                    is_error: true,
                    ..Default::default()
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
                ..Default::default()
            };
        }

        if occurrences > 1 && !replace_all {
            return ToolResult {
                content: format!(
                    "Found {} occurrences of the search text in {}. Use replace_all=true to replace all occurrences, or provide more surrounding context to make the match unique.",
                    occurrences, file_path
                ),
                is_error: true,
                ..Default::default()
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
                    content.clone()
                }
            }
        };

        match std::fs::write(path, &new_content) {
            Ok(()) => {
                let old_lines: Vec<&str> = content.lines().collect();
                let new_lines: Vec<&str> = new_content.lines().collect();

                let mut hunks: Vec<serde_json::Value> = Vec::new();
                let mut diff_lines: Vec<serde_json::Value> = Vec::new();
                let mut old_start = 0usize;
                let mut new_start = 0usize;
                let mut in_change = false;

                let max_len = old_lines.len().max(new_lines.len());
                for i in 0..max_len {
                    let old_line = old_lines.get(i).copied();
                    let new_line = new_lines.get(i).copied();
                    if old_line != new_line {
                        if !in_change {
                            old_start = i.saturating_sub(1);
                            new_start = old_start;
                            if old_start < i && old_start < old_lines.len() {
                                diff_lines.push(serde_json::json!({"kind": "context", "line": old_lines[old_start]}));
                            }
                            in_change = true;
                        }
                        if let Some(l) = old_line {
                            diff_lines.push(serde_json::json!({"kind": "remove", "line": l}));
                        }
                        if let Some(l) = new_line {
                            diff_lines.push(serde_json::json!({"kind": "add", "line": l}));
                        }
                    } else if in_change {
                        if let Some(l) = old_line {
                            diff_lines.push(serde_json::json!({"kind": "context", "line": l}));
                        }
                        if !diff_lines.is_empty() {
                            hunks.push(serde_json::json!({
                                "old_start": old_start + 1,
                                "new_start": new_start + 1,
                                "lines": diff_lines
                            }));
                        }
                        diff_lines = Vec::new();
                        in_change = false;
                    }
                }
                if !diff_lines.is_empty() {
                    hunks.push(serde_json::json!({
                        "old_start": old_start + 1,
                        "new_start": new_start + 1,
                        "lines": diff_lines
                    }));
                }

                let result_json = serde_json::json!({
                    "file_path": file_path,
                    "hunks": hunks,
                    "replaced": occurrences,
                    "replace_all": replace_all
                });

                // Notify LSP of file change + save
                if let Some(ref mut guard) = crate::lsp::get_lsp_manager() {
                    if let Some(ref mut manager) = guard.as_mut() {
                        let rt = tokio::runtime::Handle::current();
                        let _ = rt.block_on(manager.change_file(file_path, &new_content));
                        let _ = rt.block_on(manager.save_file(file_path));
                    }
                }

                let mut meta = std::collections::HashMap::new();
                meta.insert("replaced".to_string(), occurrences.to_string());
                meta.insert("replace_all".to_string(), replace_all.to_string());

                ToolResult {
                    content: result_json.to_string(),
                    is_error: false,
                    metadata: Some(meta),
                    ..Default::default()
                }
            }
            Err(e) => ToolResult {
                content: format!("Error writing file {}: {}", file_path, e),
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
