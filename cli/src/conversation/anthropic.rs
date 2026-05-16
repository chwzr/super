use serde_json::{json, Value};
use std::sync::Arc;

use crate::sdk::protocol::ContentBlockFinal;
use crate::tools::contract::{DescriptionCtx, Tool};

/// Build an Anthropic-shaped `/v1/messages` request body.
///
/// * `system` — rendered system prompt, sent as a top-level string.
/// * `history` — full conversation history, each entry already structured as
///   `{ "role": "user"|"assistant", "content": [<blocks>] }`.
/// * `tools` — the active tool pool. Each tool's `name`, `description`, and
///   `input_schema` are projected into the Anthropic tools array.
/// * `model` — model id (e.g. `"anthropic/claude-sonnet-4-5"`).
/// * `max_tokens` — output cap.
/// * `stream` — when true, the server responds with SSE.
pub fn build_request_body(
    model: &str,
    system: &str,
    history: &[HistoryEntry],
    tools: &[Arc<dyn Tool>],
    max_tokens: u32,
    stream: bool,
) -> Value {
    let tools_json: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name(),
                "description": t.description(None, &DescriptionCtx::default()),
                "input_schema": t.input_schema(),
            })
        })
        .collect();

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

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    pub role: Role,
    pub content: Vec<ContentBlockFinal>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_includes_system_messages_and_stream_flag() {
        let body = build_request_body(
            "anthropic/claude-sonnet-4-5",
            "you are super",
            &[
                HistoryEntry {
                    role: Role::User,
                    content: vec![ContentBlockFinal::Text { text: "hi".into() }],
                },
            ],
            &[],
            4096,
            true,
        );
        assert_eq!(body["model"], "anthropic/claude-sonnet-4-5");
        assert_eq!(body["system"], "you are super");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
        assert_eq!(body["messages"][0]["content"][0]["text"], "hi");
        assert_eq!(body["tools"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn assistant_history_with_tool_use_serializes() {
        let body = build_request_body(
            "anthropic/claude-sonnet-4-5",
            "sys",
            &[HistoryEntry {
                role: Role::Assistant,
                content: vec![ContentBlockFinal::ToolUse {
                    id: "tu_1".into(),
                    name: "Read".into(),
                    input: serde_json::json!({"file_path": "/tmp/x"}),
                }],
            }],
            &[],
            4096,
            true,
        );
        let tu = &body["messages"][0]["content"][0];
        assert_eq!(tu["type"], "tool_use");
        assert_eq!(tu["id"], "tu_1");
        assert_eq!(tu["name"], "Read");
        assert_eq!(tu["input"]["file_path"], "/tmp/x");
    }

    #[test]
    fn role_serializes_lowercase() {
        let entry = HistoryEntry {
            role: Role::User,
            content: vec![ContentBlockFinal::Text { text: "x".into() }],
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["role"], "user");

        let entry2 = HistoryEntry {
            role: Role::Assistant,
            content: vec![ContentBlockFinal::Text { text: "y".into() }],
        };
        let json2 = serde_json::to_value(&entry2).unwrap();
        assert_eq!(json2["role"], "assistant");
    }
}
