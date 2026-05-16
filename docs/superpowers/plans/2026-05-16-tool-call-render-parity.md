# Tool-call rendering parity with Claude Code — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring super's TUI tool-call rendering to 1:1 visual parity with Claude Code v2.1.143 — read/search calls collapse into a single dim-gray summary line, mutating calls render per-call blocks with proper display-name aliases, diff hunks for Edit, line-numbered content for Write, and Bash output with 3-line truncation + error coloring.

**Architecture:** A new `TranscriptItem::ToolBatch` variant captures consecutive read-only tool calls and renders them as one collapsed line; a post-fold transformer in `transcript.rs` produces it. The existing per-call `ToolCall` path stays for mutating tools but gets a richer renderer: tool-name aliasing, colored prefixes (green/orange), structured diffs via the `similar` crate, and Bash-specific truncation. The existing `Ctrl+O` / `show_detailed_transcript` toggle is wired into the new `ToolBatch` so collapsed batches expand into per-call blocks when the flag is on. All work is additive — no protocol changes.

**Tech Stack:** Rust 2021 edition, `ratatui` 0.28 for TUI primitives, `crossterm` 0.28 for keys, new dep `similar = "2"` for diff generation. Pure-function rendering helpers stay in `cli/src/tui/render.rs` with unit tests.

**Reference docs:** `cli/docs/tool-call-render-spec.md` — the empirically-derived visual spec this plan implements. Open it side-by-side while working.

---

## File Structure

**New files:**
- `cli/src/tui/render/diff.rs` — Diff hunk extraction (uses `similar`) and ANSI-colored line rendering.
- `cli/src/tui/render/tool_family.rs` — Tool-name → family classifier (`ReadSearch` / `Mutating` / `Subagent`) and per-tool collapsed-summary phrasing.

**Modified files:**
- `cli/Cargo.toml` — add `similar = "2"`.
- `cli/src/tui/colors.rs` — add orange/red/green diff hunk palette constants.
- `cli/src/tui/transcript.rs` — add `TranscriptItem::ToolBatch`, add `group_tool_batches()` post-fold transformer.
- `cli/src/tui/render.rs` — split into module: keep file as `mod.rs`-style entrypoint; add display-name aliasing, OSC8 hyperlink helper, per-tool result renderers, ToolBatch renderer. Wire `show_detailed_transcript` through.
- `cli/src/tui/app.rs` — call `group_tool_batches()` after `fold()` in both `flush_to_scrollback` and `render_live_tail`; update `is_stable` for the new variant; thread `show_detailed_transcript` through to `item_to_lines`; update hint text.
- `cli/src/tools/bash.rs` — prefix error content with `Error: Exit code <N>` so the renderer can show it without extending the protocol.

---

## Task 1: Add `similar` diff crate dependency

**Files:**
- Modify: `cli/Cargo.toml`

- [ ] **Step 1: Add the dependency**

Open `cli/Cargo.toml` and add this line in the `[dependencies]` block (alphabetical order; insert just before `tokio`):

```toml
similar = "2"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p super-cli`
Expected: build succeeds, `similar` resolves to `2.x`.

- [ ] **Step 3: Commit**

```bash
git add cli/Cargo.toml Cargo.lock
git commit -m "build(cli): add similar crate for diff rendering"
```

---

## Task 2: Extend `colors.rs` with diff + bash-error palette

**Files:**
- Modify: `cli/src/tui/colors.rs`
- Test: `cli/src/tui/colors.rs` (inline `#[cfg(test)]`)

The spec calls for these new colors (256-color ANSI indexes, captured from Claude Code):

| Constant            | Index | Use                                         |
| ------------------- | ----- | ------------------------------------------- |
| `CC_ORANGE`         | 211   | Error `⏺` and Bash error body text          |
| `CC_DIFF_DEL_FG`    | 167   | Diff removed-line foreground                |
| `CC_DIFF_DEL_BG`    | 52    | Diff removed-line background                |
| `CC_DIFF_ADD_FG`    | 77    | Diff added-line foreground                  |
| `CC_DIFF_ADD_BG`    | 22    | Diff added-line background                  |

- [ ] **Step 1: Write the constants and a smoke test**

Append to `cli/src/tui/colors.rs`:

```rust
// Captured from Claude Code v2.1.143 (see cli/docs/tool-call-render-spec.md).
pub const CC_ORANGE: Color      = Color::Indexed(211); // error tool prefix, bash error text
pub const CC_DIFF_DEL_FG: Color = Color::Indexed(167); // removed line fg
pub const CC_DIFF_DEL_BG: Color = Color::Indexed(52);  // removed line bg
pub const CC_DIFF_ADD_FG: Color = Color::Indexed(77);  // added line fg
pub const CC_DIFF_ADD_BG: Color = Color::Indexed(22);  // added line bg

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn diff_palette_uses_expected_ansi_indexes() {
        assert_eq!(CC_ORANGE,      Color::Indexed(211));
        assert_eq!(CC_DIFF_DEL_FG, Color::Indexed(167));
        assert_eq!(CC_DIFF_DEL_BG, Color::Indexed(52));
        assert_eq!(CC_DIFF_ADD_FG, Color::Indexed(77));
        assert_eq!(CC_DIFF_ADD_BG, Color::Indexed(22));
    }
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p super-cli --lib tui::colors -- --nocapture`
Expected: 1 passed.

- [ ] **Step 3: Commit**

```bash
git add cli/src/tui/colors.rs
git commit -m "feat(tui): add diff + bash-error color palette constants"
```

---

## Task 3: Create `tool_family` classifier module

A pure-function classifier that maps a tool name to its rendering family. This is the gate for the read/search collapse.

**Files:**
- Create: `cli/src/tui/render/mod.rs` (move existing `cli/src/tui/render.rs` content here)
- Create: `cli/src/tui/render/tool_family.rs`

> **Note:** This task only restructures `render.rs` into a directory module and adds the classifier file. The renderer itself isn't changed yet.

- [ ] **Step 1: Convert `render.rs` into a directory module**

```bash
mkdir -p cli/src/tui/render
git mv cli/src/tui/render.rs cli/src/tui/render/mod.rs
```

- [ ] **Step 2: Verify it still compiles**

Run: `cargo build -p super-cli`
Expected: build succeeds. No code changes — just file moved.

- [ ] **Step 3: Write the failing classifier test**

Create `cli/src/tui/render/tool_family.rs`:

```rust
//! Maps a tool name (and optionally its input) to a rendering family.
//!
//! - `ReadSearch` — collapses into a dim-gray `Read N files` / `Searched for N`
//!   summary line. Per spec, this covers tools that don't mutate state.
//! - `Mutating` — renders as a per-call `⏺ Name(args)` block with results.
//! - `Subagent` — `Task`/`Agent` calls; rendered per-call for now (Claude's
//!   grouped subagent render is a separate follow-up).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolFamily {
    ReadSearch,
    Mutating,
    Subagent,
}

/// Classify a tool by name. Unknown names default to `Mutating` — safer to
/// render them in full than to silently hide them in a collapsed summary.
pub fn classify(name: &str) -> ToolFamily {
    match name {
        "Read" | "Grep" | "Glob" | "LSP" => ToolFamily::ReadSearch,
        "Task" | "Agent" => ToolFamily::Subagent,
        _ => ToolFamily::Mutating,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_grep_glob_lsp_classify_as_read_search() {
        for n in ["Read", "Grep", "Glob", "LSP"] {
            assert_eq!(classify(n), ToolFamily::ReadSearch, "tool={n}");
        }
    }

    #[test]
    fn task_and_agent_classify_as_subagent() {
        assert_eq!(classify("Task"), ToolFamily::Subagent);
        assert_eq!(classify("Agent"), ToolFamily::Subagent);
    }

    #[test]
    fn mutating_tools_classify_as_mutating() {
        for n in ["Bash", "Write", "Edit", "MultiEdit", "NotebookEdit", "WebFetch", "WebSearch"] {
            assert_eq!(classify(n), ToolFamily::Mutating, "tool={n}");
        }
    }

    #[test]
    fn unknown_tool_defaults_to_mutating() {
        assert_eq!(classify("SomeMcpTool"), ToolFamily::Mutating);
    }
}
```

Wire the module in `cli/src/tui/render/mod.rs`. Add at the very top of the file:

```rust
pub mod tool_family;
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p super-cli --lib tui::render::tool_family`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/render/
git commit -m "feat(tui): add tool_family classifier for read/search collapse"
```

---

## Task 4: Tool display-name aliasing

Claude renames `Edit` → `Update` / `Create` / `Updated plan` and `Write` → `Write` / `Updated plan` for display. Underlying tool ID stays the same.

**Files:**
- Modify: `cli/src/tui/render/mod.rs` (add helper + tests)

- [ ] **Step 1: Write the failing tests**

Add inside the existing `#[cfg(test)] mod tests { ... }` block in `cli/src/tui/render/mod.rs`:

```rust
    #[test]
    fn display_name_edit_with_empty_old_string_is_create() {
        let input = serde_json::json!({
            "file_path": "/x/new.txt",
            "old_string": "",
            "new_string": "hello",
        });
        assert_eq!(tool_display_name("Edit", &input), "Create");
    }

    #[test]
    fn display_name_edit_normal_is_update() {
        let input = serde_json::json!({
            "file_path": "/x/notes.md",
            "old_string": "foo",
            "new_string": "bar",
        });
        assert_eq!(tool_display_name("Edit", &input), "Update");
    }

    #[test]
    fn display_name_write_is_write() {
        let input = serde_json::json!({"file_path": "/x/notes.md", "content": "hi"});
        assert_eq!(tool_display_name("Write", &input), "Write");
    }

    #[test]
    fn display_name_bash_is_bash() {
        let input = serde_json::json!({"command": "echo hi"});
        assert_eq!(tool_display_name("Bash", &input), "Bash");
    }

    #[test]
    fn display_name_falls_back_to_tool_name() {
        let input = serde_json::json!({});
        assert_eq!(tool_display_name("Grep", &input), "Grep");
        assert_eq!(tool_display_name("SomeMcpTool", &input), "SomeMcpTool");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p super-cli --lib tui::render -- display_name`
Expected: FAIL with "cannot find function `tool_display_name` in this scope".

- [ ] **Step 3: Implement the helper**

Add to `cli/src/tui/render/mod.rs` just below the existing `summarize_tool_call` function:

```rust
/// Claude Code's `userFacingName` aliasing. The underlying tool ID is unchanged;
/// only the rendered label differs. See cli/docs/tool-call-render-spec.md
/// "Per-tool display names".
pub fn tool_display_name(name: &str, input: &serde_json::Value) -> String {
    match name {
        "Edit" => {
            let old = input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
            if old.is_empty() {
                "Create".to_string()
            } else {
                "Update".to_string()
            }
        }
        // Future: detect plan-files dir → "Updated plan". The plans dir lives
        // outside super's purview today; revisit when /plan lands.
        _ => name.to_string(),
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p super-cli --lib tui::render -- display_name`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/render/mod.rs
git commit -m "feat(tui): alias Edit→Update/Create for tool display name"
```

---

## Task 5: Switch tool-call prefix color from cyan to green (success) / orange (error)

Currently `⏺` for tool calls uses `tool_style()` (Cyan + Bold). Per spec: success `⏺` is `CC_GREEN` (114), error `⏺` is `CC_ORANGE` (211).

**Files:**
- Modify: `cli/src/tui/render/mod.rs`
- Modify: `cli/src/tui/render/mod.rs` (tests)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
    use crate::tui::colors::{CC_GREEN, CC_ORANGE};
    use crate::tui::transcript::{TranscriptItem, ToolResultRender};

    fn first_span_color(line: &Line) -> Option<ratatui::style::Color> {
        line.spans.first().and_then(|s| s.style.fg)
    }

    #[test]
    fn tool_call_prefix_is_green_on_success() {
        let item = TranscriptItem::ToolCall {
            tool_use_id: "tu1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "echo hi"}),
            result: Some(ToolResultRender { content: "hi".into(), is_error: false }),
            elapsed_ms: 0,
        };
        let lines = item_to_lines(&item, 0);
        // Find the line whose first span is the `⏺ ` prefix.
        let prefix = lines.iter().find(|l| l.spans.first().map(|s| s.content.contains('⏺')).unwrap_or(false)).expect("⏺ prefix line");
        assert_eq!(first_span_color(prefix), Some(CC_GREEN));
    }

    #[test]
    fn tool_call_prefix_is_orange_on_error() {
        let item = TranscriptItem::ToolCall {
            tool_use_id: "tu1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "false"}),
            result: Some(ToolResultRender { content: "Error: Exit code 1".into(), is_error: true }),
            elapsed_ms: 0,
        };
        let lines = item_to_lines(&item, 0);
        let prefix = lines.iter().find(|l| l.spans.first().map(|s| s.content.contains('⏺')).unwrap_or(false)).expect("⏺ prefix line");
        assert_eq!(first_span_color(prefix), Some(CC_ORANGE));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p super-cli --lib tui::render -- tool_call_prefix`
Expected: FAIL — prefix is currently cyan, not green.

- [ ] **Step 3: Update the `ToolCall` branch in `item_to_lines`**

In `cli/src/tui/render/mod.rs`, locate the `TranscriptItem::ToolCall { name, input, result, .. }` match arm inside `item_to_lines` (currently around line 149). Replace the body with:

```rust
        TranscriptItem::ToolCall { name, input, result, .. } => {
            use crate::tui::colors::{CC_GREEN, CC_ORANGE};
            let is_error = result.as_ref().map(|r| r.is_error).unwrap_or(false);
            let prefix_color = if is_error { CC_ORANGE } else { CC_GREEN };
            let display = tool_display_name(name, input);
            let summary = summarize_tool_call(name, input);

            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("⏺ ", Style::default().fg(prefix_color)),
                Span::styled(display, Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" "),
                Span::styled(summary, dim),
            ]));
            if let Some(r) = result {
                render_tool_result(&mut lines, r, &dim);
            }
        }
```

Also update the `Message::ToolCall` branch in `message_to_lines` (around line 50) the same way — switch `assistant_prefix_style()` to `Style::default().fg(CC_GREEN)` and use bold for the name. The legacy `Message::ToolCall` doesn't carry `is_error`, so always use `CC_GREEN`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p super-cli --lib tui::render`
Expected: all tests pass including the two new prefix-color tests.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/render/mod.rs
git commit -m "feat(tui): color tool-call ⏺ green on success, orange on error"
```

---

## Task 6: OSC8 terminal hyperlink helper

Wrap a file path in an OSC8 escape so terminals that support it (iTerm2, WezTerm, recent Kitty/Alacritty) render a clickable underline.

**Files:**
- Modify: `cli/src/tui/render/mod.rs`

The OSC8 wire format is:
```
ESC ] 8 ; <id> ; <uri> ESC \   <label>   ESC ] 8 ; ; ESC \
```

For terminals that don't support it, the escape sequences are stripped (most modern terminals) or rendered as visible junk (very old ones). Risk is low.

- [ ] **Step 1: Write the failing tests**

Add to `tests` module:

```rust
    #[test]
    fn osc8_link_wraps_label_with_escape_sequence() {
        let s = osc8_link("/abs/path.txt", "path.txt");
        assert!(s.contains("\x1b]8;;file:///abs/path.txt\x1b\\"), "got: {s:?}");
        assert!(s.contains("path.txt"));
        assert!(s.ends_with("\x1b]8;;\x1b\\"));
    }

    #[test]
    fn osc8_link_handles_relative_path_by_prefixing_file_uri() {
        let s = osc8_link("relative/note.md", "note.md");
        // Even a relative path gets a file:// URI; modern terminals resolve
        // against their own cwd. The label is what the user clicks on.
        assert!(s.contains("file://"));
        assert!(s.contains("note.md"));
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p super-cli --lib tui::render -- osc8_link`
Expected: FAIL — `osc8_link` not defined.

- [ ] **Step 3: Implement**

Add to `cli/src/tui/render/mod.rs`:

```rust
/// Wrap `label` in an OSC8 terminal hyperlink pointing at `path`. Terminals
/// that don't support OSC8 render the label without the underline.
pub fn osc8_link(path: &str, label: &str) -> String {
    let uri = if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file://{path}")
    };
    format!("\x1b]8;;{uri}\x1b\\{label}\x1b]8;;\x1b\\")
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p super-cli --lib tui::render -- osc8_link`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/render/mod.rs
git commit -m "feat(tui): add OSC8 hyperlink helper for file paths in tool args"
```

---

## Task 7: Bash multi-line output truncation (3-line rule)

Per spec: show first 3 lines of stdout; if total > 3, append `     … +K lines (ctrl+o to expand)` in faint dim style. K = total − 3.

**Files:**
- Modify: `cli/src/tui/render/mod.rs` (`render_tool_result` becomes Bash-aware)

The current `render_tool_result` truncates at 20 lines. We need to replace it with a per-tool dispatch: Bash gets the 3-line rule; everything else stays as-is for now (a follow-up task tightens Edit/Write).

- [ ] **Step 1: Write the failing tests**

Add to `tests` module:

```rust
    #[test]
    fn bash_result_renders_at_most_3_output_lines_then_ellipsis() {
        let item = TranscriptItem::ToolCall {
            tool_use_id: "tu1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "seq 1 8"}),
            result: Some(ToolResultRender {
                content: "1\n2\n3\n4\n5\n6\n7\n8".into(),
                is_error: false,
            }),
            elapsed_ms: 0,
        };
        let lines = item_to_lines(&item, 0);
        let body = rendered_text(&lines);
        assert!(body.contains("  ⎿  1"), "first output line under corner: {body:?}");
        assert!(body.contains("     2"), "second line aligned: {body:?}");
        assert!(body.contains("     3"), "third line aligned: {body:?}");
        assert!(body.contains("… +5 lines (ctrl+o to expand)"), "ellipsis present: {body:?}");
        assert!(!body.contains("\n4\n") && !body.contains("     4"), "line 4 must be hidden: {body:?}");
    }

    #[test]
    fn bash_result_with_3_or_fewer_lines_shows_no_ellipsis() {
        let item = TranscriptItem::ToolCall {
            tool_use_id: "tu1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "seq 1 3"}),
            result: Some(ToolResultRender {
                content: "1\n2\n3".into(),
                is_error: false,
            }),
            elapsed_ms: 0,
        };
        let body = rendered_text(&item_to_lines(&item, 0));
        assert!(body.contains("  ⎿  1"));
        assert!(body.contains("     2"));
        assert!(body.contains("     3"));
        assert!(!body.contains("ctrl+o"), "no expand hint when nothing truncated: {body:?}");
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p super-cli --lib tui::render -- bash_result`
Expected: FAIL — current renderer shows up to 20 lines and uses `… N more lines` phrasing.

- [ ] **Step 3: Replace `render_tool_result` with a per-tool dispatch**

In `cli/src/tui/render/mod.rs`, replace the existing `render_tool_result` and update the callers. First, change the `ToolCall` branch in `item_to_lines` to pass the tool name:

```rust
            if let Some(r) = result {
                render_tool_result_for(name, &mut lines, r, &dim);
            }
```

Then replace `render_tool_result` with this new dispatcher:

```rust
fn render_tool_result_for(
    tool_name: &str,
    lines: &mut Vec<Line<'static>>,
    r: &ToolResultRender,
    dim: &Style,
) {
    match tool_name {
        "Bash" => render_bash_result(lines, r, dim),
        // Future: "Edit"/"Write" get dedicated renderers in later tasks.
        _ => render_generic_result(lines, r, dim),
    }
}

/// Generic fallback: behaves like the prior renderer (cap at 20 lines).
fn render_generic_result(lines: &mut Vec<Line<'static>>, r: &ToolResultRender, dim: &Style) {
    let max_lines = 20;
    let body: Vec<&str> = r.content.lines().take(max_lines).collect();
    let total = r.content.lines().count();
    let mut first = true;
    for line in body {
        let prefix = if first { "  ⎿  " } else { "     " };
        first = false;
        lines.push(Line::from(vec![
            Span::styled(prefix, *dim),
            Span::styled(line.to_string(), *dim),
        ]));
    }
    if total > max_lines {
        lines.push(Line::from(vec![
            Span::styled("     ", *dim),
            Span::styled(format!("… +{} lines (ctrl+o to expand)", total - max_lines),
                         Style::default().add_modifier(Modifier::DIM)),
        ]));
    }
}

/// Bash-specific: 3-line truncation, `… +K lines (ctrl+o to expand)` suffix.
/// Errors render in orange.
fn render_bash_result(lines: &mut Vec<Line<'static>>, r: &ToolResultRender, dim: &Style) {
    use crate::tui::colors::CC_ORANGE;
    const MAX: usize = 3;
    let body_color = if r.is_error { Style::default().fg(CC_ORANGE) } else { Style::default() };
    let all: Vec<&str> = r.content.lines().collect();
    let shown = all.iter().take(MAX);
    let total = all.len();

    for (i, line) in shown.enumerate() {
        let prefix = if i == 0 { "  ⎿  " } else { "     " };
        lines.push(Line::from(vec![
            Span::styled(prefix, *dim),
            Span::styled(line.to_string(), body_color),
        ]));
    }
    if total > MAX {
        let remaining = total - MAX;
        lines.push(Line::from(vec![
            Span::styled("     ", *dim),
            Span::styled(
                format!("… +{remaining} lines (ctrl+o to expand)"),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]));
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p super-cli --lib tui::render`
Expected: existing tests + 2 new bash tests pass.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/render/mod.rs
git commit -m "feat(tui): truncate Bash output to 3 lines with '… +K lines' suffix"
```

---

## Task 8: Bash error path — exit code in content + orange body

Super's `BashTool` currently puts raw stderr in `content` and sets `is_error = true`. Claude shows `Error: Exit code N` as the first result line and colors all error output orange. We get the exit code from `out.status.code()` and prepend it.

**Files:**
- Modify: `cli/src/tools/bash.rs`
- Modify: `cli/src/tools/bash.rs` (tests — add inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Append to `cli/src/tools/bash.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::PermissionMode;
    use crate::tools::contract::{Tool, ToolCallContext};

    fn ctx() -> ToolCallContext {
        ToolCallContext {
            cwd: std::env::temp_dir(),
            permission_mode: PermissionMode::default(),
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: true,
            tool_use_id: String::new(),
        }
    }

    #[tokio::test]
    async fn bash_error_content_starts_with_exit_code_header() {
        let t = BashTool;
        let out = t.call(serde_json::json!({"command": "bash -c 'echo ohno >&2; exit 2'"}), &ctx()).await;
        assert!(out.is_error, "expected error");
        assert!(out.content.starts_with("Error: Exit code 2"), "got: {:?}", out.content);
        assert!(out.content.contains("ohno"), "stderr preserved: {:?}", out.content);
    }

    #[tokio::test]
    async fn bash_success_content_does_not_prepend_exit_header() {
        let t = BashTool;
        let out = t.call(serde_json::json!({"command": "echo hello"}), &ctx()).await;
        assert!(!out.is_error);
        assert!(!out.content.starts_with("Error:"), "got: {:?}", out.content);
        assert!(out.content.trim() == "hello");
    }
}
```

> **Note:** If `PermissionMode` does not implement `Default`, replace `PermissionMode::default()` with whichever variant the production code uses for unrestricted calls (look at `tools/contract.rs` test sites and `cli/src/state/store.rs` for the variant name — likely `Bypass` or `Auto`).

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p super-cli --lib tools::bash`
Expected: FAIL — current content does not have the `Error: Exit code N` header.

- [ ] **Step 3: Update the result construction**

In `cli/src/tools/bash.rs`, locate the `Ok(Ok(out)) => { ... }` branch (around line 58). Replace it with:

```rust
                Ok(Ok(out)) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let success = out.status.success();
                    let content = if success {
                        if stderr.is_empty() {
                            stdout.to_string()
                        } else {
                            format!("stdout:\n{stdout}\nstderr:\n{stderr}")
                        }
                    } else {
                        let code = out.status.code().unwrap_or(-1);
                        // Match Claude's format exactly: header line followed by
                        // captured output (stderr first, then stdout if any).
                        let mut body = String::new();
                        if !stderr.is_empty() {
                            body.push_str(stderr.trim_end_matches('\n'));
                        }
                        if !stdout.is_empty() {
                            if !body.is_empty() { body.push('\n'); }
                            body.push_str(stdout.trim_end_matches('\n'));
                        }
                        if body.is_empty() {
                            format!("Error: Exit code {code}")
                        } else {
                            format!("Error: Exit code {code}\n{body}")
                        }
                    };
                    let truncated = if content.len() > 50000 {
                        format!("{}...\n[output truncated]", &content[..50000])
                    } else {
                        content
                    };
                    ToolResult { content: truncated, is_error: !success, ..Default::default() }
                }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p super-cli --lib tools::bash`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tools/bash.rs
git commit -m "feat(bash): prefix error output with 'Error: Exit code N' header"
```

---

## Task 9: Edit/Update diff body — `Added N lines, removed M lines` + colored hunks

When a tool call is `Edit` (display: `Update` or `Create`), the renderer should compute a diff from the input's `old_string`/`new_string` and emit:

```
  ⎿  Added 1 line, removed 1 line
      1 -hello
      2 +hi
```

For `Create` (empty `old_string`), there are no removals — render just `Added N lines` plus the new lines as additions.

**Files:**
- Create: `cli/src/tui/render/diff.rs`
- Modify: `cli/src/tui/render/mod.rs` (dispatch Edit to the new renderer)

- [ ] **Step 1: Write the failing diff-helper tests**

Create `cli/src/tui/render/diff.rs`:

```rust
//! Diff helpers: compute additions/removals, render hunks as colored lines.
//!
//! Uses the `similar` crate for line-based diffing. The output mirrors Claude
//! Code's `StructuredDiffList` layout (see cli/docs/tool-call-render-spec.md).

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use similar::{ChangeTag, TextDiff};

use crate::tui::colors::{CC_DIFF_ADD_BG, CC_DIFF_ADD_FG, CC_DIFF_DEL_BG, CC_DIFF_DEL_FG, CC_DIM};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffCounts {
    pub additions: usize,
    pub removals: usize,
}

/// Count added (`+`) and removed (`-`) lines between `old` and `new`.
pub fn count_changes(old: &str, new: &str) -> DiffCounts {
    let diff = TextDiff::from_lines(old, new);
    let mut additions = 0;
    let mut removals = 0;
    for ch in diff.iter_all_changes() {
        match ch.tag() {
            ChangeTag::Insert => additions += 1,
            ChangeTag::Delete => removals += 1,
            ChangeTag::Equal => {}
        }
    }
    DiffCounts { additions, removals }
}

/// Format the "Added X line[s], removed Y line[s]" summary string, matching
/// Claude's grammar (lowercase 'r' when there are also additions, uppercase R
/// when only removals). Returns spans so the numerals can be bold.
pub fn summary_spans(counts: DiffCounts) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if counts.additions > 0 {
        spans.push(Span::raw("Added "));
        spans.push(Span::styled(
            counts.additions.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(if counts.additions == 1 { " line" } else { " lines" }));
    }
    if counts.removals > 0 {
        let lead = if counts.additions > 0 { ", removed " } else { "Removed " };
        spans.push(Span::raw(lead));
        spans.push(Span::styled(
            counts.removals.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(if counts.removals == 1 { " line" } else { " lines" }));
    }
    spans
}

/// Render colored diff hunks. Each output `Line` is indented 5 spaces (so it
/// aligns under `⎿  `). Format per line:
///     " <N> -content"  (removed) — fg 167 bg 52
///     " <N> +content"  (added)   — fg 77  bg 22
///     " <N>  content"  (context) — dim
pub fn render_hunks(old: &str, new: &str) -> Vec<Line<'static>> {
    let diff = TextDiff::from_lines(old, new);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut old_lineno = 1usize;
    let mut new_lineno = 1usize;
    let lineno_width = diff.iter_all_changes().count().to_string().len().max(2);
    for change in diff.iter_all_changes() {
        let (lineno_for_display, tag_char, style) = match change.tag() {
            ChangeTag::Delete => {
                let n = old_lineno;
                old_lineno += 1;
                (n, '-', Style::default().fg(CC_DIFF_DEL_FG).bg(CC_DIFF_DEL_BG))
            }
            ChangeTag::Insert => {
                let n = new_lineno;
                new_lineno += 1;
                (n, '+', Style::default().fg(CC_DIFF_ADD_FG).bg(CC_DIFF_ADD_BG))
            }
            ChangeTag::Equal => {
                let n = new_lineno;
                old_lineno += 1;
                new_lineno += 1;
                (n, ' ', Style::default().fg(CC_DIM))
            }
        };
        let content = change.value().trim_end_matches('\n').to_string();
        let body = format!(" {:>width$} {}{}", lineno_for_display, tag_char, content,
                           width = lineno_width);
        out.push(Line::from(vec![
            Span::raw("    "),     // 4-space indent so the body's leading " "
                                   //    aligns at column 5 (under `⎿  `).
            Span::styled(body, style),
        ]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_changes_single_line_swap() {
        let c = count_changes("hello\n", "hi\n");
        assert_eq!(c, DiffCounts { additions: 1, removals: 1 });
    }

    #[test]
    fn count_changes_pure_insert() {
        let c = count_changes("", "a\nb\n");
        assert_eq!(c, DiffCounts { additions: 2, removals: 0 });
    }

    #[test]
    fn summary_spans_adds_and_removes_uses_lowercase_r() {
        let spans = summary_spans(DiffCounts { additions: 1, removals: 1 });
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Added 1 line, removed 1 line");
    }

    #[test]
    fn summary_spans_only_removed_uses_capital_R() {
        let spans = summary_spans(DiffCounts { additions: 0, removals: 3 });
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Removed 3 lines");
    }

    #[test]
    fn summary_spans_only_added_no_removed_suffix() {
        let spans = summary_spans(DiffCounts { additions: 5, removals: 0 });
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Added 5 lines");
    }

    #[test]
    fn render_hunks_marks_added_and_removed_with_color() {
        let lines = render_hunks("hello\n", "hi\n");
        // Expect at least one '-' line and one '+' line.
        let texts: Vec<String> = lines.iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect();
        assert!(texts.iter().any(|t| t.contains(" 1 -hello")), "removed: {texts:?}");
        assert!(texts.iter().any(|t| t.contains(" 1 +hi")), "added: {texts:?}");
    }
}
```

Register the module in `cli/src/tui/render/mod.rs`:

```rust
pub mod diff;
```

- [ ] **Step 2: Run the diff helper tests**

Run: `cargo test -p super-cli --lib tui::render::diff`
Expected: 5 passed.

- [ ] **Step 3: Wire Edit into the per-tool dispatch**

In `cli/src/tui/render/mod.rs`, extend `render_tool_result_for` so `Edit` (and only `Edit`) uses the new diff renderer. Add this arm before the catch-all `_`:

```rust
        "Edit" => render_edit_result(lines, input_for_edit, r, dim),
```

To plumb the `input` through, change the dispatcher signature to also take `input: &serde_json::Value` and pass it from the `ToolCall` branch. The new signatures:

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
        _ => render_generic_result(lines, r, dim),
    }
}

fn render_edit_result(
    lines: &mut Vec<Line<'static>>,
    input: &serde_json::Value,
    _r: &ToolResultRender,
    dim: &Style,
) {
    let old = input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
    let new = input.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
    let counts = diff::count_changes(old, new);

    // Summary row: `  ⎿  Added N line[s], removed M line[s]`
    let mut summary: Vec<Span<'static>> = vec![Span::styled("  ⎿  ", *dim)];
    summary.extend(diff::summary_spans(counts));
    lines.push(Line::from(summary));

    // Hunk rows: indented 4 spaces (body starts at column 5).
    lines.extend(diff::render_hunks(old, new));
}
```

And in the `ToolCall` arm of `item_to_lines`, change the call to pass `input`:

```rust
            if let Some(r) = result {
                render_tool_result_for(name, input, &mut lines, r, &dim);
            }
```

- [ ] **Step 4: Write the failing end-to-end test**

Add to the `tests` module in `cli/src/tui/render/mod.rs`:

```rust
    #[test]
    fn edit_result_summary_uses_added_removed_phrasing() {
        let item = TranscriptItem::ToolCall {
            tool_use_id: "tu1".into(),
            name: "Edit".into(),
            input: serde_json::json!({
                "file_path": "/tmp/a.txt",
                "old_string": "hello\n",
                "new_string": "hi\n",
            }),
            result: Some(ToolResultRender {
                content: "Successfully replaced 1 occurrence(s) in /tmp/a.txt".into(),
                is_error: false,
            }),
            elapsed_ms: 0,
        };
        let body = rendered_text(&item_to_lines(&item, 0));
        assert!(body.contains("Update"), "display name: {body:?}");
        assert!(body.contains("Added 1 line, removed 1 line"), "summary: {body:?}");
        assert!(body.contains(" 1 -hello"), "removed hunk: {body:?}");
        assert!(body.contains(" 1 +hi"), "added hunk: {body:?}");
        assert!(!body.contains("Successfully replaced"), "raw result text should not leak: {body:?}");
    }

    #[test]
    fn create_result_renders_only_additions() {
        let item = TranscriptItem::ToolCall {
            tool_use_id: "tu1".into(),
            name: "Edit".into(),
            input: serde_json::json!({
                "file_path": "/tmp/new.txt",
                "old_string": "",
                "new_string": "first line\nsecond line\n",
            }),
            result: Some(ToolResultRender { content: "ok".into(), is_error: false }),
            elapsed_ms: 0,
        };
        let body = rendered_text(&item_to_lines(&item, 0));
        assert!(body.contains("Create"), "display name: {body:?}");
        assert!(body.contains("Added 2 lines"), "summary: {body:?}");
        assert!(!body.contains("removed"), "no removed phrase when no removals: {body:?}");
    }
```

- [ ] **Step 5: Run the integration tests**

Run: `cargo test -p super-cli --lib tui::render`
Expected: all tests pass including the two new Edit/Create tests.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tui/render/
git commit -m "feat(tui): render Edit results as 'Added/removed' summary + colored diff"
```

---

## Task 10: Write result — `Wrote N lines to <path>` + line-numbered content

Per spec, Write should render:
```
⏺ Write(notes.txt)
  ⎿  Wrote 2 lines to notes.txt
      1 hello
      2 world
```

Truncate to first 10 lines for now (cheap, predictable). Use the original `input.content` rather than the tool result text (which is just "Successfully wrote N bytes").

**Files:**
- Modify: `cli/src/tui/render/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to `tests`:

```rust
    #[test]
    fn write_result_renders_wrote_n_lines_with_numbered_content() {
        let item = TranscriptItem::ToolCall {
            tool_use_id: "tu1".into(),
            name: "Write".into(),
            input: serde_json::json!({
                "file_path": "/tmp/notes.txt",
                "content": "alpha\nbeta\ngamma\n",
            }),
            result: Some(ToolResultRender {
                content: "Successfully wrote 18 bytes to /tmp/notes.txt".into(),
                is_error: false,
            }),
            elapsed_ms: 0,
        };
        let body = rendered_text(&item_to_lines(&item, 0));
        assert!(body.contains("Write"), "display name: {body:?}");
        assert!(body.contains("Wrote 3 lines to /tmp/notes.txt"), "summary: {body:?}");
        assert!(body.contains(" 1 alpha"), "numbered line 1: {body:?}");
        assert!(body.contains(" 2 beta"),  "numbered line 2: {body:?}");
        assert!(body.contains(" 3 gamma"), "numbered line 3: {body:?}");
        assert!(!body.contains("Successfully wrote"), "raw result text should not leak: {body:?}");
    }
```

- [ ] **Step 2: Confirm failure**

Run: `cargo test -p super-cli --lib tui::render -- write_result`
Expected: FAIL — Write currently goes through the generic renderer which shows the raw "Successfully wrote ..." line.

- [ ] **Step 3: Add the renderer and wire it**

In `cli/src/tui/render/mod.rs`, add a new arm in `render_tool_result_for`:

```rust
        "Write" => render_write_result(lines, input, dim),
```

And the new function:

```rust
fn render_write_result(
    lines: &mut Vec<Line<'static>>,
    input: &serde_json::Value,
    dim: &Style,
) {
    let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
    let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let total_lines = content.lines().count();
    let plural = if total_lines == 1 { "line" } else { "lines" };

    let mut summary: Vec<Span<'static>> = vec![Span::styled("  ⎿  ", *dim)];
    summary.push(Span::raw("Wrote "));
    summary.push(Span::styled(total_lines.to_string(), Style::default().add_modifier(Modifier::BOLD)));
    summary.push(Span::raw(format!(" {plural} to {path}")));
    lines.push(Line::from(summary));

    const MAX: usize = 10;
    let lineno_width = total_lines.to_string().len().max(2);
    for (i, line) in content.lines().take(MAX).enumerate() {
        let lineno = i + 1;
        let body = format!(" {:>width$} {}", lineno, line, width = lineno_width);
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(body, *dim),
        ]));
    }
    if total_lines > MAX {
        let remaining = total_lines - MAX;
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(
                format!("… +{remaining} lines (ctrl+o to expand)"),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]));
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p super-cli --lib tui::render`
Expected: all tests pass including the new `write_result_*` test.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/render/mod.rs
git commit -m "feat(tui): render Write results as 'Wrote N lines' + numbered content"
```

---

## Task 11: Add `TranscriptItem::ToolBatch` variant

Introduce a new transcript-item variant that groups consecutive read/search tool calls. The variant is populated by a post-fold transformer (Task 12) and rendered by Task 13.

**Files:**
- Modify: `cli/src/tui/transcript.rs`

- [ ] **Step 1: Add the variant + a struct for grouped calls**

Append to the `TranscriptItem` enum in `cli/src/tui/transcript.rs`:

```rust
    /// One or more consecutive read/search tool calls within an assistant turn.
    /// Renders as a single dim-gray summary line by default (e.g. "Read 3 files
    /// (ctrl+o to expand)"); when the global `show_detailed_transcript` flag is
    /// on, each call renders as an individual block. See
    /// cli/docs/tool-call-render-spec.md "Render mode 1".
    ToolBatch { calls: Vec<BatchCall> },
```

And add the supporting struct just below `ToolResultRender`:

```rust
#[derive(Debug, Clone)]
pub struct BatchCall {
    pub tool_use_id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub result: Option<ToolResultRender>,
}
```

- [ ] **Step 2: Add stub match arms so it compiles**

Adding a new variant breaks any exhaustive `match`. Two known sites need a stub now:

In `cli/src/tui/render/mod.rs`, inside `item_to_lines`, add this arm just before the closing `}`:

```rust
        TranscriptItem::ToolBatch { .. } => {
            // Real rendering wired in Task 13.
        }
```

In `cli/src/tui/app.rs`, inside `is_stable`, add:

```rust
        TranscriptItem::ToolBatch { .. } => true,
```

Run: `cargo build -p super-cli`
Expected: build succeeds. If the build still complains about unmatched arms, locate the sites with:

```bash
cargo build -p super-cli 2>&1 | grep -E "non-exhaustive|patterns.*not covered"
```

Add the same stub at each site (Vec is left empty, bool returns `true`).

- [ ] **Step 3: Commit**

```bash
git add cli/src/tui/transcript.rs cli/src/tui/render/mod.rs cli/src/tui/app.rs
git commit -m "feat(tui): add TranscriptItem::ToolBatch variant (stub arms)"
```

---

## Task 12: `group_tool_batches()` post-fold transformer

Walk a `Vec<TranscriptItem>` and collapse runs of consecutive `ToolCall` items whose names classify as `ReadSearch` into a single `ToolBatch`. Any non-`ToolCall` item (User text, AssistantText, System, etc.) — or a `ToolCall` for a mutating tool — breaks the run.

**Files:**
- Modify: `cli/src/tui/transcript.rs`

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests` block in `cli/src/tui/transcript.rs`:

```rust
    use crate::tui::render::tool_family::{classify, ToolFamily};

    fn tc(name: &str, id: &str) -> TranscriptItem {
        TranscriptItem::ToolCall {
            tool_use_id: id.into(),
            name: name.into(),
            input: serde_json::json!({}),
            result: Some(ToolResultRender { content: "ok".into(), is_error: false }),
            elapsed_ms: 0,
        }
    }

    #[test]
    fn group_collapses_consecutive_reads_into_one_batch() {
        let items = vec![tc("Read", "1"), tc("Read", "2"), tc("Read", "3")];
        let g = group_tool_batches(items);
        assert_eq!(g.len(), 1);
        match &g[0] {
            TranscriptItem::ToolBatch { calls } => assert_eq!(calls.len(), 3),
            other => panic!("expected ToolBatch, got {other:?}"),
        }
    }

    #[test]
    fn group_does_not_collapse_a_single_mutating_tool() {
        let items = vec![tc("Bash", "1")];
        let g = group_tool_batches(items);
        assert_eq!(g.len(), 1);
        assert!(matches!(g[0], TranscriptItem::ToolCall { .. }));
    }

    #[test]
    fn group_collapses_single_read_into_a_one_call_batch() {
        // Per spec: even N=1 read collapses into "Read 1 file (ctrl+o to expand)".
        let items = vec![tc("Read", "1")];
        let g = group_tool_batches(items);
        assert_eq!(g.len(), 1);
        match &g[0] {
            TranscriptItem::ToolBatch { calls } => assert_eq!(calls.len(), 1),
            other => panic!("expected ToolBatch, got {other:?}"),
        }
    }

    #[test]
    fn group_breaks_at_mutating_tool() {
        let items = vec![
            tc("Read", "1"),
            tc("Read", "2"),
            tc("Bash", "3"),
            tc("Read", "4"),
        ];
        let g = group_tool_batches(items);
        assert_eq!(g.len(), 3);
        assert!(matches!(&g[0], TranscriptItem::ToolBatch { calls } if calls.len() == 2));
        assert!(matches!(&g[1], TranscriptItem::ToolCall { name, .. } if name == "Bash"));
        assert!(matches!(&g[2], TranscriptItem::ToolBatch { calls } if calls.len() == 1));
    }

    #[test]
    fn group_breaks_at_user_or_assistant_text() {
        let items = vec![
            tc("Read", "1"),
            TranscriptItem::AssistantText { text: "thinking".into(), complete: true },
            tc("Read", "2"),
        ];
        let g = group_tool_batches(items);
        assert_eq!(g.len(), 3);
        assert!(matches!(&g[0], TranscriptItem::ToolBatch { .. }));
        assert!(matches!(&g[1], TranscriptItem::AssistantText { .. }));
        assert!(matches!(&g[2], TranscriptItem::ToolBatch { .. }));
    }

    #[test]
    fn group_mixes_grep_and_glob_into_one_batch() {
        let items = vec![tc("Grep", "1"), tc("Glob", "2")];
        let g = group_tool_batches(items);
        assert_eq!(g.len(), 1);
        match &g[0] {
            TranscriptItem::ToolBatch { calls } => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].name, "Grep");
                assert_eq!(calls[1].name, "Glob");
            }
            other => panic!("expected ToolBatch, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p super-cli --lib tui::transcript -- group_`
Expected: FAIL — `group_tool_batches` doesn't exist yet.

- [ ] **Step 3: Implement the transformer**

Add to `cli/src/tui/transcript.rs` (above the `#[cfg(test)]` block):

```rust
/// Post-fold transformation: collapse consecutive read/search tool calls into
/// a single `ToolBatch`. Mirrors Claude Code's `collapseReadSearch` pass.
pub fn group_tool_batches(items: Vec<TranscriptItem>) -> Vec<TranscriptItem> {
    use crate::tui::render::tool_family::{classify, ToolFamily};

    let mut out: Vec<TranscriptItem> = Vec::with_capacity(items.len());
    let mut pending: Vec<BatchCall> = Vec::new();

    fn flush(out: &mut Vec<TranscriptItem>, pending: &mut Vec<BatchCall>) {
        if !pending.is_empty() {
            let calls = std::mem::take(pending);
            out.push(TranscriptItem::ToolBatch { calls });
        }
    }

    for item in items {
        match item {
            TranscriptItem::ToolCall { tool_use_id, name, input, result, .. }
                if classify(&name) == ToolFamily::ReadSearch =>
            {
                pending.push(BatchCall { tool_use_id, name, input, result });
            }
            other => {
                flush(&mut out, &mut pending);
                out.push(other);
            }
        }
    }
    flush(&mut out, &mut pending);
    out
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p super-cli --lib tui::transcript -- group_`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/transcript.rs
git commit -m "feat(tui): group consecutive read/search calls into ToolBatch"
```

---

## Task 13: Render `ToolBatch` — summary line + per-tool count phrasing

Add per-tool phrasing helpers and a renderer. Summary forms (verbatim from Claude):
- `Read 1 file` / `Read N files`
- `Searched for 1 pattern` / `Searched for N patterns`
- `listed 1 directory` / `listed N directories`
- Mixed: comma-joined in input order, lowercase "l" on second clause: `Searched for 1 pattern, listed 1 directory`

**Files:**
- Modify: `cli/src/tui/render/tool_family.rs` (add phrasing)
- Modify: `cli/src/tui/render/mod.rs` (add ToolBatch arm in `item_to_lines`)

- [ ] **Step 1: Write the failing phrasing tests**

Append to `cli/src/tui/render/tool_family.rs`:

```rust
/// Per-spec phrasing for the collapsed-batch summary line. Returns a fragment
/// (e.g. "Read 3 files", "Searched for 2 patterns") suitable for joining with
/// commas. Returns None for unsupported names — caller falls back to the tool
/// name.
pub fn batch_fragment(name: &str, count: usize) -> Option<String> {
    let (sing, plur) = match name {
        "Read" => ("Read 1 file", "Read N files"),
        "Grep" => ("Searched for 1 pattern", "Searched for N patterns"),
        "Glob" => ("listed 1 directory", "listed N directories"),
        "LSP"  => ("queried 1 symbol", "queried N symbols"),
        _ => return None,
    };
    let s = if count == 1 { sing.to_string() } else { plur.replace("N", &count.to_string()) };
    Some(s)
}

#[cfg(test)]
mod fragment_tests {
    use super::*;

    #[test]
    fn read_fragment_singular_and_plural() {
        assert_eq!(batch_fragment("Read", 1).unwrap(), "Read 1 file");
        assert_eq!(batch_fragment("Read", 5).unwrap(), "Read 5 files");
    }

    #[test]
    fn grep_glob_fragments() {
        assert_eq!(batch_fragment("Grep", 1).unwrap(), "Searched for 1 pattern");
        assert_eq!(batch_fragment("Grep", 2).unwrap(), "Searched for 2 patterns");
        assert_eq!(batch_fragment("Glob", 1).unwrap(), "listed 1 directory");
        assert_eq!(batch_fragment("Glob", 4).unwrap(), "listed 4 directories");
    }

    #[test]
    fn unknown_tool_returns_none() {
        assert!(batch_fragment("Mystery", 3).is_none());
    }
}
```

Run: `cargo test -p super-cli --lib tui::render::tool_family -- batch_fragment`
Expected: 3 passed.

- [ ] **Step 2: Write the failing ToolBatch-rendering tests**

Add to the `tests` module in `cli/src/tui/render/mod.rs`:

```rust
    use crate::tui::transcript::BatchCall;

    fn batch_call(name: &str, id: &str) -> BatchCall {
        BatchCall {
            tool_use_id: id.into(),
            name: name.into(),
            input: serde_json::json!({"file_path": "/x"}),
            result: Some(ToolResultRender { content: "ok".into(), is_error: false }),
        }
    }

    #[test]
    fn toolbatch_renders_collapsed_summary_for_reads() {
        let item = TranscriptItem::ToolBatch {
            calls: vec![batch_call("Read", "1"), batch_call("Read", "2"), batch_call("Read", "3")],
        };
        let body = rendered_text(&item_to_lines(&item, 0));
        assert!(body.contains("  Read 3 files (ctrl+o to expand)"), "got: {body:?}");
        assert!(!body.contains("⏺"), "no ⏺ glyph on collapsed batch: {body:?}");
    }

    #[test]
    fn toolbatch_collapsed_for_single_read_uses_singular_form() {
        let item = TranscriptItem::ToolBatch { calls: vec![batch_call("Read", "1")] };
        let body = rendered_text(&item_to_lines(&item, 0));
        assert!(body.contains("  Read 1 file (ctrl+o to expand)"), "got: {body:?}");
    }

    #[test]
    fn toolbatch_mixed_grep_glob_joins_fragments_with_comma() {
        let item = TranscriptItem::ToolBatch {
            calls: vec![batch_call("Grep", "1"), batch_call("Glob", "2")],
        };
        let body = rendered_text(&item_to_lines(&item, 0));
        assert!(
            body.contains("Searched for 1 pattern, listed 1 directory (ctrl+o to expand)"),
            "got: {body:?}"
        );
    }
```

- [ ] **Step 3: Run to confirm failure**

Run: `cargo test -p super-cli --lib tui::render -- toolbatch_`
Expected: FAIL.

- [ ] **Step 4: Implement the ToolBatch arm**

Replace the stub arm added in Task 11 with:

```rust
        TranscriptItem::ToolBatch { calls } => {
            use crate::tui::render::tool_family::batch_fragment;
            // Group calls by their tool name in insertion order so we can emit
            // one fragment per kind.
            let mut order: Vec<String> = Vec::new();
            let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
            for c in calls {
                if !counts.contains_key(&c.name) {
                    order.push(c.name.clone());
                }
                *counts.entry(c.name.clone()).or_insert(0) += 1;
            }
            let fragments: Vec<String> = order.iter()
                .filter_map(|n| batch_fragment(n, counts[n]).or_else(|| Some(format!("{n} x{}", counts[n]))))
                .collect();
            let summary = format!("{} (ctrl+o to expand)", fragments.join(", "));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(summary, dim),
            ]));
        }
```

> **Note:** Bold counts are a nice-to-have but require splitting the fragment into spans. For v1, the entire fragment is one dim span. Revisit in a follow-up if visual diff vs Claude is bothersome.

- [ ] **Step 5: Run tests**

Run: `cargo test -p super-cli --lib tui::render`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tui/render/
git commit -m "feat(tui): render ToolBatch as 'Read N files'/'Searched for N' summary"
```

---

## Task 14: Wire the transformer into `flush_to_scrollback` + `render_live_tail`

Both call sites currently call `fold(...)` and iterate. Replace those with `group_tool_batches(fold(...))` so the new variant flows through.

**Files:**
- Modify: `cli/src/tui/app.rs`

- [ ] **Step 1: Apply the wiring**

In `cli/src/tui/app.rs`, find the two `fold(&self.scroll_area.events, None)` call sites (currently lines 659 and 762). Wrap each in `transcript::group_tool_batches(...)`:

```rust
        let items = crate::tui::transcript::group_tool_batches(
            fold(&self.scroll_area.events, None),
        );
```

If there isn't already a `use` for `group_tool_batches`, leave the fully-qualified path inline.

- [ ] **Step 2: Update `is_stable` to handle ToolBatch**

In `cli/src/tui/app.rs`, locate `fn is_stable` (around line 117). Replace it with:

```rust
fn is_stable(item: &TranscriptItem) -> bool {
    match item {
        TranscriptItem::User { .. } => true,
        TranscriptItem::AssistantText { complete, .. } => *complete,
        TranscriptItem::Thinking { complete, .. } => *complete,
        TranscriptItem::ToolCall { result, .. } => result.is_some(),
        TranscriptItem::ToolBatch { calls } => calls.iter().all(|c| c.result.is_some()),
        TranscriptItem::System { .. } => true,
    }
}
```

- [ ] **Step 3: Write a test for the new `is_stable` branch**

In the `#[cfg(test)] mod tests` block of `cli/src/tui/app.rs` (search for `is_stable_classifies_each_variant`), add:

```rust
    #[test]
    fn is_stable_toolbatch_requires_all_results() {
        use crate::tui::transcript::BatchCall;
        let mk = |has_result: bool| TranscriptItem::ToolBatch {
            calls: vec![BatchCall {
                tool_use_id: "1".into(), name: "Read".into(), input: serde_json::json!({}),
                result: if has_result {
                    Some(crate::tui::transcript::ToolResultRender { content: "ok".into(), is_error: false })
                } else { None },
            }],
        };
        assert!(is_stable(&mk(true)));
        assert!(!is_stable(&mk(false)));
    }
```

- [ ] **Step 4: Build + run all tests**

Run: `cargo test -p super-cli --lib`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/app.rs
git commit -m "feat(tui): plumb group_tool_batches through flush+live-tail; handle ToolBatch in is_stable"
```

---

## Task 15: Wire `show_detailed_transcript` to expand `ToolBatch`

When the global flag is on, render each `BatchCall` as an individual `ToolCall`-style block (dim gray, no `⏺` glyph per Claude's render) instead of the single summary line. Update the hint text accordingly.

**Files:**
- Modify: `cli/src/tui/render/mod.rs` (extend `item_to_lines` to accept the flag)
- Modify: `cli/src/tui/app.rs` (pass the flag, update hint text)

- [ ] **Step 1: Refactor `item_to_lines` to accept the flag**

Change the signature in `cli/src/tui/render/mod.rs`:

```rust
pub fn item_to_lines(item: &TranscriptItem, text_offset: usize, detailed: bool) -> Vec<Line<'static>> {
```

Update the `ToolBatch` arm to branch:

```rust
        TranscriptItem::ToolBatch { calls } => {
            if detailed {
                // Render each call as its own dim-gray per-call block.
                for call in calls {
                    let display = tool_display_name(&call.name, &call.input);
                    let summary = summarize_tool_call(&call.name, &call.input);
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        Span::styled("  ", dim),
                        Span::styled(display, Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD)),
                        Span::raw(" "),
                        Span::styled(summary, dim),
                    ]));
                    if let Some(r) = &call.result {
                        render_tool_result_for(&call.name, &call.input, &mut lines, r, &dim);
                    }
                }
            } else {
                // Existing collapsed summary line from Task 13.
                use crate::tui::render::tool_family::batch_fragment;
                let mut order: Vec<String> = Vec::new();
                let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
                for c in calls {
                    if !counts.contains_key(&c.name) {
                        order.push(c.name.clone());
                    }
                    *counts.entry(c.name.clone()).or_insert(0) += 1;
                }
                let fragments: Vec<String> = order.iter()
                    .filter_map(|n| batch_fragment(n, counts[n]).or_else(|| Some(format!("{n} x{}", counts[n]))))
                    .collect();
                // Collapsed line always says "to expand" — the line *is* the
                // collapsed state. The status row in render_hint says
                // "ctrl+o to collapse" when detailed mode is on.
                let summary = format!("{} (ctrl+o to expand)", fragments.join(", "));
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(summary, dim),
                ]));
            }
        }
```

- [ ] **Step 2: Update all call sites**

In `cli/src/tui/app.rs`, update the three callers of `item_to_lines` to pass the flag:

```rust
        let detailed = self.scroll_area.is_detailed_transcript();
        // ...wherever item_to_lines is called:
        let lines = item_to_lines(item, already, detailed);
```

(Look for `item_to_lines(item, already)`, `item_to_lines(&chunk_item, already)`, and the one in `render_live_tail` — three sites total.)

- [ ] **Step 3: Update existing render tests to pass the new arg**

In `cli/src/tui/render/mod.rs` tests, all calls to `item_to_lines(...)` now need the new `detailed` arg. Use this batch fix and then verify manually:

```bash
# Catches both `item_to_lines(&item, 0)` and `item_to_lines(&item, <expr>)`
perl -i -pe 's/item_to_lines\(&item,\s*([^)]+)\)/item_to_lines(\&item, $1, false)/g' cli/src/tui/render/mod.rs
```

Verify:

```bash
grep -n "item_to_lines(" cli/src/tui/render/mod.rs
```

Every call should now end with `, false)`. If any still has only two args, edit it by hand.

- [ ] **Step 4: Write a test for the detailed expansion**

Add to the `tests` module:

```rust
    #[test]
    fn toolbatch_detailed_expands_into_per_call_blocks() {
        let item = TranscriptItem::ToolBatch {
            calls: vec![batch_call("Read", "1"), batch_call("Read", "2")],
        };
        let body = rendered_text(&item_to_lines(&item, 0, true));
        // No collapsed summary line.
        assert!(!body.contains("Read 2 files (ctrl+o"), "should not show collapsed line: {body:?}");
        // Two Read entries are rendered.
        let occurrences = body.matches("Read ").count();
        assert!(occurrences >= 2, "expected >=2 'Read ' occurrences, got {occurrences} in: {body:?}");
    }
```

- [ ] **Step 5: Update the hint text in `render_hint`**

In `cli/src/tui/app.rs`, find the `render_hint` block (around line 739) and update the detailed-transcript line:

```rust
        } else if self.scroll_area.is_detailed_transcript() {
            Line::from(vec![Span::styled("  Showing detailed transcript · ctrl+o to collapse", dim)])
```

The user already had a similar line; just confirm the verb pair is `expand` (hint when collapsed mode is implicit in the batch line itself) ↔ `collapse` (status line says how to revert). The collapsed batch always says "ctrl+o to expand" — that's the per-batch affordance.

- [ ] **Step 6: Run all tests**

Run: `cargo test -p super-cli --lib`
Expected: all pass.

- [ ] **Step 7: Manual smoke test (optional)**

Build and run super:
```bash
cargo build -p super-cli && /Users/chwzr/flxkpe/superworkspace/super/.claude/worktrees/fix-provider-error-and-system-reminder/target/debug/super
```
Trigger a few parallel Read calls (e.g. "read CLAUDE.md and README.md in parallel"). Verify the collapsed line shows. Press Ctrl+O — future renders show the expanded form in the live tail.

- [ ] **Step 8: Commit**

```bash
git add cli/src/tui/render/mod.rs cli/src/tui/app.rs
git commit -m "feat(tui): expand ToolBatch into per-call blocks under detailed transcript"
```

---

## Task 16: Apply OSC8 hyperlinks to file paths in tool args

Use the `osc8_link` helper from Task 6 to wrap `file_path`-style args inside the `⏺ Update(notes.txt)` / `⏺ Write(notes.txt)` parens so modern terminals show them as clickable underlines.

**Files:**
- Modify: `cli/src/tui/render/mod.rs` (`summarize_tool_call`)

- [ ] **Step 1: Write the failing test**

Add to `tests`:

```rust
    #[test]
    fn summarize_tool_call_read_wraps_path_in_osc8_link() {
        let input = serde_json::json!({"file_path": "/tmp/notes.txt"});
        let summary = summarize_tool_call("Read", &input);
        assert!(summary.contains("\x1b]8;;file:///tmp/notes.txt"), "OSC8 open: {summary:?}");
        assert!(summary.contains("/tmp/notes.txt"), "label present: {summary:?}");
        assert!(summary.contains("\x1b]8;;\x1b\\"), "OSC8 close: {summary:?}");
    }
```

- [ ] **Step 2: Confirm failure**

Run: `cargo test -p super-cli --lib tui::render -- osc8_link`
Expected: FAIL.

- [ ] **Step 3: Update `summarize_tool_call`**

In `cli/src/tui/render/mod.rs`, modify the `Read` / `Edit` / `Write` arms of `summarize_tool_call`:

```rust
        "Read" => {
            let p = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("({})", osc8_link(p, p))
        }
        "Edit" | "Write" => {
            let p = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("({})", osc8_link(p, p))
        }
```

Leave `Task`, `Bash`, and the generic catch-all alone (they don't carry a file path).

- [ ] **Step 4: Adjust the existing test for `Read`**

The old test asserts `summarize_tool_call("Read", ...) == "(/tmp/notes.txt)"`. Update it to a substring match:

```rust
    #[test]
    fn summarize_tool_call_read_shows_file_path() {
        let input = serde_json::json!({"file_path": "/tmp/notes.txt"});
        let summary = summarize_tool_call("Read", &input);
        assert!(summary.contains("/tmp/notes.txt"), "got: {summary:?}");
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p super-cli --lib tui::render`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tui/render/mod.rs
git commit -m "feat(tui): wrap tool-arg file paths in OSC8 hyperlinks"
```

---

## Task 17: Manual tmux parity verification

Confirm visually that the rendered output matches Claude Code v2.1.143.

**Files:**
- None (verification only)

- [ ] **Step 1: Build the release binary for super**

Run: `cargo build -p super-cli`
Expected: build succeeds, no warnings introduced by this branch.

- [ ] **Step 2: Reset the parity scratch dirs**

```bash
rm -rf /tmp/parity-claude /tmp/parity-super
mkdir -p /tmp/parity-claude /tmp/parity-super
```

- [ ] **Step 3: Start both clients side-by-side**

```bash
/tmp/tmuxdrive.sh nuke || true
/tmp/tmuxdrive.sh start claude bash -c "cd /tmp/parity-claude && claude --dangerously-skip-permissions"
/tmp/tmuxdrive.sh start super  bash -c "cd /tmp/parity-super  && /Users/chwzr/flxkpe/superworkspace/super/.claude/worktrees/fix-provider-error-and-system-reminder/target/debug/super"
```

Wait for both to be ready (use `/tmp/tmuxdrive.sh see claude` and `/tmp/tmuxdrive.sh see super`).

- [ ] **Step 4: Run each scenario in both and capture**

For each prompt below, type it into both panes, hit Enter, wait for completion, capture, and diff. Use `/tmp/tmuxdrive.sh see-color <pane>` to capture colors.

| Scenario | Prompt |
| -------- | ------ |
| Single Read | `read CLAUDE.md` |
| Parallel Reads | `create a.txt b.txt c.txt with hello in each then read all three in parallel` |
| Bash success multi-line | `run: seq 1 8` |
| Bash error | `run: bash -c "echo oops >&2; exit 2"` |
| Edit | `edit CLAUDE.md to change one occurrence of the word "Plan" to "PLAN"` |
| Mixed batch + bash | `grep "foo" in this dir AND run "ls" — in parallel` |

Save each capture to `$CLAUDE_JOB_DIR`. Open the spec (`cli/docs/tool-call-render-spec.md`) and compare line-by-line.

- [ ] **Step 5: Fix any deltas found**

If any visual differences surface, either:
- Adjust the renderer to match the spec, or
- Update the spec if Claude's actual behavior contradicts a spec assumption.

Each fix is a separate small commit (do not bundle).

- [ ] **Step 6: Clean up**

```bash
/tmp/tmuxdrive.sh nuke
```

- [ ] **Step 7: Final commit (if no fixes were needed)**

```bash
git commit --allow-empty -m "test(tui): tool-call render parity verified via tmux capture"
```

---

## Out of scope (follow-up plans)

These deliberately deferred items are listed so a future plan can pick them up:

1. **Per-batch ctrl+o toggle.** Super has no in-app selection model, so the global `show_detailed_transcript` is what gates expansion. Adding a per-item focus + per-item collapsed/expanded state requires building a selection model first.
2. **`Task` (subagent) grouped render.** Claude has a separate grouping mechanism for `≥2 Task` calls in one assistant message. Super renders them per-call today.
3. **Web fetch/search collapse.** Likely belongs in `ReadSearch` per Claude, but we haven't captured the rendering yet. Hold until verified.
4. **MCP read-style tools (`mcp__*__list_*`, `mcp__*__get_*`).** Same — needs a tmux capture before adding to the classifier.
5. **Bold counts in the ToolBatch summary line.** Claude bolds the numeric in `Read **5** files`; we render the whole line as one dim span. Aesthetics only.
6. **Plans-directory aliasing (`Updated plan`).** Wire this up once super has a plan-files directory convention (out of scope for the current `/plan` work).
7. **Diff context lines.** The `similar` crate gives full line-by-line diffs but no surrounding-context window. Claude shows about 3 lines of context around each hunk; matching that needs a small post-process on the diff iterator.
