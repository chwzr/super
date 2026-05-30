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
use crate::tui::transcript::{ToolResultRender, TranscriptItem};

fn user_prefix_style() -> Style {
    Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
}
fn assistant_prefix_style() -> Style {
    Style::default().fg(Color::Cyan)
}
fn body_style() -> Style {
    Style::default().fg(Color::White)
}
fn dim_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

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
        Message::ToolCall {
            name,
            input,
            result,
        } => {
            use crate::tui::colors::CC_GREEN;
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("⏺ ", Style::default().fg(CC_GREEN)),
                Span::styled(name.clone(), Style::default().add_modifier(Modifier::BOLD)),
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
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
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
pub fn item_to_lines(
    item: &TranscriptItem,
    text_offset: usize,
    detailed: bool,
) -> Vec<Line<'static>> {
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
        TranscriptItem::Thinking {
            text: _,
            collapsed: _,
            ..
        } => {
            if text_offset == 0 {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                "thinking…",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
        }
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
            let has_terminal = !matches!(result_spec, Some(shared::RenderSpec::Nothing))
                && (result_spec.is_some() || rejected_spec.is_some() || error_spec.is_some());
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
                // Legacy raw-field rendering (used by tools that haven't
                // migrated to specs yet: Read, Grep, Glob).
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
        TranscriptItem::System { subtype, message } => {
            lines.push(Line::from(""));
            let recap_style = Style::default().fg(Color::DarkGray);
            let error_style = Style::default().fg(Color::Red);
            let (prefix, style) = match subtype {
                SystemSubtype::PostTurnSummary => ("※ recap: ", recap_style),
                SystemSubtype::CompactBoundary => ("※ ", recap_style),
                SystemSubtype::Error => ("⚠ ", error_style),
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
        TranscriptItem::ToolBatch { calls } => {
            use crate::tui::render::tool_family::batch_fragment;
            if detailed {
                // Render each call as its own dim-gray per-call block.
                for call in calls {
                    let display = tool_display_name(&call.name, &call.input);
                    let summary = summarize_tool_call(&call.name, &call.input);
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        Span::styled("  ", dim),
                        Span::styled(
                            display,
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(summary, dim),
                    ]));
                    if let Some(r) = &call.result {
                        render_tool_result_for(&call.name, &call.input, &mut lines, r, &dim);
                    }
                }
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled("(ctrl+o to collapse)".to_string(), dim),
                ]));
            } else {
                // Group calls by their tool name in insertion order so we can emit
                // one fragment per kind.
                let mut order: Vec<String> = Vec::new();
                let mut counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                for c in calls {
                    if !counts.contains_key(&c.name) {
                        order.push(c.name.clone());
                    }
                    *counts.entry(c.name.clone()).or_insert(0) += 1;
                }
                let mut spans: Vec<Span<'static>> = vec![Span::raw("  ")];
                for (i, name) in order.iter().enumerate() {
                    if i > 0 {
                        spans.push(Span::styled(", ".to_string(), dim));
                    }
                    spans.extend(batch_fragment(name, counts[name]));
                }
                spans.push(Span::styled(" (ctrl+o to expand)".to_string(), dim));
                lines.push(Line::from(""));
                lines.push(Line::from(spans));
            }
        }
        TranscriptItem::Render { spec } => {
            lines.extend(render_spec(spec, detailed));
        }
    }
    lines
}

/// Tool-call argument summary in Claude-Code's "Read 1 file" style.
pub fn summarize_tool_call(name: &str, input: &serde_json::Value) -> String {
    match name {
        "Task" => {
            let agent = input
                .get("subagent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let desc = input
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if desc.is_empty() {
                format!("({agent})")
            } else {
                format!("({agent}) {desc}")
            }
        }
        "Read" => {
            let p = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("({})", osc8_link(p, p))
        }
        "Bash" => {
            let c = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let trimmed: String = c.lines().next().unwrap_or("").chars().take(80).collect();
            format!("({trimmed})")
        }
        "Edit" | "Write" => {
            let p = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("({})", osc8_link(p, p))
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
            let old = input
                .get("old_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
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
/// that don't support OSC8 render the label without the underline. Relative
/// paths still get a `file://` URI prefix — modern terminals resolve them
/// against their own cwd.
pub fn osc8_link(path: &str, label: &str) -> String {
    format!("\x1b]8;;file://{path}\x1b\\{label}\x1b]8;;\x1b\\")
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
        "Write" => render_write_result(lines, input, dim),
        _ => render_generic_result(lines, r, dim),
    }
}

fn render_edit_result(
    lines: &mut Vec<Line<'static>>,
    input: &serde_json::Value,
    _r: &ToolResultRender,
    dim: &Style,
) {
    let old = input
        .get("old_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let new = input
        .get("new_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let counts = diff::count_changes(old, new);

    // Summary row: `  ⎿  Added N line[s], removed M line[s]`
    let mut summary: Vec<Span<'static>> = vec![Span::styled("  ⎿  ", *dim)];
    summary.extend(diff::summary_spans(counts));
    lines.push(Line::from(summary));

    // Hunk rows.
    lines.extend(diff::render_hunks(old, new));
}

fn render_write_result(lines: &mut Vec<Line<'static>>, input: &serde_json::Value, dim: &Style) {
    let path = input
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let total_lines = content.lines().count();
    let plural = if total_lines == 1 { "line" } else { "lines" };

    let mut summary: Vec<Span<'static>> = vec![Span::styled("  ⎿  ", *dim)];
    summary.push(Span::raw("Wrote "));
    summary.push(Span::styled(
        total_lines.to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    ));
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
    let body_color = if r.is_error {
        Style::default().fg(CC_ORANGE)
    } else {
        Style::default()
    };
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
    if width == 0 {
        return 1;
    }
    let char_count: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    if char_count == 0 {
        return 1;
    }
    char_count.div_ceil(width as usize) as u16
}

/// Map a portable `TextStyle` hint to a concrete ratatui `Style` using the
/// CLI palette in `colors.rs`.
fn text_style(style: shared::TextStyle) -> Style {
    use shared::TextStyle;
    match style {
        TextStyle::Plain   => Style::default().fg(Color::White),
        TextStyle::Dim     => Style::default().fg(Color::DarkGray),
        TextStyle::Error   => Style::default().fg(Color::Red),
        TextStyle::Success => Style::default().fg(Color::Indexed(114)),
        TextStyle::Warn    => Style::default().fg(Color::Indexed(214)),
        TextStyle::Strong  => Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    }
}

/// True when `s` has a path separator or a single dotted extension (e.g.
/// `foo.rs`, `notes.md`). Used to decide whether to OSC8-link a target.
fn looks_like_path(s: &str) -> bool {
    s.contains('/')
        || s.contains('\\')
        || s.rsplit('.')
            .next()
            .is_some_and(|ext| !ext.is_empty() && ext.len() <= 5 && s.len() > ext.len() + 1)
}

/// Format a `Tag` into the bracket form shown inline in tool headers.
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

/// Reconstruct synthetic `old` / `new` text from a `RenderSpec::Diff`'s hunks
/// so we can reuse `diff::render_hunks` and `diff::count_changes`.
fn reconstruct_old_new(hunks: &[shared::DiffHunk]) -> (String, String) {
    let mut old = String::new();
    let mut new = String::new();
    for hunk in hunks {
        for l in &hunk.lines {
            match l {
                shared::DiffLine::Context { line } => {
                    old.push_str(line);
                    if !line.ends_with('\n') {
                        old.push('\n');
                    }
                    new.push_str(line);
                    if !line.ends_with('\n') {
                        new.push('\n');
                    }
                }
                shared::DiffLine::Remove { line } => {
                    old.push_str(line);
                    if !line.ends_with('\n') {
                        old.push('\n');
                    }
                }
                shared::DiffLine::Add { line } => {
                    new.push_str(line);
                    if !line.ends_with('\n') {
                        new.push('\n');
                    }
                }
            }
        }
    }
    (old, new)
}

/// Dispatcher: turns a `RenderSpec` into TUI lines.
pub fn render_spec(spec: &shared::RenderSpec, detailed: bool) -> Vec<Line<'static>> {
    match spec {
        shared::RenderSpec::Nothing => Vec::new(),
        shared::RenderSpec::Text { body, style } => {
            let s = text_style(*style);
            body.lines()
                .map(|l| Line::from(Span::styled(l.to_string(), s)))
                .collect()
        }
        shared::RenderSpec::Header { verb, target, tag } => {
            let mut spans: Vec<Span<'static>> = Vec::new();
            spans.push(Span::styled(
                "⏺ ",
                Style::default().fg(Color::Indexed(114)),
            ));
            spans.push(Span::styled(
                verb.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ));
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
        shared::RenderSpec::Code {
            language: _,
            body,
            truncated,
        } => {
            let dim = dim_style();
            let mut out: Vec<Line<'static>> = Vec::new();
            for (i, l) in body.lines().enumerate() {
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
                        "\u{2026} (ctrl+o to expand)".to_string(),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ]));
            }
            out
        }
        shared::RenderSpec::Diff {
            file_path: _,
            hunks,
        } => {
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
        shared::RenderSpec::PathList {
            entries,
            total: _,
            truncated,
        } => {
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
                        format!("\u{2026} +{remaining} paths (ctrl+o to expand)"),
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
        shared::RenderSpec::Status { state, message } => {
            use shared::StatusState;
            let (glyph, color) = match state {
                StatusState::Queued     => ("…", Color::DarkGray),
                StatusState::InProgress => ("›", Color::DarkGray),
                StatusState::Success    => ("✓", Color::Indexed(114)),
                StatusState::Error      => ("✗", Color::Red),
                StatusState::Rejected   => ("⚠", Color::Indexed(211)),
            };
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
                child_renders.into_iter().flat_map(|child| {
                    child.into_iter().map(|line| {
                        let mut spans = vec![Span::styled("│ ", dim)];
                        spans.extend(line.spans);
                        Line::from(spans)
                    })
                }).collect()
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::protocol::SystemSubtype;

    fn line_to_string(line: &Line) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    fn rendered_text(lines: &[Line]) -> String {
        lines
            .iter()
            .map(|l| line_to_string(l))
            .collect::<Vec<_>>()
            .join("\n")
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
        let summary = summarize_tool_call("Read", &input);
        assert!(summary.contains("/tmp/notes.txt"), "got: {summary:?}");
    }

    #[test]
    fn summarize_tool_call_read_wraps_path_in_osc8_link() {
        let input = serde_json::json!({"file_path": "/tmp/notes.txt"});
        let summary = summarize_tool_call("Read", &input);
        assert!(
            summary.contains("\x1b]8;;file:///tmp/notes.txt"),
            "OSC8 open: {summary:?}"
        );
        assert!(
            summary.contains("/tmp/notes.txt"),
            "label present: {summary:?}"
        );
        assert!(
            summary.contains("\x1b]8;;\x1b\\"),
            "OSC8 close: {summary:?}"
        );
    }

    #[test]
    fn item_to_lines_assistant_text_full_block() {
        let item = TranscriptItem::AssistantText {
            text: "first line\nsecond line".into(),
            complete: true,
        };
        let lines = item_to_lines(&item, 0, false);
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
        let lines = item_to_lines(&item, "first line\n".len(), false);
        let body = rendered_text(&lines);
        assert!(
            !body.contains("⏺"),
            "continuation should not have ⏺ prefix: {body:?}"
        );
        // No leading blank line on a continuation.
        assert!(
            !body.starts_with('\n'),
            "continuation should not lead with blank line: {body:?}"
        );
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
        let lines = item_to_lines(&item, 0, false);
        let body = rendered_text(&lines);
        assert!(body.contains("thinking…"), "got: {body:?}");
        assert!(
            !body.contains("long chain"),
            "raw thinking text must not leak: {body:?}"
        );
    }

    #[test]
    fn item_to_lines_system_post_turn_summary_uses_recap_prefix() {
        let item = TranscriptItem::System {
            subtype: SystemSubtype::PostTurnSummary,
            message: "summary of prior turn".into(),
        };
        let lines = item_to_lines(&item, 0, false);
        let body = rendered_text(&lines);
        assert!(
            body.contains("※ recap: summary of prior turn"),
            "got: {body:?}"
        );
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
        let lines = item_to_lines(&item, 0, false);
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
            result: Some(ToolResultRender {
                content: "hi".into(),
                is_error: false,
            }),
            elapsed_ms: 0,
            message_spec: None,
            tag_spec: None,
            progress_specs: Vec::new(),
            queued_spec: None,
            result_spec: None,
            rejected_spec: None,
            error_spec: None,
        };
        let lines = item_to_lines(&item, 0, false);
        // Find the line whose first span is the `⏺ ` prefix.
        let prefix = lines
            .iter()
            .find(|l| {
                l.spans
                    .first()
                    .map(|s| s.content.contains('⏺'))
                    .unwrap_or(false)
            })
            .expect("⏺ prefix line");
        assert_eq!(first_span_color(prefix), Some(CC_GREEN));
    }

    #[test]
    fn tool_call_prefix_is_orange_on_error() {
        let item = TranscriptItem::ToolCall {
            tool_use_id: "tu1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "false"}),
            result: Some(ToolResultRender {
                content: "Error: Exit code 1".into(),
                is_error: true,
            }),
            elapsed_ms: 0,
            message_spec: None,
            tag_spec: None,
            progress_specs: Vec::new(),
            queued_spec: None,
            result_spec: None,
            rejected_spec: None,
            error_spec: None,
        };
        let lines = item_to_lines(&item, 0, false);
        let prefix = lines
            .iter()
            .find(|l| {
                l.spans
                    .first()
                    .map(|s| s.content.contains('⏺'))
                    .unwrap_or(false)
            })
            .expect("⏺ prefix line");
        assert_eq!(first_span_color(prefix), Some(CC_ORANGE));
    }

    #[test]
    fn osc8_link_wraps_label_with_escape_sequence() {
        let s = osc8_link("/abs/path.txt", "path.txt");
        assert!(
            s.contains("\x1b]8;;file:///abs/path.txt\x1b\\"),
            "got: {s:?}"
        );
        assert!(s.contains("path.txt"));
        assert!(s.ends_with("\x1b]8;;\x1b\\"));
    }

    use crate::tui::transcript::BatchCall;

    fn batch_call(name: &str, id: &str) -> BatchCall {
        BatchCall {
            tool_use_id: id.into(),
            name: name.into(),
            input: serde_json::json!({"file_path": "/x"}),
            result: Some(ToolResultRender {
                content: "ok".into(),
                is_error: false,
            }),
        }
    }

    #[test]
    fn toolbatch_renders_collapsed_summary_for_reads() {
        let item = TranscriptItem::ToolBatch {
            calls: vec![
                batch_call("Read", "1"),
                batch_call("Read", "2"),
                batch_call("Read", "3"),
            ],
        };
        let body = rendered_text(&item_to_lines(&item, 0, false));
        assert!(
            body.contains("  Read 3 files (ctrl+o to expand)"),
            "got: {body:?}"
        );
        assert!(
            !body.contains("⏺"),
            "no ⏺ glyph on collapsed batch: {body:?}"
        );
    }

    #[test]
    fn toolbatch_collapsed_for_single_read_uses_singular_form() {
        let item = TranscriptItem::ToolBatch {
            calls: vec![batch_call("Read", "1")],
        };
        let body = rendered_text(&item_to_lines(&item, 0, false));
        assert!(
            body.contains("  Read 1 file (ctrl+o to expand)"),
            "got: {body:?}"
        );
    }

    #[test]
    fn toolbatch_mixed_grep_glob_joins_fragments_with_comma() {
        let item = TranscriptItem::ToolBatch {
            calls: vec![batch_call("Grep", "1"), batch_call("Glob", "2")],
        };
        let body = rendered_text(&item_to_lines(&item, 0, false));
        assert!(
            body.contains("Searched for 1 pattern, listed 1 directory (ctrl+o to expand)"),
            "got: {body:?}"
        );
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
            message_spec: None,
            tag_spec: None,
            progress_specs: Vec::new(),
            queued_spec: None,
            result_spec: None,
            rejected_spec: None,
            error_spec: None,
        };
        let lines = item_to_lines(&item, 0, false);
        let body = rendered_text(&lines);
        assert!(
            body.contains("  ⎿  1"),
            "first output line under corner: {body:?}"
        );
        assert!(body.contains("     2"), "second line aligned: {body:?}");
        assert!(body.contains("     3"), "third line aligned: {body:?}");
        assert!(
            body.contains("… +5 lines (ctrl+o to expand)"),
            "ellipsis present: {body:?}"
        );
        assert!(
            !body.contains("\n4\n") && !body.contains("     4"),
            "line 4 must be hidden: {body:?}"
        );
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
            message_spec: None,
            tag_spec: None,
            progress_specs: Vec::new(),
            queued_spec: None,
            result_spec: None,
            rejected_spec: None,
            error_spec: None,
        };
        let body = rendered_text(&item_to_lines(&item, 0, false));
        assert!(body.contains("  ⎿  1"));
        assert!(body.contains("     2"));
        assert!(body.contains("     3"));
        assert!(
            !body.contains("ctrl+o"),
            "no expand hint when nothing truncated: {body:?}"
        );
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
            message_spec: None,
            tag_spec: None,
            progress_specs: Vec::new(),
            queued_spec: None,
            result_spec: None,
            rejected_spec: None,
            error_spec: None,
        };
        let body = rendered_text(&item_to_lines(&item, 0, false));
        assert!(body.contains("Update"), "display name: {body:?}");
        assert!(
            body.contains("Added 1 line, removed 1 line"),
            "summary: {body:?}"
        );
        assert!(body.contains(" 1 -hello"), "removed hunk: {body:?}");
        assert!(body.contains(" 1 +hi"), "added hunk: {body:?}");
        assert!(
            !body.contains("Successfully replaced"),
            "raw result text should not leak: {body:?}"
        );
    }

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
            message_spec: None,
            tag_spec: None,
            progress_specs: Vec::new(),
            queued_spec: None,
            result_spec: None,
            rejected_spec: None,
            error_spec: None,
        };
        let body = rendered_text(&item_to_lines(&item, 0, false));
        assert!(body.contains("Write"), "display name: {body:?}");
        assert!(
            body.contains("Wrote 3 lines to /tmp/notes.txt"),
            "summary: {body:?}"
        );
        assert!(body.contains(" 1 alpha"), "numbered line 1: {body:?}");
        assert!(body.contains(" 2 beta"), "numbered line 2: {body:?}");
        assert!(body.contains(" 3 gamma"), "numbered line 3: {body:?}");
        assert!(
            !body.contains("Successfully wrote"),
            "raw result text should not leak: {body:?}"
        );
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
            result: Some(ToolResultRender {
                content: "ok".into(),
                is_error: false,
            }),
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
        assert!(body.contains("Create"), "display name: {body:?}");
        assert!(body.contains("Added 2 lines"), "summary: {body:?}");
        assert!(
            !body.contains("removed"),
            "no removed phrase when no removals: {body:?}"
        );
    }

    #[test]
    fn toolbatch_detailed_expands_into_per_call_blocks() {
        let item = TranscriptItem::ToolBatch {
            calls: vec![batch_call("Read", "1"), batch_call("Read", "2")],
        };
        let body = rendered_text(&item_to_lines(&item, 0, true));
        // No collapsed summary line.
        assert!(
            !body.contains("Read 2 files (ctrl+o"),
            "should not show collapsed line: {body:?}"
        );
        // Two Read entries are rendered (label "Read" followed immediately by "(path)").
        let occurrences = body.matches("Read(").count();
        assert!(
            occurrences >= 2,
            "expected >=2 'Read(' occurrences, got {occurrences} in: {body:?}"
        );
    }

    #[test]
    fn item_to_lines_tool_call_uses_result_spec_when_present() {
        let item = TranscriptItem::ToolCall {
            tool_use_id: "tu1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "echo hi"}),
            result: Some(ToolResultRender {
                content: "hi".into(),
                is_error: false,
            }),
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
            result: Some(ToolResultRender {
                content: "hi".into(),
                is_error: false,
            }),
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
}

#[cfg(test)]
mod render_spec_tests {
    use super::*;
    use shared::RenderSpec;

    fn line_text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn nothing_renders_empty_vec() {
        let lines = render_spec(&RenderSpec::Nothing, false);
        assert!(lines.is_empty());
    }

    // ── Header ────────────────────────────────────────────────────────

    #[test]
    fn header_renders_verb_and_target() {
        let spec = RenderSpec::Header {
            verb: "Reading".into(),
            target: Some("src/foo.rs".into()),
            tag: None,
        };
        let lines = render_spec(&spec, false);
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
        let lines = render_spec(&spec, false);
        let body = line_text(&lines[1]);
        assert!(
            body.contains("\x1b]8;;file:///abs/path.txt"),
            "OSC8 open: {body:?}"
        );
    }

    #[test]
    fn header_renders_tag_inline_in_dim_brackets() {
        let spec = RenderSpec::Header {
            verb: "Bash".into(),
            target: Some("ls".into()),
            tag: Some(shared::Tag::Timeout { ms: 30000 }),
        };
        let lines = render_spec(&spec, false);
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
        let lines = render_spec(&spec, false);
        let body = line_text(&lines[1]);
        assert!(body.contains("[truncated]"), "got: {body:?}");
    }

    // ── Code ────────────────────────────────────────────────────────────

    #[test]
    fn code_renders_body_with_corner_prefix() {
        let spec = RenderSpec::Code {
            language: None,
            body: "line1\nline2".into(),
            truncated: false,
        };
        let lines = render_spec(&spec, false);
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
        let lines = render_spec(&spec, false);
        let body: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(body.contains("(ctrl+o to expand)"), "got: {body:?}");
    }

    // ── Diff ────────────────────────────────────────────────────────────

    #[test]
    fn diff_renders_summary_and_hunks() {
        let spec = RenderSpec::Diff {
            file_path: "/tmp/a".into(),
            hunks: vec![shared::DiffHunk {
                old_start: 1,
                new_start: 1,
                lines: vec![
                    shared::DiffLine::Remove {
                        line: "old\n".into(),
                    },
                    shared::DiffLine::Add {
                        line: "new\n".into(),
                    },
                ],
            }],
        };
        let lines = render_spec(&spec, false);
        let body: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            body.contains("Added 1 line, removed 1 line"),
            "summary: {body:?}"
        );
        assert!(body.contains("-old"), "removed hunk: {body:?}");
        assert!(body.contains("+new"), "added hunk: {body:?}");
    }

    // ── PathList ────────────────────────────────────────────────────────

    #[test]
    fn path_list_renders_entries_with_optional_line_and_preview() {
        let spec = RenderSpec::PathList {
            entries: vec![
                shared::PathEntry {
                    path: std::path::PathBuf::from("/a/b.txt"),
                    line: Some(42),
                    preview: Some("fn foo".into()),
                },
                shared::PathEntry {
                    path: std::path::PathBuf::from("/c/d.txt"),
                    line: None,
                    preview: None,
                },
            ],
            total: 2,
            truncated: false,
        };
        let lines = render_spec(&spec, false);
        let body: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(body.contains("/a/b.txt:42"), "with line: {body:?}");
        assert!(body.contains("fn foo"), "preview: {body:?}");
        assert!(body.contains("/c/d.txt"), "no-line entry: {body:?}");
    }

    // ── KeyValues ───────────────────────────────────────────────────────

    #[test]
    fn key_values_renders_each_row_as_key_value_pairs() {
        let spec = RenderSpec::KeyValues {
            rows: vec![
                ("model".into(), "claude-opus-4-7".into()),
                ("tokens".into(), "1234".into()),
            ],
        };
        let lines = render_spec(&spec, false);
        let body: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(body.contains("model: claude-opus-4-7"), "got: {body:?}");
        assert!(body.contains("tokens: 1234"), "got: {body:?}");
    }

    #[test]
    fn text_plain_renders_each_body_line() {
        let spec = RenderSpec::Text {
            body: "alpha\nbeta".into(),
            style: shared::TextStyle::Plain,
        };
        let lines = render_spec(&spec, false);
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
        let lines = render_spec(&spec, false);
        let color = lines[0].spans.first().and_then(|s| s.style.fg);
        assert_eq!(color, Some(Color::DarkGray));
    }

    #[test]
    fn text_error_carries_red_color() {
        let spec = RenderSpec::Text {
            body: "boom".into(),
            style: shared::TextStyle::Error,
        };
        let lines = render_spec(&spec, false);
        let color = lines[0].spans.first().and_then(|s| s.style.fg);
        assert_eq!(color, Some(Color::Red));
    }

    // ── Status ──────────────────────────────────────────────────────────

    #[test]
    fn status_success_glyph_is_check() {
        let spec = RenderSpec::Status {
            state: shared::StatusState::Success,
            message: Some("done".into()),
        };
        let lines = render_spec(&spec, false);
        assert_eq!(lines.len(), 1);
        let glyph = lines[0].spans.first().unwrap();
        assert!(glyph.content.contains('✓'), "glyph: {:?}", glyph.content);
    }

    #[test]
    fn status_error_glyph_is_cross() {
        let spec = RenderSpec::Status {
            state: shared::StatusState::Error,
            message: Some("nope".into()),
        };
        let lines = render_spec(&spec, false);
        let glyph = lines[0].spans.first().unwrap();
        assert!(glyph.content.contains('✗'));
    }

    // ── Group ───────────────────────────────────────────────────────────

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

    // ── Row ─────────────────────────────────────────────────────────────

    #[test]
    fn row_joins_single_line_children_horizontally() {
        let spec = RenderSpec::Row {
            children: vec![
                RenderSpec::Text { body: "L".into(), style: shared::TextStyle::Plain },
                RenderSpec::Text { body: "R".into(), style: shared::TextStyle::Plain },
            ],
        };
        let lines = render_spec(&spec, false);
        assert_eq!(lines.len(), 1);
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
        for line in &lines {
            assert!(
                line.spans.first().map(|s| s.content.starts_with('│')).unwrap_or(false),
                "line missing rule: {line:?}"
            );
        }
    }

    // ── Collapsible ─────────────────────────────────────────────────────

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

    // ── Interactive ─────────────────────────────────────────────────────

    #[test]
    fn interactive_renders_awaiting_input_placeholder() {
        let spec = RenderSpec::Interactive {
            widget: shared::InteractiveWidget::MultiQuestion { questions: vec![] },
            response_schema: serde_json::json!({}),
        };
        let body = line_text(&render_spec(&spec, false)[0]);
        assert!(body.contains("awaiting input"), "got: {body:?}");
    }

    #[test]
    fn group_of_status_and_diff_renders_in_order() {
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
                            shared::DiffLine::Remove {
                                line: "old\n".into(),
                            },
                            shared::DiffLine::Add {
                                line: "new\n".into(),
                            },
                        ],
                    }],
                },
            ],
        };
        let body: String = render_spec(&spec, false)
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(body.contains('\u{2713}'), "status glyph: {body:?}");
        assert!(
            body.contains("Updated /tmp/a.txt"),
            "status msg: {body:?}"
        );
        assert!(
            body.contains("Added 1 line, removed 1 line"),
            "diff summary: {body:?}"
        );
        assert!(body.contains("-old"), "removed hunk: {body:?}");
        assert!(body.contains("+new"), "added hunk: {body:?}");
        // Status comes before the diff summary in rendered order.
        let status_pos = body.find("Updated").unwrap();
        let diff_pos = body.find("Added").unwrap();
        assert!(status_pos < diff_pos, "order: {body:?}");
    }
}
