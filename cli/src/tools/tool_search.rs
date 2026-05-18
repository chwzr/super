use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

const MAX_SEARCH_RESULTS: usize = 20;

pub struct ToolSearchTool {
    pub registry: Arc<super::ToolRegistry>,
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "ToolSearch"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Search for tools by name or description. Use select: prefix for exact tool name lookup. Returns matching tools with their full schemas.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/tool_search.txt").into()
    }

    fn search_hint(&self) -> Option<&'static str> {
        Some("discover and load deferred tools by name or description")
    }

    fn should_defer(&self) -> bool {
        false
    }
    fn always_load(&self) -> bool {
        true
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query to match against tool names and descriptions. Use 'select:ToolName1,ToolName2' for exact tool name lookup."
                }
            },
            "required": ["query"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "matches": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "description": {"type": "string"},
                            "input_schema": {"type": "object"}
                        }
                    }
                }
            }
        }))
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) => q.trim(),
            None => {
                return ToolResult {
                    content: "Missing required parameter: query".into(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let all_tools: Vec<Arc<dyn Tool>> = {
            let tools = self.registry.tools.read().unwrap();
            tools.clone()
        };

        // --- select: prefix ---
        if let Some(names_str) = query.strip_prefix("select:") {
            let names: Vec<&str> = names_str.split(',').map(|s| s.trim()).collect();
            let mut matches = Vec::new();
            for name in &names {
                if let Some(tool) = all_tools.iter().find(|t| t.name() == *name) {
                    matches.push(format_tool_entry(&**tool));
                }
            }
            if matches.is_empty() {
                return ToolResult {
                    content: format!("No tools found matching select names: {names_str}"),
                    is_error: false,
                    ..Default::default()
                };
            }
            let result = format!("<functions>\n{}\n</functions>", matches.join("\n"));
            return ToolResult {
                content: result,
                is_error: false,
                ..Default::default()
            };
        }

        // --- +prefix: require term in name ---
        let require_in_name: Option<String> = query
            .strip_prefix('+')
            .and_then(|s| s.split_whitespace().next().map(|t| t.to_lowercase()));

        let search_terms: Vec<&str> = if require_in_name.is_some() {
            let after_plus = query.strip_prefix('+').unwrap();
            let parts: Vec<&str> = after_plus.splitn(2, ' ').collect();
            if parts.len() > 1 {
                parts[1].split_whitespace().collect()
            } else {
                vec![]
            }
        } else {
            query.split_whitespace().collect()
        };

        let query_lower = query.to_lowercase();
        let mut scored: Vec<(i32, &Arc<dyn Tool>)> = Vec::new();

        for tool in &all_tools {
            let name = tool.name();
            let desc = tool.description(None, &DescriptionCtx::default());
            let hint = tool.search_hint().unwrap_or("");
            let name_lower = name.to_lowercase();
            let desc_lower = desc.to_lowercase();
            let hint_lower = hint.to_lowercase();

            // +prefix filter: must contain term in name
            if let Some(ref req) = require_in_name {
                if !name_lower.contains(req) {
                    continue;
                }
            }

            let mut score: i32 = 0;

            // Exact name match = highest
            if name_lower == query_lower {
                score += 1000;
            }
            // Name starts with query
            else if name_lower.starts_with(&query_lower) {
                score += 500;
            }
            // Name contains query
            else if name_lower.contains(&query_lower) {
                score += 200;
            }

            // Word-level matching in name
            for term in &search_terms {
                let term_lower = term.to_lowercase();
                if name_lower.contains(&term_lower) {
                    score += 100;
                }
                if desc_lower.contains(&term_lower) {
                    score += 50;
                }
                if hint_lower.contains(&term_lower) {
                    score += 30;
                }
            }

            // Full query in description or hint
            if desc_lower.contains(&query_lower) {
                score += 40;
            }
            if hint_lower.contains(&query_lower) {
                score += 20;
            }

            if score > 0 || require_in_name.is_some() {
                scored.push((score, tool));
            }
        }

        // Sort by score descending, then by name
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name().cmp(b.1.name())));

        if scored.is_empty() {
            return ToolResult {
                content: format!("No tools found matching query: {query}"),
                is_error: false,
                ..Default::default()
            };
        }

        let matches: Vec<String> = scored
            .iter()
            .take(MAX_SEARCH_RESULTS)
            .map(|(_, tool)| format_tool_entry(&***tool))
            .collect();

        let result = format!("<functions>\n{}\n</functions>", matches.join("\n"));
        ToolResult {
            content: result,
            is_error: false,
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
                output
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}

fn format_tool_entry(tool: &dyn Tool) -> String {
    let desc = tool.description(None, &DescriptionCtx::default());
    let schema = tool.input_schema();
    format!(
        "<function>{}</function>",
        json!({
            "name": tool.name(),
            "description": desc,
            "parameters": schema
        })
    )
}
