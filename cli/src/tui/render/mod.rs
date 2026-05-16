//! Stateless rendering helpers that turn legacy `Message` enums and folded
//! `TranscriptItem`s into ratatui `Line`s.
//!
//! Used by both the live region (in-flight items) and the scrollback writer
//! (completed items pushed via `Terminal::insert_before`).

pub mod diff;
pub mod tool_family;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::sdk::protocol::SystemSubtype;
use crate::tui::scroll_area::Message;
use crate::tui::transcript::{TranscriptItem, ToolResultRender};

fn user_prefix_style()      -> Style { Style::default().fg(Color::White).add_modifier(Modifier::BOLD) }
fn assistant_prefix_style() -> Style { Style::default().fg(Color::Cyan) }
fn body_style()             -> Style { Style::default().fg(Color::White) }
fn dim_style()              -> Style { Style::default().fg(Color::DarkGray) }
fn tool_style()             -> Style { Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD) }

/// Render a legacy `Message` to lines.
///
/// Leading blank-line separators are caller-aware: this function emits one
/// leading blank line before its body so consecutive items remain visually
/// spaced (matches the prior in-process render).
pub fn message_to_lines(m: &Message) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    match m {
        Message::User(text) => {
            lines.push(Line::from(""));
            for (i, body_line) in text.lines().enumerate() {
                let prefix = if i == 0 { "❯ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(prefix, user_prefix_style()),
                    Span::styled(body_line.to_string(), body_style()),
                ]));
            }
        }
        Message::Assistant(text) => {
            lines.push(Line::from(""));
            for (i, body_line) in text.lines().enumerate() {
                let prefix = if i == 0 { "⏺ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(prefix, assistant_prefix_style()),
                    Span::styled(body_line.to_string(), body_style()),
                ]));
            }
        }
        Message::ToolCall { name, input, result } => {
            use crate::tui::colors::CC_GREEN;
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("⏺ ", Style::default().fg(CC_GREEN)),
                Span::styled(name.clone(), Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" "),
                Span::styled(format!("({input})"), dim_style()),
            ]));
            if let Some(r) = result {
                for r_line in r.lines() {
                    lines.push(Line::from(vec![
                        Span::styled("  ⎿  ", dim_style()),
                        Span::styled(r_line.to_string(), dim_style()),
                    ]));
                }
            }
        }
        Message::System(text) => {
            lines.push(Line::from(""));
            for body_line in text.lines() {
                lines.push(Line::from(vec![
                    Span::styled("※ ", dim_style()),
                    Span::styled(body_line.to_string(), dim_style()),
                ]));
            }
        }
        Message::Trail(text) => {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("◈ ", assistant_prefix_style()),
                Span::styled(text.clone(), dim_style()),
            ]));
        }
        Message::Thinking => {
            lines.push(Line::from(Span::styled(
                "thinking…",
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
            )));
        }
    }
    lines
}

/// Render a single `TranscriptItem` to lines.
///
/// `text_offset` — for `AssistantText`/`Thinking` items, the caller may pass
/// the number of leading chars that have already been pushed to scrollback.
/// The returned lines render only the unflushed tail. If `text_offset == 0`
/// the rendering includes the leading prefix glyph (e.g. `⏺ `); otherwise the
/// continuation rows use indentation only (no glyph) so the visual prefix in
/// scrollback isn't duplicated in the live region.
pub fn item_to_lines(item: &TranscriptItem, text_offset: usize) -> Vec<Line<'static>> {
    let dim = dim_style();
    let mut lines: Vec<Line<'static>> = Vec::new();
    match item {
        TranscriptItem::User { text } => {
            if text_offset == 0 {
                lines.push(Line::from(""));
            }
            let body = &text[text_offset.min(text.len())..];
            for (i, body_line) in body.lines().enumerate() {
                let is_first_visual = text_offset == 0 && i == 0;
                let prefix = if is_first_visual { "❯ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(prefix, user_prefix_style()),
                    Span::styled(body_line.to_string(), body_style()),
                ]));
            }
        }
        TranscriptItem::AssistantText { text, .. } => {
            if text_offset == 0 {
                lines.push(Line::from(""));
            }
            let body = &text[text_offset.min(text.len())..];
            for (i, body_line) in body.lines().enumerate() {
                let is_first_visual = text_offset == 0 && i == 0;
                let prefix = if is_first_visual { "⏺ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(prefix, assistant_prefix_style()),
                    Span::styled(body_line.to_string(), body_style()),
                ]));
            }
            // `text.lines()` strips a trailing empty line. For an in-flight
            // partial chunk that ends mid-line (no trailing \n), we still want
            // the tail rendered. text.lines() handles that correctly. If the
            // chunk ends with \n, text.lines() will drop the trailing empty —
            // that's fine since the next chunk's leading text will be on a
            // fresh line via this same code path.
            let _ = body;
        }
        TranscriptItem::Thinking { text: _, collapsed: _, .. } => {
            if text_offset == 0 {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                "thinking…",
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
            )));
        }
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
                render_tool_result_for(name, input, &mut lines, r, &dim);
            }
        }
        TranscriptItem::System { subtype, message } => {
            lines.push(Line::from(""));
            let recap_style = Style::default().fg(Color::DarkGray);
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
    lines
}

/// Tool-call argument summary in Claude-Code's "Read 1 file" style.
pub fn summarize_tool_call(name: &str, input: &serde_json::Value) -> String {
    match name {
        "Task" => {
            let agent = input.get("subagent_type").and_then(|v| v.as_str()).unwrap_or("?");
            let desc  = input.get("description").and_then(|v| v.as_str()).unwrap_or("");
            if desc.is_empty() { format!("({agent})") } else { format!("({agent}) {desc}") }
        }
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
            if let Some((k, v)) = input.as_object().and_then(|o| o.iter().next()) {
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

    // Hunk rows.
    lines.extend(diff::render_hunks(old, new));
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
            Span::styled(
                format!("… +{} lines (ctrl+o to expand)", total - max_lines),
                Style::default().add_modifier(Modifier::DIM),
            ),
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
    let total = all.len();

    for (i, line) in all.iter().take(MAX).enumerate() {
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

/// Total visual rows a slice of lines occupies when wrapped at `width` columns.
pub fn lines_height(lines: &[Line], width: u16) -> u16 {
    lines.iter().map(|l| visual_rows(l, width)).sum()
}

fn visual_rows(line: &Line, width: u16) -> u16 {
    if width == 0 { return 1; }
    let char_count: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    if char_count == 0 { return 1; }
    ((char_count + width as usize - 1) / width as usize) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::protocol::SystemSubtype;

    fn line_to_string(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect::<String>()
    }

    fn rendered_text(lines: &[Line]) -> String {
        lines.iter().map(|l| line_to_string(l)).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn summarize_tool_call_task_shows_description() {
        let input = serde_json::json!({
            "description": "find auth code",
            "prompt": "search the codebase for auth handlers",
            "subagent_type": "Explore",
        });
        let summary = summarize_tool_call("Task", &input);
        assert!(summary.contains("Explore"), "got: {summary}");
        assert!(summary.contains("find auth code"), "got: {summary}");
    }

    #[test]
    fn summarize_tool_call_read_shows_file_path() {
        let input = serde_json::json!({"file_path": "/tmp/notes.txt"});
        assert_eq!(summarize_tool_call("Read", &input), "(/tmp/notes.txt)");
    }

    #[test]
    fn item_to_lines_assistant_text_full_block() {
        let item = TranscriptItem::AssistantText {
            text: "first line\nsecond line".into(),
            complete: true,
        };
        let lines = item_to_lines(&item, 0);
        let body = rendered_text(&lines);
        // Leading blank, prefix on first body row, indent continuation.
        assert!(body.contains("⏺ first line"), "got: {body:?}");
        assert!(body.contains("  second line"), "got: {body:?}");
    }

    #[test]
    fn item_to_lines_assistant_text_continuation_skips_blank_and_prefix() {
        let item = TranscriptItem::AssistantText {
            text: "first line\nsecond line".into(),
            complete: true,
        };
        // Pretend "first line\n" (11 chars) was already flushed.
        let lines = item_to_lines(&item, "first line\n".len());
        let body = rendered_text(&lines);
        assert!(!body.contains("⏺"), "continuation should not have ⏺ prefix: {body:?}");
        // No leading blank line on a continuation.
        assert!(!body.starts_with('\n'), "continuation should not lead with blank line: {body:?}");
        assert!(body.contains("  second line"), "got: {body:?}");
    }

    #[test]
    fn item_to_lines_thinking_renders_collapsed_placeholder() {
        let item = TranscriptItem::Thinking {
            text: "long chain of thought here".into(),
            collapsed: true,
            elapsed_ms: 0,
            complete: true,
        };
        let lines = item_to_lines(&item, 0);
        let body = rendered_text(&lines);
        assert!(body.contains("thinking…"), "got: {body:?}");
        assert!(!body.contains("long chain"), "raw thinking text must not leak: {body:?}");
    }

    #[test]
    fn item_to_lines_system_post_turn_summary_uses_recap_prefix() {
        let item = TranscriptItem::System {
            subtype: SystemSubtype::PostTurnSummary,
            message: "summary of prior turn".into(),
        };
        let lines = item_to_lines(&item, 0);
        let body = rendered_text(&lines);
        assert!(body.contains("※ recap: summary of prior turn"), "got: {body:?}");
    }

    #[test]
    fn message_to_lines_system_renders_with_marker() {
        let lines = message_to_lines(&Message::System("hello".into()));
        let body = rendered_text(&lines);
        assert!(body.contains("※ hello"), "got: {body:?}");
    }

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

    #[test]
    fn lines_height_wraps_long_text() {
        // 30 chars wide; the body line is "⏺ " (2) + "hello".
        // Render an assistant text and verify lines_height counts visual rows.
        let item = TranscriptItem::AssistantText {
            text: "hello".into(),
            complete: true,
        };
        let lines = item_to_lines(&item, 0);
        // 1 blank + 1 body = 2 rows at width 80.
        assert_eq!(lines_height(&lines, 80), 2);
    }

    use crate::tui::colors::{CC_GREEN, CC_ORANGE};

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
}
