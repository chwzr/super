# WebSearch: OpenRouter Server Tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace DuckDuckGo Instant Answer API in WebSearch with a delegation to OpenRouter's `openrouter:web_search` server-side tool.

**Architecture:** `WebSearch.call()` becomes a thin proxy that makes a secondary, non-streaming model call to `/v1/messages` with `{ "type": "openrouter:web_search" }` in the tools array. OpenRouter intercepts the tool call, executes the search server-side, and returns results. A new helper in `anthropic.rs` builds the request body with server tool entries. The main conversation loop is untouched.

**Tech Stack:** Rust, rig-core, reqwest, serde_json

---

### File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `cli/src/conversation/anthropic.rs` | Modify | Add `build_request_body_with_server_tools` helper that accepts extra server tool `Value` entries |
| `cli/src/tools/web_search.rs` | Rewrite | Replace DuckDuckGo call with secondary model call using the new helper |

---

### Task 1: Add `build_request_body_with_server_tools` helper

**Files:**
- Modify: `cli/src/conversation/anthropic.rs`

- [ ] **Step 1: Add the helper function**

Add a new public function `build_request_body_with_server_tools` that works like `build_request_body` but accepts an additional `server_tools: &[Value]` parameter. These entries are appended directly to the tools array (they already have the correct shape like `{ "type": "openrouter:web_search" }`).

Insert after line 57 (after the closing `}` of `build_request_body`):

```rust
/// Build an Anthropic-shaped `/v1/messages` request body with additional
/// server-side tool entries (e.g. `{ "type": "openrouter:web_search" }`).
/// Server tool entries are appended as-is after the user-defined tools.
pub fn build_request_body_with_server_tools(
    model: &str,
    system: &str,
    history: &[HistoryEntry],
    tools: &[Arc<dyn Tool>],
    server_tools: &[Value],
    max_tokens: u32,
    stream: bool,
) -> Value {
    let mut tools_json: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name(),
                "description": t.description(None, &DescriptionCtx::default()),
                "input_schema": t.input_schema(),
            })
        })
        .collect();

    // Append server-side tool entries directly — they are already
    // fully-formed JSON objects like { "type": "openrouter:web_search" }.
    tools_json.extend(server_tools.iter().cloned());

    let messages_json: Vec<Value> = history
        .iter()
        .map(|m| {
            json!({
                "role": match m.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                "content": m.content,
            })
        })
        .collect();

    json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": messages_json,
        "tools": tools_json,
        "stream": stream,
    })
}
```

- [ ] **Step 2: Run existing tests to verify no regression**

```bash
cargo test -p super-cli -- conversation::anthropic
```

Expected: all existing tests pass (the new function is additive, existing code is unchanged).

- [ ] **Step 3: Commit**

```bash
git add cli/src/conversation/anthropic.rs
git commit -m "feat: add build_request_body_with_server_tools helper

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 2: Rewrite WebSearch.call() to use OpenRouter server tool

**Files:**
- Modify: `cli/src/tools/web_search.rs`

- [ ] **Step 1: Rewrite the file**

Replace lines 1–211 of `cli/src/tools/web_search.rs` (everything except the
`map_tool_result_to_block` impl at the bottom) with the following. This removes
`url_encode`, the DuckDuckGo HTTP call, and rewrites the entire `impl Tool for
WebSearchTool` block with the new secondary-model-call approach.

```rust
use crate::conversation::anthropic::{
    build_request_body_with_server_tools, HistoryEntry, Role,
};
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
            content: vec![ContentBlockFinal::Text {
                text: user_message,
            }],
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
                                blocks.iter().find_map(|block| {
                                    block["text"].as_str().map(String::from)
                                })
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
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p super-cli
```

Expected: the crate compiles, all existing tests pass.

- [ ] **Step 3: Run clippy**

```bash
cargo clippy -p super-cli -- -D warnings
```

Expected: no warnings.

- [ ] **Step 4: Commit**

```bash
git add cli/src/tools/web_search.rs
git commit -m "feat: replace DuckDuckGo with OpenRouter server tool in WebSearch

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```
