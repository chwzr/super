# RenderSpec → TUI Dispatcher — Design

## Summary

`shared::RenderSpec` becomes the canonical render description for every surface (TUI, server, web). The TUI stops calling hand-rolled per-tool helpers (`render_bash_result`, `render_edit_result`, `render_write_result`, etc.) and instead routes every visible artifact through a single `render_spec()` dispatcher. Tools emit specs through their lifecycle render hooks; the executor publishes those specs as `BusMessage::RenderEvent`s; the transcript folder attaches them to the corresponding `TranscriptItem::ToolCall` by `tool_use_id`; `item_to_lines` walks the slots and dispatches each spec through `render_spec`.

This collapses two parallel render code paths into one and gives the server and (future) web frontend the same rendering source of truth.

## Scope

**In:** dispatcher implementations for every `RenderSpec` variant, schema widening for colored text, `RenderSlot` enum on bus events, spec-slot fields on `TranscriptItem::ToolCall`, executor wiring to emit RenderEvents at lifecycle transitions, migration of every existing tool with custom rendering, deletion of legacy per-tool helpers.

**Out (v1):** syntax highlighting for `Code` (no crate per CLAUDE.md), true 2D composition for `Row` (scrollback is vertical), web-frontend consumption (existing protocol round-trip is sufficient; web rendering is a separate spec).

## Render Model

```
                                          tool_use_id matches
                                          ┌────────────────────────┐
                                          │                        │
Tool                ─emit→  BusMessage::  │  fold (transcript.rs)  │  ─attach→  TranscriptItem::ToolCall
.render_*_message(...)     RenderEvent {  │                        │              ├─ message_spec
                              tool_use_id,│                        │              ├─ tag_spec
                              slot,       │                        │              ├─ progress_specs
                              spec        │                        │              ├─ result_spec
                            }             │                        │              ├─ rejected_spec
                                          └────────────────────────┘              └─ error_spec
                                                                                          │
                                                                                          ▼
                                                                  item_to_lines(item)  → walks slots
                                                                                          │
                                                                                          ▼
                                                                  render_spec(spec)   → Vec<Line<'static>>
```

**No registry lookup at render time.** Render hooks run once, in the executor, and ship specs onto the bus. The renderer is a pure function of `RenderSpec`.

**Orphan RenderEvents** (no matching `tool_use_id` — server-emitted system specs, hook output, etc.) materialize as `TranscriptItem::Render { spec }`. `item_to_lines` dispatches the spec directly.

## Schema Changes

### Widen `RenderSpec::Text` with a style hint

`Text { body, dim: bool }` can't express colored bodies (Bash error output in orange, success Status messages in green). Replace with:

```rust
pub enum TextStyle { Plain, Dim, Error, Success, Warn, Strong }

RenderSpec::Text { body: String, style: TextStyle }
```

Five named hints cover every existing use without leaking raw color codes into the schema. Renderer maps to its palette (CLI uses `CC_*` constants; web maps to design-system tokens).

Migration: `dim: bool` → `style: TextStyle` is a breaking change to the on-wire enum. Since `RenderEvent` doesn't cross the persistence boundary (server doesn't store specs; it streams them), no DB migration is needed. Bump the protocol version constant; old clients see "unknown style" and fall back to Plain.

### Add `RenderSlot` to `BusMessage::RenderEvent`

```rust
pub enum RenderSlot { Message, Tag, Progress, Queued, Result, Rejected, Error }

BusMessage::RenderEvent {
    tool_use_id: String,
    parent_tool_use_id: Option<String>,
    slot: RenderSlot,                  // NEW
    spec: shared::RenderSpec,
    uuid: Uuid,
    session_id: String,
}
```

The slot disambiguates which hook produced the spec so the transcript folder can route it to the correct field.

### Add spec slots to `TranscriptItem::ToolCall`

```rust
ToolCall {
    tool_use_id: String,
    name: String,                      // kept solely for ToolBatch classify()
    elapsed_ms: u64,
    message_spec:   Option<RenderSpec>,
    tag_spec:       Option<RenderSpec>,
    progress_specs: Vec<RenderSpec>,   // multiple progress updates accumulate
    queued_spec:    Option<RenderSpec>,
    result_spec:    Option<RenderSpec>,
    rejected_spec:  Option<RenderSpec>,
    error_spec:     Option<RenderSpec>,
}
```

Raw `input` and `result` fields are removed in the final migration step (after every tool emits specs). Until then they coexist with the slots so the existing `render_tool_result_for` dispatch keeps working through the transition.

### Add `TranscriptItem::Render` for orphans

```rust
Render { spec: RenderSpec }
```

For RenderEvents whose `tool_use_id` doesn't match any in-flight `ToolCall` (system specs from server, hook output).

## Dispatcher — per-variant treatment

The implementation lives in `cli/src/tui/render/mod.rs::render_spec`. Reuses existing helpers: `diff::render_hunks`, `osc8_link`, `dim_style`, `tool_family::batch_fragment`. New helpers added only where existing ones don't cover a variant.

| Variant       | Treatment |
|---------------|-----------|
| `Header { verb, target, tag }` | `⏺ verb target` with bold name + OSC8-linked target if `target` contains `/` or matches `\w+\.\w+` (looks like a path). `tag` rendered inline in dim style: `Timeout{ms}` → `[timeout {ms}ms]`, `Model{id}` → `[{id}]`, `Truncated` → `[truncated]`, `ResumeId{value}` → `[resume:{value}]`, `Custom{value}` → `[{value}]`. |
| `Text { body, style }` | One Line per body line. Style maps to a `ratatui::Style` via a `text_style()` helper using `CC_*` palette constants. |
| `Code { language, body, truncated }` | First line gets the `  ⎿  ` corner; subsequent lines indented `     `. No syntax highlighting v1; `language` is preserved for the web frontend. `truncated` appends `… +K lines (ctrl+o to expand)` in dim style. |
| `Diff { file_path, hunks }` | Reuse `diff::render_hunks(hunks)` — already exists and matches Claude Code's visual spec. Summary row produced from `count_changes` (also already present). |
| `PathList { entries, total, truncated }` | One row per entry: `  ⎿  path:line  preview`. Truncate at 20 with `… +K paths (ctrl+o)` footer. |
| `KeyValues { rows }` | Two-column: dim key, `:` separator, plain value. One row per pair. |
| `Collapsible { summary, expanded_by_default, children }` | Expand iff `expanded_by_default \|\| opts.detailed`. Collapsed: summary line + `(ctrl+o to expand)` hint. Expanded: summary line followed by recursive `render_spec` of each child. |
| `Group { children }` | Recurse: `children.iter().flat_map(render_spec).collect()`. |
| `Row { children }` | If every child renders to ≤1 Line, horizontally join spans with a single space separator. Otherwise fall back to vertical stack with a dim `│ ` left rule on each line (visual marker that the children were intended as a row). Scrollback is fundamentally vertical; tools that need real side-by-side composition shouldn't reach for `Row` in v1. |
| `Status { state, message }` | Glyph + colored message on one line. `Queued` → `…` dim, `InProgress` → `›` dim, `Success` → `✓` green, `Error` → `✗` red, `Rejected` → `⚠` orange. Empty when `message: None` (just the glyph). |
| `Interactive { widget, .. }` | Render `(awaiting input)` placeholder in dim style. The modal is the real UX, already wired via `BusMessage::InteractionRequested`. |
| `Nothing` | Empty `Vec`. |

## Migration phasing

Each step is independently shippable; the system stays correct between steps.

1. **Schema** — widen `RenderSpec::Text`, add `RenderSlot`, add `Render` transcript variant, add spec slots on `ToolCall`, keep raw `input`/`result` fields. No behavior change.
2. **Executor wiring** — `tool_loop.rs` calls render hooks at lifecycle transitions and emits a `RenderEvent` per hook. `transcript.rs::fold` routes events into slots by `tool_use_id`. At this point both code paths coexist; `item_to_lines` still uses raw fields.
3. **Dispatcher impls** — fill in every `render_spec` arm. Add `render_spec_tests` mirroring the existing `render::tests` for each variant. No caller change yet (everything still ignores the slots).
4. **Switch `item_to_lines(ToolCall)`** to read slots first; fall back to legacy raw-field rendering only when a slot is `None`. Tools still emit raw shapes.
5. **Migrate `Edit`** — `EditTool::render_tool_result_message` returns `RenderSpec::Group { children: [Status{Success}, Diff{...}] }`. Parity test: capture `Vec<Line>` from the legacy `render_edit_result` path for a fixed input, then assert byte-equal against the spec-driven output for the same input.
6. **Migrate `Write`** — `render_tool_result_message` returns `Group { [Status{Success, "Wrote N lines"}, Code { language: None, body: numbered_content, truncated: total>10 }] }`.
7. **Migrate `Bash`** — `render_tool_result_message` returns `Text { body, style: Error \| Plain }` for the body, with truncation handled by the dispatcher's `Code` variant (Bash output goes through `Code` for the truncation footer; orange error text goes through `Text` style).
8. **Migrate `Read`/`Grep`/`Glob`** — these flow through `ToolBatch` collapse today. Per-call specs are only rendered in expanded mode; the collapsed summary still uses `tool_family::batch_fragment` (orthogonal — grouping, not rendering).
9. **Delete legacy** — drop `render_tool_result_for`, `render_bash_result`, `render_edit_result`, `render_write_result`, `render_generic_result`; drop `name`/`input`/`result` raw fields from `ToolCall` (keep `name` for ToolBatch classify). The renderer is now slots-only.

## File Structure

**Modified:**
- `shared/src/render_spec.rs` — widen `Text` variant, add `TextStyle` enum.
- `cli/src/sdk/protocol.rs` — add `slot: RenderSlot` field on `BusMessage::RenderEvent`; add `RenderSlot` enum.
- `cli/src/tui/transcript.rs` — extend `TranscriptItem::ToolCall` with spec slots; add `TranscriptItem::Render { spec }`; bus folder routes RenderEvents to slots by `tool_use_id`, creates `Render` items for orphans.
- `cli/src/tui/render/mod.rs` — flesh out `render_spec` dispatcher per §"Dispatcher — per-variant treatment"; add `text_style()` palette helper.
- `cli/src/conversation/tool_loop.rs` — emit RenderEvent at each lifecycle transition (start, queued, progress, complete, rejected, error) by calling the corresponding `tool.render_*` hook.
- `cli/src/tui/app.rs` — drop the "ignore RenderEvent" comment; the folder now consumes them. `item_to_lines` callers thread `RenderOpts { detailed: show_detailed_transcript, .. }`.
- `cli/src/tools/edit.rs`, `write.rs`, `bash.rs`, `read.rs`, `grep.rs`, `glob_tool.rs` — implement `render_tool_use_message` / `render_tool_result_message` to return real specs (step-by-step per §migration).

**Deleted (final migration step):**
- `render_tool_result_for`, `render_bash_result`, `render_edit_result`, `render_write_result`, `render_generic_result` in `cli/src/tui/render/mod.rs`.

## Open trade-offs (acknowledged)

- **`Row` in vertical scrollback.** Multi-line children inside a `Row` stack vertically with a `│` rule. Real 2D composition is out of scope v1.
- **Spec slot memory** on `ToolCall` — seven `Option<RenderSpec>` per item. Specs are small and most slots are `None`; acceptable.
- **`TextStyle` is a closed enum.** Tools can't express arbitrary colors. By design — keeps the schema portable across surfaces.
- **No syntax highlighting** for `Code` v1. Respects the "no extra crates" rule in CLAUDE.md.

## Testing

- Unit tests per variant in `cli/src/tui/render/render_spec_tests`. Each variant gets at least one "renders nonempty output" test and one structural test (e.g., `Group` recurses, `Collapsible` respects `expanded_by_default`).
- Routing tests in `cli/src/tui/transcript::tests` — RenderEvent with `tool_use_id` X attaches to the right slot on the matching `ToolCall`; with a missing id, materializes as `Render`.
- Parity test for each migrated tool: capture `Vec<Line>` from the legacy renderer for fixed inputs (one happy path, one truncation case, one error case where applicable), assert byte-equal against the spec-driven output. Tests live alongside the migration commit and are deleted in step 9 with the legacy code.
