use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use futures_util::StreamExt;
use shared::CliConfig;
use tokio::sync::watch;
use uuid::Uuid;

use crate::conversation::anthropic::{build_request_body, HistoryEntry, Role};
use crate::conversation::session_bus::SessionBus;
use crate::conversation::sse::SseParser;
use crate::conversation::system_prompt::SystemPrompt;
use crate::conversation::tool_loop::run_tool_uses;
use crate::sdk::protocol::{
    AnthropicUsage, AssistantPayload, BlockDelta, BusMessage, ContentBlockFinal,
    ContentBlockStream, StreamEvent, SystemSubtype, UserPayload,
};
use crate::state::store::Store;
use crate::tools::ToolRegistry;

#[derive(Clone)]
pub struct ConversationEngine {
    pub store: Arc<Store>,
    pub config: CliConfig,
    pub registry: Arc<ToolRegistry>,
    pub bus: Arc<SessionBus>,
    pub abort: Option<watch::Receiver<bool>>,
    /// When `Some`, every emit uses this as the session_id instead of
    /// `bus.session_id()`. Set by child engines spawned for subagents.
    pub session_id_override: Option<String>,
}

impl ConversationEngine {
    pub fn new(
        store: Arc<Store>,
        config: CliConfig,
        registry: Arc<ToolRegistry>,
        bus: Arc<SessionBus>,
    ) -> Self {
        Self { store, config, registry, bus, abort: None, session_id_override: None }
    }

    /// Effective session id for emits: override if set, else bus.session_id().
    pub fn effective_session_id(&self) -> String {
        self.session_id_override
            .clone()
            .unwrap_or_else(|| self.bus.session_id().to_string())
    }

    /// Drive one user turn end-to-end: emit the user message, loop on
    /// (request → stream → tool execution) until the model returns
    /// a terminal stop reason or there are no more tool_use blocks.
    pub async fn process_prompt(
        &self,
        user_input: String,
        system_prompt: &SystemPrompt,
        parent_tool_use_id: Option<String>,
    ) -> Result<String, String> {
        let session_id = self.effective_session_id();
        let model = self.config.model.clone();
        let base_url = self.config.api_messages_base_url.clone();
        let api_key = self
            .config
            .openrouter_api_key
            .clone()
            .ok_or_else(|| "no OpenRouter API key configured".to_string())?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let permission_mode = self.store.get_state().permission_mode.clone();
        let tools = self.registry.assemble_for_mode(&permission_mode);

        // Seed history from the store and append the new user message.
        let mut history: Vec<HistoryEntry> = self.store.get_state().history.clone();
        let user_block = ContentBlockFinal::Text { text: user_input.clone() };
        history.push(HistoryEntry {
            role: Role::User,
            content: vec![user_block.clone()],
        });

        self.bus.emit(BusMessage::User {
            message: UserPayload {
                role: "user".to_string(),
                content: vec![user_block],
            },
            parent_tool_use_id: parent_tool_use_id.clone(),
            uuid: Uuid::new_v4(),
            session_id: session_id.clone(),
        });

        let started = Instant::now();
        let mut total_input_tokens: u64 = 0;
        let mut total_output_tokens: u64 = 0;
        let mut last_assistant_text = String::new();
        let mut num_turns: u32 = 0;
        let client = reqwest::Client::new();
        let system_rendered = system_prompt.render();

        loop {
            num_turns += 1;
            if num_turns >= 50 {
                // Persist the trail of work before bailing, so /resume has something to load.
                self.store.set_state(|s| {
                    s.history = history.clone();
                });
                self.bus
                    .emit_system(SystemSubtype::Notice, "max turns (50) reached without end_turn");
                return Err("max turns (50) reached without end_turn".to_string());
            }

            let body = build_request_body(
                &model,
                &system_rendered,
                &history,
                &tools,
                8192,
                true,
            );

            let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
            let response = client
                .post(&url)
                .header("Authorization", format!("Bearer {api_key}"))
                .header("Content-Type", "application/json")
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("HTTP request failed: {e}"))?;

            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                return Err(format!("API error ({status}): {}", truncate_error_body(&text)));
            }

            // Per-turn fold state.
            let mut current_blocks: Vec<Option<PartialBlock>> = Vec::new();
            let mut stop_reason: Option<String> = None;
            let mut message_id = String::new();
            let mut response_model = model.clone();

            let mut parser = SseParser::new();
            let mut byte_stream = response.bytes_stream();
            while let Some(chunk) = byte_stream.next().await {
                let chunk = chunk.map_err(|e| format!("stream error: {e}"))?;
                for event in parser.feed(&chunk) {
                    self.bus.emit(BusMessage::StreamEvent {
                        event: event.clone(),
                        parent_tool_use_id: parent_tool_use_id.clone(),
                        uuid: Uuid::new_v4(),
                        session_id: session_id.clone(),
                    });
                    fold_event(
                        event,
                        &mut current_blocks,
                        &mut stop_reason,
                        &mut message_id,
                        &mut response_model,
                        &mut total_input_tokens,
                        &mut total_output_tokens,
                    );
                }
            }

            // Finalize this turn's assistant message.
            let content: Vec<ContentBlockFinal> = current_blocks
                .into_iter()
                .flatten()
                .map(|pb| pb.finalize())
                .collect();

            let usage = AnthropicUsage {
                input_tokens: total_input_tokens,
                output_tokens: total_output_tokens,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            };
            let assistant_msg = AssistantPayload {
                id: message_id.clone(),
                model: response_model.clone(),
                role: "assistant".to_string(),
                content: content.clone(),
                stop_reason: stop_reason.clone(),
                usage: usage.clone(),
            };
            self.bus.emit(BusMessage::Assistant {
                message: assistant_msg.clone(),
                parent_tool_use_id: parent_tool_use_id.clone(),
                uuid: Uuid::new_v4(),
                session_id: session_id.clone(),
            });
            history.push(HistoryEntry {
                role: Role::Assistant,
                content: content.clone(),
            });

            // Pick out tool_use blocks and the trailing text.
            let mut tool_uses: Vec<(String, String, serde_json::Value)> = Vec::new();
            for b in &content {
                match b {
                    ContentBlockFinal::ToolUse { id, name, input } => {
                        tool_uses.push((id.clone(), name.clone(), input.clone()));
                    }
                    ContentBlockFinal::Text { text } => {
                        last_assistant_text = text.clone();
                    }
                    _ => {}
                }
            }

            if tool_uses.is_empty() {
                self.bus.emit(BusMessage::Result {
                    stop_reason: stop_reason.clone(),
                    usage,
                    total_cost_usd: 0.0,
                    duration_ms: started.elapsed().as_millis() as u64,
                    num_turns,
                    uuid: Uuid::new_v4(),
                    session_id: session_id.clone(),
                });
                self.store.set_state(|s| {
                    s.history = history.clone();
                });
                return Ok(last_assistant_text);
            }

            // Execute the tools, emit the synthetic user turn, loop.
            let tool_results = run_tool_uses(
                &self.registry,
                tool_uses,
                cwd.clone(),
                permission_mode.clone(),
                self.abort.clone(),
                self.bus.clone(),
                parent_tool_use_id.clone(),
                session_id.clone(),
                false, // root engines never auto-deny; AgentTool sets true on async children via the child engine's child registry
            )
            .await;

            self.bus.emit(BusMessage::User {
                message: UserPayload {
                    role: "user".to_string(),
                    content: tool_results.clone(),
                },
                parent_tool_use_id: parent_tool_use_id.clone(),
                uuid: Uuid::new_v4(),
                session_id: session_id.clone(),
            });
            history.push(HistoryEntry {
                role: Role::User,
                content: tool_results,
            });
        }
    }
}

#[derive(Debug, Clone)]
enum PartialBlock {
    Text { text: String },
    Thinking { thinking: String, signature: String },
    ToolUse { id: String, name: String, input_json: String },
}

impl PartialBlock {
    fn finalize(self) -> ContentBlockFinal {
        match self {
            PartialBlock::Text { text } => ContentBlockFinal::Text { text },
            PartialBlock::Thinking { thinking, signature } => {
                ContentBlockFinal::Thinking { thinking, signature }
            }
            PartialBlock::ToolUse { id, name, input_json } => {
                let input: serde_json::Value =
                    serde_json::from_str(&input_json).unwrap_or(serde_json::json!({}));
                ContentBlockFinal::ToolUse { id, name, input }
            }
        }
    }
}

fn fold_event(
    event: StreamEvent,
    blocks: &mut Vec<Option<PartialBlock>>,
    stop_reason: &mut Option<String>,
    message_id: &mut String,
    response_model: &mut String,
    in_tokens: &mut u64,
    out_tokens: &mut u64,
) {
    match event {
        StreamEvent::MessageStart { message } => {
            *message_id = message.id;
            *response_model = message.model;
            // input_tokens is reported once on message_start. output_tokens here
            // is typically 0; the cumulative final value arrives on message_delta,
            // so we ignore it here to avoid double-counting.
            *in_tokens += message.usage.input_tokens;
        }
        StreamEvent::ContentBlockStart { index, content_block } => {
            let idx = index as usize;
            while blocks.len() <= idx {
                blocks.push(None);
            }
            blocks[idx] = Some(match content_block {
                ContentBlockStream::Text { text } => PartialBlock::Text { text },
                ContentBlockStream::Thinking { thinking, signature } => {
                    PartialBlock::Thinking { thinking, signature }
                }
                ContentBlockStream::ToolUse { id, name, .. } => PartialBlock::ToolUse {
                    id,
                    name,
                    input_json: String::new(),
                },
            });
        }
        StreamEvent::ContentBlockDelta { index, delta } => {
            let idx = index as usize;
            if let Some(Some(pb)) = blocks.get_mut(idx) {
                match (pb, delta) {
                    (PartialBlock::Text { text }, BlockDelta::TextDelta { text: d }) => {
                        text.push_str(&d)
                    }
                    (
                        PartialBlock::Thinking { thinking, .. },
                        BlockDelta::ThinkingDelta { thinking: d },
                    ) => thinking.push_str(&d),
                    (
                        PartialBlock::Thinking { signature, .. },
                        BlockDelta::SignatureDelta { signature: s },
                    ) => *signature = s,
                    (
                        PartialBlock::ToolUse { input_json, .. },
                        BlockDelta::InputJsonDelta { partial_json },
                    ) => input_json.push_str(&partial_json),
                    _ => {} // mismatched delta type — skip
                }
            }
        }
        StreamEvent::ContentBlockStop { .. } => {}
        StreamEvent::MessageDelta { delta, usage } => {
            if let Some(sr) = delta.stop_reason {
                *stop_reason = Some(sr);
            }
            *out_tokens += usage.output_tokens;
        }
        StreamEvent::MessageStop => {}
        StreamEvent::Ping => {}
    }
}

/// Squash an API error body down to something a single TUI line can carry.
/// Tries to surface `error.message` from a JSON-shaped error first; falls back
/// to a hard byte truncation.
fn truncate_error_body(body: &str) -> String {
    const MAX_LEN: usize = 512;
    // Anthropic / OpenRouter errors are usually `{"error":{"message":"..."}}`.
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(msg) = json.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
            return clip(msg, MAX_LEN);
        }
        if let Some(msg) = json.get("message").and_then(|m| m.as_str()) {
            return clip(msg, MAX_LEN);
        }
    }
    clip(body.trim(), MAX_LEN)
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Avoid splitting in the middle of a UTF-8 sequence.
        let mut end = max;
        while !s.is_char_boundary(end) && end > 0 { end -= 1; }
        format!("{}… ({} more bytes)", &s[..end], s.len() - end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_accumulates_text_deltas() {
        let mut blocks: Vec<Option<PartialBlock>> = Vec::new();
        let mut sr = None;
        let mut id = String::new();
        let mut model = String::new();
        let mut in_t = 0u64;
        let mut out_t = 0u64;
        fold_event(
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockStream::Text { text: String::new() },
            },
            &mut blocks,
            &mut sr,
            &mut id,
            &mut model,
            &mut in_t,
            &mut out_t,
        );
        fold_event(
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: BlockDelta::TextDelta { text: "hel".into() },
            },
            &mut blocks,
            &mut sr,
            &mut id,
            &mut model,
            &mut in_t,
            &mut out_t,
        );
        fold_event(
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: BlockDelta::TextDelta { text: "lo".into() },
            },
            &mut blocks,
            &mut sr,
            &mut id,
            &mut model,
            &mut in_t,
            &mut out_t,
        );
        let final_block = blocks.into_iter().flatten().next().unwrap().finalize();
        match final_block {
            ContentBlockFinal::Text { text } => assert_eq!(text, "hello"),
            other => panic!("wrong: {other:?}"),
        }
    }

    #[test]
    fn fold_accumulates_tool_use_input_json() {
        let mut blocks: Vec<Option<PartialBlock>> = Vec::new();
        let mut sr = None;
        let mut id = String::new();
        let mut model = String::new();
        let mut in_t = 0u64;
        let mut out_t = 0u64;
        fold_event(
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockStream::ToolUse {
                    id: "tu_1".into(),
                    name: "Read".into(),
                    input: serde_json::json!({}),
                },
            },
            &mut blocks,
            &mut sr,
            &mut id,
            &mut model,
            &mut in_t,
            &mut out_t,
        );
        fold_event(
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: BlockDelta::InputJsonDelta {
                    partial_json: r#"{"file_path":"#.into(),
                },
            },
            &mut blocks,
            &mut sr,
            &mut id,
            &mut model,
            &mut in_t,
            &mut out_t,
        );
        fold_event(
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: BlockDelta::InputJsonDelta {
                    partial_json: r#""PLAN.md"}"#.into(),
                },
            },
            &mut blocks,
            &mut sr,
            &mut id,
            &mut model,
            &mut in_t,
            &mut out_t,
        );
        let final_block = blocks.into_iter().flatten().next().unwrap().finalize();
        match final_block {
            ContentBlockFinal::ToolUse { name, input, .. } => {
                assert_eq!(name, "Read");
                assert_eq!(input["file_path"], "PLAN.md");
            }
            other => panic!("wrong: {other:?}"),
        }
    }

    #[test]
    fn truncate_error_body_extracts_anthropic_error_message() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens too large"}}"#;
        assert_eq!(truncate_error_body(body), "max_tokens too large");
    }

    #[test]
    fn truncate_error_body_clips_long_strings() {
        let big = "x".repeat(2_000);
        let out = truncate_error_body(&big);
        assert!(out.len() < big.len());
        assert!(out.ends_with("more bytes)"));
    }

    #[test]
    fn truncate_error_body_falls_back_to_raw_text() {
        let body = "<html><body>502 Bad Gateway</body></html>";
        let out = truncate_error_body(body);
        assert!(out.contains("502"));
    }

    #[test]
    fn engine_session_id_resolution_default_uses_bus() {
        use crate::conversation::session_bus::SessionBus;
        let bus = std::sync::Arc::new(SessionBus::new("s-root".into()));
        let store_arc = std::sync::Arc::new(crate::state::store::Store::new());
        let agent_reg = std::sync::Arc::new(crate::agents::AgentRegistry::built_in_only());
        let engine = ConversationEngine {
            store: store_arc.clone(),
            config: shared::CliConfig::default(),
            registry: std::sync::Arc::new(crate::tools::ToolRegistry::new(
                store_arc.clone(),
                shared::CliConfig::default(),
            )),
            bus: bus.clone(),
            abort: None,
            session_id_override: None,
        };
        let _ = agent_reg; // touch to silence unused
        assert_eq!(engine.effective_session_id(), "s-root");
    }

    #[test]
    fn engine_session_id_override_used_when_set() {
        use crate::conversation::session_bus::SessionBus;
        let bus = std::sync::Arc::new(SessionBus::new("s-root".into()));
        let store_arc = std::sync::Arc::new(crate::state::store::Store::new());
        let engine = ConversationEngine {
            store: store_arc.clone(),
            config: shared::CliConfig::default(),
            registry: std::sync::Arc::new(crate::tools::ToolRegistry::new(
                store_arc,
                shared::CliConfig::default(),
            )),
            bus,
            abort: None,
            session_id_override: Some("agent-1".into()),
        };
        assert_eq!(engine.effective_session_id(), "agent-1");
    }
}
