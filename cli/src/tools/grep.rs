use crate::tools::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, SearchReadKind, Tool, ToolCallContext, ToolResult,
    ToolResultBlock, ToolResultContent,
};
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
                "type": {
                    "type": "string",
                    "description": "File type filter by extension (e.g., \"js\", \"py\", \"rust\")"
                },
                "output_mode": {
                    "type": "string",
                    "enum": ["content", "files_with_matches", "count"],
                    "description": "Output mode: \"content\" shows matching lines, \"files_with_matches\" shows file paths (default), \"count\" shows match counts"
                },
                "-A": {
                    "type": "integer",
                    "description": "Number of lines to show after each match (rg -A)"
                },
                "-B": {
                    "type": "integer",
                    "description": "Number of lines to show before each match (rg -B)"
                },
                "-C": {
                    "type": "integer",
                    "description": "Number of lines to show before and after each match (rg -C). Shorthand for setting both -B and -C."
                },
                "context": {
                    "type": "integer",
                    "description": "Alias for -C: number of lines to show before and after each match"
                },
                "-n": {
                    "type": "boolean",
                    "description": "Show line numbers in output (default true)"
                },
                "-i": {
                    "type": "boolean",
                    "description": "Case insensitive search"
                },
                "offset": {
                    "type": "integer",
                    "description": "Skip first N matches before returning results"
                },
                "head_limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default 250)",
                    "exclusiveMinimum": 0
                },
                "multiline": {
                    "type": "boolean",
                    "description": "Enable multiline mode where . matches newlines and patterns can span lines"
                }
            },
            "required": ["pattern"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "anyOf": [
                {
                    "type": "object",
                    "description": "content mode: matching lines with context",
                    "properties": {
                        "mode": { "const": "content" },
                        "matches": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "file": { "type": "string" },
                                    "line": { "type": "integer" },
                                    "column": { "type": "integer" },
                                    "match": { "type": "string" },
                                    "context_before": {
                                        "type": "array",
                                        "items": { "type": "string" }
                                    },
                                    "context_after": {
                                        "type": "array",
                                        "items": { "type": "string" }
                                    }
                                }
                            }
                        },
                        "total": { "type": "integer" },
                        "truncated": { "type": "boolean" }
                    }
                },
                {
                    "type": "object",
                    "description": "files_with_matches mode: only file paths",
                    "properties": {
                        "mode": { "const": "files_with_matches" },
                        "filenames": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "total": { "type": "integer" },
                        "truncated": { "type": "boolean" }
                    }
                },
                {
                    "type": "object",
                    "description": "count mode: match counts per file",
                    "properties": {
                        "mode": { "const": "count" },
                        "counts": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "file": { "type": "string" },
                                    "count": { "type": "integer" }
                                }
                            }
                        },
                        "total": { "type": "integer" },
                        "truncated": { "type": "boolean" }
                    }
                }
            ]
        }))
    }

    fn is_search_or_read_command(&self, _input: &serde_json::Value) -> SearchReadKind {
        SearchReadKind {
            is_search: true,
            ..Default::default()
        }
    }

    fn extract_search_text(&self, output: &serde_json::Value) -> Option<String> {
        if let Some(matches) = output["matches"].as_array() {
            let lines: Vec<&str> = matches.iter().filter_map(|m| m["match"].as_str()).collect();
            if !lines.is_empty() {
                return Some(lines.join("\n"));
            }
        }
        if let Some(filenames) = output["filenames"].as_array() {
            let lines: Vec<&str> = filenames.iter().filter_map(|v| v.as_str()).collect();
            return Some(lines.join("\n"));
        }
        None
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
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

        let output_mode = input
            .get("output_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("content");
        let context_before = input
            .get("-B")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .or_else(|| input.get("-C").and_then(|v| v.as_u64()).map(|v| v as usize))
            .or_else(|| {
                input
                    .get("context")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
            })
            .unwrap_or(0);
        let context_after = input
            .get("-A")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .or_else(|| input.get("-C").and_then(|v| v.as_u64()).map(|v| v as usize))
            .or_else(|| {
                input
                    .get("context")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
            })
            .unwrap_or(0);
        let _show_line_numbers = input.get("-n").and_then(|v| v.as_bool()).unwrap_or(true);
        let case_insensitive = input.get("-i").and_then(|v| v.as_bool()).unwrap_or(false);
        let offset = input
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(0);
        let type_filter = input.get("type").and_then(|v| v.as_str());
        let multiline = input
            .get("multiline")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let regex = {
            let mut builder = regex::RegexBuilder::new(pattern_str);
            if case_insensitive {
                builder.case_insensitive(true);
            }
            if multiline {
                builder.multi_line(true).dot_matches_new_line(true);
            }
            match builder.build() {
                Ok(r) => r,
                Err(e) => {
                    return ToolResult {
                        content: format!("Invalid regex pattern '{}': {}", pattern_str, e),
                        is_error: true,
                        ..Default::default()
                    };
                }
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

        let raw_glob = input.get("glob").and_then(|v| v.as_str());

        // Apply type filter if glob not already set
        let glob_filter = match (raw_glob, type_filter) {
            (Some(g), _) => Some(g.to_string()),
            (None, Some(t)) => type_to_glob(t).map(String::from),
            (None, None) => None,
        };

        let head_limit = input
            .get("head_limit")
            .and_then(|v| v.as_i64())
            .map(|v| v as usize)
            .unwrap_or(250);

        let cwd = &context.cwd;
        let mut results: Vec<serde_json::Value> = Vec::new();
        let mut skipped = 0usize;

        // Build a glob matcher if glob_filter is provided
        let glob_set = glob_filter.as_ref().map(|g| {
            let full_glob = if g.starts_with('/') {
                let rel = g.trim_start_matches('/');
                let gs = base_path.join(rel).to_string_lossy().to_string();
                if cfg!(windows) {
                    gs.replace('\\', "/")
                } else {
                    gs
                }
            } else {
                let gs = base_path.join(g).to_string_lossy().to_string();
                if cfg!(windows) {
                    gs.replace('\\', "/")
                } else {
                    gs
                }
            };
            glob::glob(&full_glob)
        });

        // If glob filter is provided, walk only matching files
        if let Some(Ok(glob_paths)) = glob_set {
            for entry in glob_paths {
                if results.len() >= head_limit {
                    break;
                }
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
                                if skipped < offset {
                                    skipped += 1;
                                    continue;
                                }
                                if results.len() >= head_limit {
                                    break;
                                }

                                let rel = pathdiff::diff_paths(&path, cwd)
                                    .unwrap_or_else(|| path.clone());
                                let column = regex.find(line).map(|m| m.start() + 1).unwrap_or(1);
                                let mut entry = serde_json::json!({
                                    "file": rel.to_string_lossy(),
                                    "line": line_num + 1,
                                    "column": column,
                                    "match": line.to_string()
                                });

                                // Add context lines if requested
                                if context_before > 0 {
                                    let before_start = line_num.saturating_sub(context_before);
                                    let before_lines: Vec<String> = content
                                        .lines()
                                        .skip(before_start)
                                        .take(line_num - before_start)
                                        .map(String::from)
                                        .collect();
                                    entry["context_before"] = serde_json::json!(before_lines);
                                }
                                if context_after > 0 {
                                    let after_start = line_num + 1;
                                    let after_lines: Vec<String> = content
                                        .lines()
                                        .skip(after_start)
                                        .take(context_after)
                                        .map(String::from)
                                        .collect();
                                    entry["context_after"] = serde_json::json!(after_lines);
                                }

                                results.push(entry);
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
                        "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "pdf", "zip", "gz",
                        "tar", "bz2", "xz", "so", "dylib", "dll", "exe", "bin", "o", "obj",
                        "class", "pyc", "ttf", "otf", "woff", "woff2", "eot", "mp3", "mp4", "avi",
                        "mov", "mkv", "db", "sqlite", "lock",
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
                        if skipped < offset {
                            skipped += 1;
                            continue;
                        }
                        if results.len() >= head_limit {
                            break;
                        }

                        let rel =
                            pathdiff::diff_paths(path, cwd).unwrap_or_else(|| path.to_path_buf());
                        let column = regex.find(line).map(|m| m.start() + 1).unwrap_or(1);
                        let mut entry = serde_json::json!({
                            "file": rel.to_string_lossy(),
                            "line": line_num + 1,
                            "column": column,
                            "match": line.to_string()
                        });

                        // Add context lines if requested
                        if context_before > 0 {
                            let before_start = line_num.saturating_sub(context_before);
                            let before_lines: Vec<String> = content
                                .lines()
                                .skip(before_start)
                                .take(line_num - before_start)
                                .map(String::from)
                                .collect();
                            entry["context_before"] = serde_json::json!(before_lines);
                        }
                        if context_after > 0 {
                            let after_start = line_num + 1;
                            let after_lines: Vec<String> = content
                                .lines()
                                .skip(after_start)
                                .take(context_after)
                                .map(String::from)
                                .collect();
                            entry["context_after"] = serde_json::json!(after_lines);
                        }

                        results.push(entry);
                    }
                }
            }
        }

        let total = results.len();
        let truncated = total >= head_limit;

        let result_json = match output_mode {
            "files_with_matches" => {
                let mut filenames: Vec<String> = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for entry in &results {
                    let file = entry["file"].as_str().unwrap_or("").to_string();
                    if seen.insert(file.clone()) {
                        filenames.push(file);
                    }
                }
                serde_json::json!({
                    "mode": "files_with_matches",
                    "filenames": filenames,
                    "total": total,
                    "truncated": truncated
                })
            }
            "count" => {
                let mut file_counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                for entry in &results {
                    let file = entry["file"].as_str().unwrap_or("").to_string();
                    *file_counts.entry(file).or_insert(0) += 1;
                }
                let counts: Vec<serde_json::Value> = file_counts
                    .into_iter()
                    .map(|(file, count)| serde_json::json!({"file": file, "count": count}))
                    .collect();
                serde_json::json!({
                    "mode": "count",
                    "counts": counts,
                    "total": total,
                    "truncated": truncated
                })
            }
            _ => {
                serde_json::json!({
                    "mode": "content",
                    "matches": results,
                    "total": total,
                    "truncated": truncated
                })
            }
        };

        let mut meta = std::collections::HashMap::new();
        meta.insert("total".to_string(), total.to_string());

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
            content: ToolResultContent::Text(output.to_string()),
            is_error: false,
        }
    }
}

fn type_to_glob(type_name: &str) -> Option<&'static str> {
    match type_name {
        "rs" | "rust" => Some("*.rs"),
        "js" | "javascript" => Some("*.js"),
        "ts" | "typescript" => Some("*.ts"),
        "tsx" => Some("*.tsx"),
        "jsx" => Some("*.jsx"),
        "py" | "python" => Some("*.py"),
        "go" => Some("*.go"),
        "java" => Some("*.java"),
        "c" => Some("*.c"),
        "h" => Some("*.h"),
        "cpp" | "c++" => Some("*.cpp"),
        "hpp" => Some("*.hpp"),
        "rb" | "ruby" => Some("*.rb"),
        "sh" | "bash" => Some("*.sh"),
        "md" | "markdown" => Some("*.md"),
        "json" => Some("*.json"),
        "yaml" | "yml" => Some("*.yml"),
        "toml" => Some("*.toml"),
        "html" => Some("*.html"),
        "css" => Some("*.css"),
        "scss" => Some("*.scss"),
        "sql" => Some("*.sql"),
        "swift" => Some("*.swift"),
        "kt" | "kotlin" => Some("*.kt"),
        "vue" => Some("*.vue"),
        "svelte" => Some("*.svelte"),
        _ => None,
    }
}
