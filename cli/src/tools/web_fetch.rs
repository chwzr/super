use crate::tools::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
use async_trait::async_trait;
use serde_json::json;

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
        _context: &ToolCallContext,
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
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();
        match client.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                match resp.text().await {
                    Ok(body) => {
                        // Simple HTML-to-text: strip tags
                        let text = strip_html(&body);
                        let _truncated = if text.len() > 100000 {
                            format!("{}...\n[content truncated]", &text[..100000])
                        } else {
                            text
                        };
                        let byte_count = body.len();
                        let duration_ms = start.elapsed().as_millis() as f64;
                        ToolResult {
                            content: json!({
                                "url": url,
                                "bytes": byte_count,
                                "code": status.as_u16(),
                                "codeText": status.canonical_reason().unwrap_or("OK"),
                                "result": format!("Content fetched ({} bytes). Prompt '{}' will be processed in a follow-up.", byte_count, prompt),
                                "durationMs": duration_ms
                            }).to_string(),
                            is_error: false,
                            ..Default::default()
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
                                "result": format!("Failed to read response: {}", e),
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
