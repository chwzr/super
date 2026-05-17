use crate::tools::contract::{DescriptionCtx, PromptCtx, Tool, ToolCallContext, ToolResult, ProgressSink};
use async_trait::async_trait;
use serde_json::json;

pub struct WebSearchTool;

fn url_encode(s: &str) -> String {
    s.chars().map(|c| match c {
        'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
        ' ' => "+".to_string(),
        c => format!("%{:02X}", c as u8),
    }).collect()
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str { "WebSearch" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Performs a web search. Returns search results with titles and URLs. \
         Use 'allowed_domains' to restrict to specific domains, \
         'blocked_domains' to exclude domains."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/web_search.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "minLength": 2, "description": "The search query"},
                "allowed_domains": {"type": "array", "items": {"type": "string"}},
                "blocked_domains": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["query"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "results": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": {"type": "string"},
                            "url": {"type": "string"},
                            "snippet": {"type": "string"}
                        }
                    }
                },
                "durationMs": {"type": "number"}
            },
            "required": ["query", "results"]
        }))
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let start = std::time::Instant::now();
        let query = input["query"].as_str().unwrap_or("");

        // Use DuckDuckGo Instant Answer API as a simple web search fallback
        let url = format!("https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
            url_encode(query)
        );

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap();

        match client.get(&url).send().await {
            Ok(resp) => match resp.text().await {
                Ok(body) => {
                    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();

                    // Parse results into structured items
                    let mut items: Vec<serde_json::Value> = Vec::new();
                    if let Some(abstract_text) = v["AbstractText"].as_str() {
                        if !abstract_text.is_empty() {
                            items.push(json!({
                                "title": "Abstract",
                                "url": v["AbstractURL"].as_str().unwrap_or(""),
                                "snippet": abstract_text
                            }));
                        }
                    }
                    if let Some(topics) = v["RelatedTopics"].as_array() {
                        for topic in topics.iter().take(20) {
                            if let Some(text) = topic["Text"].as_str() {
                                if !text.is_empty() {
                                    items.push(json!({
                                        "title": text.split(" - ").next().unwrap_or(text),
                                        "url": topic["FirstURL"].as_str().unwrap_or(""),
                                        "snippet": text
                                    }));
                                }
                            }
                        }
                    }

                    // Apply domain filtering
                    let allowed: Option<Vec<String>> = input.get("allowed_domains")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());

                    let blocked: Option<Vec<String>> = input.get("blocked_domains")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());

                    let items: Vec<_> = items.into_iter().filter(|item| {
                        let item_url = item["url"].as_str().unwrap_or("");
                        if let Some(ref allow) = allowed {
                            if !allow.iter().any(|domain| item_url.contains(domain)) {
                                return false;
                            }
                        }
                        if let Some(ref block) = blocked {
                            if block.iter().any(|domain| item_url.contains(domain)) {
                                return false;
                            }
                        }
                        true
                    }).collect();

                    let duration_ms = start.elapsed().as_millis() as f64;

                    ToolResult {
                        content: json!({
                            "query": query,
                            "results": items,
                            "durationMs": duration_ms
                        }).to_string(),
                        is_error: false,
                        ..Default::default()
                    }
                }
                Err(e) => ToolResult { content: format!("Failed to read response: {e}"), is_error: true, ..Default::default() },
            },
            Err(e) => ToolResult { content: format!("Search failed: {e}"), is_error: true, ..Default::default() },
        }
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> crate::tools::contract::ToolResultBlock {
        crate::tools::contract::ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: crate::tools::contract::ToolResultContent::Text(
                output.as_str().map(String::from).unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}
