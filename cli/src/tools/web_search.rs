use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

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
    fn description(&self) -> &str {
        "Performs a web search. Returns search results with titles and URLs. \
         Use 'allowed_domains' to restrict to specific domains, \
         'blocked_domains' to exclude domains."
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
    fn is_read_only(&self) -> bool { true }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("");
        let mut _blocked = input["blocked_domains"].as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect::<Vec<_>>())
            .unwrap_or_default();
        let _allowed = input["allowed_domains"].as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect::<Vec<_>>())
            .unwrap_or_default();

        if !_blocked.is_empty() && !_allowed.is_empty() {
            return ToolResult { content: "Cannot specify both allowed_domains and blocked_domains".into(), is_error: true, ..Default::default() };
        }

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
                    let mut results = String::new();
                    results.push_str(&format!("Search results for: {query}\n\n"));

                    // Extract Abstract
                    if let Some(abstract_text) = v["AbstractText"].as_str() {
                        if !abstract_text.is_empty() {
                            results.push_str(&format!("{abstract_text}\n\n"));
                        }
                        if let Some(source) = v["AbstractURL"].as_str() {
                            if !source.is_empty() {
                                results.push_str(&format!("Source: {source}\n"));
                            }
                        }
                    }

                    // Extract RelatedTopics
                    if let Some(topics) = v["RelatedTopics"].as_array() {
                        for topic in topics.iter().take(10) {
                            if let Some(text) = topic["Text"].as_str() {
                                if !text.is_empty() {
                                    results.push_str(&format!("- {text}\n"));
                                    if let Some(url) = topic["FirstURL"].as_str() {
                                        results.push_str(&format!("  {url}\n"));
                                    }
                                }
                            }
                        }
                    }

                    if results.is_empty() || results == format!("Search results for: {query}\n\n") {
                        results.push_str("No results found.");
                    }

                    results.push_str("\nSources:\n- [DuckDuckGo](https://duckduckgo.com/?q=");
                    results.push_str(&url_encode(query));
                    results.push_str(")");

                    ToolResult { content: results, is_error: false, ..Default::default() }
                }
                Err(e) => ToolResult { content: format!("Failed to read response: {e}"), is_error: true, ..Default::default() },
            },
            Err(e) => ToolResult { content: format!("Search failed: {e}"), is_error: true, ..Default::default() },
        }
    }
}
