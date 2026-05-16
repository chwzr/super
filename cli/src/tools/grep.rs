use crate::tools::contract::{DescriptionCtx, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent, ProgressSink};
use async_trait::async_trait;

#[derive(Default)]
pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "Grep"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Searches file contents using regex patterns.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/grep.txt").into()
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "The directory to search in (defaults to current working directory)"
                },
                "glob": {
                    "type": "string",
                    "description": "A glob pattern to filter files (e.g., \"**/*.rs\")"
                },
                "head_limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default 250)",
                    "minimum": 1
                }
            },
            "required": ["pattern"]
        })
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(&self, input: serde_json::Value, context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let pattern_str = match input.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return ToolResult {
                    content: "Missing required parameter: pattern".to_string(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let regex = match regex::Regex::new(pattern_str) {
            Ok(r) => r,
            Err(e) => {
                return ToolResult {
                    content: format!("Invalid regex pattern '{}': {}", pattern_str, e),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let base_path = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => {
                let p = std::path::PathBuf::from(p);
                if p.is_relative() {
                    context.cwd.join(&p)
                } else {
                    p
                }
            }
            None => context.cwd.clone(),
        };

        let glob_filter = input.get("glob").and_then(|v| v.as_str());

        let head_limit = input
            .get("head_limit")
            .and_then(|v| v.as_i64())
            .map(|v| v as usize)
            .unwrap_or(250);

        let cwd = &context.cwd;
        let mut results: Vec<String> = Vec::new();

        // Build a glob matcher if glob_filter is provided
        let glob_set = glob_filter.map(|g| {
            let full_glob = if g.starts_with('/') {
                let rel = g.trim_start_matches('/');
                let gs = base_path.join(rel).to_string_lossy().to_string();
                if cfg!(windows) { gs.replace('\\', "/") } else { gs }
            } else {
                let gs = base_path.join(g).to_string_lossy().to_string();
                if cfg!(windows) { gs.replace('\\', "/") } else { gs }
            };
            glob::glob(&full_glob)
        });

        // If glob filter is provided, walk only matching files
        if let Some(Ok(glob_paths)) = glob_set {
            for entry in glob_paths {
                if let Ok(path) = entry {
                    if !path.is_file() {
                        continue;
                    }
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        for (line_num, line) in content.lines().enumerate() {
                            if results.len() >= head_limit {
                                break;
                            }
                            if regex.is_match(line) {
                                let rel = pathdiff::diff_paths(&path, cwd)
                                    .unwrap_or_else(|| path.clone());
                                results.push(format!("{}:{}:{}", rel.display(), line_num + 1, line));
                            }
                        }
                    }
                    if results.len() >= head_limit {
                        break;
                    }
                }
            }
        } else {
            // No glob filter: walk the filesystem
            let walker = walkdir::WalkDir::new(&base_path)
                .follow_links(true)
                .into_iter()
                .filter_entry(|e| {
                    let name = e.file_name().to_string_lossy();
                    // Skip hidden directories and common non-text paths
                    if e.depth() == 0 {
                        return true;
                    }
                    if e.file_type().is_dir() {
                        !name.starts_with('.') && name != "node_modules" && name != "target"
                    } else {
                        true
                    }
                });

            for entry in walker {
                if results.len() >= head_limit {
                    break;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                if !entry.file_type().is_file() {
                    continue;
                }

                let path = entry.path();

                // Skip binary files by checking common binary extensions
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    let binary_exts = [
                        "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp",
                        "pdf", "zip", "gz", "tar", "bz2", "xz",
                        "so", "dylib", "dll", "exe", "bin",
                        "o", "obj", "class", "pyc",
                        "ttf", "otf", "woff", "woff2", "eot",
                        "mp3", "mp4", "avi", "mov", "mkv",
                        "db", "sqlite", "lock",
                    ];
                    if binary_exts.contains(&ext) {
                        continue;
                    }
                }

                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                for (line_num, line) in content.lines().enumerate() {
                    if results.len() >= head_limit {
                        break;
                    }
                    if regex.is_match(line) {
                        let rel = pathdiff::diff_paths(path, cwd)
                            .unwrap_or_else(|| path.to_path_buf());
                        results.push(format!("{}:{}:{}", rel.display(), line_num + 1, line));
                    }
                }
            }
        }

        let total = results.len();
        let result = if results.is_empty() {
            format!(
                "No matches found for pattern '{}' in {}",
                pattern_str,
                base_path.display()
            )
        } else {
            results.join("\n")
        };

        let mut meta = std::collections::HashMap::new();
        meta.insert("total".to_string(), total.to_string());

        ToolResult {
            content: result,
            is_error: false,
            metadata: Some(meta),
            inject_messages: Vec::new(),
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
