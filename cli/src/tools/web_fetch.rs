use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str { "WebFetch" }
    fn description(&self) -> &str {
        "Fetches content from a URL and processes it. \
         HTTP URLs are upgraded to HTTPS. Returns the page content as text."
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
    fn is_read_only(&self) -> bool { true }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let url = input["url"].as_str().unwrap_or("");
        let _prompt = input["prompt"].as_str().unwrap_or("");

        // Validate URL
        if url.len() > 2000 {
            return ToolResult { content: "URL exceeds 2000 character limit".into(), is_error: true, ..Default::default() };
        }
        if url.contains('@') {
            return ToolResult { content: "URLs with credentials are not supported".into(), is_error: true, ..Default::default() };
        }

        // Upgrade HTTP to HTTPS
        let url = if url.starts_with("http://") {
            url.replacen("http://", "https://", 1)
        } else {
            url.to_string()
        };

        // Fetch
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();
        match client.get(&url).send().await {
            Ok(resp) => match resp.text().await {
                Ok(body) => {
                    // Simple HTML-to-text: strip tags
                    let text = strip_html(&body);
                    let truncated = if text.len() > 100000 { format!("{}...\n[content truncated]", &text[..100000]) } else { text };
                    ToolResult {
                        content: format!("Content from {url}:\n\n{truncated}\n\nSources:\n- [{url}]({url})"),
                        is_error: false,
                        ..Default::default()
                    }
                }
                Err(e) => ToolResult { content: format!("Failed to read response: {e}"), is_error: true, ..Default::default() },
            },
            Err(e) => ToolResult { content: format!("Failed to fetch {url}: {e}"), is_error: true, ..Default::default() },
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
