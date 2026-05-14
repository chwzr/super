# Scroll-area & tool-execution parity with Claude Code — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Super CLI execute tools end-to-end via OpenRouter's Anthropic-compatible endpoint, emit a Claude-Code-shaped event bus, and fold those events into a scroll-area that renders the same primitives as Claude Code.

**Architecture:** A single `tokio::sync::broadcast::Sender<BusMessage>` per session, owned by `App` and threaded into `ConversationEngine`. Engine POSTs to `https://openrouter.ai/api/v1/messages` with Anthropic-shaped request bodies (system, messages with content blocks, tools array, stream=true). SSE events are parsed into `StreamEvent`s and emitted on the bus as `BusMessage::StreamEventEnvelope` envelopes; finalized blocks emit `BusMessage::Assistant`. Tool calls execute through the existing `ToolRegistry`, producing synthetic `BusMessage::User` envelopes whose content is `tool_result` blocks that get fed into the next API round-trip. The TUI subscribes to the bus and folds the event stream into a list of `TranscriptItem`s rendered with Claude-matching prefixes (`⏺`, `❯`, `⎿`, `※`).

**Tech Stack:** Rust, tokio, reqwest (existing), serde_json, ratatui, uuid. No new crates.

**Spec:** `docs/superpowers/specs/2026-05-15-scroll-area-and-tool-execution-parity-design.md`

---

## File structure

**Create:**
- `cli/src/conversation/session_bus.rs` — `SessionBus` wrapper around `broadcast::Sender<BusMessage>` with `subscribe()` / `emit()`.
- `cli/src/conversation/sse.rs` — `parse_sse_line` and `SseParser` state machine (turns reqwest byte chunks into `StreamEvent`).
- `cli/src/conversation/anthropic.rs` — Anthropic-shaped request body builder (system, messages, tools, stream).
- `cli/src/conversation/tool_loop.rs` — Per-turn tool-call execution: partition by `is_concurrency_safe`, run, collect `tool_result` blocks.
- `cli/src/tui/transcript.rs` — `TranscriptItem` enum + pure `fold(events, filter) -> Vec<TranscriptItem>` function.
- `cli/tests/sse_parser_test.rs` — Integration test for SSE parser against a recorded stream.
- `cli/tests/fold_test.rs` — Integration test for the event fold.

**Modify:**
- `cli/src/sdk/protocol.rs` — Extend with `StreamEvent`, `BusMessage`, `ContentBlock` variants, `BlockDelta`, `MessageMeta`, `MessageDeltaInfo`, `UsageInfo`, `SystemSubtype`.
- `cli/src/conversation/engine.rs` — Replace OpenAI-shaped `process_prompt` with Anthropic-shaped streaming tool loop that emits to the bus.
- `cli/src/conversation/mod.rs` — `pub mod session_bus; pub mod sse; pub mod anthropic; pub mod tool_loop;`
- `cli/src/tui/mod.rs` — `pub mod transcript;`
- `cli/src/tui/scroll_area.rs` — Replace `Message` enum with `TranscriptItem`-rendering; consume bus subscriber.
- `cli/src/tui/app.rs` — Own `Arc<SessionBus>`, pass to engine, subscribe scroll_area to it; always render `Header` above scroll_area.
- `cli/src/bootstrap.rs` — Construct `SessionBus` and pass into engine.
- `cli/src/tools/contract.rs` — Add `parent_tool_use_id: Option<String>` and `bus: Option<Arc<SessionBus>>` to `ToolCallContext` (defaulted, unused v1).
- `shared/src/lib.rs` — Add `api_messages_base_url: String` to `CliConfig` (defaults to `"https://openrouter.ai/api"`).

**Test:**
- `cli/tests/sse_parser_test.rs`
- `cli/tests/fold_test.rs`
- Plus `#[cfg(test)] mod tests { ... }` blocks inside each new module.

---

## Task 1: Add protocol types

**Files:**
- Modify: `cli/src/sdk/protocol.rs`

- [ ] **Step 1: Append the new types**

First, at the very top of `cli/src/sdk/protocol.rs` (after the existing `use serde::{Deserialize, Serialize};` line), add:

```rust
use uuid::Uuid;
```

Then append the following to `cli/src/sdk/protocol.rs` (after the existing `SdkSession` impl block):

```rust
/// Layer 1 — raw Anthropic-shaped stream events (verbatim wire format).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: MessageMeta },
    #[serde(rename = "content_block_start")]
    ContentBlockStart { index: u32, content_block: ContentBlockStream },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: BlockDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },
    #[serde(rename = "message_delta")]
    MessageDelta { delta: MessageDeltaInfo, usage: UsageInfo },
    #[serde(rename = "message_stop")]
    MessageStop,
    /// Anthropic also sends `ping` events for keep-alive. We accept them silently.
    #[serde(rename = "ping")]
    Ping,
}

/// Content blocks as they appear on the wire during streaming (text/thinking start
/// with empty strings; tool_use starts with empty input).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlockStream {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, #[serde(default)] signature: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, #[serde(default)] input: serde_json::Value },
}

/// Content blocks in finalized assistant/user messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlockFinal {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: serde_json::Value },
    #[serde(rename = "tool_result")]
    ToolResult { tool_use_id: String, content: String, is_error: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BlockDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageMeta {
    pub id: String,
    pub model: String,
    pub role: String,
    #[serde(default)]
    pub content: Vec<ContentBlockFinal>,
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
    pub usage: UsageInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDeltaInfo {
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
}

/// Reuses existing UsageInfo from this file but with all fields optional/defaulted
/// so partial usage deltas parse cleanly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnthropicUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
}

/// Layer 2 — envelopes carried on the session bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BusMessage {
    #[serde(rename = "user")]
    User {
        message: UserPayload,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "assistant")]
    Assistant {
        message: AssistantPayload,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "stream_event")]
    StreamEventEnvelope {
        event: StreamEvent,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "tool_progress")]
    ToolProgress {
        tool_use_id: String,
        tool_name: String,
        elapsed_seconds: f32,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "system")]
    SystemEvent {
        subtype: SystemSubtype,
        message: String,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "result")]
    Result {
        stop_reason: Option<String>,
        usage: AnthropicUsage,
        total_cost_usd: f64,
        duration_ms: u64,
        num_turns: u32,
        uuid: Uuid,
        session_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPayload {
    pub role: String, // always "user"
    pub content: Vec<ContentBlockFinal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantPayload {
    pub id: String,
    pub model: String,
    pub role: String, // always "assistant"
    pub content: Vec<ContentBlockFinal>,
    pub stop_reason: Option<String>,
    pub usage: AnthropicUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemSubtype {
    CompactBoundary,
    PostTurnSummary,
    ApiRetry,
    PermissionRequest,
    Notice,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_event_text_delta_roundtrip() {
        let json = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#;
        let parsed: StreamEvent = serde_json::from_str(json).unwrap();
        match parsed {
            StreamEvent::ContentBlockDelta { index, delta: BlockDelta::TextDelta { text } } => {
                assert_eq!(index, 0);
                assert_eq!(text, "hi");
            }
            _ => panic!("wrong variant: {parsed:?}"),
        }
    }

    #[test]
    fn message_start_parses() {
        let json = r#"{"type":"message_start","message":{"id":"msg_1","model":"anthropic/claude-sonnet-4-5","role":"assistant","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":0}}}"#;
        let parsed: StreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(parsed, StreamEvent::MessageStart { .. }));
    }

    #[test]
    fn tool_use_input_json_delta_parses() {
        let json = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\":\"PLAN"}}"#;
        let parsed: StreamEvent = serde_json::from_str(json).unwrap();
        match parsed {
            StreamEvent::ContentBlockDelta { delta: BlockDelta::InputJsonDelta { partial_json }, .. } => {
                assert!(partial_json.starts_with("{\"file_path\""));
            }
            _ => panic!("wrong variant: {parsed:?}"),
        }
    }

    #[test]
    fn ping_event_parses() {
        let json = r#"{"type":"ping"}"#;
        let parsed: StreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(parsed, StreamEvent::Ping));
    }

    #[test]
    fn bus_message_assistant_roundtrip() {
        let msg = BusMessage::Assistant {
            message: AssistantPayload {
                id: "m1".into(),
                model: "anthropic/claude-sonnet-4-5".into(),
                role: "assistant".into(),
                content: vec![ContentBlockFinal::Text { text: "hi".into() }],
                stop_reason: Some("end_turn".into()),
                usage: AnthropicUsage::default(),
            },
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: BusMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, BusMessage::Assistant { .. }));
    }
}
```

- [ ] **Step 2: Run the new tests**

Run: `cargo test -p super-cli --lib sdk::protocol::tests`

Expected: 5 tests pass.

- [ ] **Step 3: Commit**

```bash
git add cli/src/sdk/protocol.rs
git commit -m "feat(sdk): add StreamEvent, BusMessage, and content block types

Adds the two-layer event protocol per the spec: StreamEvent matches
Anthropic's SSE wire format verbatim; BusMessage envelopes carry
session_id and parent_tool_use_id for subagent compatibility."
```


## Task 2: Add `SessionBus`

**Files:**
- Create: `cli/src/conversation/session_bus.rs`
- Modify: `cli/src/conversation/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `cli/src/conversation/session_bus.rs`:

```rust
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::sdk::protocol::{BusMessage, SystemSubtype};

#[derive(Clone)]
pub struct SessionBus {
    sender: Arc<broadcast::Sender<BusMessage>>,
    pub session_id: String,
}

impl SessionBus {
    pub fn new(session_id: String) -> Self {
        let (sender, _) = broadcast::channel(256);
        Self { sender: Arc::new(sender), session_id }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BusMessage> {
        self.sender.subscribe()
    }

    pub fn emit(&self, msg: BusMessage) {
        // It's fine if there are no subscribers — drop silently.
        let _ = self.sender.send(msg);
    }

    /// Convenience for emitting a system notice.
    pub fn emit_system(&self, subtype: SystemSubtype, message: impl Into<String>) {
        self.emit(BusMessage::SystemEvent {
            subtype,
            message: message.into(),
            uuid: Uuid::new_v4(),
            session_id: self.session_id.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::protocol::AnthropicUsage;

    #[tokio::test]
    async fn subscribe_receives_emitted_messages() {
        let bus = SessionBus::new("s1".into());
        let mut rx = bus.subscribe();
        bus.emit(BusMessage::Result {
            stop_reason: Some("end_turn".into()),
            usage: AnthropicUsage::default(),
            total_cost_usd: 0.0,
            duration_ms: 0,
            num_turns: 1,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        });
        let msg = rx.recv().await.unwrap();
        assert!(matches!(msg, BusMessage::Result { .. }));
    }

    #[tokio::test]
    async fn emit_with_no_subscribers_does_not_panic() {
        let bus = SessionBus::new("s1".into());
        bus.emit_system(SystemSubtype::Notice, "no one listening");
    }

    #[tokio::test]
    async fn multiple_subscribers_each_receive() {
        let bus = SessionBus::new("s1".into());
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        bus.emit_system(SystemSubtype::Notice, "hello");
        assert!(matches!(rx1.recv().await.unwrap(), BusMessage::SystemEvent { .. }));
        assert!(matches!(rx2.recv().await.unwrap(), BusMessage::SystemEvent { .. }));
    }
}
```

- [ ] **Step 2: Wire the module**

Modify `cli/src/conversation/mod.rs` — add a line:

```rust
pub mod session_bus;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p super-cli --lib conversation::session_bus`

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add cli/src/conversation/session_bus.rs cli/src/conversation/mod.rs
git commit -m "feat(conversation): add SessionBus for fanout of BusMessages

broadcast::Sender wrapper with subscribe/emit/emit_system helpers.
Owned by App and threaded into ConversationEngine; future server
forwarder subscribes the same way."
```


## Task 3: SSE parser

**Files:**
- Create: `cli/src/conversation/sse.rs`
- Modify: `cli/src/conversation/mod.rs`
- Create: `cli/tests/sse_parser_test.rs`

- [ ] **Step 1: Write the failing test**

Create `cli/tests/sse_parser_test.rs`:

```rust
use super_cli::conversation::sse::SseParser;
use super_cli::sdk::protocol::{BlockDelta, StreamEvent};

/// One SSE frame split across two byte chunks must still produce one event.
#[test]
fn parses_event_split_across_chunks() {
    let mut parser = SseParser::new();
    let chunk_a = b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\"";
    let chunk_b = b",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n";

    let events_a = parser.feed(chunk_a);
    assert!(events_a.is_empty(), "no complete frames in first chunk yet");

    let events_b = parser.feed(chunk_b);
    assert_eq!(events_b.len(), 1);
    match &events_b[0] {
        StreamEvent::ContentBlockDelta { index, delta: BlockDelta::TextDelta { text } } => {
            assert_eq!(*index, 0);
            assert_eq!(text, "hi");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn parses_multiple_events_in_one_chunk() {
    let mut parser = SseParser::new();
    let chunk = b"event: ping\ndata: {\"type\":\"ping\"}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let events = parser.feed(chunk);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], StreamEvent::Ping));
    assert!(matches!(events[1], StreamEvent::MessageStop));
}

#[test]
fn ignores_comments_and_unknown_fields() {
    let mut parser = SseParser::new();
    let chunk = b": this is a comment\nretry: 1000\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let events = parser.feed(chunk);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], StreamEvent::MessageStop));
}

#[test]
fn data_only_frames_use_data_field_only() {
    // Anthropic typically sends an `event:` field, but the SSE spec allows
    // data-only frames. Our parser should still try to deserialize them.
    let mut parser = SseParser::new();
    let chunk = b"data: {\"type\":\"message_stop\"}\n\n";
    let events = parser.feed(chunk);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], StreamEvent::MessageStop));
}

#[test]
fn malformed_json_is_skipped_not_panicked() {
    let mut parser = SseParser::new();
    let chunk = b"event: garbage\ndata: not-json\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let events = parser.feed(chunk);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], StreamEvent::MessageStop));
}
```

- [ ] **Step 2: Run the test, confirm it fails**

Run: `cargo test -p super-cli --test sse_parser_test`

Expected: FAIL — `unresolved import super_cli::conversation::sse`.

- [ ] **Step 3: Write the minimal implementation**

Create `cli/src/conversation/sse.rs`:

```rust
use crate::sdk::protocol::StreamEvent;

/// Incremental SSE parser. Accumulates bytes from a streaming HTTP response and
/// emits `StreamEvent`s for each complete `event/data` frame (frames are
/// separated by a blank line per the SSE spec).
///
/// Malformed JSON in a `data:` field is silently skipped — the upstream may
/// send proprietary frame types we don't model yet, and we don't want one
/// unknown frame to kill the stream.
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self { buffer: String::new() }
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<StreamEvent> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut out = Vec::new();

        // SSE frames end at "\n\n". Split, parse complete frames, keep the
        // trailing partial frame in the buffer.
        loop {
            let Some(end) = self.buffer.find("\n\n") else { break };
            let frame = self.buffer[..end].to_string();
            self.buffer.drain(..end + 2);

            if let Some(event) = parse_frame(&frame) {
                out.push(event);
            }
        }

        out
    }
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_frame(frame: &str) -> Option<StreamEvent> {
    let mut data_lines: Vec<&str> = Vec::new();
    for line in frame.lines() {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let Some((field, value)) = line.split_once(':') else { continue };
        // Per SSE, a single leading space after the colon is stripped.
        let value = value.strip_prefix(' ').unwrap_or(value);
        if field == "data" {
            data_lines.push(value);
        }
        // We deliberately ignore `event:` — the type discriminator lives
        // inside the JSON payload (`"type": "..."`), which is what serde uses.
    }
    if data_lines.is_empty() {
        return None;
    }
    let joined = data_lines.join("\n");
    serde_json::from_str::<StreamEvent>(&joined).ok()
}
```

- [ ] **Step 4: Wire the module**

Modify `cli/src/conversation/mod.rs` — add `pub mod sse;` and also `pub mod` exposure for `protocol` via `pub use` if needed. Check the existing `mod.rs` and add only the missing `pub mod sse;` line.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p super-cli --test sse_parser_test`

Expected: 5 tests pass.

- [ ] **Step 6: Commit**

```bash
git add cli/src/conversation/sse.rs cli/src/conversation/mod.rs cli/tests/sse_parser_test.rs
git commit -m "feat(conversation): incremental SSE parser for Anthropic stream

Splits byte chunks from reqwest into complete event frames, deserializes
their JSON payloads into StreamEvent. Tolerates split frames, comments,
unknown event types, and malformed JSON."
```


## Task 4: Anthropic request body builder

**Files:**
- Create: `cli/src/conversation/anthropic.rs`
- Modify: `cli/src/conversation/mod.rs`
- Modify: `shared/src/lib.rs`

- [ ] **Step 1: Add config field for the base URL**

Modify `shared/src/lib.rs` — inside `CliConfig`, add a new field. Find the struct (around line 43) and add:

```rust
    #[serde(default = "default_messages_base_url")]
    pub api_messages_base_url: String,
```

And add the default fn next to other defaults:

```rust
fn default_messages_base_url() -> String {
    "https://openrouter.ai/api".to_string()
}
```

And in `impl Default for CliConfig`, add:

```rust
            api_messages_base_url: default_messages_base_url(),
```

- [ ] **Step 2: Verify config still loads**

Run: `cargo build -p super-cli`

Expected: builds. No new warnings.

- [ ] **Step 3: Write the failing test**

Create `cli/src/conversation/anthropic.rs`:

```rust
use serde_json::{json, Value};
use std::sync::Arc;

use crate::sdk::protocol::ContentBlockFinal;
use crate::tools::contract::Tool;

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
                "description": t.description(),
                "input_schema": t.input_schema(),
            })
        })
        .collect();

    let messages_json: Vec<Value> = history.iter().map(|m| {
        json!({
            "role": match m.role { Role::User => "user", Role::Assistant => "assistant" },
            "content": m.content,
        })
    }).collect();

    json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": messages_json,
        "tools": tools_json,
        "stream": stream,
    })
}

#[derive(Debug, Clone, Copy)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
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
}
```

- [ ] **Step 4: Wire the module**

Modify `cli/src/conversation/mod.rs` — add `pub mod anthropic;`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p super-cli --lib conversation::anthropic`

Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add cli/src/conversation/anthropic.rs cli/src/conversation/mod.rs shared/src/lib.rs
git commit -m "feat(conversation): Anthropic /v1/messages request body builder

Builds the Anthropic-shaped request body (system, messages, tools,
stream) used by both streaming and non-streaming engine paths. Adds
api_messages_base_url to CliConfig defaulting to OpenRouter's base."
```


## Task 5: Extend `ToolCallContext` (additive, no behavior change)

**Files:**
- Modify: `cli/src/tools/contract.rs`

- [ ] **Step 1: Add the two new fields**

Modify `cli/src/tools/contract.rs` — extend `ToolCallContext`:

```rust
pub struct ToolCallContext {
    pub cwd: std::path::PathBuf,
    pub permission_mode: crate::state::store::PermissionMode,
    pub abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
    /// When the tool is invoked from inside a subagent, this carries the
    /// parent's invoking tool_use_id so emitted events can be demuxed by
    /// consumers (TUI, server forwarder, sidechain transcript writer).
    /// Always `None` in v1 — subagent execution is out of scope here.
    pub parent_tool_use_id: Option<String>,
    /// Handle to the session bus, available to tools that need to emit
    /// events (currently only AgentTool when subagents are wired up).
    /// `None` v1.
    pub bus: Option<std::sync::Arc<crate::conversation::session_bus::SessionBus>>,
}
```

- [ ] **Step 2: Find existing constructors and add defaults**

Run: `grep -rn "ToolCallContext {" cli/src/`

For every call site that constructs `ToolCallContext`, add the two new fields:

```rust
parent_tool_use_id: None,
bus: None,
```

This will be at least one site in the engine and any tool-loop test helper.

- [ ] **Step 3: Verify build**

Run: `cargo build -p super-cli`

Expected: builds. Some "unused field" warnings on the two new fields are acceptable for now.

- [ ] **Step 4: Commit**

```bash
git add cli/src/tools/contract.rs cli/src/
git commit -m "feat(tools): add parent_tool_use_id and bus to ToolCallContext

Additive fields for subagent compatibility. Both are None in v1; the
fields exist so subagent execution can be wired in later without
re-threading ToolCallContext through every tool."
```


## Task 6: Tool loop (per-turn execution of tool_use blocks)

**Files:**
- Create: `cli/src/conversation/tool_loop.rs`
- Modify: `cli/src/conversation/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `cli/src/conversation/tool_loop.rs`:

```rust
use std::sync::Arc;
use tokio::sync::watch;

use crate::sdk::protocol::ContentBlockFinal;
use crate::state::store::PermissionMode;
use crate::tools::contract::{Tool, ToolCallContext, ToolResult};
use crate::tools::ToolRegistry;

/// Execute all tool_use blocks from one assistant turn, returning the
/// corresponding tool_result blocks in emission order. Concurrency-safe tools
/// run in parallel via tokio::join_all; others run sequentially.
pub async fn run_tool_uses(
    registry: &ToolRegistry,
    tool_uses: Vec<(String, String, serde_json::Value)>, // (id, name, input)
    cwd: std::path::PathBuf,
    permission_mode: PermissionMode,
    abort_signal: Option<watch::Receiver<bool>>,
) -> Vec<ContentBlockFinal> {
    // Partition by concurrency safety.
    let mut safe: Vec<(String, Arc<dyn Tool>, serde_json::Value)> = Vec::new();
    let mut unsafe_: Vec<(String, Arc<dyn Tool>, serde_json::Value)> = Vec::new();
    for (id, name, input) in tool_uses {
        let Some(tool) = registry.get(&name) else {
            // Unknown tool — emit error result so the model sees it.
            unsafe_.push((id, Arc::new(MissingTool { name }), input));
            continue;
        };
        if tool.is_concurrency_safe() {
            safe.push((id, tool, input));
        } else {
            unsafe_.push((id, tool, input));
        }
    }

    let cwd_a = cwd.clone();
    let abort_a = abort_signal.clone();
    let pm_a = permission_mode.clone();

    let safe_futs = safe.into_iter().map(|(id, tool, input)| {
        let ctx = ToolCallContext {
            cwd: cwd_a.clone(),
            permission_mode: pm_a.clone(),
            abort_signal: abort_a.clone(),
            parent_tool_use_id: None,
            bus: None,
        };
        async move {
            let res = tool.call(input, &ctx).await;
            (id, res)
        }
    });

    let safe_results = futures::future::join_all(safe_futs).await;

    let mut unsafe_results = Vec::new();
    for (id, tool, input) in unsafe_ {
        let ctx = ToolCallContext {
            cwd: cwd.clone(),
            permission_mode: permission_mode.clone(),
            abort_signal: abort_signal.clone(),
            parent_tool_use_id: None,
            bus: None,
        };
        let res = tool.call(input, &ctx).await;
        unsafe_results.push((id, res));
    }

    let mut out: Vec<ContentBlockFinal> = Vec::with_capacity(safe_results.len() + unsafe_results.len());
    for (id, res) in safe_results.into_iter().chain(unsafe_results.into_iter()) {
        out.push(ContentBlockFinal::ToolResult {
            tool_use_id: id,
            content: res.content,
            is_error: res.is_error,
        });
    }
    out
}

/// Sentinel for unknown tool names — produces an error tool_result so the
/// model sees the failure on the next round-trip and can recover.
struct MissingTool {
    name: String,
}

#[async_trait::async_trait]
impl Tool for MissingTool {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { "missing" }
    fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
    async fn call(&self, _input: serde_json::Value, _ctx: &ToolCallContext) -> ToolResult {
        ToolResult {
            content: format!("Unknown tool: {}", self.name),
            is_error: true,
            metadata: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::Store;
    use shared::CliConfig;

    #[tokio::test]
    async fn unknown_tool_produces_error_result() {
        let store = Arc::new(Store::new());
        let registry = ToolRegistry::new(store, CliConfig::default());
        let results = run_tool_uses(
            &registry,
            vec![("tu_1".into(), "DoesNotExist".into(), serde_json::json!({}))],
            std::env::current_dir().unwrap(),
            PermissionMode::Default,
            None,
        )
        .await;
        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlockFinal::ToolResult { tool_use_id, content, is_error } => {
                assert_eq!(tool_use_id, "tu_1");
                assert!(*is_error);
                assert!(content.contains("Unknown tool"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_tool_executes_against_real_file() {
        let store = Arc::new(Store::new());
        let registry = ToolRegistry::new(store, CliConfig::default());
        let tmpfile = std::env::temp_dir().join("super_tool_loop_test.txt");
        std::fs::write(&tmpfile, "hello\nworld\n").unwrap();

        let results = run_tool_uses(
            &registry,
            vec![("tu_2".into(), "Read".into(), serde_json::json!({"file_path": tmpfile.to_string_lossy()}))],
            std::env::current_dir().unwrap(),
            PermissionMode::Default,
            None,
        )
        .await;
        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlockFinal::ToolResult { tool_use_id, content, is_error } => {
                assert_eq!(tool_use_id, "tu_2");
                assert!(!*is_error, "got error: {content}");
                assert!(content.contains("hello"));
                assert!(content.contains("world"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
        std::fs::remove_file(&tmpfile).ok();
    }
}
```

- [ ] **Step 2: Add the `futures` dependency**

Modify `cli/Cargo.toml` `[dependencies]` block — add a line if `futures` is not already present:

```toml
futures = "0.3"
```

- [ ] **Step 3: Wire the module**

Modify `cli/src/conversation/mod.rs` — add `pub mod tool_loop;`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p super-cli --lib conversation::tool_loop`

Expected: 2 tests pass.

- [ ] **Step 5: Commit**

```bash
git add cli/Cargo.toml cli/src/conversation/tool_loop.rs cli/src/conversation/mod.rs
git commit -m "feat(conversation): per-turn tool execution loop

Partitions tool_use blocks by is_concurrency_safe, runs the safe set in
parallel via futures::join_all, runs the rest sequentially, and returns
tool_result content blocks tagged with each tool_use_id."
```


## Task 7: Replace `ConversationEngine` with the streaming Anthropic loop

This is the largest task. It replaces the engine wholesale — old OpenAI/`chat/completions` path goes away.

**Files:**
- Modify: `cli/src/conversation/engine.rs`

- [ ] **Step 1: Rewrite the engine module**

Overwrite `cli/src/conversation/engine.rs` with the following. Read the existing file first (so you preserve the `pub use` and `compaction` integration), then replace:

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

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
use crate::state::store::{PermissionMode, Store};
use crate::tools::ToolRegistry;

#[derive(Clone)]
pub struct ConversationEngine {
    pub store: Arc<Store>,
    pub config: CliConfig,
    pub registry: Arc<ToolRegistry>,
    pub bus: Arc<SessionBus>,
    pub abort: Option<watch::Receiver<bool>>,
}

impl ConversationEngine {
    pub fn new(
        store: Arc<Store>,
        config: CliConfig,
        registry: Arc<ToolRegistry>,
        bus: Arc<SessionBus>,
    ) -> Self {
        Self { store, config, registry, bus, abort: None }
    }

    /// Drive one user turn end-to-end: emit the user message, loop on
    /// (request → stream → tool execution) until the model returns
    /// `stop_reason == "end_turn"` (or another terminal reason).
    pub async fn process_prompt(
        &self,
        user_input: String,
        system_prompt: &SystemPrompt,
    ) -> Result<String, String> {
        let session_id = self.bus.session_id.clone();
        let model = self.config.model.clone();
        let base_url = self.config.api_messages_base_url.clone();
        let api_key = self.config.openrouter_api_key.clone()
            .ok_or_else(|| "no OpenRouter API key configured".to_string())?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let permission_mode = self.store.get_state().permission_mode.clone();
        let tools = self.registry.assemble_for_mode(&permission_mode);

        // Emit the initial user message and seed history.
        let mut history: Vec<HistoryEntry> = self.store.get_state().history.clone();
        history.push(HistoryEntry {
            role: Role::User,
            content: vec![ContentBlockFinal::Text { text: user_input.clone() }],
        });
        self.bus.emit(BusMessage::User {
            message: UserPayload {
                role: "user".to_string(),
                content: vec![ContentBlockFinal::Text { text: user_input.clone() }],
            },
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: session_id.clone(),
        });

        let started = Instant::now();
        let mut total_input_tokens: u64 = 0;
        let mut total_output_tokens: u64 = 0;
        let mut last_assistant_text: String = String::new();
        let mut num_turns: u32 = 0;

        let client = reqwest::Client::new();
        let system_rendered = system_prompt.render();

        loop {
            num_turns += 1;

            let body = build_request_body(
                &model,
                &system_rendered,
                &history,
                &tools,
                8192,
                true,
            );

            let response = client
                .post(format!("{}/v1/messages", base_url.trim_end_matches('/')))
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
                return Err(format!("API error ({status}): {text}"));
            }

            // Per-turn state used to fold deltas into a final AssistantPayload.
            let mut current_blocks: Vec<Option<PartialBlock>> = Vec::new();
            let mut stop_reason: Option<String> = None;
            let mut message_id: String = String::new();
            let mut response_model: String = model.clone();

            let mut parser = SseParser::new();
            let mut byte_stream = response.bytes_stream();
            use futures::StreamExt;
            while let Some(chunk) = byte_stream.next().await {
                let chunk = chunk.map_err(|e| format!("stream error: {e}"))?;
                let events = parser.feed(&chunk);
                for event in events {
                    // Always echo to the bus first so the TUI gets live deltas.
                    self.bus.emit(BusMessage::StreamEventEnvelope {
                        event: event.clone(),
                        parent_tool_use_id: None,
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

            // Build the finalized assistant message from folded blocks.
            let content: Vec<ContentBlockFinal> = current_blocks
                .into_iter()
                .flatten()
                .filter_map(|pb| pb.finalize())
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
                parent_tool_use_id: None,
                uuid: Uuid::new_v4(),
                session_id: session_id.clone(),
            });

            history.push(HistoryEntry {
                role: Role::Assistant,
                content: content.clone(),
            });

            // Collect tool_use blocks for execution.
            let tool_uses: Vec<(String, String, serde_json::Value)> = content
                .iter()
                .filter_map(|b| match b {
                    ContentBlockFinal::ToolUse { id, name, input } => {
                        Some((id.clone(), name.clone(), input.clone()))
                    }
                    _ => None,
                })
                .collect();

            // Track last assistant text for the return value (for the TUI's
            // legacy "engine result" handler).
            for b in &content {
                if let ContentBlockFinal::Text { text } = b {
                    last_assistant_text = text.clone();
                }
            }

            if tool_uses.is_empty() {
                // Turn complete.
                self.bus.emit(BusMessage::Result {
                    stop_reason: stop_reason.clone(),
                    usage,
                    total_cost_usd: 0.0,
                    duration_ms: started.elapsed().as_millis() as u64,
                    num_turns,
                    uuid: Uuid::new_v4(),
                    session_id: session_id.clone(),
                });
                // Persist the history into the store for compaction / next turn.
                self.store.set_state(|s| { s.history = history.clone(); });
                return Ok(last_assistant_text);
            }

            // Otherwise execute tools, build synthetic user message, loop.
            let tool_results = run_tool_uses(
                &self.registry,
                tool_uses,
                cwd.clone(),
                permission_mode.clone(),
                self.abort.clone(),
            )
            .await;

            self.bus.emit(BusMessage::User {
                message: UserPayload {
                    role: "user".to_string(),
                    content: tool_results.clone(),
                },
                parent_tool_use_id: None,
                uuid: Uuid::new_v4(),
                session_id: session_id.clone(),
            });

            history.push(HistoryEntry { role: Role::User, content: tool_results });

            // Safety valve.
            if num_turns >= 50 {
                self.bus.emit_system(SystemSubtype::Notice, "max turns (50) reached");
                self.store.set_state(|s| { s.history = history.clone(); });
                return Ok(last_assistant_text);
            }
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
    fn finalize(self) -> Option<ContentBlockFinal> {
        match self {
            PartialBlock::Text { text } => Some(ContentBlockFinal::Text { text }),
            PartialBlock::Thinking { thinking, signature } => {
                Some(ContentBlockFinal::Thinking { thinking, signature })
            }
            PartialBlock::ToolUse { id, name, input_json } => {
                let input: serde_json::Value = serde_json::from_str(&input_json).unwrap_or(serde_json::json!({}));
                Some(ContentBlockFinal::ToolUse { id, name, input })
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
            *in_tokens += message.usage.input_tokens;
            *out_tokens += message.usage.output_tokens;
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
                ContentBlockStream::ToolUse { id, name, .. } => {
                    PartialBlock::ToolUse { id, name, input_json: String::new() }
                }
            });
        }
        StreamEvent::ContentBlockDelta { index, delta } => {
            let idx = index as usize;
            if let Some(Some(pb)) = blocks.get_mut(idx) {
                match (pb, delta) {
                    (PartialBlock::Text { text }, BlockDelta::TextDelta { text: d }) => text.push_str(&d),
                    (PartialBlock::Thinking { thinking, .. }, BlockDelta::ThinkingDelta { thinking: d }) => thinking.push_str(&d),
                    (PartialBlock::Thinking { signature, .. }, BlockDelta::SignatureDelta { signature: s }) => *signature = s,
                    (PartialBlock::ToolUse { input_json, .. }, BlockDelta::InputJsonDelta { partial_json }) => input_json.push_str(&partial_json),
                    _ => {} // mismatched delta type — skip
                }
            }
        }
        StreamEvent::ContentBlockStop { .. } => {}
        StreamEvent::MessageDelta { delta, usage } => {
            if let Some(sr) = delta.stop_reason { *stop_reason = Some(sr); }
            *out_tokens += usage.output_tokens;
        }
        StreamEvent::MessageStop => {}
        StreamEvent::Ping => {}
    }
}
```

- [ ] **Step 2: Extend `Store::State` with conversation history**

Modify `cli/src/state/store.rs` — the existing `State` struct has `messages: Vec<Message>` from the TUI scroll area. Add the protocol-level history field next to it:

```rust
use crate::conversation::anthropic::HistoryEntry;

pub struct State {
    // ... existing fields ...
    pub history: Vec<HistoryEntry>,
}
```

And in the constructor / Default impl, initialize `history: Vec::new()`. Add `history: self.history.clone()` to any `Clone` impl that exists.

- [ ] **Step 3: Update `bootstrap.rs` to wire bus + registry into engine**

Modify `cli/src/bootstrap.rs`. Find the `ConversationEngine::new(store.clone(), config.clone())` call and replace with:

```rust
    let bus = std::sync::Arc::new(crate::conversation::session_bus::SessionBus::new(
        uuid::Uuid::new_v4().to_string(),
    ));
    let registry = std::sync::Arc::new(crate::tools::ToolRegistry::new(store.clone(), config.clone()));
    // ... existing skills registration uses `registry` (now Arc-wrapped) ...
    let engine = crate::conversation::engine::ConversationEngine::new(
        store.clone(),
        config.clone(),
        registry.clone(),
        bus.clone(),
    );
```

Also update the call to `run_with_engine` to also pass `bus`. Adjust signatures as needed (next task wires bus into App).

- [ ] **Step 4: Verify it builds**

Run: `cargo build -p super-cli`

Expected: builds. May surface mismatches in `App::new` / `run_with_engine` signatures — fix in the next task.

- [ ] **Step 5: Commit**

```bash
git add cli/src/conversation/engine.rs cli/src/state/store.rs cli/src/bootstrap.rs
git commit -m "feat(engine): streaming Anthropic tool-call loop over OpenRouter

Replaces the OpenAI chat/completions stub with the real Anthropic
/v1/messages SSE loop: stream → fold into AssistantPayload → run
tool_uses → emit synthetic UserPayload with tool_result blocks → loop
until stop_reason terminates. All events are emitted on the SessionBus
so the TUI (and future server forwarder) can render them live."
```


## Task 8: `TranscriptItem` + pure fold function

**Files:**
- Create: `cli/src/tui/transcript.rs`
- Modify: `cli/src/tui/mod.rs`
- Create: `cli/tests/fold_test.rs`

- [ ] **Step 1: Write the failing test**

Create `cli/tests/fold_test.rs`:

```rust
use super_cli::sdk::protocol::{
    AnthropicUsage, AssistantPayload, BlockDelta, BusMessage, ContentBlockFinal,
    ContentBlockStream, StreamEvent, UserPayload,
};
use super_cli::tui::transcript::{fold, TranscriptItem};
use uuid::Uuid;

fn env(event: StreamEvent) -> BusMessage {
    BusMessage::StreamEventEnvelope {
        event,
        parent_tool_use_id: None,
        uuid: Uuid::new_v4(),
        session_id: "s1".into(),
    }
}

#[test]
fn user_message_appended() {
    let events = vec![BusMessage::User {
        message: UserPayload {
            role: "user".into(),
            content: vec![ContentBlockFinal::Text { text: "hello".into() }],
        },
        parent_tool_use_id: None,
        uuid: Uuid::new_v4(),
        session_id: "s1".into(),
    }];
    let t = fold(&events, None);
    assert_eq!(t.len(), 1);
    assert!(matches!(&t[0], TranscriptItem::User { text } if text == "hello"));
}

#[test]
fn streaming_text_deltas_accumulate_into_one_assistant_text_item() {
    let events = vec![
        env(StreamEvent::ContentBlockStart { index: 0, content_block: ContentBlockStream::Text { text: String::new() } }),
        env(StreamEvent::ContentBlockDelta { index: 0, delta: BlockDelta::TextDelta { text: "hel".into() } }),
        env(StreamEvent::ContentBlockDelta { index: 0, delta: BlockDelta::TextDelta { text: "lo".into() } }),
        env(StreamEvent::ContentBlockStop { index: 0 }),
    ];
    let t = fold(&events, None);
    assert_eq!(t.len(), 1);
    match &t[0] {
        TranscriptItem::AssistantText { text, complete } => {
            assert_eq!(text, "hello");
            assert!(*complete);
        }
        other => panic!("wrong: {other:?}"),
    }
}

#[test]
fn tool_use_followed_by_tool_result_attaches() {
    let events = vec![
        env(StreamEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlockStream::ToolUse {
                id: "tu_1".into(), name: "Read".into(), input: serde_json::json!({}),
            },
        }),
        env(StreamEvent::ContentBlockDelta {
            index: 0,
            delta: BlockDelta::InputJsonDelta { partial_json: "{\"file_path\":\"/x\"}".into() },
        }),
        env(StreamEvent::ContentBlockStop { index: 0 }),
        BusMessage::User {
            message: UserPayload {
                role: "user".into(),
                content: vec![ContentBlockFinal::ToolResult {
                    tool_use_id: "tu_1".into(),
                    content: "file contents".into(),
                    is_error: false,
                }],
            },
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        },
    ];
    let t = fold(&events, None);
    assert_eq!(t.len(), 1);
    match &t[0] {
        TranscriptItem::ToolCall { tool_use_id, name, input, result, .. } => {
            assert_eq!(tool_use_id, "tu_1");
            assert_eq!(name, "Read");
            assert_eq!(input["file_path"], "/x");
            let r = result.as_ref().expect("result attached");
            assert_eq!(r.content, "file contents");
            assert!(!r.is_error);
        }
        other => panic!("wrong: {other:?}"),
    }
}

#[test]
fn result_message_does_not_add_transcript_item() {
    let events = vec![BusMessage::Result {
        stop_reason: Some("end_turn".into()),
        usage: AnthropicUsage::default(),
        total_cost_usd: 0.0, duration_ms: 0, num_turns: 1,
        uuid: Uuid::new_v4(), session_id: "s1".into(),
    }];
    let t = fold(&events, None);
    assert_eq!(t.len(), 0);
}

#[test]
fn assistant_final_message_is_skipped_when_already_folded_from_deltas() {
    // The engine emits BusMessage::Assistant after all deltas. If we've
    // already folded the deltas, the Assistant message must not double-add.
    let final_id = "msg_1".to_string();
    let events = vec![
        env(StreamEvent::MessageStart { message: super_cli::sdk::protocol::MessageMeta {
            id: final_id.clone(), model: "anthropic/claude-sonnet-4-5".into(), role: "assistant".into(),
            content: vec![], stop_reason: None, stop_sequence: None,
            usage: AnthropicUsage::default(),
        }}),
        env(StreamEvent::ContentBlockStart { index: 0, content_block: ContentBlockStream::Text { text: String::new() } }),
        env(StreamEvent::ContentBlockDelta { index: 0, delta: BlockDelta::TextDelta { text: "hi".into() } }),
        env(StreamEvent::ContentBlockStop { index: 0 }),
        BusMessage::Assistant {
            message: AssistantPayload {
                id: final_id, model: "anthropic/claude-sonnet-4-5".into(), role: "assistant".into(),
                content: vec![ContentBlockFinal::Text { text: "hi".into() }],
                stop_reason: Some("end_turn".into()), usage: AnthropicUsage::default(),
            },
            parent_tool_use_id: None, uuid: Uuid::new_v4(), session_id: "s1".into(),
        },
    ];
    let t = fold(&events, None);
    assert_eq!(t.len(), 1);
    assert!(matches!(&t[0], TranscriptItem::AssistantText { text, .. } if text == "hi"));
}
```

- [ ] **Step 2: Run, confirm failure**

Run: `cargo test -p super-cli --test fold_test`

Expected: FAIL — `unresolved import super_cli::tui::transcript`.

- [ ] **Step 3: Write the implementation**

Create `cli/src/tui/transcript.rs`:

```rust
use crate::sdk::protocol::{
    BlockDelta, BusMessage, ContentBlockFinal, ContentBlockStream, StreamEvent,
    SystemSubtype,
};

/// One renderable item in the transcript.
#[derive(Debug, Clone)]
pub enum TranscriptItem {
    User { text: String },
    AssistantText { text: String, complete: bool },
    Thinking { text: String, collapsed: bool, elapsed_ms: u64 },
    ToolCall {
        tool_use_id: String,
        name: String,
        input: serde_json::Value,
        result: Option<ToolResultRender>,
        elapsed_ms: u64,
    },
    System { subtype: SystemSubtype, message: String },
}

#[derive(Debug, Clone)]
pub struct ToolResultRender {
    pub content: String,
    pub is_error: bool,
}

/// Pure fold from a slice of bus events to a transcript.
///
/// `filter` — if `Some(tool_use_id)`, only events whose `parent_tool_use_id`
/// equals that id are folded (used for drilling into a subagent's transcript).
/// `None` returns the root view.
pub fn fold(events: &[BusMessage], filter: Option<&str>) -> Vec<TranscriptItem> {
    let mut out: Vec<TranscriptItem> = Vec::new();
    // index in content blocks (this turn) -> position in `out`
    let mut block_to_idx: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    // tool_use_id -> position in `out` (so tool_result can attach across turns)
    let mut tool_use_idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for ev in events {
        if !matches_filter(ev, filter) { continue; }
        match ev {
            BusMessage::User { message, .. } => {
                for block in &message.content {
                    match block {
                        ContentBlockFinal::Text { text } => {
                            out.push(TranscriptItem::User { text: text.clone() });
                        }
                        ContentBlockFinal::ToolResult { tool_use_id, content, is_error } => {
                            if let Some(idx) = tool_use_idx.get(tool_use_id) {
                                if let TranscriptItem::ToolCall { result, .. } = &mut out[*idx] {
                                    *result = Some(ToolResultRender {
                                        content: content.clone(),
                                        is_error: *is_error,
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            BusMessage::Assistant { .. } => {
                // Already folded from stream events; no-op.
                // (If a future code path emits Assistant without preceding
                // deltas, we'd render it here — currently unused.)
            }
            BusMessage::StreamEventEnvelope { event, .. } => match event {
                StreamEvent::ContentBlockStart { index, content_block } => {
                    match content_block {
                        ContentBlockStream::Text { text } => {
                            let pos = out.len();
                            out.push(TranscriptItem::AssistantText {
                                text: text.clone(), complete: false,
                            });
                            block_to_idx.insert(*index, pos);
                        }
                        ContentBlockStream::Thinking { thinking, .. } => {
                            let pos = out.len();
                            out.push(TranscriptItem::Thinking {
                                text: thinking.clone(), collapsed: true, elapsed_ms: 0,
                            });
                            block_to_idx.insert(*index, pos);
                        }
                        ContentBlockStream::ToolUse { id, name, input } => {
                            let pos = out.len();
                            out.push(TranscriptItem::ToolCall {
                                tool_use_id: id.clone(),
                                name: name.clone(),
                                input: input.clone(),
                                result: None,
                                elapsed_ms: 0,
                            });
                            block_to_idx.insert(*index, pos);
                            tool_use_idx.insert(id.clone(), pos);
                        }
                    }
                }
                StreamEvent::ContentBlockDelta { index, delta } => {
                    let Some(&pos) = block_to_idx.get(index) else { continue };
                    match (&mut out[pos], delta) {
                        (TranscriptItem::AssistantText { text, .. }, BlockDelta::TextDelta { text: d }) => {
                            text.push_str(d);
                        }
                        (TranscriptItem::Thinking { text, .. }, BlockDelta::ThinkingDelta { thinking: d }) => {
                            text.push_str(d);
                        }
                        (TranscriptItem::ToolCall { input, .. }, BlockDelta::InputJsonDelta { partial_json }) => {
                            // Accumulate partial JSON in a side string; finalize on Stop.
                            // We stash it in `input` as a string under "__partial__".
                            let cur = input.get("__partial__")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let next = format!("{cur}{partial_json}");
                            *input = serde_json::json!({"__partial__": next});
                        }
                        _ => {}
                    }
                }
                StreamEvent::ContentBlockStop { index } => {
                    let Some(&pos) = block_to_idx.get(index) else { continue };
                    if let TranscriptItem::AssistantText { complete, .. } = &mut out[pos] {
                        *complete = true;
                    }
                    if let TranscriptItem::ToolCall { input, .. } = &mut out[pos] {
                        if let Some(partial) = input.get("__partial__").and_then(|v| v.as_str()) {
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(partial) {
                                *input = parsed;
                            }
                        }
                    }
                }
                _ => {}
            },
            BusMessage::ToolProgress { tool_use_id, elapsed_seconds, .. } => {
                if let Some(&pos) = tool_use_idx.get(tool_use_id) {
                    if let TranscriptItem::ToolCall { elapsed_ms, .. } = &mut out[pos] {
                        *elapsed_ms = (*elapsed_seconds as u64) * 1000;
                    }
                }
            }
            BusMessage::SystemEvent { subtype, message, .. } => {
                out.push(TranscriptItem::System {
                    subtype: subtype.clone(),
                    message: message.clone(),
                });
            }
            BusMessage::Result { .. } => {
                // Renderers may show usage in the header; no transcript item.
            }
        }
    }

    out
}

fn matches_filter(ev: &BusMessage, filter: Option<&str>) -> bool {
    let parent = match ev {
        BusMessage::User { parent_tool_use_id, .. }
        | BusMessage::Assistant { parent_tool_use_id, .. }
        | BusMessage::StreamEventEnvelope { parent_tool_use_id, .. }
        | BusMessage::ToolProgress { parent_tool_use_id, .. } => parent_tool_use_id.as_deref(),
        BusMessage::SystemEvent { .. } | BusMessage::Result { .. } => None,
    };
    match filter {
        None => parent.is_none(),
        Some(want) => parent == Some(want),
    }
}
```

- [ ] **Step 4: Wire the module**

Modify `cli/src/tui/mod.rs` — add `pub mod transcript;`.

- [ ] **Step 5: Run the fold tests**

Run: `cargo test -p super-cli --test fold_test`

Expected: 5 tests pass.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tui/transcript.rs cli/src/tui/mod.rs cli/tests/fold_test.rs
git commit -m "feat(tui): TranscriptItem + pure fold(events, filter) function

Folds a slice of BusMessages into a list of renderable transcript
items. Stream-event deltas accumulate into in-progress AssistantText /
Thinking / ToolCall items; ToolResults attach to their matching
ToolCall by tool_use_id; the filter argument supports drilling into a
subagent's view by parent_tool_use_id (None for the root view)."
```


## Task 9: Rewrite `ScrollArea` to consume the bus and render `TranscriptItem`

**Files:**
- Modify: `cli/src/tui/scroll_area.rs`

- [ ] **Step 1: Replace the file body**

Overwrite `cli/src/tui/scroll_area.rs`:

```rust
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::sdk::protocol::{BusMessage, SystemSubtype};
use crate::tui::transcript::{fold, TranscriptItem, ToolResultRender};

pub struct ScrollArea {
    pub events: Vec<BusMessage>,
    scroll_offset: u16,
}

impl ScrollArea {
    pub fn new() -> Self {
        Self { events: Vec::new(), scroll_offset: 0 }
    }

    pub fn push_event(&mut self, msg: BusMessage) {
        self.events.push(msg);
    }

    pub fn clear(&mut self) {
        self.events.clear();
        self.scroll_offset = 0;
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let user_prefix_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
        let assistant_prefix_style = Style::default().fg(Color::Cyan);
        let body_style = Style::default().fg(Color::White);
        let dim = Style::default().fg(Color::DarkGray);
        let recap_style = Style::default().fg(Color::DarkGray);
        let tool_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

        let items = fold(&self.events, None);
        let mut lines: Vec<Line> = Vec::new();
        for item in items {
            match item {
                TranscriptItem::User { text } => {
                    lines.push(Line::from(""));
                    for (i, body_line) in text.lines().enumerate() {
                        let prefix = if i == 0 { "❯ " } else { "  " };
                        lines.push(Line::from(vec![
                            Span::styled(prefix, user_prefix_style),
                            Span::styled(body_line.to_string(), body_style),
                        ]));
                    }
                }
                TranscriptItem::AssistantText { text, .. } => {
                    lines.push(Line::from(""));
                    for (i, body_line) in text.lines().enumerate() {
                        let prefix = if i == 0 { "⏺ " } else { "  " };
                        lines.push(Line::from(vec![
                            Span::styled(prefix, assistant_prefix_style),
                            Span::styled(body_line.to_string(), body_style),
                        ]));
                    }
                }
                TranscriptItem::Thinking { text, collapsed, .. } => {
                    lines.push(Line::from(""));
                    if collapsed {
                        lines.push(Line::from(Span::styled(
                            "thinking…",
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                        )));
                    } else {
                        for body_line in text.lines() {
                            lines.push(Line::from(Span::styled(body_line.to_string(), dim)));
                        }
                    }
                }
                TranscriptItem::ToolCall { name, input, result, .. } => {
                    lines.push(Line::from(""));
                    let summary = summarize_tool_call(&name, &input);
                    lines.push(Line::from(vec![
                        Span::styled("⏺ ", assistant_prefix_style),
                        Span::styled(name.clone(), tool_style),
                        Span::raw(" "),
                        Span::styled(summary, dim),
                    ]));
                    if let Some(r) = result {
                        render_tool_result(&mut lines, &r, &dim);
                    }
                }
                TranscriptItem::System { subtype, message } => {
                    lines.push(Line::from(""));
                    let (prefix, style) = match subtype {
                        SystemSubtype::PostTurnSummary => ("※ recap: ", recap_style),
                        SystemSubtype::CompactBoundary => ("※ ", recap_style),
                        _ => ("※ ", dim),
                    };
                    for (i, body_line) in message.lines().enumerate() {
                        let p = if i == 0 { prefix } else { "  " };
                        lines.push(Line::from(vec![
                            Span::styled(p, style),
                            Span::styled(body_line.to_string(), dim),
                        ]));
                    }
                }
            }
        }

        let height = area.height as usize;
        let total = lines.len();
        let max_offset = total.saturating_sub(height);
        let offset = (self.scroll_offset as usize).min(max_offset);
        let paragraph = Paragraph::new(lines).scroll((offset as u16, 0));
        f.render_widget(paragraph, area);
    }
}

fn summarize_tool_call(name: &str, input: &serde_json::Value) -> String {
    // Match Claude's "Read 1 file" style where it makes sense; otherwise show
    // a single key argument.
    match name {
        "Read" => {
            let p = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("({p})")
        }
        "Bash" => {
            let c = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let trimmed: String = c.lines().next().unwrap_or("").chars().take(80).collect();
            format!("({trimmed})")
        }
        "Edit" | "Write" => {
            let p = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("({p})")
        }
        _ => {
            if let Some(s) = input.as_object().and_then(|o| o.iter().next()) {
                let (k, v) = s;
                let v_str = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                format!("({k}={})", v_str.chars().take(60).collect::<String>())
            } else {
                String::new()
            }
        }
    }
}

fn render_tool_result(lines: &mut Vec<Line>, r: &ToolResultRender, dim: &Style) {
    let max_lines = 20;
    let body: Vec<&str> = r.content.lines().take(max_lines).collect();
    let total = r.content.lines().count();
    for line in body {
        lines.push(Line::from(vec![
            Span::styled("  ⎿  ", *dim),
            Span::styled(line.to_string(), *dim),
        ]));
    }
    if total > max_lines {
        lines.push(Line::from(vec![
            Span::styled("  ⎿  ", *dim),
            Span::styled(format!("… {} more lines", total - max_lines), *dim),
        ]));
    }
    if r.is_error {
        lines.push(Line::from(vec![
            Span::styled("  ⎿  ", *dim),
            Span::styled("(error)".to_string(), Style::default().fg(Color::Red)),
        ]));
    }
}

impl Default for ScrollArea {
    fn default() -> Self { Self::new() }
}
```

- [ ] **Step 2: Delete the old `Message` enum**

Search for usages: `grep -rn "tui::scroll_area::Message\|scroll_area::Message" cli/src/`

These are all in `app.rs` and `commands/` — they push `Message::User`, `Message::System`, `Message::Assistant` directly into the scroll area. Replace each with the equivalent BusMessage emission via the bus (handled in Task 10).

- [ ] **Step 3: Run all tests**

Run: `cargo test -p super-cli`

Expected: tests pass; `app.rs` compile errors are expected and fixed in Task 10.

- [ ] **Step 4: Commit (broken-build OK; gets fixed in Task 10)**

```bash
git add cli/src/tui/scroll_area.rs
git commit -m "feat(tui): scroll_area renders TranscriptItem from BusMessage events

Replaces the legacy Message enum with the fold-driven render path.
Adds Claude-matching prefixes: ⏺ for assistant, ❯ for user, ⎿ for tool
results, ※ for system/recap. Tool-call summaries are name-aware
(Read/Bash/Edit/Write get tailored single-line summaries)."
```


## Task 10: Wire `SessionBus` into `App` and replace direct `Message` pushes

**Files:**
- Modify: `cli/src/tui/app.rs`
- Modify: `cli/src/bootstrap.rs` (if not fully done in Task 7)

- [ ] **Step 1: Update `App` to own a bus subscriber**

In `cli/src/tui/app.rs`:

1. Replace the `use super::scroll_area::{Message, ScrollArea};` import with `use super::scroll_area::ScrollArea;`.
2. Add `use crate::conversation::session_bus::SessionBus;` and `use crate::sdk::protocol::{BusMessage, SystemSubtype, UserPayload, ContentBlockFinal};` and `use tokio::sync::broadcast;`.
3. Add fields to `App`:

```rust
    bus: Arc<SessionBus>,
    bus_rx: broadcast::Receiver<BusMessage>,
```

4. Update `App::new` signature to take `bus: Arc<SessionBus>` and initialize:

```rust
    let bus_rx = bus.subscribe();
    Self {
        // ...
        bus: bus.clone(),
        bus_rx,
        // ...
    }
```

5. In the main TUI tick loop (wherever `inflight` is polled), also drain the bus and push received events:

```rust
    loop {
        match self.bus_rx.try_recv() {
            Ok(msg) => self.scroll_area.push_event(msg),
            Err(broadcast::error::TryRecvError::Empty) => break,
            Err(broadcast::error::TryRecvError::Closed) => break,
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
        }
    }
```

- [ ] **Step 2: Replace every legacy `Message::*` push**

Find all in `app.rs`:

`grep -n "Message::User\|Message::Assistant\|Message::System\|scroll_area.push" cli/src/tui/app.rs`

For each push, replace with a `BusMessage::SystemEvent` or `BusMessage::User` emission via `self.bus.emit_*`. Examples:

```rust
// OLD: self.scroll_area.push(Message::System("Login successful.".into()));
self.bus.emit_system(SystemSubtype::Notice, "Login successful.");
```

```rust
// OLD: self.scroll_area.push(Message::User(text.clone()));
//      self.store.set_state(|s| { s.messages.push(Message::User(text.clone())); });
// NEW: the engine itself emits BusMessage::User at the start of process_prompt,
//      so remove these direct pushes — they would duplicate.
```

Also remove `s.messages.push(...)` calls; the legacy `store.messages` field is no longer rendered (the scroll_area now reads from `self.events`, not `store.messages`).

- [ ] **Step 3: Drop the legacy `messages: Vec<Message>` from `Store::State`**

Modify `cli/src/state/store.rs`. Remove the `messages: Vec<Message>` field and any related push/clear methods. Remove `crate::tui::scroll_area::Message` imports from `store.rs`.

- [ ] **Step 4: Update bootstrap to pass bus**

Modify `cli/src/bootstrap.rs` — `run_with_engine` (or whichever launches the TUI) must receive and forward `bus`. The call shape becomes:

```rust
    crate::tui::app::run_with_engine(config, store, engine, registry, system_prompt, bus).await;
```

And `run_with_engine` itself accepts `bus: Arc<SessionBus>` and passes it into `App::new`.

- [ ] **Step 5: Rebuild**

Run: `cargo build -p super-cli`

Expected: builds clean. Remaining warnings should only be unused-field for things we'll clean up in Task 13.

- [ ] **Step 6: Smoke test**

Run: `cargo run -p super-cli --bin super` in a tmux pane. Type `What is in PLAN.md` and press Enter. Expected:
- A real `⏺ Read (PLAN.md)` tool-call line appears.
- The file contents render in `⎿`-indented lines.
- An assistant summary follows.

- [ ] **Step 7: Commit**

```bash
git add cli/src/tui/app.rs cli/src/state/store.rs cli/src/bootstrap.rs
git commit -m "feat(tui): App owns SessionBus, scroll_area driven by bus events

Replaces direct scroll_area.push(Message::*) call sites with
bus.emit_*. The engine emits BusMessage::User on user input and the
full assistant/tool sequence on each turn; App drains the bus
subscriber on every tick and feeds events into ScrollArea. Drops the
legacy Store.messages field and the tui::scroll_area::Message enum."
```


## Task 11: `ToolProgress` ticker drives the activity row

**Files:**
- Modify: `cli/src/conversation/tool_loop.rs`
- Modify: `cli/src/tui/activity.rs`
- Modify: `cli/src/tui/app.rs`

- [ ] **Step 1: Emit ToolProgress while tools run**

Modify `cli/src/conversation/tool_loop.rs` — wrap each tool invocation with a 1-second ticker that emits `BusMessage::ToolProgress`. Change the function signature to accept the bus:

```rust
pub async fn run_tool_uses(
    registry: &ToolRegistry,
    tool_uses: Vec<(String, String, serde_json::Value)>,
    cwd: std::path::PathBuf,
    permission_mode: PermissionMode,
    abort_signal: Option<watch::Receiver<bool>>,
    bus: Arc<SessionBus>, // <-- NEW
) -> Vec<ContentBlockFinal> {
```

For each invocation, spawn a ticker:

```rust
let bus_ticker = bus.clone();
let id_for_tick = id.clone();
let name_for_tick = tool.name().to_string();
let session_id = bus_ticker.session_id.clone();
let ticker = tokio::spawn(async move {
    let start = std::time::Instant::now();
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    interval.tick().await; // first tick fires immediately; skip it
    loop {
        interval.tick().await;
        bus_ticker.emit(BusMessage::ToolProgress {
            tool_use_id: id_for_tick.clone(),
            tool_name: name_for_tick.clone(),
            elapsed_seconds: start.elapsed().as_secs_f32(),
            parent_tool_use_id: None,
            uuid: uuid::Uuid::new_v4(),
            session_id: session_id.clone(),
        });
    }
});

let res = tool.call(input, &ctx).await;
ticker.abort();
```

(Apply the same pattern to both safe and unsafe branches.)

- [ ] **Step 2: Pass bus from engine into tool_loop**

Modify `cli/src/conversation/engine.rs` — change the `run_tool_uses(...)` call to pass `self.bus.clone()` as the last argument.

- [ ] **Step 3: Drive activity row from the bus subscriber**

Modify `cli/src/tui/app.rs` — when draining the bus subscriber and we receive a `BusMessage::ToolProgress`, update `self.activity` to show the running tool name + elapsed:

```rust
BusMessage::ToolProgress { tool_name, elapsed_seconds, .. } => {
    self.activity = ActivityState::active(&format!("{tool_name}… ({}s)", elapsed_seconds as u64));
    // Also push to scroll_area events for the fold (transcripts elapsed_ms).
    self.scroll_area.push_event(msg.clone());
}
```

And when `BusMessage::Result` arrives, set `self.activity = ActivityState::idle();`.

- [ ] **Step 4: Rebuild + smoke test**

Run: `cargo build -p super-cli && cargo run -p super-cli --bin super`

In the TUI, run `What is in PLAN.md`. Expected: while Read executes (briefly), the activity row glyph cycles and reads `Read… (0s)` (or 1s for slower tools). After completion, returns to idle `♦`.

- [ ] **Step 5: Commit**

```bash
git add cli/src/conversation/tool_loop.rs cli/src/conversation/engine.rs cli/src/tui/app.rs
git commit -m "feat(activity): ToolProgress ticker drives the activity row

Each tool invocation in run_tool_uses spawns a 1Hz ticker that emits
BusMessage::ToolProgress; App's bus subscriber translates those into
ActivityState updates so the spinner shows the running tool name and
elapsed seconds. The fold also updates the corresponding ToolCall
item's elapsed_ms for in-transcript display."
```


## Task 12: Visual polish — persistent header, spec glyphs, Claude verbs

**Files:**
- Modify: `cli/src/tui/app.rs`
- Modify: `cli/src/tui/activity.rs`

- [ ] **Step 1: Render header above scroll-area on every frame**

In `cli/src/tui/app.rs`, locate the `draw`/`render` function (search for `fn draw` or `terminal.draw`). The current layout shows the splash on startup but not the header during conversation. Adjust the layout so that whenever the conversation has begun (i.e. `self.scroll_area.events` is non-empty OR the user has typed at least once), a 3-line header region is reserved above the scroll-area and `self.header.render(f, header_area)` is called.

Concretely the constraint stack becomes:

```rust
use ratatui::layout::{Constraint, Direction, Layout};

let chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(3),  // header
        Constraint::Min(0),     // scroll_area
        Constraint::Length(1),  // activity row
        Constraint::Length(3),  // input bar + hint line
    ])
    .split(f.size());

self.header.render(f, chunks[0]);
self.scroll_area.render(f, chunks[1]);
self.activity.render(f, chunks[2]);
self.input.render(f, chunks[3]);
```

If the splash is currently rendered as a separate state, keep that for the truly empty initial state (no events yet); otherwise default to the header.

- [ ] **Step 2: Make the activity row use the CLAUDE.md glyph set**

Modify `cli/src/tui/activity.rs`. Find the glyph rotation constants. Replace whatever is there with:

```rust
pub const ACTIVE_GLYPHS: &[&str] = &["⟣", "⟡", "⟐", "◈", "⟢"];
pub const IDLE_GLYPH: &str = "♦";
```

And make the `render` cycle through `ACTIVE_GLYPHS` based on a tick counter (mod 5) when active, or display `IDLE_GLYPH` when idle.

- [ ] **Step 3: Use Claude's verb set for thinking**

Modify `cli/src/tui/activity.rs`. Add a constant:

```rust
pub const THINKING_VERBS: &[&str] = &[
    "Brewing", "Sautéing", "Crunching", "Mulling", "Pondering",
    "Steeping", "Distilling", "Composing", "Drafting", "Plotting",
    "Spinning", "Weaving", "Polishing", "Reviewing", "Proofing",
];
```

Pick the verb by hashing the turn index (`turn_idx % THINKING_VERBS.len()`) so the same turn always shows the same verb, but different turns differ. The state struct already has a generic "verb" — replace activations that say "Thinking" with `THINKING_VERBS[turn_idx % …]`.

- [ ] **Step 4: Smoke test**

Run: `cargo run -p super-cli --bin super`. Confirm:
- Header is visible during conversation.
- Active glyph cycles through `⟣ ⟡ ⟐ ◈ ⟢`.
- Idle shows `♦`.
- Thinking text reads e.g. `Brewing… (3s)` not `Thinking… (3s)`.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/app.rs cli/src/tui/activity.rs
git commit -m "feat(tui): persistent header, spec glyphs, Claude-style thinking verbs

Header now renders above scroll_area on every frame after the splash.
Activity glyphs use CLAUDE.md's mandated set (active ⟣ ⟡ ⟐ ◈ ⟢, idle
♦). Thinking verbs cycle Claude's witty per-turn set."
```


## Task 13: Cleanup pass

**Files:** various.

- [ ] **Step 1: Delete dead code**

Search and remove:
- `tui::scroll_area::Message` enum (already replaced).
- `Store::messages` field (if any traces remain).
- The underscore `_registry: Arc<ToolRegistry>` field in `App` — should now be a real `registry` field if the engine still needs to consult it; otherwise drop entirely.
- Any commented-out OpenAI-shaped chat-completions code in `engine.rs`.

Run: `cargo build -p super-cli` after each removal. Fix compile errors.

- [ ] **Step 2: Run full test suite + clippy**

Run: `cargo test -p super-cli`
Expected: all tests pass.

Run: `cargo clippy -p super-cli -- -D warnings` (allowing existing pre-task warnings if any — be conservative and fix the new ones introduced by this work).

- [ ] **Step 3: End-to-end parity check vs Claude Code**

Use the tmux harness in `RUNNING.md`:

```bash
SOCK=parity-cmp
tmux -L "$SOCK" kill-server 2>/dev/null
tmux -L "$SOCK" -f /dev/null new-session -d -s claude -x 120 -y 40 -e 'TERM=xterm-256color' 'claude'
tmux -L "$SOCK" -f /dev/null new-session -d -s super  -x 120 -y 40 -e 'TERM=xterm-256color' "cd $(pwd) && ./target/debug/super"
sleep 1
tmux -L "$SOCK" send-keys -t claude -l "What is in PLAN.md"
tmux -L "$SOCK" send-keys -t claude Enter
tmux -L "$SOCK" send-keys -t super  -l "What is in PLAN.md"
tmux -L "$SOCK" send-keys -t super  Enter
sleep 20
tmux -L "$SOCK" capture-pane -t claude -p > /tmp/claude.txt
tmux -L "$SOCK" capture-pane -t super  -p > /tmp/super.txt
diff /tmp/claude.txt /tmp/super.txt
```

Acceptance criteria:
- `super` ran the `Read` tool against `PLAN.md` (no XML in the output).
- A `⏺ Read (PLAN.md)` summary line appears.
- The file contents render in `⎿`-indented lines.
- An assistant summary follows.
- Visible differences between the two outputs are limited to: model identity, our spec'd glyph set divergence (`⟣ ⟡ ⟐ ◈ ⟢` vs Claude's `✻`), and version strings.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: drop legacy Message enum, dead engine path, _registry wart"
```

