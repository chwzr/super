use crate::conversation::anthropic::{build_request_body_with_server_tools, HistoryEntry, Role};
use crate::sdk::protocol::ContentBlockFinal;
use crate::tools::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
use async_trait::async_trait;
use serde_json::json;

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "WebSearch"
    }

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

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let start = std::time::Instant::now();
        let query = input["query"].as_str().unwrap_or("");

        if query.is_empty() {
            return ToolResult {
                content: "Missing required parameter: query".to_string(),
                is_error: true,
                ..Default::default()
            };
        }

        let api_key = match &context.api_key {
            Some(key) => key.clone(),
            None => {
                return ToolResult {
                    content: "API key not available for WebSearch".to_string(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let model = crate::providers::resolve_slug(&context.provider, "haiku");
        let base_url = &context.api_messages_base_url;
        let system = "You are a web search assistant. Search the web and return \
                      results with titles, URLs, and snippets. Cite sources as \
                      markdown links.";

        let user_message = format!("Search the web for: {}", query);

        let history = vec![HistoryEntry {
            role: Role::User,
            content: vec![ContentBlockFinal::Text { text: user_message }],
        }];

        // Build server tool parameters with domain filtering
        let mut params = serde_json::Map::new();
        if let Some(allowed) = input.get("allowed_domains").and_then(|v| v.as_array()) {
            params.insert(
                "allowed_domains".to_string(),
                serde_json::Value::Array(allowed.clone()),
            );
        }
        if let Some(blocked) = input.get("blocked_domains").and_then(|v| v.as_array()) {
            params.insert(
                "excluded_domains".to_string(),
                serde_json::Value::Array(blocked.clone()),
            );
        }

        let server_tool = if params.is_empty() {
            json!({ "type": "openrouter:web_search" })
        } else {
            json!({ "type": "openrouter:web_search", "parameters": params })
        };

        let body = build_request_body_with_server_tools(
            model,
            system,
            &history,
            &[],
            &[server_tool],
            4096,
            false,
        );

        let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("HTTP client init failed: {e}"),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        match client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => {
                let duration_ms = start.elapsed().as_millis() as f64;

                if !resp.status().is_success() {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    return ToolResult {
                        content: format!("Search API error ({}): {}", status, text),
                        is_error: true,
                        ..Default::default()
                    };
                }

                match resp.json::<serde_json::Value>().await {
                    Ok(resp_body) => {
                        let text = resp_body["content"]
                            .as_array()
                            .and_then(|blocks| {
                                blocks
                                    .iter()
                                    .find_map(|block| block["text"].as_str().map(String::from))
                            })
                            .unwrap_or_default();

                        let mut items: Vec<serde_json::Value> = Vec::new();
                        if let Some(content_blocks) = resp_body["content"].as_array() {
                            for block in content_blocks {
                                if let Some(annotations) = block["annotations"].as_array() {
                                    for ann in annotations {
                                        if let Some(citation) = ann.get("url_citation") {
                                            items.push(json!({
                                                "title": citation["title"].as_str().unwrap_or(""),
                                                "url": citation["url"].as_str().unwrap_or(""),
                                                "snippet": citation["content"].as_str().unwrap_or("")
                                            }));
                                        }
                                    }
                                }
                            }
                        }

                        if items.is_empty() && !text.is_empty() {
                            items.push(json!({
                                "title": "",
                                "url": "",
                                "snippet": text
                            }));
                        }

                        ToolResult {
                            content: json!({
                                "query": query,
                                "results": items,
                                "durationMs": duration_ms
                            })
                            .to_string(),
                            is_error: false,
                            ..Default::default()
                        }
                    }
                    Err(e) => ToolResult {
                        content: format!("Failed to parse search response: {e}"),
                        is_error: true,
                        ..Default::default()
                    },
                }
            }
            Err(e) => ToolResult {
                content: format!("Search request failed: {e}"),
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
