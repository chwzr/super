use crate::tools::contract::{DescriptionCtx, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent, ProgressSink};
use async_trait::async_trait;
use shared;

#[derive(Default)]
pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Finds files matching a glob pattern.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/glob_tool.txt").into()
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match (e.g., \"**/*.rs\", \"src/**/*.toml\")"
                },
                "path": {
                    "type": "string",
                    "description": "The directory to search in (defaults to current working directory)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "durationMs": { "type": "number", "description": "Time the search took in milliseconds" },
                "numFiles": { "type": "integer", "description": "Number of matching files found" },
                "filenames": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Matching file paths"
                },
                "truncated": { "type": "boolean", "description": "True when results exceed the limit" }
            }
        }))
    }

    fn render_tool_result_message(
        &self,
        output: &serde_json::Value,
        _progress: &[crate::tools::contract::ProgressEvent],
        _opts: &crate::tools::contract::RenderOpts,
    ) -> Option<shared::RenderSpec> {
        let filenames: Vec<shared::PathEntry> = output["filenames"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| shared::PathEntry {
                        path: std::path::PathBuf::from(s),
                        line: None,
                        preview: None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let num_files = output["numFiles"].as_u64().unwrap_or(filenames.len() as u64) as usize;
        let truncated = output["truncated"].as_bool().unwrap_or(false);

        Some(shared::RenderSpec::PathList {
            entries: filenames,
            total: num_files,
            truncated,
        })
    }

    fn extract_search_text(&self, output: &serde_json::Value) -> Option<String> {
        output["filenames"].as_array().map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(&self, input: serde_json::Value, context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let start = std::time::Instant::now();

        let pattern = match input.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return ToolResult {
                    content: "Missing required parameter: pattern".to_string(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let base_path = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => std::path::PathBuf::from(p),
            None => context.cwd.clone(),
        };

        // If base_path is relative, make absolute from cwd
        let base_path = if base_path.is_relative() {
            context.cwd.join(&base_path)
        } else {
            base_path
        };

        // Construct the full glob pattern
        let full_pattern = if pattern.starts_with('/') {
            // Absolute pattern: strip leading slash and combine
            let rel_pattern = pattern.trim_start_matches('/');
            let glob_str = base_path.join(rel_pattern).to_string_lossy().to_string();
            // Ensure forward slashes for glob crate
            if cfg!(windows) {
                glob_str.replace('\\', "/")
            } else {
                glob_str
            }
        } else {
            let glob_str = base_path.join(pattern).to_string_lossy().to_string();
            if cfg!(windows) {
                glob_str.replace('\\', "/")
            } else {
                glob_str
            }
        };

        // We need to strip the base_path prefix from results
        // First create a canonical form for matching
        let _canon_base = if base_path.as_os_str().is_empty() {
            &context.cwd
        } else {
            &base_path
        };

        let mut entries: Vec<(std::path::PathBuf, std::time::SystemTime)> = Vec::new();

        match glob::glob(&full_pattern) {
            Ok(paths) => {
                for entry in paths {
                    match entry {
                        Ok(p) => {
                            if p.is_file() || p.is_dir() {
                                let mtime = std::fs::metadata(&p)
                                    .ok()
                                    .and_then(|m| m.modified().ok())
                                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                                entries.push((p, mtime));
                            }
                        }
                        Err(_) => continue,
                    }
                }
            }
            Err(e) => {
                return ToolResult {
                    content: format!("Invalid glob pattern '{}': {}", pattern, e),
                    is_error: true,
                    ..Default::default()
                };
            }
        }

        // Sort by mtime (newest first)
        entries.sort_by(|a, b| b.1.cmp(&a.1));

        // Limit to 100 results
        let total = entries.len();
        let limited: Vec<_> = if entries.len() > 100 {
            entries[..100].to_vec()
        } else {
            entries
        };

        // Relativize paths
        let cwd = &context.cwd;
        let result_lines: Vec<String> = limited
            .iter()
            .map(|(p, _)| {
                let rel = pathdiff::diff_paths(p, cwd).unwrap_or_else(|| p.clone());
                rel.to_string_lossy().to_string()
            })
            .collect();

        let duration_ms = start.elapsed().as_millis() as u64;
        let num_files = total;
        let truncated = total > 100;

        let result_json = serde_json::json!({
            "durationMs": duration_ms,
            "numFiles": num_files,
            "filenames": result_lines,
            "truncated": truncated
        });

        let mut meta = std::collections::HashMap::new();
        meta.insert("total".to_string(), total.to_string());
        meta.insert("returned".to_string(), limited.len().to_string());

        ToolResult {
            content: result_json.to_string(),
            is_error: false,
            metadata: Some(meta),
            ..Default::default()
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
