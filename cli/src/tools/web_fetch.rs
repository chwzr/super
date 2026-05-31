use crate::tools::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
use crate::tools::web_fetch_preapproved;
use async_trait::async_trait;
use serde_json::json;

const MAX_MARKDOWN_LENGTH: usize = 100_000;

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "WebFetch"
    }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Fetches content from a URL and processes it. \
         HTTP URLs are upgraded to HTTPS. Returns the page content as text."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/web_fetch.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "The URL to fetch"},
                "prompt": {"type": "string", "description": "What information to extract from the page"}
            },
            "required": ["url", "prompt"]
        })
    }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "bytes": {"type": "integer"},
                "code": {"type": "integer"},
                "codeText": {"type": "string"},
                "result": {"type": "string", "description": "AI-processed result of the fetch"},
                "durationMs": {"type": "number"},
                "url": {"type": "string"}
            },
            "required": ["bytes", "code", "result", "durationMs", "url"]
        }))
    }

    async fn call(
        &self,
        input: serde_json::Value,
        context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let url = input["url"].as_str().unwrap_or("");
        let prompt = input["prompt"].as_str().unwrap_or("");

        // Validate URL
        if url.len() > 2000 {
            return ToolResult {
                content: json!({
                    "url": url,
                    "bytes": 0,
                    "code": 400,
                    "codeText": "Bad Request",
                    "result": "URL exceeds 2000 character limit",
                    "durationMs": 0
                })
                .to_string(),
                is_error: true,
                ..Default::default()
            };
        }
        if url.contains('@') {
            return ToolResult {
                content: json!({
                    "url": url,
                    "bytes": 0,
                    "code": 400,
                    "codeText": "Bad Request",
                    "result": "URLs with credentials are not supported",
                    "durationMs": 0
                })
                .to_string(),
                is_error: true,
                ..Default::default()
            };
        }

        // Upgrade HTTP to HTTPS
        let url = if url.starts_with("http://") {
            url.replacen("http://", "https://", 1)
        } else {
            url.to_string()
        };

        // Fetch with timing
        let start = std::time::Instant::now();
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                let duration_ms = start.elapsed().as_millis() as f64;
                return ToolResult {
                    content: json!({
                        "url": url,
                        "bytes": 0,
                        "code": 0,
                        "codeText": "ClientInitFailed",
                        "result": format!("HTTP client init failed: {e}"),
                        "durationMs": duration_ms
                    })
                    .to_string(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };
        match client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                match resp.text().await {
                    Ok(body) => {
                        let byte_count = body.len();
                        let duration_ms = start.elapsed().as_millis() as f64;

                        // Convert HTML to markdown
                        let markdown = html2md::parse_html(&body);

                        // Fast path: preapproved URL with content under 100KB
                        if web_fetch_preapproved::is_preapproved_url(&url)
                            && markdown.len() < MAX_MARKDOWN_LENGTH
                        {
                            return ToolResult {
                                content: json!({
                                    "url": url,
                                    "bytes": byte_count,
                                    "code": status.as_u16(),
                                    "codeText": status.canonical_reason().unwrap_or("OK"),
                                    "result": markdown,
                                    "durationMs": duration_ms
                                })
                                .to_string(),
                                is_error: false,
                                ..Default::default()
                            };
                        }

                        // Resolve the small model for AI processing
                        let model = crate::providers::resolve_slug(&context.provider, "haiku");

                        // Get API key from context — error if unavailable
                        let api_key = match &context.api_key {
                            Some(key) => key.clone(),
                            None => {
                                return ToolResult {
                                    content: json!({
                                        "url": url,
                                        "bytes": byte_count,
                                        "code": status.as_u16(),
                                        "codeText": status.canonical_reason().unwrap_or("Error"),
                                        "result": "API key not available for WebFetch AI processing",
                                        "durationMs": duration_ms
                                    })
                                    .to_string(),
                                    is_error: true,
                                    ..Default::default()
                                };
                            }
                        };

                        // AI processing via small model
                        match apply_prompt_to_content(
                            prompt,
                            &markdown,
                            &api_key,
                            &context.api_messages_base_url,
                            model,
                        )
                        .await
                        {
                            Ok(result) => ToolResult {
                                content: json!({
                                    "url": url,
                                    "bytes": byte_count,
                                    "code": status.as_u16(),
                                    "codeText": status.canonical_reason().unwrap_or("OK"),
                                    "result": result,
                                    "durationMs": duration_ms
                                })
                                .to_string(),
                                is_error: false,
                                ..Default::default()
                            },
                            Err(e) => ToolResult {
                                content: json!({
                                    "url": url,
                                    "bytes": byte_count,
                                    "code": status.as_u16(),
                                    "codeText": status.canonical_reason().unwrap_or("Error"),
                                    "result": format!("AI processing failed: {e}"),
                                    "durationMs": duration_ms
                                })
                                .to_string(),
                                is_error: true,
                                ..Default::default()
                            },
                        }
                    }
                    Err(e) => {
                        let duration_ms = start.elapsed().as_millis() as f64;
                        ToolResult {
                            content: json!({
                                "url": url,
                                "bytes": 0,
                                "code": status.as_u16(),
                                "codeText": status.canonical_reason().unwrap_or("Error"),
                                "result": format!("Failed to read response: {e}"),
                                "durationMs": duration_ms
                            })
                            .to_string(),
                            is_error: true,
                            ..Default::default()
                        }
                    }
                }
            }
            Err(e) => {
                let duration_ms = start.elapsed().as_millis() as f64;
                ToolResult {
                    content: json!({
                        "url": url,
                        "bytes": 0,
                        "code": 0,
                        "codeText": "Connection Error",
                        "result": format!("Failed to fetch {}: {}", url, e),
                        "durationMs": duration_ms
                    })
                    .to_string(),
                    is_error: true,
                    ..Default::default()
                }
            }
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

fn strip_html(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut in_script = false;

    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !in_tag && chars[i] == '<' {
            // Check if this is a <script tag (or </script)
            if i + 1 < chars.len() && (chars[i + 1] == '/' || chars[i + 1] != '!') {
                let rest = chars[i..].iter().collect::<String>().to_lowercase();
                if rest.starts_with("<script") {
                    in_script = true;
                } else if in_script && rest.starts_with("</script") {
                    in_script = false;
                }
            }
            in_tag = true;
        } else if in_tag && chars[i] == '>' {
            in_tag = false;
        } else if !in_tag && !in_script {
            result.push(chars[i]);
        }
        i += 1;
    }
    // Collapse whitespace
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn apply_prompt_to_content(
    prompt: &str,
    content: &str,
    api_key: &str,
    base_url: &str,
    model: &str,
) -> Result<String, String> {
    let truncated = if content.len() > MAX_MARKDOWN_LENGTH {
        format!(
            "{}\n\n[Content truncated due to length...]",
            &content[..MAX_MARKDOWN_LENGTH]
        )
    } else {
        content.to_string()
    };

    let user_message = format!(
        "Web page content:\n---\n{}\n---\n\n{}\n\nProvide a concise response based on the content above. Include relevant details, code examples, and documentation excerpts as needed. Enforce a strict 125-character maximum for quotes from any source document. Use quotation marks for exact language from articles; any language outside of the quotation should never be word-for-word the same.",
        truncated, prompt
    );

    let body = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": user_message
            }
        ],
        "max_tokens": 4096,
        "stream": false
    });

    let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Secondary model request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Secondary model API error ({}): {}", status, text));
    }

    let resp_body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {e}"))?;

    let text = resp_body["content"]
        .as_array()
        .and_then(|blocks| blocks.first())
        .and_then(|block| block["text"].as_str())
        .unwrap_or("No response from model");

    Ok(text.to_string())
}
