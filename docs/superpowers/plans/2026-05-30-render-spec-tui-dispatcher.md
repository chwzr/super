# RenderSpec → TUI Dispatcher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `shared::RenderSpec` into the canonical TUI render description: tools emit specs through lifecycle render hooks; the executor publishes them as `BusMessage::RenderEvent`s; the transcript folder routes them to spec slots on `TranscriptItem::ToolCall`; `item_to_lines` walks the slots and dispatches each through a fleshed-out `render_spec()` kernel. The hand-rolled per-tool helpers (`render_bash_result`, `render_edit_result`, `render_write_result`) become dead and are deleted.

**Architecture:** Six phases shipped end-to-end. Schema changes first (widen `Text`, add `RenderSlot`, add slot fields on `ToolCall`, add orphan `Render` variant). Then bus wiring (executor emits RenderEvent at each lifecycle transition; folder routes by `tool_use_id`). Then per-variant dispatcher impls (11 variants). Then `item_to_lines` reads spec slots when present, falls back to legacy raw-field rendering otherwise. Then per-tool migration with byte-equal parity tests against the legacy output. Finally, delete the legacy helpers and the raw `input`/`result` fields from `ToolCall`.

**Tech Stack:** Rust 2021, `ratatui` 0.28, `tokio::sync::broadcast` (bus), `similar = "2"` (already a dep) for diff. No new crates.

**Reference spec:** `docs/superpowers/specs/2026-05-30-render-spec-tui-dispatcher-design.md`.

---

## File Structure

**Modified:**
- `shared/src/render_spec.rs` — widen `Text` variant; add `TextStyle` enum.
- `cli/src/sdk/protocol.rs` — add `RenderSlot` enum; add `slot: RenderSlot` on `BusMessage::RenderEvent`.
- `cli/src/tui/transcript.rs` — extend `TranscriptItem::ToolCall` with spec slots; add `TranscriptItem::Render { spec }`; route RenderEvents through `fold`.
- `cli/src/tui/render/mod.rs` — flesh out `render_spec` dispatcher per variant; add `text_style()` palette helper; add `render_tag()` formatter; switch `item_to_lines(ToolCall)` to prefer spec slots.
- `cli/src/conversation/tool_loop.rs` — emit RenderEvent at lifecycle transitions (Message + Tag at start; Result/Error/Rejected at completion).
- `cli/src/tui/app.rs` — drop the "ignore RenderEvent" comment; thread `RenderOpts { detailed }` into `item_to_lines` callers.
- `cli/src/tools/edit.rs` — implement `render_tool_result_message` to return `Group{[Status, Diff]}`.
- `cli/src/tools/write.rs` — implement `render_tool_result_message` to return `Group{[Status, Code]}`.
- `cli/src/tools/bash.rs` — implement `render_tool_result_message` to return `Text{Error}` / `Code` per success/error.

**Deleted (final task):**
- `render_tool_result_for`, `render_bash_result`, `render_edit_result`, `render_write_result`, `render_generic_result` in `cli/src/tui/render/mod.rs`.
- `input` and `result` raw fields on `TranscriptItem::ToolCall` (kept fields: `tool_use_id`, `name`, `elapsed_ms`, spec slots).

---

## Phase 1 — Schema

### Task 1: Widen `RenderSpec::Text` with `TextStyle`

**Files:**
- Modify: `shared/src/render_spec.rs`

The current `Text { body: String, dim: bool }` can't express colored text. Replace with a closed `TextStyle` enum so renderers can map to their palettes without raw colors leaking into the schema.

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `shared/src/render_spec.rs`:

```rust
    #[test]
    fn text_serializes_with_style_tag() {
        let spec = RenderSpec::Text {
            body: "boom".into(),
            style: TextStyle::Error,
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains(r#""kind":"text""#), "got: {json}");
        assert!(json.contains(r#""style":"error""#), "got: {json}");
        assert!(json.contains(r#""body":"boom""#), "got: {json}");
    }

    #[test]
    fn text_style_round_trips() {
        for s in [
            TextStyle::Plain,
            TextStyle::Dim,
            TextStyle::Error,
            TextStyle::Success,
            TextStyle::Warn,
            TextStyle::Strong,
        ] {
            let spec = RenderSpec::Text {
                body: "x".into(),
                style: s,
            };
            let json = serde_json::to_string(&spec).unwrap();
            let back: RenderSpec = serde_json::from_str(&json).unwrap();
            match back {
                RenderSpec::Text { style, .. } => assert_eq!(style, s),
                _ => panic!("wrong variant"),
            }
        }
    }
```

- [ ] **Step 2: Run to verify compile failure**

Run: `cargo test -p shared --lib render_spec -- text_`
Expected: compile error — `TextStyle` undefined.

- [ ] **Step 3: Add `TextStyle` and update the `Text` variant**

In `shared/src/render_spec.rs`, find the `Text { body: String, dim: bool }` arm and replace it with:

```rust
    /// Multiline plain text with a portable style hint. Renderer maps the
    /// hint to its own palette (CLI: `cli/src/tui/colors.rs`; web: design tokens).
    Text { body: String, style: TextStyle },
```

Add the enum definition just below the `StatusState` enum:

```rust
/// Closed set of style hints for `RenderSpec::Text`. Renderers map each hint
/// to a palette entry; tools cannot express arbitrary colors by design.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextStyle {
    Plain,
    Dim,
    Error,
    Success,
    Warn,
    Strong,
}
```

Re-export `TextStyle` from `shared/src/lib.rs`:

```rust
pub use render_spec::{
    DiffHunk, DiffLine, InteractiveWidget, PathEntry, Question,
    QuestionOption, RenderSpec, RuleSuggestion, StatusState, Tag, TextStyle,
};
```

- [ ] **Step 4: Update existing callers of `Text { body, dim }`**

Three call sites today pass `dim: bool`. Update each:

- `cli/src/tools/send_message.rs:127` — find `RenderSpec::Text { body: ..., dim: ... }` and change `dim: true` → `style: TextStyle::Dim`, `dim: false` → `style: TextStyle::Plain`. Add `use shared::TextStyle;` near the top of the file.
- `cli/src/tools/exit_plan_mode.rs:174` and `:193` — same replacement. Add `TextStyle` to the existing `use shared::{InteractiveWidget, RenderSpec, StatusState};` import.
- `cli/src/tools/enter_plan_mode.rs:98` and `:102` — same replacement. Add `TextStyle` to existing `use shared::RenderSpec;`.

Find every occurrence with:

```bash
grep -rn 'RenderSpec::Text *{' /Users/chwzr/flxkpe/superworkspace/super/cli/src --include="*.rs"
```

Fix every match.

- [ ] **Step 5: Run all tests**

Run: `cargo test -p shared && cargo build -p super-cli`
Expected: shared tests pass; cli builds clean.

- [ ] **Step 6: Commit**

```bash
git add shared/src/render_spec.rs shared/src/lib.rs cli/src/tools/send_message.rs cli/src/tools/exit_plan_mode.rs cli/src/tools/enter_plan_mode.rs
git commit -m "feat(shared): widen RenderSpec::Text with closed TextStyle enum"
```

---

### Task 2: Add `RenderSlot` enum and field on `BusMessage::RenderEvent`

**Files:**
- Modify: `shared/src/render_spec.rs` (add `RenderSlot`)
- Modify: `shared/src/lib.rs` (re-export)
- Modify: `cli/src/sdk/protocol.rs`

The slot disambiguates which lifecycle hook produced the spec, so the bus folder can route the event to the correct field on `TranscriptItem::ToolCall`.

- [ ] **Step 1: Add `RenderSlot` to `shared`**

Append to `shared/src/render_spec.rs`:

```rust
/// Which lifecycle hook produced a `RenderSpec`. The transcript folder uses
/// this to route a `BusMessage::RenderEvent` to the right slot on the
/// matching `TranscriptItem::ToolCall`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RenderSlot {
    Message,
    Tag,
    Progress,
    Queued,
    Result,
    Rejected,
    Error,
}
```

Add `RenderSlot` to the re-export in `shared/src/lib.rs`.

- [ ] **Step 2: Write the failing test**

Add to the `render_event_tests` mod in `cli/src/sdk/protocol.rs`:

```rust
    #[test]
    fn render_event_carries_slot() {
        let msg = BusMessage::RenderEvent {
            tool_use_id: "tu_1".into(),
            slot: shared::RenderSlot::Result,
            spec: shared::RenderSpec::Nothing,
            parent_tool_use_id: None,
            uuid: uuid::Uuid::new_v4(),
            session_id: "s".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""slot":"result""#), "got: {json}");
        let back: BusMessage = serde_json::from_str(&json).unwrap();
        match back {
            BusMessage::RenderEvent { slot, .. } => {
                assert_eq!(slot, shared::RenderSlot::Result);
            }
            _ => panic!("wrong variant"),
        }
    }
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p super-cli --lib sdk::protocol::render_event_tests -- render_event_carries_slot`
Expected: compile error — `slot` field unknown.

- [ ] **Step 4: Add the field**

In `cli/src/sdk/protocol.rs`, the `RenderEvent` variant currently looks like:

```rust
    #[serde(rename = "render_event")]
    RenderEvent {
        tool_use_id: String,
        spec: shared::RenderSpec,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
```

Replace with:

```rust
    #[serde(rename = "render_event")]
    RenderEvent {
        tool_use_id: String,
        slot: shared::RenderSlot,
        spec: shared::RenderSpec,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
```

- [ ] **Step 5: Fix existing emitters and test sites**

Two emit sites in `cli/src/conversation/tool_loop.rs` (around lines 183 and 213) currently construct `RenderEvent` without `slot`. Add `slot: shared::RenderSlot::Message` to both (they emit the interactive widget's render_tool_use_message and the rejection message — Task 5 reclassifies these properly, for now use `Message`).

The other existing test (`render_event_serializes_with_type_tag`, `render_event_round_trips`) in `cli/src/sdk/protocol.rs` needs `slot: shared::RenderSlot::Message` added to its constructor.

- [ ] **Step 6: Run tests**

Run: `cargo test -p shared && cargo test -p super-cli --lib sdk::protocol`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add shared/ cli/src/sdk/protocol.rs cli/src/conversation/tool_loop.rs
git commit -m "feat(shared): add RenderSlot enum and slot field on RenderEvent"
```

---

### Task 3: Add spec slots to `TranscriptItem::ToolCall`; add `TranscriptItem::Render` orphan variant

**Files:**
- Modify: `cli/src/tui/transcript.rs`

Add the slot fields and the orphan variant. Keep existing `input` and `result` raw fields for now — they're still consumed by the legacy renderer; we drop them in Task 21 once every tool emits specs.

- [ ] **Step 1: Add the new variant + slot fields**

In `cli/src/tui/transcript.rs`, replace the `TranscriptItem::ToolCall` variant with:

```rust
    ToolCall {
        tool_use_id: String,
        name: String,
        input: serde_json::Value,
        result: Option<ToolResultRender>,
        elapsed_ms: u64,
        // Spec slots populated by lifecycle RenderEvent routing in `fold`.
        // Each slot holds the latest spec for that hook. `Nothing` is a
        // distinct, valid value; `None` means "the tool never emitted".
        message_spec: Option<shared::RenderSpec>,
        tag_spec: Option<shared::RenderSpec>,
        progress_specs: Vec<shared::RenderSpec>,
        queued_spec: Option<shared::RenderSpec>,
        result_spec: Option<shared::RenderSpec>,
        rejected_spec: Option<shared::RenderSpec>,
        error_spec: Option<shared::RenderSpec>,
    },
```

Add a new variant at the end of the enum:

```rust
    /// Orphan render event with no matching in-flight `ToolCall`. Created
    /// from `BusMessage::RenderEvent`s whose `tool_use_id` is unknown
    /// (server-emitted system specs, hook output, etc.).
    Render { spec: shared::RenderSpec },
```

- [ ] **Step 2: Update every `TranscriptItem::ToolCall { ... }` constructor and matcher**

Adding fields breaks every constructor. The first to fix is in `fold()` in this same file (around line 148) — the `ContentBlockStream::ToolUse` arm:

```rust
                    ContentBlockStream::ToolUse { id, name, input } => {
                        let pos = out.len();
                        out.push(TranscriptItem::ToolCall {
                            tool_use_id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                            result: None,
                            elapsed_ms: 0,
                            message_spec: None,
                            tag_spec: None,
                            progress_specs: Vec::new(),
                            queued_spec: None,
                            result_spec: None,
                            rejected_spec: None,
                            error_spec: None,
                        });
                        block_to_idx.insert(*index, pos);
                        tool_use_idx.insert(id.clone(), pos);
                    }
```

Find every other `TranscriptItem::ToolCall { ... }` constructor with:

```bash
grep -rn 'TranscriptItem::ToolCall *{' /Users/chwzr/flxkpe/superworkspace/super/cli/src --include="*.rs"
```

Add the seven new fields to each (all default `None` / empty `Vec`). Test sites (in `transcript.rs`, `render/mod.rs`, and possibly `app.rs`) usually only set the original fields — add the rest at the end.

For destructuring matchers (`if let TranscriptItem::ToolCall { result, .. } = ...`), the `..` rest-pattern catches the new fields automatically; no change needed there.

For exhaustive `match` blocks, add `TranscriptItem::Render { .. } => { ... }` arms. Find them with:

```bash
grep -rn 'TranscriptItem::ToolBatch' /Users/chwzr/flxkpe/superworkspace/super/cli/src --include="*.rs"
```

(Every site that lists every variant exhaustively will appear here too.) Add a stub arm for `TranscriptItem::Render` to each — body details come in Task 11 (item_to_lines wiring); for now make it an empty `Vec::new()` for renderers and `true` for `is_stable`.

- [ ] **Step 3: Build to verify**

Run: `cargo build -p super-cli`
Expected: clean. If anything still fails, fix.

- [ ] **Step 4: Write a routing test placeholder (will pass once Task 4 lands)**

Add to the `#[cfg(test)]` block at the bottom of `transcript.rs`:

```rust
    #[test]
    fn fold_creates_orphan_render_when_tool_use_id_unknown() {
        let ev = BusMessage::RenderEvent {
            tool_use_id: "tu_does_not_exist".into(),
            slot: shared::RenderSlot::Message,
            spec: shared::RenderSpec::Text {
                body: "hello".into(),
                style: shared::TextStyle::Plain,
            },
            parent_tool_use_id: None,
            uuid: uuid::Uuid::new_v4(),
            session_id: "s".into(),
        };
        let items = fold(&[ev], None);
        assert_eq!(items.len(), 1);
        match &items[0] {
            TranscriptItem::Render { spec } => {
                assert!(matches!(spec, shared::RenderSpec::Text { .. }));
            }
            other => panic!("expected Render orphan, got {other:?}"),
        }
    }
```

This test fails today (fold drops RenderEvent on line 253). Task 4 makes it pass.

- [ ] **Step 5: Run to confirm failure**

Run: `cargo test -p super-cli --lib tui::transcript -- fold_creates_orphan_render`
Expected: FAIL — `items.len() == 0`.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tui/transcript.rs
git commit -m "feat(tui): add spec slots on TranscriptItem::ToolCall + Render orphan variant"
```

---

## Phase 2 — Bus wiring

### Task 4: Route RenderEvent into ToolCall slots / orphan Render in `fold`

**Files:**
- Modify: `cli/src/tui/transcript.rs`

Currently the `BusMessage::RenderEvent` arm in `fold` (around line 253) drops the event. Route it: if `tool_use_id` matches an existing `ToolCall` in `tool_use_idx`, write the spec into the slot named by `slot`. Otherwise push a `TranscriptItem::Render { spec }`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)]` block:

```rust
    fn tool_use_event(id: &str) -> BusMessage {
        BusMessage::StreamEvent {
            event: StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockStream::ToolUse {
                    id: id.into(),
                    name: "Bash".into(),
                    input: serde_json::json!({"command": "echo hi"}),
                },
            },
            parent_tool_use_id: None,
            uuid: uuid::Uuid::new_v4(),
            session_id: "s".into(),
        }
    }

    fn render_event(id: &str, slot: shared::RenderSlot) -> BusMessage {
        BusMessage::RenderEvent {
            tool_use_id: id.into(),
            slot,
            spec: shared::RenderSpec::Text {
                body: format!("{:?}", slot),
                style: shared::TextStyle::Plain,
            },
            parent_tool_use_id: None,
            uuid: uuid::Uuid::new_v4(),
            session_id: "s".into(),
        }
    }

    #[test]
    fn fold_routes_render_event_to_matching_tool_call_slot() {
        let events = vec![
            tool_use_event("tu1"),
            render_event("tu1", shared::RenderSlot::Message),
            render_event("tu1", shared::RenderSlot::Result),
        ];
        let items = fold(&events, None);
        assert_eq!(items.len(), 1, "single ToolCall expected: {items:#?}");
        match &items[0] {
            TranscriptItem::ToolCall { message_spec, result_spec, .. } => {
                assert!(message_spec.is_some(), "message_spec populated");
                assert!(result_spec.is_some(), "result_spec populated");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn fold_appends_progress_specs() {
        let events = vec![
            tool_use_event("tu1"),
            render_event("tu1", shared::RenderSlot::Progress),
            render_event("tu1", shared::RenderSlot::Progress),
            render_event("tu1", shared::RenderSlot::Progress),
        ];
        let items = fold(&events, None);
        match &items[0] {
            TranscriptItem::ToolCall { progress_specs, .. } => {
                assert_eq!(progress_specs.len(), 3);
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p super-cli --lib tui::transcript -- fold_routes fold_appends`
Expected: FAIL — slots stay None.

- [ ] **Step 3: Implement routing**

In `cli/src/tui/transcript.rs`, replace the existing arm:

```rust
            BusMessage::RenderEvent { .. } => {
                // RenderSpec events are consumed by the TUI render dispatcher;
                // no transcript item needed.
            }
```

with:

```rust
            BusMessage::RenderEvent {
                tool_use_id,
                slot,
                spec,
                ..
            } => {
                if let Some(&pos) = tool_use_idx.get(tool_use_id) {
                    if let TranscriptItem::ToolCall {
                        message_spec,
                        tag_spec,
                        progress_specs,
                        queued_spec,
                        result_spec,
                        rejected_spec,
                        error_spec,
                        ..
                    } = &mut out[pos]
                    {
                        use shared::RenderSlot;
                        match slot {
                            RenderSlot::Message => *message_spec = Some(spec.clone()),
                            RenderSlot::Tag => *tag_spec = Some(spec.clone()),
                            RenderSlot::Progress => progress_specs.push(spec.clone()),
                            RenderSlot::Queued => *queued_spec = Some(spec.clone()),
                            RenderSlot::Result => *result_spec = Some(spec.clone()),
                            RenderSlot::Rejected => *rejected_spec = Some(spec.clone()),
                            RenderSlot::Error => *error_spec = Some(spec.clone()),
                        }
                    }
                } else {
                    // Orphan — no matching in-flight tool call.
                    out.push(TranscriptItem::Render { spec: spec.clone() });
                }
            }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p super-cli --lib tui::transcript`
Expected: all pass including the new routing tests AND the orphan test from Task 3.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/transcript.rs
git commit -m "feat(tui): route RenderEvent into ToolCall slots or Render orphan in fold"
```

---

### Task 5: Emit RenderEvent at lifecycle transitions in the executor

**Files:**
- Modify: `cli/src/conversation/tool_loop.rs`

Today the executor only emits the `Interactive` spec (around line 183) and the `Rejected` message (line 213). Extend it: at tool start, emit `RenderSlot::Message` and `RenderSlot::Tag`; at completion, emit `RenderSlot::Result` (success) or `RenderSlot::Error` (failure that isn't a user rejection). Keep the existing emissions but tag them with the correct slot.

In v1 we don't wire `Progress` (no progress events flow from tools today) or `Queued` (no queueing in the current parallel/serial executor). Their slots exist for forward-compat.

- [ ] **Step 1: Add a helper for emitting RenderEvents**

Add near the top of `cli/src/conversation/tool_loop.rs` (below the `use` lines, above `run_tool_uses`):

```rust
/// Emit a `BusMessage::RenderEvent` if the given spec is `Some` and not
/// `RenderSpec::Nothing`. Tools whose hook returns `Nothing` mean "no opinion";
/// we don't push empty events onto the bus.
#[allow(clippy::too_many_arguments)]
fn emit_render_event(
    bus: &SessionBus,
    tool_use_id: &str,
    parent_tool_use_id: Option<&str>,
    session_id: &str,
    slot: shared::RenderSlot,
    spec: shared::RenderSpec,
) {
    if matches!(spec, shared::RenderSpec::Nothing) {
        return;
    }
    bus.emit(BusMessage::RenderEvent {
        tool_use_id: tool_use_id.into(),
        slot,
        spec,
        parent_tool_use_id: parent_tool_use_id.map(String::from),
        uuid: uuid::Uuid::new_v4(),
        session_id: session_id.into(),
    });
}
```

- [ ] **Step 2: Emit Message + Tag at tool start in the safe (parallel) loop**

In `run_tool_uses`, the safe-loop `set.spawn(async move { ... })` (around line 79) needs Message + Tag emission *before* the tool runs. Inside the spawned future, immediately after the ticker spawns and before `tokio::task::spawn(async move { tool.call(...) })`, add:

```rust
            let opts = RenderOpts { verbose: false, is_transcript_mode: false };
            emit_render_event(
                &bus_for_task,
                &id,
                parent_for_tick.as_deref(),
                &session_for_tick,
                shared::RenderSlot::Message,
                tool.render_tool_use_message(&input, &opts),
            );
            if let Some(tag) = tool.render_tool_use_tag(&input) {
                emit_render_event(
                    &bus_for_task,
                    &id,
                    parent_for_tick.as_deref(),
                    &session_for_tick,
                    shared::RenderSlot::Tag,
                    tag,
                );
            }
```

Note: the closure captures move semantics; `tool` is `Arc<dyn Tool>`. Calling `tool.render_tool_use_message(&input, ..)` before `tool.call(input, ..)` requires keeping a borrow of `input` here; the `tool.call(input, ...)` further down consumes the owned `input`. Restructure as:

```rust
            // (existing ticker spawn)

            let opts = RenderOpts { verbose: false, is_transcript_mode: false };
            let message_spec = tool.render_tool_use_message(&input, &opts);
            let tag_spec = tool.render_tool_use_tag(&input);
            emit_render_event(
                &bus_for_task,
                &id,
                parent_for_tick.as_deref(),
                &session_for_tick,
                shared::RenderSlot::Message,
                message_spec,
            );
            if let Some(tag) = tag_spec {
                emit_render_event(
                    &bus_for_task,
                    &id,
                    parent_for_tick.as_deref(),
                    &session_for_tick,
                    shared::RenderSlot::Tag,
                    tag,
                );
            }

            let tool_for_call = tool.clone();
            let inner = tokio::task::spawn(async move {
                tool_for_call.call(input, &ctx, None).await
            });
```

(Move `tool.clone()` into a binding used by the inner spawn; the outer `tool` survives so we can use it after for `Result`/`Error` rendering.)

- [ ] **Step 3: Emit Result or Error at completion in the safe loop**

After `let res = match inner.await { ... };` and before `ticker.abort();`, add:

```rust
            // Re-derive result/error spec from the tool's hook. Map our internal
            // ToolResult into the JSON shape the hook expects (`output`): use the
            // content as the sole input. Tools that need richer output shapes
            // can refine this later.
            let output_json = serde_json::json!({
                "content": res.content,
                "is_error": res.is_error,
            });
            let slot = if res.is_error {
                shared::RenderSlot::Error
            } else {
                shared::RenderSlot::Result
            };
            let result_spec = if res.is_error {
                tool.render_tool_use_error_message(&output_json, &opts)
                    .unwrap_or(shared::RenderSpec::Nothing)
            } else {
                tool.render_tool_result_message(&output_json, &[], &opts)
                    .unwrap_or(shared::RenderSpec::Nothing)
            };
            emit_render_event(
                &bus_for_task,
                &id,
                parent_for_tick.as_deref(),
                &session_for_tick,
                slot,
                result_spec,
            );
```

- [ ] **Step 4: Apply the same start + completion emissions in the unsafe (serial) loop**

The unsafe loop runs serially around line 155. Apply the same pattern there too — emit Message + Tag *before* the tool runs, and Result/Error *after* it completes. The interactive path (which already emits its own `Interactive` spec around line 183) keeps doing so; the new emissions wrap it.

For the existing Interactive emit at lines 183 and 213 in the rejected branch, tag them explicitly:
- Line 183: replace the hand-built `BusMessage::RenderEvent { .. }` with a call to `emit_render_event(..., shared::RenderSlot::Message, spec.clone())` (the interactive spec IS the message for that tool).
- Line 213 (rejection): replace with `emit_render_event(..., shared::RenderSlot::Rejected, rejection)`.

- [ ] **Step 5: Build to verify**

Run: `cargo build -p super-cli`
Expected: clean. If a borrow-checker error fires on `tool` (it's now used both before AND after the inner spawn), introduce `let tool_for_render = tool.clone();` near the top of the closure and use that for the render-hook calls.

- [ ] **Step 6: Write a smoke test**

Add to `cli/src/conversation/tool_loop.rs`:

```rust
#[cfg(test)]
mod render_emission_tests {
    use super::*;
    use crate::conversation::session_bus::SessionBus;

    fn drain(bus: &SessionBus) -> Vec<BusMessage> {
        let mut rx = bus.subscribe();
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            out.push(msg);
        }
        out
    }

    #[tokio::test]
    async fn safe_loop_emits_message_and_result_for_simple_tool() {
        // BashTool with `echo hi` is the simplest concurrency-safe tool that
        // doesn't require permissions in BypassPermissions mode.
        use crate::conversation::message_queue::MessageQueue;
        let bus = Arc::new(SessionBus::new());
        let registry = crate::tools::ToolRegistry::default_for_mode(
            PermissionMode::BypassPermissions,
        );
        let queue = Arc::new(MessageQueue::new());

        let uses = vec![(
            "tu_test".into(),
            "Bash".into(),
            serde_json::json!({"command": "echo hi"}),
        )];

        let _ = run_tool_uses(
            &registry,
            uses,
            std::env::temp_dir(),
            PermissionMode::BypassPermissions,
            None,
            bus.clone(),
            None,
            "s".into(),
            true,
            queue,
        ).await;

        let messages = drain(&bus);
        let slots: Vec<shared::RenderSlot> = messages
            .iter()
            .filter_map(|m| match m {
                BusMessage::RenderEvent { slot, .. } => Some(*slot),
                _ => None,
            })
            .collect();
        // Expect at least one Message slot and one Result slot.
        assert!(slots.contains(&shared::RenderSlot::Message), "slots: {slots:?}");
        assert!(slots.contains(&shared::RenderSlot::Result), "slots: {slots:?}");
    }
}
```

Run: `cargo test -p super-cli --lib conversation::tool_loop::render_emission_tests`
Expected: pass. If `BashTool::render_tool_use_message` returns `Nothing` (the default), the Message event is skipped per `emit_render_event` early-return — the test will fail. In that case override `BashTool::render_tool_use_message` in this same task to return:

```rust
fn render_tool_use_message(&self, input: &serde_json::Value, _opts: &RenderOpts) -> RenderSpec {
    let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let first = cmd.lines().next().unwrap_or("").chars().take(80).collect::<String>();
    RenderSpec::Header { verb: "Bash".into(), target: Some(first), tag: None }
}
```

The full per-tool migration happens in Phase 5; this minimal override is needed here just so a smoke RenderEvent fires.

- [ ] **Step 7: Commit**

```bash
git add cli/src/conversation/tool_loop.rs cli/src/tools/bash.rs
git commit -m "feat(executor): emit RenderEvent at tool lifecycle transitions"
```

---

## Phase 3 — Dispatcher implementations

### Task 6: Add `text_style()` palette helper and implement `Text` variant

**Files:**
- Modify: `cli/src/tui/render/mod.rs`
- Modify: `cli/src/tui/colors.rs` (add success/warn/error constants if missing)

- [ ] **Step 1: Ensure palette constants exist**

Open `cli/src/tui/colors.rs` and confirm these exist (most do — they were added by the tool-call-render-parity plan):
- `CC_GREEN` (success) — already there.
- `CC_ORANGE` (warn / error prefix) — already there.

Add (if missing — check first with `grep -n 'CC_' cli/src/tui/colors.rs`):

```rust
pub const CC_ERROR_FG: Color = Color::Red;
pub const CC_WARN_FG: Color  = Color::Indexed(214);
pub const CC_STRONG_FG: Color = Color::White;
```

- [ ] **Step 2: Write failing tests for `Text` rendering**

Add to the `render_spec_tests` mod at the bottom of `cli/src/tui/render/mod.rs`:

```rust
    fn line_text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn text_plain_renders_each_body_line() {
        let spec = RenderSpec::Text {
            body: "alpha\nbeta".into(),
            style: shared::TextStyle::Plain,
        };
        let lines = render_spec(&spec);
        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[0]), "alpha");
        assert_eq!(line_text(&lines[1]), "beta");
    }

    #[test]
    fn text_dim_carries_dark_gray_color() {
        let spec = RenderSpec::Text {
            body: "hush".into(),
            style: shared::TextStyle::Dim,
        };
        let lines = render_spec(&spec);
        let color = lines[0].spans.first().and_then(|s| s.style.fg);
        assert_eq!(color, Some(Color::DarkGray));
    }

    #[test]
    fn text_error_carries_red_color() {
        use crate::tui::colors::CC_ERROR_FG;
        let spec = RenderSpec::Text {
            body: "boom".into(),
            style: shared::TextStyle::Error,
        };
        let lines = render_spec(&spec);
        let color = lines[0].spans.first().and_then(|s| s.style.fg);
        assert_eq!(color, Some(CC_ERROR_FG));
    }
```

- [ ] **Step 3: Run to confirm failure**

Run: `cargo test -p super-cli --lib tui::render::render_spec_tests -- text_`
Expected: FAIL — stub still returns the `[render_spec stub: ...]` placeholder.

- [ ] **Step 4: Implement `Text` and the palette helper**

In `cli/src/tui/render/mod.rs`, add near `dim_style()`:

```rust
/// Map a portable `TextStyle` hint to a concrete ratatui `Style` using the
/// CLI palette in `colors.rs`.
fn text_style(style: shared::TextStyle) -> Style {
    use crate::tui::colors::{CC_ERROR_FG, CC_GREEN, CC_ORANGE, CC_STRONG_FG, CC_WARN_FG};
    use shared::TextStyle;
    match style {
        TextStyle::Plain   => Style::default().fg(Color::White),
        TextStyle::Dim     => Style::default().fg(Color::DarkGray),
        TextStyle::Error   => Style::default().fg(CC_ERROR_FG),
        TextStyle::Success => Style::default().fg(CC_GREEN),
        TextStyle::Warn    => Style::default().fg(CC_WARN_FG),
        TextStyle::Strong  => Style::default().fg(CC_STRONG_FG).add_modifier(Modifier::BOLD),
    }
}
```

Replace the existing `render_spec` stub with a real match. For Task 6 we only handle `Nothing` and `Text`; later tasks fill in the rest:

```rust
pub fn render_spec(spec: &shared::RenderSpec) -> Vec<Line<'static>> {
    match spec {
        shared::RenderSpec::Nothing => Vec::new(),
        shared::RenderSpec::Text { body, style } => {
            let s = text_style(*style);
            body.lines()
                .map(|l| Line::from(Span::styled(l.to_string(), s)))
                .collect()
        }
        _ => vec![Line::from(Span::styled(
            format!("[render_spec stub: {:?}]", std::mem::discriminant(spec)),
            dim_style(),
        ))],
    }
}
```

The existing `nothing_renders_empty_vec` and `unhandled_variant_renders_placeholder` tests still pass (header is unhandled).

- [ ] **Step 5: Run tests**

Run: `cargo test -p super-cli --lib tui::render`
Expected: all pass, including the three new `text_` tests.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tui/render/mod.rs cli/src/tui/colors.rs
git commit -m "feat(tui): implement render_spec Text variant + text_style palette"
```

---

### Task 7: Implement `Header` variant + `Tag` formatter

**Files:**
- Modify: `cli/src/tui/render/mod.rs`

`Header { verb, target, tag }` renders `⏺ verb target [tag]` with bold name and OSC8-linked target if `target` looks like a path.

- [ ] **Step 1: Write the failing tests**

Add to `render_spec_tests`:

```rust
    #[test]
    fn header_renders_verb_and_target() {
        let spec = RenderSpec::Header {
            verb: "Reading".into(),
            target: Some("src/foo.rs".into()),
            tag: None,
        };
        let lines = render_spec(&spec);
        // 1 leading blank + 1 body line.
        assert_eq!(lines.len(), 2);
        let body = line_text(&lines[1]);
        assert!(body.contains("Reading"), "got: {body:?}");
        assert!(body.contains("src/foo.rs"), "got: {body:?}");
    }

    #[test]
    fn header_wraps_path_target_with_osc8() {
        let spec = RenderSpec::Header {
            verb: "Reading".into(),
            target: Some("/abs/path.txt".into()),
            tag: None,
        };
        let lines = render_spec(&spec);
        let body = line_text(&lines[1]);
        assert!(body.contains("\x1b]8;;file:///abs/path.txt"), "OSC8 open: {body:?}");
    }

    #[test]
    fn header_renders_tag_inline_in_dim_brackets() {
        let spec = RenderSpec::Header {
            verb: "Bash".into(),
            target: Some("ls".into()),
            tag: Some(shared::Tag::Timeout { ms: 30000 }),
        };
        let lines = render_spec(&spec);
        let body = line_text(&lines[1]);
        assert!(body.contains("[timeout 30000ms]"), "got: {body:?}");
    }

    #[test]
    fn header_renders_truncated_tag() {
        let spec = RenderSpec::Header {
            verb: "Read".into(),
            target: None,
            tag: Some(shared::Tag::Truncated),
        };
        let lines = render_spec(&spec);
        let body = line_text(&lines[1]);
        assert!(body.contains("[truncated]"), "got: {body:?}");
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p super-cli --lib tui::render -- header_`
Expected: FAIL — header still hits the stub.

- [ ] **Step 3: Implement**

Add to `cli/src/tui/render/mod.rs` (near other helpers):

```rust
fn looks_like_path(s: &str) -> bool {
    s.contains('/') || s.contains('\\') || s.rsplit('.').next().is_some_and(|ext| !ext.is_empty() && ext.len() <= 5 && s.len() > ext.len() + 1)
}

fn format_tag(tag: &shared::Tag) -> String {
    use shared::Tag;
    match tag {
        Tag::Timeout { ms } => format!("[timeout {ms}ms]"),
        Tag::Model { id } => format!("[{id}]"),
        Tag::Truncated => "[truncated]".to_string(),
        Tag::ResumeId { value } => format!("[resume:{value}]"),
        Tag::Custom { value } => format!("[{value}]"),
    }
}
```

In the `render_spec` match, add the `Header` arm above the `_` catch-all:

```rust
        shared::RenderSpec::Header { verb, target, tag } => {
            use crate::tui::colors::CC_GREEN;
            let mut spans: Vec<Span<'static>> = Vec::new();
            spans.push(Span::styled("⏺ ", Style::default().fg(CC_GREEN)));
            spans.push(Span::styled(verb.clone(), Style::default().add_modifier(Modifier::BOLD)));
            if let Some(t) = target {
                spans.push(Span::raw(" "));
                let rendered = if looks_like_path(t) {
                    osc8_link(t, t)
                } else {
                    t.clone()
                };
                spans.push(Span::styled(rendered, dim_style()));
            }
            if let Some(tg) = tag {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(format_tag(tg), dim_style()));
            }
            vec![Line::from(""), Line::from(spans)]
        }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p super-cli --lib tui::render -- header_`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/render/mod.rs
git commit -m "feat(tui): implement render_spec Header variant + Tag formatter"
```

---

### Task 8: Implement `Code`, `Diff`, `PathList`, `KeyValues` variants

These four share the "tabular text body" shape. Bundle into one task — each variant is small and reuses existing helpers.

**Files:**
- Modify: `cli/src/tui/render/mod.rs`

- [ ] **Step 1: Write the failing tests**

Add to `render_spec_tests`:

```rust
    #[test]
    fn code_renders_body_with_corner_prefix() {
        let spec = RenderSpec::Code {
            language: None,
            body: "line1\nline2".into(),
            truncated: false,
        };
        let lines = render_spec(&spec);
        let body: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(body.contains("  ⎿  line1"), "got: {body:?}");
        assert!(body.contains("     line2"), "got: {body:?}");
    }

    #[test]
    fn code_truncated_appends_expand_hint() {
        let spec = RenderSpec::Code {
            language: None,
            body: "a\nb\nc".into(),
            truncated: true,
        };
        let lines = render_spec(&spec);
        let body: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(body.contains("(ctrl+o to expand)"), "got: {body:?}");
    }

    #[test]
    fn diff_renders_summary_and_hunks() {
        let spec = RenderSpec::Diff {
            file_path: "/tmp/a".into(),
            hunks: vec![shared::DiffHunk {
                old_start: 1,
                new_start: 1,
                lines: vec![
                    shared::DiffLine::Remove { line: "old\n".into() },
                    shared::DiffLine::Add { line: "new\n".into() },
                ],
            }],
        };
        let lines = render_spec(&spec);
        let body: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(body.contains("Added 1 line, removed 1 line"), "summary: {body:?}");
        assert!(body.contains("-old"), "removed hunk: {body:?}");
        assert!(body.contains("+new"), "added hunk: {body:?}");
    }

    #[test]
    fn path_list_renders_entries_with_optional_line_and_preview() {
        let spec = RenderSpec::PathList {
            entries: vec![
                shared::PathEntry {
                    path: "/a/b.txt".into(),
                    line: Some(42),
                    preview: Some("fn foo".into()),
                },
                shared::PathEntry {
                    path: "/c/d.txt".into(),
                    line: None,
                    preview: None,
                },
            ],
            total: 2,
            truncated: false,
        };
        let lines = render_spec(&spec);
        let body: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(body.contains("/a/b.txt:42"), "with line: {body:?}");
        assert!(body.contains("fn foo"), "preview: {body:?}");
        assert!(body.contains("/c/d.txt"), "no-line entry: {body:?}");
    }

    #[test]
    fn key_values_renders_each_row_as_dim_key_plain_value() {
        let spec = RenderSpec::KeyValues {
            rows: vec![
                ("model".into(), "claude-opus-4-7".into()),
                ("tokens".into(), "1234".into()),
            ],
        };
        let lines = render_spec(&spec);
        let body: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(body.contains("model: claude-opus-4-7"), "got: {body:?}");
        assert!(body.contains("tokens: 1234"), "got: {body:?}");
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p super-cli --lib tui::render -- code_ diff_renders path_list key_values`
Expected: all FAIL — these still hit the stub.

- [ ] **Step 3: Implement**

In `cli/src/tui/render/mod.rs`, add these arms to the `render_spec` match (above `_`):

```rust
        shared::RenderSpec::Code { language: _, body, truncated } => {
            let dim = dim_style();
            let mut out: Vec<Line<'static>> = Vec::new();
            let body_lines: Vec<&str> = body.lines().collect();
            for (i, l) in body_lines.iter().enumerate() {
                let prefix = if i == 0 { "  ⎿  " } else { "     " };
                out.push(Line::from(vec![
                    Span::styled(prefix, dim),
                    Span::styled(l.to_string(), dim),
                ]));
            }
            if *truncated {
                out.push(Line::from(vec![
                    Span::styled("     ", dim),
                    Span::styled(
                        "… (ctrl+o to expand)".to_string(),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ]));
            }
            out
        }
        shared::RenderSpec::Diff { file_path: _, hunks } => {
            // Recompute the old/new text from the hunks so we can reuse
            // diff::render_hunks / diff::count_changes — the existing
            // renderers already produce the visual treatment the spec mandates.
            let (old, new) = reconstruct_old_new(hunks);
            let counts = diff::count_changes(&old, &new);
            let dim = dim_style();
            let mut out: Vec<Line<'static>> = Vec::new();
            let mut summary: Vec<Span<'static>> = vec![Span::styled("  ⎿  ", dim)];
            summary.extend(diff::summary_spans(counts));
            out.push(Line::from(summary));
            out.extend(diff::render_hunks(&old, &new));
            out
        }
        shared::RenderSpec::PathList { entries, total: _, truncated } => {
            let dim = dim_style();
            let mut out: Vec<Line<'static>> = Vec::new();
            const MAX: usize = 20;
            for (i, entry) in entries.iter().take(MAX).enumerate() {
                let prefix = if i == 0 { "  ⎿  " } else { "     " };
                let path_str = entry.path.display().to_string();
                let head = match entry.line {
                    Some(n) => format!("{path_str}:{n}"),
                    None => path_str,
                };
                let mut spans: Vec<Span<'static>> = vec![
                    Span::styled(prefix, dim),
                    Span::styled(head, dim),
                ];
                if let Some(p) = &entry.preview {
                    spans.push(Span::styled(format!("  {p}"), dim));
                }
                out.push(Line::from(spans));
            }
            if entries.len() > MAX || *truncated {
                let remaining = entries.len().saturating_sub(MAX);
                out.push(Line::from(vec![
                    Span::styled("     ", dim),
                    Span::styled(
                        format!("… +{remaining} paths (ctrl+o to expand)"),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ]));
            }
            out
        }
        shared::RenderSpec::KeyValues { rows } => {
            let dim = dim_style();
            rows.iter()
                .map(|(k, v)| {
                    Line::from(vec![
                        Span::styled(format!("{k}: "), dim),
                        Span::styled(v.clone(), Style::default().fg(Color::White)),
                    ])
                })
                .collect()
        }
```

Add this helper above the `render_spec` function:

```rust
/// Reconstruct synthetic `old` / `new` text from a `RenderSpec::Diff`'s hunks
/// so we can reuse `diff::render_hunks` and `diff::count_changes`. This is
/// lossy across hunk boundaries (no @@ markers carry through) but the
/// rendered output is identical for the per-hunk display we want.
fn reconstruct_old_new(hunks: &[shared::DiffHunk]) -> (String, String) {
    let mut old = String::new();
    let mut new = String::new();
    for hunk in hunks {
        for l in &hunk.lines {
            match l {
                shared::DiffLine::Context { line } => {
                    old.push_str(line);
                    if !line.ends_with('\n') { old.push('\n'); }
                    new.push_str(line);
                    if !line.ends_with('\n') { new.push('\n'); }
                }
                shared::DiffLine::Remove { line } => {
                    old.push_str(line);
                    if !line.ends_with('\n') { old.push('\n'); }
                }
                shared::DiffLine::Add { line } => {
                    new.push_str(line);
                    if !line.ends_with('\n') { new.push('\n'); }
                }
            }
        }
    }
    (old, new)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p super-cli --lib tui::render`
Expected: all new tests pass.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/render/mod.rs
git commit -m "feat(tui): implement render_spec Code, Diff, PathList, KeyValues variants"
```

---

### Task 9: Implement `Status`, `Group`, `Row`, `Collapsible`, `Interactive` variants

The remaining five variants. Group and Row are recursive; Collapsible respects the global `detailed` opt; Status is a one-line glyph; Interactive renders a placeholder (modal is the real UX).

**Files:**
- Modify: `cli/src/tui/render/mod.rs`

- [ ] **Step 1: Add `detailed` parameter to `render_spec`**

`Collapsible` needs to know whether the global "expand everything" toggle is on. Change the signature:

```rust
pub fn render_spec(spec: &shared::RenderSpec, detailed: bool) -> Vec<Line<'static>> {
```

Update every existing call site and every existing test in `render_spec_tests` to pass `false`. Find them:

```bash
grep -rn 'render_spec(' /Users/chwzr/flxkpe/superworkspace/super/cli/src --include="*.rs"
```

The function is used in the test module and (after Task 11) in `item_to_lines`. Update each call.

- [ ] **Step 2: Write the failing tests**

Add to `render_spec_tests`:

```rust
    #[test]
    fn status_success_glyph_is_green_check() {
        use crate::tui::colors::CC_GREEN;
        let spec = RenderSpec::Status {
            state: shared::StatusState::Success,
            message: Some("done".into()),
        };
        let lines = render_spec(&spec, false);
        assert_eq!(lines.len(), 1);
        let glyph = lines[0].spans.first().unwrap();
        assert!(glyph.content.contains('✓'), "glyph: {:?}", glyph.content);
        assert_eq!(glyph.style.fg, Some(CC_GREEN));
    }

    #[test]
    fn status_error_glyph_is_red_cross() {
        use crate::tui::colors::CC_ERROR_FG;
        let spec = RenderSpec::Status {
            state: shared::StatusState::Error,
            message: Some("nope".into()),
        };
        let lines = render_spec(&spec, false);
        let glyph = lines[0].spans.first().unwrap();
        assert!(glyph.content.contains('✗'));
        assert_eq!(glyph.style.fg, Some(CC_ERROR_FG));
    }

    #[test]
    fn group_renders_each_child_in_order() {
        let spec = RenderSpec::Group {
            children: vec![
                RenderSpec::Text { body: "a".into(), style: shared::TextStyle::Plain },
                RenderSpec::Text { body: "b".into(), style: shared::TextStyle::Plain },
            ],
        };
        let lines = render_spec(&spec, false);
        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).contains('a'));
        assert!(line_text(&lines[1]).contains('b'));
    }

    #[test]
    fn row_joins_single_line_children_horizontally() {
        let spec = RenderSpec::Row {
            children: vec![
                RenderSpec::Text { body: "L".into(), style: shared::TextStyle::Plain },
                RenderSpec::Text { body: "R".into(), style: shared::TextStyle::Plain },
            ],
        };
        let lines = render_spec(&spec, false);
        assert_eq!(lines.len(), 1, "single-line row should join: {lines:?}");
        let body = line_text(&lines[0]);
        assert!(body.contains('L') && body.contains('R'), "body: {body:?}");
    }

    #[test]
    fn row_stacks_multiline_children_with_dim_rule() {
        let spec = RenderSpec::Row {
            children: vec![
                RenderSpec::Text { body: "a\nb".into(), style: shared::TextStyle::Plain },
                RenderSpec::Text { body: "x".into(), style: shared::TextStyle::Plain },
            ],
        };
        let lines = render_spec(&spec, false);
        assert!(lines.len() >= 3, "expected stacked: {lines:?}");
        // Every line gets the dim │ left rule.
        for line in &lines {
            assert!(
                line.spans.first().map(|s| s.content.starts_with('│')).unwrap_or(false),
                "line missing rule: {line:?}"
            );
        }
    }

    #[test]
    fn collapsible_shows_summary_when_collapsed() {
        let spec = RenderSpec::Collapsible {
            summary: "click to see more".into(),
            expanded_by_default: false,
            children: vec![RenderSpec::Text { body: "secret".into(), style: shared::TextStyle::Plain }],
        };
        let lines = render_spec(&spec, false);
        let body: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(body.contains("click to see more"));
        assert!(body.contains("(ctrl+o to expand)"));
        assert!(!body.contains("secret"));
    }

    #[test]
    fn collapsible_expands_when_default_true() {
        let spec = RenderSpec::Collapsible {
            summary: "summary".into(),
            expanded_by_default: true,
            children: vec![RenderSpec::Text { body: "inner".into(), style: shared::TextStyle::Plain }],
        };
        let body: String = render_spec(&spec, false).iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(body.contains("inner"));
    }

    #[test]
    fn collapsible_expands_when_detailed_opt_on() {
        let spec = RenderSpec::Collapsible {
            summary: "summary".into(),
            expanded_by_default: false,
            children: vec![RenderSpec::Text { body: "inner".into(), style: shared::TextStyle::Plain }],
        };
        let body: String = render_spec(&spec, true).iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(body.contains("inner"));
    }

    #[test]
    fn interactive_renders_awaiting_input_placeholder() {
        let spec = RenderSpec::Interactive {
            widget: shared::InteractiveWidget::MultiQuestion { questions: vec![] },
            response_schema: serde_json::json!({}),
        };
        let body = line_text(&render_spec(&spec, false)[0]);
        assert!(body.contains("awaiting input"), "got: {body:?}");
    }
```

- [ ] **Step 3: Run to confirm failure**

Run: `cargo test -p super-cli --lib tui::render -- status_ group_ row_ collapsible_ interactive_`
Expected: all FAIL.

- [ ] **Step 4: Implement the variants**

In `cli/src/tui/render/mod.rs`, replace the existing `_` catch-all with these arms:

```rust
        shared::RenderSpec::Status { state, message } => {
            use crate::tui::colors::{CC_ERROR_FG, CC_GREEN, CC_ORANGE, CC_WARN_FG};
            use shared::StatusState;
            let (glyph, color) = match state {
                StatusState::Queued     => ("…", Color::DarkGray),
                StatusState::InProgress => ("›", Color::DarkGray),
                StatusState::Success    => ("✓", CC_GREEN),
                StatusState::Error      => ("✗", CC_ERROR_FG),
                StatusState::Rejected   => ("⚠", CC_ORANGE),
            };
            let _ = CC_WARN_FG; // silence unused-import warning if added later
            let mut spans: Vec<Span<'static>> = vec![
                Span::styled(format!("{glyph} "), Style::default().fg(color)),
            ];
            if let Some(m) = message {
                spans.push(Span::styled(m.clone(), Style::default().fg(color)));
            }
            vec![Line::from(spans)]
        }
        shared::RenderSpec::Group { children } => {
            children.iter().flat_map(|c| render_spec(c, detailed)).collect()
        }
        shared::RenderSpec::Row { children } => {
            // Try horizontal join: only succeeds when every child renders to
            // exactly one Line. Otherwise stack with a dim │ left rule.
            let child_renders: Vec<Vec<Line<'static>>> =
                children.iter().map(|c| render_spec(c, detailed)).collect();
            let all_single_line = child_renders.iter().all(|r| r.len() == 1);
            if all_single_line && !child_renders.is_empty() {
                let mut joined_spans: Vec<Span<'static>> = Vec::new();
                for (i, child) in child_renders.iter().enumerate() {
                    if i > 0 {
                        joined_spans.push(Span::raw(" "));
                    }
                    joined_spans.extend(child[0].spans.iter().cloned());
                }
                vec![Line::from(joined_spans)]
            } else {
                let dim = dim_style();
                let mut out: Vec<Line<'static>> = Vec::new();
                for child in child_renders {
                    for line in child {
                        let mut spans: Vec<Span<'static>> = vec![Span::styled("│ ", dim)];
                        spans.extend(line.spans);
                        out.push(Line::from(spans));
                    }
                }
                out
            }
        }
        shared::RenderSpec::Collapsible { summary, expanded_by_default, children } => {
            let dim = dim_style();
            let mut out: Vec<Line<'static>> = Vec::new();
            let is_expanded = *expanded_by_default || detailed;
            if is_expanded {
                out.push(Line::from(Span::styled(summary.clone(), dim)));
                for child in children {
                    out.extend(render_spec(child, detailed));
                }
            } else {
                out.push(Line::from(vec![
                    Span::styled(summary.clone(), dim),
                    Span::styled(" (ctrl+o to expand)".to_string(), dim),
                ]));
            }
            out
        }
        shared::RenderSpec::Interactive { .. } => {
            vec![Line::from(Span::styled(
                "(awaiting input)".to_string(),
                dim_style().add_modifier(Modifier::ITALIC),
            ))]
        }
```

Remove the `_` catch-all and the stub placeholder line — every variant is now handled.

- [ ] **Step 5: Run all tests**

Run: `cargo test -p super-cli --lib tui::render`
Expected: every test passes; `unhandled_variant_renders_placeholder` from the old stub will now fail because there are no unhandled variants. Delete that test (it was a stub-era assertion).

- [ ] **Step 6: Commit**

```bash
git add cli/src/tui/render/mod.rs
git commit -m "feat(tui): implement render_spec Status, Group, Row, Collapsible, Interactive"
```

---

### Task 10: Add an integration test combining the dispatcher

Quick sanity-check that recursive variants compose correctly.

**Files:**
- Modify: `cli/src/tui/render/mod.rs` (tests only)

- [ ] **Step 1: Add the test**

Add to `render_spec_tests`:

```rust
    #[test]
    fn group_of_status_and_diff_renders_in_order() {
        // The shape EditTool will emit in Task 14.
        let spec = RenderSpec::Group {
            children: vec![
                RenderSpec::Status {
                    state: shared::StatusState::Success,
                    message: Some("Updated /tmp/a.txt".into()),
                },
                RenderSpec::Diff {
                    file_path: "/tmp/a.txt".into(),
                    hunks: vec![shared::DiffHunk {
                        old_start: 1,
                        new_start: 1,
                        lines: vec![
                            shared::DiffLine::Remove { line: "old\n".into() },
                            shared::DiffLine::Add { line: "new\n".into() },
                        ],
                    }],
                },
            ],
        };
        let body: String = render_spec(&spec, false).iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(body.contains("✓"), "status glyph: {body:?}");
        assert!(body.contains("Updated /tmp/a.txt"), "status msg: {body:?}");
        assert!(body.contains("Added 1 line, removed 1 line"), "diff summary: {body:?}");
        assert!(body.contains("-old"), "removed hunk: {body:?}");
        assert!(body.contains("+new"), "added hunk: {body:?}");
        // Status comes before the diff summary in the rendered order.
        let status_pos = body.find("Updated").unwrap();
        let diff_pos = body.find("Added").unwrap();
        assert!(status_pos < diff_pos, "order: {body:?}");
    }
```

- [ ] **Step 2: Run**

Run: `cargo test -p super-cli --lib tui::render -- group_of_status_and_diff`
Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add cli/src/tui/render/mod.rs
git commit -m "test(tui): integration test for Group{Status, Diff} composition"
```

---

## Phase 4 — Switch `item_to_lines` to slots

### Task 11: `item_to_lines(ToolCall)` prefers spec slots, falls back to legacy

**Files:**
- Modify: `cli/src/tui/render/mod.rs`
- Modify: `cli/src/tui/app.rs` (thread `detailed` flag through)

`item_to_lines` for `ToolCall` checks each spec slot in display order (Message → Tag → Result/Rejected/Error). If at least one terminal slot (Result/Rejected/Error) is `Some` and non-`Nothing`, render via specs. Otherwise fall back to the legacy raw-field rendering. This keeps the two paths coexisting cleanly during Phase 5 migrations.

`TranscriptItem::Render { spec }` always dispatches via `render_spec`.

- [ ] **Step 1: Write the failing test**

Add to the existing `tests` mod (the one above `render_spec_tests`, not in `render_spec_tests`):

```rust
    #[test]
    fn item_to_lines_tool_call_uses_result_spec_when_present() {
        let item = TranscriptItem::ToolCall {
            tool_use_id: "tu1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "echo hi"}),
            result: Some(ToolResultRender { content: "hi".into(), is_error: false }),
            elapsed_ms: 0,
            message_spec: Some(shared::RenderSpec::Header {
                verb: "Bash".into(),
                target: Some("echo hi".into()),
                tag: None,
            }),
            tag_spec: None,
            progress_specs: Vec::new(),
            queued_spec: None,
            result_spec: Some(shared::RenderSpec::Text {
                body: "from spec".into(),
                style: shared::TextStyle::Plain,
            }),
            rejected_spec: None,
            error_spec: None,
        };
        let body = rendered_text(&item_to_lines(&item, 0, false));
        assert!(body.contains("from spec"), "spec rendered: {body:?}");
        // Legacy renderer would have produced "  ⎿  hi" via render_bash_result;
        // since slots win, that should NOT appear.
        assert!(!body.contains("  ⎿  hi"), "legacy must not run: {body:?}");
    }

    #[test]
    fn item_to_lines_tool_call_falls_back_to_legacy_without_slots() {
        let item = TranscriptItem::ToolCall {
            tool_use_id: "tu1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "echo hi"}),
            result: Some(ToolResultRender { content: "hi".into(), is_error: false }),
            elapsed_ms: 0,
            message_spec: None,
            tag_spec: None,
            progress_specs: Vec::new(),
            queued_spec: None,
            result_spec: None,
            rejected_spec: None,
            error_spec: None,
        };
        let body = rendered_text(&item_to_lines(&item, 0, false));
        assert!(body.contains("  ⎿  hi"), "legacy must render: {body:?}");
    }

    #[test]
    fn item_to_lines_render_orphan_dispatches_spec() {
        let item = TranscriptItem::Render {
            spec: shared::RenderSpec::Text {
                body: "orphan content".into(),
                style: shared::TextStyle::Plain,
            },
        };
        let body = rendered_text(&item_to_lines(&item, 0, false));
        assert!(body.contains("orphan content"), "orphan rendered: {body:?}");
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p super-cli --lib tui::render -- item_to_lines_tool_call_uses_result_spec item_to_lines_render_orphan`
Expected: FAIL — slots are ignored and `Render` variant has the stub arm from Task 3.

- [ ] **Step 3: Implement the slot-aware ToolCall renderer**

In `cli/src/tui/render/mod.rs`, find the `TranscriptItem::ToolCall` arm in `item_to_lines` and replace its body with:

```rust
        TranscriptItem::ToolCall {
            name,
            input,
            result,
            message_spec,
            tag_spec,
            result_spec,
            rejected_spec,
            error_spec,
            ..
        } => {
            // Spec-driven path: any terminal slot present (and not Nothing).
            let has_terminal = [result_spec, rejected_spec, error_spec]
                .iter()
                .any(|s| matches!(s, Some(spec) if !matches!(spec, shared::RenderSpec::Nothing)));
            if has_terminal {
                lines.push(Line::from(""));
                if let Some(spec) = message_spec {
                    lines.extend(render_spec(spec, detailed));
                }
                if let Some(spec) = tag_spec {
                    lines.extend(render_spec(spec, detailed));
                }
                if let Some(spec) = result_spec {
                    lines.extend(render_spec(spec, detailed));
                } else if let Some(spec) = rejected_spec {
                    lines.extend(render_spec(spec, detailed));
                } else if let Some(spec) = error_spec {
                    lines.extend(render_spec(spec, detailed));
                }
            } else {
                // Legacy raw-field rendering. (Existing body, moved verbatim.)
                use crate::tui::colors::{CC_GREEN, CC_ORANGE};
                let is_error = result.as_ref().map(|r| r.is_error).unwrap_or(false);
                let prefix_color = if is_error { CC_ORANGE } else { CC_GREEN };
                let display = tool_display_name(name, input);
                let summary = summarize_tool_call(name, input);

                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("⏺ ", Style::default().fg(prefix_color)),
                    Span::styled(display, Style::default().add_modifier(Modifier::BOLD)),
                    Span::styled(summary, dim),
                ]));
                if let Some(r) = result {
                    render_tool_result_for(name, input, &mut lines, r, &dim);
                }
            }
        }
```

Add the `TranscriptItem::Render` arm (replace the stub from Task 3):

```rust
        TranscriptItem::Render { spec } => {
            lines.extend(render_spec(spec, detailed));
        }
```

- [ ] **Step 4: Thread `detailed` into existing `render_spec` calls in `item_to_lines`**

The `item_to_lines` signature already takes `detailed: bool` (its third parameter). Pass it as the second arg to every `render_spec(spec, detailed)` call you added in Step 3.

- [ ] **Step 5: Run tests**

Run: `cargo test -p super-cli --lib tui::render`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tui/render/mod.rs
git commit -m "feat(tui): item_to_lines(ToolCall) prefers spec slots, falls back to legacy"
```

---

### Task 12: Wire `app.rs` to acknowledge RenderEvents (replace the no-op comment)

**Files:**
- Modify: `cli/src/tui/app.rs`

The existing `BusMessage::RenderEvent { .. }` arm (around line 672) has a stale comment claiming tools only emit `Nothing`. That's no longer true. The arm itself can stay a no-op (the transcript folder handles the routing; the live region picks up updates on the next paint via `scroll_area.push_event`). Just refresh the comment so it doesn't lie.

- [ ] **Step 1: Update the comment**

In `cli/src/tui/app.rs`, find:

```rust
                        BusMessage::RenderEvent { .. } => {
                            // Batch 1: tools only emit RenderSpec::Nothing,
                            // which renders to nothing. Batches 2-5 wire this
                            // into the scrollback / live region.
                        }
```

Replace with:

```rust
                        BusMessage::RenderEvent { .. } => {
                            // Routed into TranscriptItem::ToolCall spec slots
                            // (or a Render orphan) by `transcript::fold`. The
                            // next paint picks up the change via push_event below.
                        }
```

- [ ] **Step 2: Build**

Run: `cargo build -p super-cli`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add cli/src/tui/app.rs
git commit -m "chore(tui): refresh RenderEvent app.rs comment to match new routing"
```

---

## Phase 5 — Per-tool migrations

Each migration: implement `render_tool_result_message` (and `render_tool_use_message` if not already overridden), add a parity test that asserts byte-equal output against the legacy renderer for fixed inputs, and remove the legacy arm from `render_tool_result_for`. Tools are migrated one at a time so any visual regression is bisectable.

### Task 13: Migrate `EditTool` to emit `Group{[Status, Diff]}`

**Files:**
- Modify: `cli/src/tools/edit.rs`
- Modify: `cli/src/tui/render/mod.rs` (remove Edit arm from `render_tool_result_for`)

- [ ] **Step 1: Inspect current Edit emit point**

Open `cli/src/tools/edit.rs:129` — `render_tool_result_message` already returns `RenderSpec::Diff`. Verify what it returns today, and what input shape it expects.

- [ ] **Step 2: Update Edit to emit `Group{[Status, Diff]}`**

In `cli/src/tools/edit.rs`, change `render_tool_result_message` to wrap the existing diff in a Group with a Success status:

```rust
fn render_tool_result_message(
    &self,
    output: &serde_json::Value,
    _progress: &[crate::tools::contract::ProgressEvent],
    _opts: &crate::tools::contract::RenderOpts,
) -> Option<shared::RenderSpec> {
    // ... existing hunk-building logic that produces `hunks` and `file_path` ...
    Some(shared::RenderSpec::Group {
        children: vec![
            shared::RenderSpec::Status {
                state: shared::StatusState::Success,
                message: Some(format!("Updated {file_path}")),
            },
            shared::RenderSpec::Diff { file_path, hunks },
        ],
    })
}
```

(If the existing code returns the Diff directly, refactor: extract `file_path` and `hunks` first, then wrap in the Group.)

- [ ] **Step 3: Write a parity test**

Add to `cli/src/tui/render/mod.rs` `tests` module:

```rust
    #[test]
    fn edit_spec_path_byte_equal_to_legacy_for_simple_swap() {
        // Build the legacy item and the spec-driven item; assert their
        // rendered Vec<Line> match (after stripping leading blanks/headers
        // that are identical between the two paths).
        let input = serde_json::json!({
            "file_path": "/tmp/a.txt",
            "old_string": "hello\n",
            "new_string": "hi\n",
        });
        let legacy_item = TranscriptItem::ToolCall {
            tool_use_id: "tu1".into(),
            name: "Edit".into(),
            input: input.clone(),
            result: Some(ToolResultRender {
                content: "Successfully replaced 1 occurrence(s) in /tmp/a.txt".into(),
                is_error: false,
            }),
            elapsed_ms: 0,
            message_spec: None, tag_spec: None, progress_specs: Vec::new(),
            queued_spec: None, result_spec: None, rejected_spec: None, error_spec: None,
        };
        let legacy_body = rendered_text(&item_to_lines(&legacy_item, 0, false));

        // Now build the same item but with the slot populated, simulating
        // what Phase 2 wiring produces after EditTool migration.
        let tool = crate::tools::edit::EditTool;
        let result_spec = tool.render_tool_result_message(
            &serde_json::json!({"content": "ok", "is_error": false}),
            &[],
            &crate::tools::contract::RenderOpts::default(),
        ).unwrap();
        let spec_item = TranscriptItem::ToolCall {
            tool_use_id: "tu1".into(),
            name: "Edit".into(),
            input: input.clone(),
            result: None,
            elapsed_ms: 0,
            message_spec: None, tag_spec: None, progress_specs: Vec::new(),
            queued_spec: None,
            result_spec: Some(result_spec),
            rejected_spec: None, error_spec: None,
        };
        let spec_body = rendered_text(&item_to_lines(&spec_item, 0, false));

        // Both paths should include the same summary line and hunks.
        assert!(legacy_body.contains("Added 1 line, removed 1 line"));
        assert!(spec_body.contains("Added 1 line, removed 1 line"));
        assert!(legacy_body.contains(" 1 -hello"));
        assert!(spec_body.contains(" 1 -hello"));
        assert!(legacy_body.contains(" 1 +hi"));
        assert!(spec_body.contains(" 1 +hi"));
    }
```

Note: `EditTool`'s current `render_tool_result_message` reads `file_path` / `old_string` / `new_string` from the call-time input, not from the `output` JSON. That's fine — the input has to be available on the tool's `Self` or passed via context. If today's `render_tool_result_message` takes only `output`, you'll need to refactor: have `call()` stash the input into the returned `ToolResult.metadata` and have `render_tool_result_message` read it from there. Look at the existing edit.rs:129-160 for the exact signature it has now.

- [ ] **Step 4: Run tests**

Run: `cargo test -p super-cli --lib`
Expected: existing tests pass, plus the new parity test.

- [ ] **Step 5: Remove the Edit arm from `render_tool_result_for`**

In `cli/src/tui/render/mod.rs`, find:

```rust
fn render_tool_result_for(
    tool_name: &str,
    input: &serde_json::Value,
    lines: &mut Vec<Line<'static>>,
    r: &ToolResultRender,
    dim: &Style,
) {
    match tool_name {
        "Bash" => render_bash_result(lines, r, dim),
        "Edit" => render_edit_result(lines, input, r, dim),
        "Write" => render_write_result(lines, input, dim),
        _ => render_generic_result(lines, r, dim),
    }
}
```

Remove the `"Edit"` arm:

```rust
        "Bash" => render_bash_result(lines, r, dim),
        "Write" => render_write_result(lines, input, dim),
        _ => render_generic_result(lines, r, dim),
```

`render_edit_result` is now unused; mark it `#[allow(dead_code)]` for now — it gets deleted in Task 16.

- [ ] **Step 6: Visual smoke test in the running app**

Per CLAUDE.md "For UI or frontend changes, start the dev server and use the feature in a browser before reporting the task as complete." Use the project's "run" skill or `cargo run -p super-cli` and trigger an Edit tool call (e.g. ask the agent to edit a file). Confirm the rendered diff matches the pre-migration output. If you can't run the UI in this session, say so explicitly rather than claiming success.

- [ ] **Step 7: Commit**

```bash
git add cli/src/tools/edit.rs cli/src/tui/render/mod.rs
git commit -m "feat(edit): migrate to RenderSpec::Group{Status, Diff} emit"
```

---

### Task 14: Migrate `WriteTool` to emit `Group{[Status, Code]}`

**Files:**
- Modify: `cli/src/tools/write.rs`
- Modify: `cli/src/tui/render/mod.rs` (remove Write arm)

- [ ] **Step 1: Inspect current Write rendering**

Look at `cli/src/tools/write.rs` to see whether `render_tool_result_message` is already overridden. If not, it defaults to `None` and the legacy `render_write_result` (in `render/mod.rs`) handles it.

- [ ] **Step 2: Implement the hook**

In `cli/src/tools/write.rs`, add (or extend):

```rust
use shared::{RenderSpec, StatusState, TextStyle};

impl Tool for WriteTool {
    // ... existing methods ...

    fn render_tool_result_message(
        &self,
        output: &serde_json::Value,
        _progress: &[crate::tools::contract::ProgressEvent],
        _opts: &crate::tools::contract::RenderOpts,
    ) -> Option<RenderSpec> {
        // Read the original input via metadata (call() stashes file_path + content).
        let path = output.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
        let content = output.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let total = content.lines().count();
        let plural = if total == 1 { "line" } else { "lines" };

        const MAX: usize = 10;
        let lineno_width = total.to_string().len().max(2);
        let numbered: String = content
            .lines()
            .take(MAX)
            .enumerate()
            .map(|(i, l)| format!(" {:>w$} {}", i + 1, l, w = lineno_width))
            .collect::<Vec<_>>()
            .join("\n");

        Some(RenderSpec::Group {
            children: vec![
                RenderSpec::Status {
                    state: StatusState::Success,
                    message: Some(format!("Wrote {total} {plural} to {path}")),
                },
                RenderSpec::Code {
                    language: None,
                    body: numbered,
                    truncated: total > MAX,
                },
            ],
        })
    }
}
```

`output` must carry `file_path` and `content` — modify `WriteTool::call` to include them in the returned `ToolResult.metadata` (a `HashMap<String, String>`) and reconstruct the JSON in a thin wrapper here, OR have `call()` itself emit the RenderEvent so the spec is built where the input is in scope. The simpler path is to have `call()` write `file_path` and `content` into `ToolResult.metadata`:

```rust
// inside WriteTool::call, after the write succeeds:
let mut meta = std::collections::HashMap::new();
meta.insert("file_path".into(), file_path.clone());
meta.insert("content".into(), content.clone());
ToolResult {
    content: format!("Successfully wrote {} bytes to {}", content.len(), file_path),
    is_error: false,
    metadata: Some(meta),
    ..Default::default()
}
```

Then in the executor (`tool_loop.rs`), when building `output_json` for `render_tool_result_message`, merge in the metadata:

```rust
let mut output_json = serde_json::json!({
    "content": res.content,
    "is_error": res.is_error,
});
if let Some(meta) = &res.metadata {
    if let serde_json::Value::Object(map) = &mut output_json {
        for (k, v) in meta {
            map.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
    }
}
```

Apply this metadata-merging change once, in Task 5's `emit_render_event` callers (it survives the lifetime of every per-tool migration that follows).

- [ ] **Step 3: Add parity test**

Add to `cli/src/tui/render/mod.rs`:

```rust
    #[test]
    fn write_spec_renders_summary_and_numbered_content() {
        // The output JSON passed to render_tool_result_message must carry the
        // file_path and the *original* content (not the "Successfully wrote N
        // bytes" message). The Write tool's call() puts these into
        // ToolResult.metadata; the executor merges them into output_json
        // (Task 5/14 wiring).
        let tool = crate::tools::write::WriteTool;
        let output = serde_json::json!({
            "content": "alpha\nbeta\ngamma\n",     // original file content
            "is_error": false,
            "file_path": "/tmp/notes.txt",
        });
        let spec = tool.render_tool_result_message(&output, &[], &Default::default()).unwrap();
        let body: String = render_spec(&spec, false)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(body.contains("Wrote 3 lines to /tmp/notes.txt"), "summary: {body:?}");
        assert!(body.contains(" 1 alpha"), "line 1: {body:?}");
        assert!(body.contains(" 2 beta"),  "line 2: {body:?}");
        assert!(body.contains(" 3 gamma"), "line 3: {body:?}");
    }
```

(This test is a stub — finalize it once the metadata-merging contract is locked in. The point is to prevent regressions during the legacy delete.)

- [ ] **Step 4: Run tests, remove `"Write"` arm from `render_tool_result_for`**

```rust
        "Bash" => render_bash_result(lines, r, dim),
        _ => render_generic_result(lines, r, dim),
```

Mark `render_write_result` with `#[allow(dead_code)]`.

- [ ] **Step 5: Run all tests**

Run: `cargo test -p super-cli --lib`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tools/write.rs cli/src/conversation/tool_loop.rs cli/src/tui/render/mod.rs
git commit -m "feat(write): migrate to RenderSpec::Group{Status, Code} emit"
```

---

### Task 15: Migrate `BashTool` to emit `Text{Error}` / `Code`

**Files:**
- Modify: `cli/src/tools/bash.rs`
- Modify: `cli/src/tui/render/mod.rs` (remove Bash arm)

`BashTool` is the trickiest: success → 3-line Code truncation, error → orange Text body with `Error: Exit code N` first line. Both flow through the same hook with different specs.

- [ ] **Step 1: Implement `render_tool_result_message`**

In `cli/src/tools/bash.rs`:

```rust
use shared::{RenderSpec, TextStyle};

impl Tool for BashTool {
    // ... existing methods ...

    fn render_tool_result_message(
        &self,
        output: &serde_json::Value,
        _progress: &[crate::tools::contract::ProgressEvent],
        _opts: &crate::tools::contract::RenderOpts,
    ) -> Option<RenderSpec> {
        let content = output.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let is_error = output.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);

        const MAX: usize = 3;
        let all: Vec<&str> = content.lines().collect();
        let shown: String = all.iter().take(MAX).copied().collect::<Vec<_>>().join("\n");
        let truncated = all.len() > MAX;

        if is_error {
            // Error body uses Text{Error}; orange coloring per the spec.
            Some(RenderSpec::Text { body: shown, style: TextStyle::Error })
        } else {
            Some(RenderSpec::Code {
                language: None,
                body: shown,
                truncated,
            })
        }
    }
}
```

- [ ] **Step 2: Update Bash error reporting flow**

`render_tool_use_error_message` should also be implemented (the executor calls it for `is_error == true`). For simplicity, route both through `render_tool_result_message` — but the executor in Task 5 picks one or the other based on `is_error`. So implement `render_tool_use_error_message` to call the same logic:

```rust
fn render_tool_use_error_message(
    &self,
    output: &serde_json::Value,
    opts: &crate::tools::contract::RenderOpts,
) -> Option<RenderSpec> {
    self.render_tool_result_message(output, &[], opts)
}
```

- [ ] **Step 3: Add parity test**

Add to `cli/src/tui/render/mod.rs`:

```rust
    #[test]
    fn bash_success_spec_renders_three_lines_with_truncation() {
        let tool = crate::tools::bash::BashTool;
        let output = serde_json::json!({
            "content": "1\n2\n3\n4\n5\n6\n7\n8",
            "is_error": false,
        });
        let spec = tool.render_tool_result_message(&output, &[], &Default::default()).unwrap();
        let body: String = render_spec(&spec, false).iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n");
        assert!(body.contains("1"));
        assert!(body.contains("2"));
        assert!(body.contains("3"));
        assert!(body.contains("(ctrl+o to expand)"));
        assert!(!body.contains("4"), "line 4 must not appear: {body:?}");
    }

    #[test]
    fn bash_error_spec_renders_error_styled_body() {
        use crate::tui::colors::CC_ERROR_FG;
        let tool = crate::tools::bash::BashTool;
        let output = serde_json::json!({
            "content": "Error: Exit code 2\nstderr line",
            "is_error": true,
        });
        let spec = tool.render_tool_result_message(&output, &[], &Default::default()).unwrap();
        let lines = render_spec(&spec, false);
        // Every body span should carry the Error style color.
        let any_error_colored = lines.iter().any(|l| {
            l.spans.iter().any(|s| s.style.fg == Some(CC_ERROR_FG))
        });
        assert!(any_error_colored, "expected error-colored span: {lines:?}");
    }
```

- [ ] **Step 4: Remove `"Bash"` arm from `render_tool_result_for`**

Now `render_tool_result_for` becomes:

```rust
fn render_tool_result_for(
    _tool_name: &str,
    _input: &serde_json::Value,
    lines: &mut Vec<Line<'static>>,
    r: &ToolResultRender,
    dim: &Style,
) {
    render_generic_result(lines, r, dim)
}
```

(All per-tool arms are gone; only the generic fallback remains, used for tools that haven't been migrated yet — Read, Grep, Glob, etc.)

Mark `render_bash_result`, `render_edit_result`, `render_write_result` with `#[allow(dead_code)]`.

- [ ] **Step 5: Run all tests**

Run: `cargo test -p super-cli --lib`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tools/bash.rs cli/src/tui/render/mod.rs
git commit -m "feat(bash): migrate to RenderSpec::Code/Text emit; collapse render_tool_result_for"
```

---

## Phase 6 — Legacy deletion

### Task 16: Delete dead per-tool helpers and the raw-field fallback path

**Files:**
- Modify: `cli/src/tui/render/mod.rs`
- Modify: `cli/src/tui/transcript.rs` (drop `input` / `result` raw fields)

The legacy `render_*_result` helpers are dead after Phase 5. `item_to_lines(ToolCall)`'s fallback branch is also no longer reachable for tools that emit specs (Edit, Write, Bash). But Read, Grep, Glob, and others still flow through the legacy generic renderer when they hit `ToolBatch` *expanded* mode. Decide: either migrate those too, or keep the generic fallback.

Recommended approach: keep `render_generic_result` for now (it handles tools that haven't been migrated). Delete the per-tool helpers (`render_bash_result`, `render_edit_result`, `render_write_result`) since their callers are gone.

- [ ] **Step 1: Delete dead per-tool helpers**

Remove from `cli/src/tui/render/mod.rs`:
- `render_bash_result`
- `render_edit_result`
- `render_write_result`

Keep `render_generic_result` and `render_tool_result_for` (now thin, only calls generic).

- [ ] **Step 2: Decide whether to drop the raw `input` / `result` fields on `TranscriptItem::ToolCall`**

The legacy fallback in `item_to_lines(ToolCall)` still uses `name`, `input`, `result` for tools that don't emit a terminal spec (Read, Grep, Glob today). If you want to keep them, leave the fields and the fallback in place. If you want to delete them, every Read/Grep/Glob tool also needs `render_tool_result_message` implemented in this task — which is a bigger surface than this single task's scope.

For v1: **keep the raw fields and the fallback.** Mark a follow-up task:

```bash
# Add to docs/superpowers/plans/ as a follow-up:
# 2026-DD-MM-render-spec-final-cleanup.md — migrate Read/Grep/Glob to RenderSpec,
# then drop input/result raw fields from TranscriptItem::ToolCall.
```

Document this decision inline in `cli/src/tui/transcript.rs` near the `ToolCall` definition:

```rust
    ToolCall {
        tool_use_id: String,
        name: String,
        input: serde_json::Value,         // kept: used by legacy fallback for Read/Grep/Glob
        result: Option<ToolResultRender>, // kept: used by legacy fallback
        // ...
    }
```

- [ ] **Step 3: Run all tests**

Run: `cargo test -p super-cli --lib`
Expected: all pass.

- [ ] **Step 4: Run the full workspace tests**

Run: `cargo test --workspace`
Expected: all pass.

- [ ] **Step 5: Smoke test the running CLI**

Per CLAUDE.md: start the CLI, trigger Edit/Write/Bash tool calls in a real session, confirm the rendered output matches the pre-migration shape. If unable to test interactively, say so explicitly.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tui/render/mod.rs cli/src/tui/transcript.rs
git commit -m "chore(tui): delete dead per-tool render helpers; document Read/Grep/Glob follow-up"
```

---

## Self-Review

After the plan was written, the following gaps were identified and addressed inline:

- **Schema sufficiency for Bash error coloring**: addressed by Task 1 widening `RenderSpec::Text` with `TextStyle::Error`.
- **Output shape needed by `render_tool_result_message`**: addressed by Task 14's executor metadata-merge pattern, which threads input data into `output_json` so result hooks can read what they need without changing the trait signature.
- **`render_spec` signature change for Collapsible's detail flag**: addressed by Task 9 changing the signature to take `detailed: bool` and updating all call sites.
- **TranscriptItem::Render coexistence with ToolBatch grouping**: the existing `group_tool_batches` post-fold transformer only groups `TranscriptItem::ToolCall { name }` items whose name classifies as `ReadSearch`; `Render` orphans are not eligible for grouping (they pass through unchanged). No code changes needed in `group_tool_batches`, but verify in Task 4's tests that `Render` orphans survive the group pass.
- **Progress and Queued slots wired but unused in v1**: explicit in Task 5 step 1 documentation. Future spec wires them when ProgressEvents start flowing.
