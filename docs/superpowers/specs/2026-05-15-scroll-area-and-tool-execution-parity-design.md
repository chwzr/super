# Scroll-area & tool-execution parity with Claude Code

**Status:** Approved, ready for implementation plan
**Date:** 2026-05-15
**Branch:** `feat/slash-command-parity`

---

## Problem

When run side-by-side with Claude Code (see `RUNNING.md` for the tmux harness), Super CLI's scroll-area diverges from Claude in two distinct ways:

1. **Behavioral.** Super is not actually wired to execute tools. The OpenRouter call goes to OpenAI's `/v1/chat/completions` endpoint with no `tools` array, and the model — coached by our system prompt and lack of structure — returns literal `<function_calls><invoke name="Read">…</invoke></function_calls>` XML as plain text. `Read PLAN.md` never actually reads the file; the user sees XML where they expected file content. The `ToolRegistry` exists and is fully implemented (35 tools, real `call()` bodies) but is held as `_registry` in `App` and never invoked.
2. **Visual.** Even if tools worked, the scroll-area renderer would not match Claude. Assistant prefix is `◆` instead of `⏺`, tool-call summary lines (`⏺ Read 1 file, listed 1 directory`) are absent, tool result blocks (`⎿ …`) are absent, recap blocks (`※ recap: …`) are absent, the persistent header disappears after the splash, and the thinking display lacks Claude's witty per-turn verbs (`Brewed for 2s`, `Sautéed for 5s`, `Crunched for 17s`).

Both gaps need closing. The user's request is "the scroll view and everything inside is not like in claude — i want it to behave and be the same."

## Goals

- Tools actually execute. `What is in PLAN.md` produces a real `Read` tool call that returns real file contents.
- The engine emits a Claude-Code-shaped event stream that the TUI and (later) the platform server consume the same way.
- The scroll-area renders the same primitives as Claude Code: assistant text with `⏺` prefix, tool-call summary lines, indented tool results, recap blocks, persistent header.
- The protocol carries the fields needed for subagents (`parent_tool_use_id`, per-agent `session_id`) so subagent execution can be added later without re-architecting.

## Non-goals (this iteration)

- Subagent execution. `AgentTool` stays a stub. The protocol carries the necessary fields.
- Platform-server forwarding. The bus is shaped to support it; no subscriber is wired.
- Hook events. Enum slots exist on `BusMessage::System`; no emission.
- MCP-tool dynamic registration. Static registry stays.
- Per-tool permission UI redesign. Existing `check_permission` + modal flow is reused.

---

## Approach: route through OpenRouter's Anthropic-compatible endpoint

OpenRouter exposes an Anthropic-shaped API at `https://openrouter.ai/api` (base URL) with models prefixed `anthropic/...`. The wire format is identical to Anthropic's `/v1/messages`: native tool use, native streaming with `message_start` / `content_block_start` / `content_block_delta` / `content_block_stop` / `message_delta` / `message_stop` SSE events, native thinking blocks.

**Consequence:** the bus speaks the same vocabulary as the wire. No translation layer. The day Super talks to Anthropic directly it's a one-line URL change.

Reference: <https://openrouter.ai/docs/guides/community/anthropic-agent-sdk.md>

---

## Architecture: event bus

A single `tokio::sync::broadcast::Sender<BusMessage>` is created per session and held by `App`. The engine is the only producer; consumers subscribe.

```
ConversationEngine (producer)
       │
       │  broadcast::Sender<BusMessage>
       ▼
   ┌───────────────┬─────────────────┬───────────────────────┐
   ▼               ▼                 ▼                       ▼
TUI scroll-     Session log       Platform-server          (future)
area renderer   (JSONL, future)   forwarder (future)       iOS / web
```

**Why broadcast, not mpsc.** Multiple independent subscribers each with their own backpressure. Lagged subscribers drop oldest events; the TUI uses a 256-slot capacity which is plenty for one human reading along.

**Owner.** `Arc<SessionBus>` lives on `App`. Threaded into `ConversationEngine::new(..., bus)`. The same handle will be exposed to the platform-server forwarder when that lands (out of scope here).

**Replaces today's model.** Today the engine pushes whole `tui::scroll_area::Message` variants into `store.messages`. After this change the engine emits `BusMessage` events, and the scroll-area folds them into a transcript of `TranscriptItem`s. `store.messages` (in its current form) goes away.

---

## Protocol: two-layer event shape

Defined in `cli/src/sdk/protocol.rs`, extending the existing `SdkMessage` types.

### Layer 1 — raw Anthropic-shaped stream events

These match Anthropic's SSE wire format verbatim. We emit them as we parse them from OpenRouter.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: MessageMeta },

    #[serde(rename = "content_block_start")]
    ContentBlockStart { index: u32, content_block: ContentBlock },

    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: BlockDelta },

    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },

    #[serde(rename = "message_delta")]
    MessageDelta { delta: MessageDeltaInfo, usage: UsageInfo },

    #[serde(rename = "message_stop")]
    MessageStop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },

    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: serde_json::Value },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
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
```

`signature_delta`, `citations`, `connector_text`, `server_tool_use` get extensibility slots but are not emitted v1.

### Layer 2 — `BusMessage` envelopes

This is what flows on the broadcast channel. Matches the shape of Claude Code's SDK messages so the same vocabulary works for TUI rendering, session logs, and remote forwarding.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BusMessage {
    #[serde(rename = "user")]
    User {
        message: UserMessage,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },

    #[serde(rename = "assistant")]
    Assistant {
        message: AssistantMessage,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },

    #[serde(rename = "stream_event")]
    StreamEvent {
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
    System {
        subtype: SystemSubtype,   // CompactBoundary | PostTurnSummary | ApiRetry | …
        message: String,
        uuid: Uuid,
        session_id: String,
    },

    #[serde(rename = "result")]
    Result {
        stop_reason: Option<String>,
        usage: UsageInfo,
        total_cost_usd: f64,
        duration_ms: u64,
        num_turns: u32,
        uuid: Uuid,
        session_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub role: Role, // "user"
    pub content: Vec<ContentBlock>, // Text or ToolResult blocks
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub id: String,
    pub model: String,
    pub role: Role,
    pub content: Vec<ContentBlock>, // Text | Thinking | ToolUse
    pub stop_reason: Option<String>,
    pub usage: UsageInfo,
}
```

**Why both layers.** `StreamEvent` is for live, token-by-token rendering (TUI cursor following deltas in real time). `BusMessage::Assistant` is the block-aggregated message emitted on each `content_block_stop` — that's what session logs and SDK consumers want. The TUI consumes both; downstream subscribers (server, replay) can stick to `Assistant` / `User` / `Result` and ignore `StreamEvent` if they don't need streaming.

**Tool results are not their own variant.** When a tool finishes locally, the engine builds a *synthetic* `BusMessage::User` whose `content` is a `tool_result` content block matching the parent's `tool_use_id`, then feeds that user message into the next API turn. This is the Anthropic tool-use round-trip.

**Activity-row driver: `ToolProgress`.** Every 1s while a tool runs, the engine emits `ToolProgress { elapsed_seconds }`. The TUI animates the activity row off these. No separate `Activity` event.

---

## Agent loop

```
process_prompt(user_input):
  1. emit BusMessage::User { content: [Text { user_input }], parent_tool_use_id: None }
  2. loop:
       a. build Anthropic request: system, message history, tools schema
       b. POST https://openrouter.ai/api/v1/messages with stream=true
       c. parse SSE; for each parsed event:
            - emit BusMessage::StreamEvent { event, … }
            - fold deltas into a `partial_message`
            - on ContentBlockStop, emit BusMessage::Assistant { message: <block-aggregated> }
       d. on MessageStop:
            - collect all tool_use blocks from the just-completed assistant turn
            - if none → emit BusMessage::Result { stop_reason="end_turn", usage, … }; return
            - else:
                * partition tool_uses into [concurrency-safe, sequential]
                * for the safe set, spawn parallel via tokio::join_all
                * for the sequential set, run one at a time
                * for each running tool, spawn a 1s ToolProgress ticker
                * collect [tool_result blocks]
            - build synthetic UserMessage { content: tool_results }
            - emit BusMessage::User { message: synthetic, parent_tool_use_id: None }
            - append to history; loop
```

### Permissions

Each tool's `check_permission(input)` returns `Allow | Ask | Deny`. On `Ask` in interactive mode the engine emits `BusMessage::System { subtype: PermissionRequest, message: … }`, suspends the loop on a `oneshot::Receiver<PermissionDecision>`, and resumes when the user answers via the existing TUI modal. `Plan` permission mode pre-filters tools via the existing `ToolRegistry::assemble_for_mode`.

### Abort

The existing `tokio::sync::watch<bool>` in `ToolCallContext::abort_signal` is set by the TUI on Esc. Long-running tools poll it. The HTTP SSE stream is cancelled by dropping the `reqwest` response (cancel-on-drop semantics).

### Compaction

Stays where it is in `cli/src/conversation/compaction.rs`. Driven off the post-fold message history (`Vec<UserMessage | AssistantMessage>`) maintained inside the engine, not off the old `tui::scroll_area::Message` enum.

### HTTP details

- **Endpoint:** `POST {base}/v1/messages` where `base` defaults to `https://openrouter.ai/api`. Configurable via `~/.super/config.json` for future direct-to-Anthropic routing.
- **Auth:** `Authorization: Bearer <openrouter_api_key>`.
- **Headers:** `anthropic-version: 2023-06-01` (per Anthropic docs), `content-type: application/json`.
- **Body shape (Anthropic):**
  ```json
  {
    "model": "anthropic/claude-sonnet-4-5",
    "max_tokens": 8192,
    "system": "<rendered system prompt>",
    "messages": [
      { "role": "user", "content": [ { "type": "text", "text": "..." } ] },
      { "role": "assistant", "content": [ { "type": "tool_use", "id": "...", "name": "Read", "input": { ... } } ] },
      { "role": "user", "content": [ { "type": "tool_result", "tool_use_id": "...", "content": "..." } ] }
    ],
    "tools": [ { "name": "Read", "description": "...", "input_schema": {...} }, ... ],
    "stream": true
  }
  ```
- **SSE parsing:** standard `event: <type>` / `data: <json>` framing. Use `eventsource-stream` or hand-roll over `reqwest::Response::bytes_stream`.

### Concurrency model for tool calls within a turn

Honor `Tool::is_concurrency_safe()`. Within a single assistant turn that emits N tool_use blocks:
- Partition into safe (read-only / pure) and unsafe (writes, shell, network with side effects).
- Run the safe partition via `tokio::join_all`.
- Run the unsafe partition serially in emission order.
- Collect results into `tool_result` blocks tagged with each `tool_use_id`.
- Build one synthetic UserMessage containing all `tool_result`s and append.

This matches Claude Code's behavior (`runToolsConcurrently` for read-only tools) and is what the trait flag was designed for.

---

## Subagent considerations (future work, contract preserved)

Subagent execution stays out of scope for this iteration, but the protocol and TUI must carry the necessary fields so subagents can be wired in later additively.

### Subagent semantics (from `claude-code-src/`)

1. **`AgentTool` is just a Tool.** From the parent's POV an agent invocation is a regular `tool_use` block (typically `name = "Task"`). The tool's `call()` opens a nested `query()` with a fresh message history, runs to completion, and returns its final text as the tool's result. Same round-trip as `Read` or `Bash`.

2. **Hierarchy field: `parent_tool_use_id`.** Every `BusMessage` carries `parent_tool_use_id: Option<String>`. The parent's messages have `None`. Every message emitted *from inside* a subagent's loop carries the `tool_use_id` of the parent's invoking `tool_use` block. Nesting is unbounded.

3. **Per-agent `session_id` (Claude Code calls it `agentId`).** A fresh UUID per spawn. The parent's broadcast channel is shared, but messages emitted from the subagent are tagged with the child's `session_id`. Consumers demux on `(session_id, parent_tool_use_id)`.

4. **Abort signals.** Sync agents share the parent's `watch<bool>`; async agents get an independent one. Cancelling the parent cancels sync children, leaves async children running until their own abort.

5. **Tool inheritance.** Two modes — **fork** (subagent inherits parent's exact tool set, system prompt, thinking config — used for continuations) and **spawn** (filtered tool set per agent definition, fresh prompt, thinking disabled for cost).

6. **Tool result round-trip.** When the subagent's loop hits `MessageStop` with no further `tool_use` blocks, its final assistant text is wrapped in a `tool_result` block matching the parent's invoking `tool_use_id` and inserted into the **parent's** message history. The parent's loop then resumes.

7. **Cleanup on completion or abort.** Per `session_id`: MCP clients, hooks, file-state cache, todos, background bash tasks, transcript subdir mapping. All keyed by the agent's session_id so cleanup is mechanical.

### What our protocol already supports

The `BusMessage` enum above already carries `session_id`, `parent_tool_use_id`, and `uuid` on every variant. No new variants are needed for subagents — they reuse `Assistant` / `User` / `StreamEvent` / `ToolProgress` / `Result` with hierarchy fields populated.

### What the TUI must already support

- The scroll-area accepts `parent_tool_use_id` on every event. Even in v1 (always `None`) the field flows through.
- The folder is parameterized: `fold(events, filter: Option<ToolUseId>) -> Transcript`. Same function produces the root view (`filter: None`) and any nested subagent view (`filter: Some(tool_use_id)`).
- The transcript schema does not change for subagent rendering — only the input filter changes.

### Engine support points (when subagents land)

1. **`AgentTool::call`** opens a nested loop. It receives `ToolCallContext` (already carries `abort_signal`) and constructs a child `ConversationEngine` that shares the same `Arc<broadcast::Sender<BusMessage>>` but uses a fresh `session_id`.
2. **`ToolCallContext` gains** `parent_tool_use_id: Option<String>` and `bus: Arc<broadcast::Sender<BusMessage>>`. Non-spawning tools don't use them.
3. **Async-agent isolation** is a `bool` on the spawn options. Async = fresh `watch<bool>`. Sync = clone of parent's.
4. **Agent definitions** loaded from `.claude/agents/*.md` (1:1 with Claude Code per CLAUDE.md). Each definition: tool whitelist, system prompt, thinking config, max turns. `AgentTool::call` consults the registry by `subagent_type`.
5. **Sidechain transcript persistence.** Per-agent JSONL at `~/.super/sessions/<root_session>/sidechains/<session_id>.jsonl`. Mirrors `recordSidechainTranscript`.

### Reference pointers in `claude-code-src/`

When implementing subagents, the authoritative source files are:

- `tools/AgentTool/AgentTool.tsx` — Tool entrypoint, schema, permissions.
- `tools/AgentTool/runAgent.ts` — spawn logic, context cloning, cleanup, message forwarding via `yield`.
- `tools/AgentTool/forkSubagent.ts` — fork path (`useExactTools`).
- `tools/AgentTool/loadAgentsDir.ts` — `.claude/agents/*.md` loader.
- `query.ts` — top-level loop; subagent loops are invocations of the same function with a child context.
- `utils/queryHelpers.ts` — where `parent_tool_use_id` is stamped on emitted messages.
- `services/api/claude.ts` lines 1980–2300 — the stream-event parser we're replicating.

---

## TUI: scroll-area rendering

### Folding events into a transcript

The TUI subscribes to `BusMessage` and maintains a `Transcript: Vec<TranscriptItem>`. The fold rules:

| Incoming `BusMessage` | Transcript effect |
|---|---|
| `User { content: [Text {...}] }` (real user input) | append `TranscriptItem::User { text }` |
| `User { content: [ToolResult {...}] }` (synthetic) | attach result to the matching `TranscriptItem::ToolCall` |
| `StreamEvent { ContentBlockStart { Text } }` | open `TranscriptItem::AssistantText { text: "", complete: false }` |
| `StreamEvent { ContentBlockDelta { TextDelta } }` | append delta to the open text item; trigger redraw |
| `StreamEvent { ContentBlockStart { ToolUse } }` | open `TranscriptItem::ToolCall { name, input_partial: "", result: None }` |
| `StreamEvent { ContentBlockDelta { InputJsonDelta } }` | accumulate into `input_partial` |
| `StreamEvent { ContentBlockStop }` | finalize open item; parse tool input JSON |
| `StreamEvent { ContentBlockStart { Thinking } } + deltas` | open `TranscriptItem::Thinking { text, collapsed: true }` |
| `Assistant { message }` | reconcile: ignore if already folded from deltas, else fall back |
| `ToolProgress { tool_use_id, elapsed }` | update running tool's elapsed time (drives activity row) |
| `Result { stop_reason, usage }` | finalize turn; clear activity row |
| `System { subtype: PostTurnSummary }` | render as `※ recap: …` |
| `System { subtype: CompactBoundary }` | render as `※ Compacted N tokens` |

The fold is pure: `fn fold(events: &[BusMessage], filter: Option<&str> /* tool_use_id */) -> Transcript`. Same function powers `/resume` and any future remote-client rendering.

### `TranscriptItem`

```rust
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
    Recap { text: String },
    CompactionBoundary { tokens_compacted: u32 },
}
```

`complete: bool` on text items lets the renderer distinguish in-progress (no trailing newline, cursor blink) vs finalized blocks.

### Visual parity targets (vs Claude Code)

| Element | Claude | Super today | Target |
|---|---|---|---|
| Assistant prefix | `⏺` | `◆` | `⏺` |
| Tool-call summary line | `⏺ Read 1 file, listed 1 directory` (cyan/dim) | not rendered | match |
| Tool result block | `⎿ <output>` (indented, dim) | not rendered | match |
| User prefix | `❯ <text>` (white bold) | `❯ <text>` | already matches |
| Recap blocks | `※ recap: ...` (dim) | not rendered | match — emit via `System { subtype: PostTurnSummary }` |
| Thinking display | `✻ Brewed for 2s` / `Sautéed for 5s` / `Crunched for 17s` | `◈ Thought for 6s` | match Claude's verb set; use Super's spec'd glyphs (CLAUDE.md mandate) |
| Active activity row | spinner + verb cycling | `⟡ Thinking… (6s)` | rotate through `⟣ ⟡ ⟐ ◈ ⟢` per CLAUDE.md spec |
| Idle activity glyph | n/a | n/a | `♦` per CLAUDE.md |
| Persistent header | shown above transcript on every turn after first prompt | shown on splash only, vanishes during conversation | restore — render header above scroll-area always |
| Bottom hint line | `← for agents · esc to interrupt` style, context-aware | static `? for shortcuts` | make context-aware |
| Compaction boundary | `※ Compacted N tokens` line | not rendered | match |

### Deliberate divergence from Claude

CLAUDE.md pins our activity glyphs to `⟣ ⟡ ⟐ ◈ ⟢` (active) and `♦` (idle), which differ from Claude Code's `✻ / ✢`. Verb set follows Claude (`Brewed`, `Sautéed`, `Crunched`, etc.) per CLAUDE.md's "Activity verbs: match Claude Code's set." Documenting this so reviewers don't try to "fix" it later.

---

## Migration order

The TUI must keep booting and Super must keep working at every commit. Order:

1. **Add protocol types** in `cli/src/sdk/protocol.rs`. Pure additive; no engine or TUI changes.
2. **Add `SessionBus`** owned by `App`; thread `Arc<broadcast::Sender<BusMessage>>` into `ConversationEngine`. Engine doesn't emit yet.
3. **Rewrite engine HTTP call** to `/v1/messages` Anthropic-shaped, non-streaming first. Emit a single `BusMessage::Assistant` per turn. No tools yet.
4. **Add SSE streaming.** Parse Anthropic SSE; emit `StreamEvent` variants. End-to-end live token rendering proves the bus + fold work.
5. **Add tool loop.** Include `tools` in request; parse `tool_use` blocks; execute via `ToolRegistry`; build synthetic `tool_result` user messages; loop. `Read PLAN.md` works end-to-end here.
6. **Add `ToolProgress` ticker.** 1s emitter per running tool; activity row driven from these.
7. **Swap scroll-area** to consume the bus. Replace `tui::scroll_area::Message` with `TranscriptItem`; folder lives in `scroll_area`. Delete old store push paths.
8. **Visual polish pass.** `⏺` prefix, recap blocks, persistent header, thinking display, compaction boundary line.
9. **Cleanup.** Remove dead code from the old engine path, remove `_registry` underscore wart, retire unused `Message` variants.

Each step is independently demoable. Steps 1–4 don't visibly change Super's behavior much (still produces text). Step 5 is the unlock. Steps 6–8 are pure polish.

---

## Test plan

End-to-end parity check uses the tmux harness in `RUNNING.md`:

```bash
SOCK=parity-cmp
tmux -L "$SOCK" -f /dev/null new-session -d -s claude -x 120 -y 40 'claude'
tmux -L "$SOCK" -f /dev/null new-session -d -s super  -x 120 -y 40 './target/debug/super'
sleep 1
tmux -L "$SOCK" send-keys -t claude -l "What is in PLAN.md"
tmux -L "$SOCK" send-keys -t claude Enter
tmux -L "$SOCK" send-keys -t super  -l "What is in PLAN.md"
tmux -L "$SOCK" send-keys -t super  Enter
sleep 15
tmux -L "$SOCK" capture-pane -t claude -p > /tmp/claude.txt
tmux -L "$SOCK" capture-pane -t super  -p > /tmp/super.txt
diff /tmp/claude.txt /tmp/super.txt
```

Acceptance: `super` actually executes the `Read` tool against `PLAN.md`, renders an `⏺ Read 1 file` summary, shows the file contents in a `⎿`-indented block, and produces an assistant reply summarizing the file. Visual diff against Claude's output should be limited to: model identity (`Sonnet` vs `Opus`), our spec'd glyph divergence, and version strings.

Unit tests at module boundaries:
- `sdk::protocol` round-trip serialize/deserialize for every `BusMessage` variant.
- Engine SSE parser: feed a recorded Anthropic SSE stream, assert the emitted `StreamEvent` sequence.
- Tool loop: mock OpenRouter response with a `tool_use` block, assert the engine executes the tool and re-issues the request with a `tool_result` block.
- Fold: deterministic event sequence → expected `Transcript` for each variant.

---

## Out of scope (recap)

- Subagent execution (protocol and TUI carry the necessary fields; `AgentTool` stays a stub).
- Platform-server forwarder (bus is ready; no subscriber).
- Hook events (slots exist on `BusMessage::System`; no emission).
- MCP-tool dynamic registration.
- Per-tool permission UI redesign.

---

## Open questions

None at the time of approval. If implementation surfaces ambiguity, escalate per CLAUDE.md.
