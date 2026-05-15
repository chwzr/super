use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use crate::sdk::protocol::{BusMessage, SystemSubtype};
use crate::tui::transcript::{fold, TranscriptItem, ToolResultRender};

/// Legacy in-TUI message type. Will be removed in Task 13 once every
/// call site is on `push_event`. For now we keep it so commands/dispatch.rs,
/// conversation/compaction.rs, and state/store.rs keep compiling.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum Message {
    User(String),
    Assistant(String),
    ToolCall {
        name: String,
        input: String,
        result: Option<String>,
    },
    System(String),
    Trail(String),
    Thinking,
}

pub struct ScrollArea {
    /// Legacy path — still used by some call sites until Task 10 lands.
    pub messages: Vec<Message>,
    /// New path — bus events folded into TranscriptItems at render time.
    pub events: Vec<BusMessage>,
    /// Visual rows from top (what ratatui's Paragraph::scroll expects).
    scroll_offset: u16,
    /// When true, render() pins scroll_offset to max so new content is always visible.
    stick_to_bottom: bool,
    /// Last rendered area height — used by scroll_up/down to scroll by page.
    last_area_height: u16,
    pub show_detailed_transcript: bool,
}

impl ScrollArea {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            events: Vec::new(),
            scroll_offset: 0,
            stick_to_bottom: true,
            last_area_height: 24,
            show_detailed_transcript: false,
        }
    }

    pub fn toggle_detailed_transcript(&mut self) {
        self.show_detailed_transcript = !self.show_detailed_transcript;
    }

    pub fn is_detailed_transcript(&self) -> bool {
        self.show_detailed_transcript
    }

    /// Legacy push — pushes onto the `messages` vec rendered before the
    /// fold-based transcript. Kept for back-compat; will be removed once
    /// every call site is on `push_event`.
    #[allow(dead_code)]
    pub fn push(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    /// New push — append a bus event. Rendering folds the whole event vec
    /// into TranscriptItems at frame time.
    pub fn push_event(&mut self, ev: BusMessage) {
        self.events.push(ev);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.events.clear();
        self.scroll_offset = 0;
    }

    pub fn scroll_up(&mut self) {
        let page = self.last_area_height.saturating_sub(2).max(1);
        self.scroll_offset = self.scroll_offset.saturating_sub(page);
        self.stick_to_bottom = false;
    }

    pub fn scroll_down(&mut self) {
        let page = self.last_area_height.saturating_sub(2).max(1);
        self.scroll_offset = self.scroll_offset.saturating_add(page);
        // render() will clamp to max_offset and re-enable stick_to_bottom if needed
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let user_prefix_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
        let assistant_prefix_style = Style::default().fg(Color::Cyan);
        let body_style = Style::default().fg(Color::White);
        let dim = Style::default().fg(Color::DarkGray);
        let recap_style = Style::default().fg(Color::DarkGray);
        let tool_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

        let mut lines: Vec<Line> = Vec::new();

        // Render legacy messages first (chronological).
        for m in &self.messages {
            match m {
                Message::User(text) => {
                    lines.push(Line::from(""));
                    for (i, body_line) in text.lines().enumerate() {
                        let prefix = if i == 0 { "❯ " } else { "  " };
                        lines.push(Line::from(vec![
                            Span::styled(prefix, user_prefix_style),
                            Span::styled(body_line.to_string(), body_style),
                        ]));
                    }
                }
                Message::Assistant(text) => {
                    lines.push(Line::from(""));
                    for (i, body_line) in text.lines().enumerate() {
                        let prefix = if i == 0 { "⏺ " } else { "  " };
                        lines.push(Line::from(vec![
                            Span::styled(prefix, assistant_prefix_style),
                            Span::styled(body_line.to_string(), body_style),
                        ]));
                    }
                }
                Message::ToolCall { name, input, result } => {
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        Span::styled("⏺ ", assistant_prefix_style),
                        Span::styled(name.clone(), tool_style),
                        Span::raw(" "),
                        Span::styled(format!("({input})"), dim),
                    ]));
                    if let Some(r) = result {
                        for r_line in r.lines() {
                            lines.push(Line::from(vec![
                                Span::styled("  ⎿  ", dim),
                                Span::styled(r_line.to_string(), dim),
                            ]));
                        }
                    }
                }
                Message::System(text) => {
                    lines.push(Line::from(""));
                    for body_line in text.lines() {
                        lines.push(Line::from(vec![
                            Span::styled("※ ", dim),
                            Span::styled(body_line.to_string(), dim),
                        ]));
                    }
                }
                Message::Trail(text) => {
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        Span::styled("◈ ", assistant_prefix_style),
                        Span::styled(text.clone(), dim),
                    ]));
                }
                Message::Thinking => {
                    lines.push(Line::from(Span::styled(
                        "thinking…",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                }
            }
        }

        // Then render the folded transcript from bus events.
        let items = fold(&self.events, None);
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
                TranscriptItem::ToolCall { tool_use_id, name, input, result, .. } => {
                    lines.push(Line::from(""));
                    let summary = summarize_tool_call(&name, &input);
                    lines.push(Line::from(vec![
                        Span::styled("⏺ ", assistant_prefix_style),
                        Span::styled(name.clone(), tool_style),
                        Span::raw(" "),
                        Span::styled(summary, dim),
                    ]));
                    if name == "Task" {
                        if self.show_detailed_transcript {
                            // Expanded: render the child transcript indented under the Task header.
                            let child_items = fold(&self.events, Some(&tool_use_id));
                            for child in &child_items {
                                render_child_indented(&mut lines, child, dim);
                            }
                        }
                        // Done summary (only if child has emitted Result)
                        if let Some(summary) = task_done_summary(&self.events, &tool_use_id) {
                            lines.push(Line::from(vec![
                                Span::styled("  ⎿  ", dim),
                                Span::styled(summary, dim),
                            ]));
                        }
                        if !self.show_detailed_transcript {
                            lines.push(Line::from(vec![
                                Span::styled("  (ctrl+o to expand)", dim),
                            ]));
                        }
                    } else if let Some(r) = result {
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

        self.last_area_height = area.height;

        // Compute the true visual row count after wrapping before consuming lines.
        let total_visual: u16 = lines.iter().map(|l| visual_rows(l, area.width)).sum();
        let max_offset = total_visual.saturating_sub(area.height);

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });

        if self.stick_to_bottom {
            self.scroll_offset = max_offset;
        } else {
            self.scroll_offset = self.scroll_offset.min(max_offset);
            if self.scroll_offset >= max_offset {
                self.stick_to_bottom = true;
            }
        }

        f.render_widget(paragraph.scroll((self.scroll_offset, 0)), area);
    }
}

impl Default for ScrollArea {
    fn default() -> Self { Self::new() }
}

/// Compute the "Done (N tool uses · K tokens · Xs)" summary for a Task
/// tool-call. Returns None if the child has not yet emitted Result.
pub fn task_done_summary(
    events: &[crate::sdk::protocol::BusMessage],
    tool_use_id: &str,
) -> Option<String> {
    use crate::sdk::protocol::BusMessage;
    let result = events.iter().rev().find(|e| {
        matches!(e, BusMessage::Result { .. }) && e.parent_tool_use_id() == Some(tool_use_id)
    })?;
    let (tokens, duration_ms) = match result {
        BusMessage::Result { usage, duration_ms, .. } => (
            usage.output_tokens + usage.input_tokens,
            *duration_ms,
        ),
        _ => unreachable!(),
    };
    let tool_count = fold(events, Some(tool_use_id))
        .iter()
        .filter(|i| matches!(i, TranscriptItem::ToolCall { .. }))
        .count();
    let secs = duration_ms / 1000;
    Some(format!(
        "Done ({tool_count} tool uses · {} · {secs}s)",
        format_tokens(tokens)
    ))
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M tokens", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k tokens", n as f64 / 1_000.0)
    } else {
        format!("{n} tokens")
    }
}

fn render_child_indented<'a>(
    lines: &mut Vec<Line<'a>>,
    item: &TranscriptItem,
    dim: Style,
) {
    match item {
        TranscriptItem::User { text } => {
            lines.push(Line::from(vec![
                Span::styled("  ⎿  ", dim),
                Span::styled("Prompt:", dim),
            ]));
            for body in text.lines() {
                lines.push(Line::from(vec![
                    Span::styled("       ", dim),
                    Span::styled(body.to_string(), dim),
                ]));
            }
        }
        TranscriptItem::AssistantText { text, .. } => {
            lines.push(Line::from(vec![
                Span::styled("  ⎿  ", dim),
                Span::styled("Response:", dim),
            ]));
            for body in text.lines() {
                lines.push(Line::from(vec![
                    Span::styled("       ", dim),
                    Span::styled(body.to_string(), dim),
                ]));
            }
        }
        TranscriptItem::ToolCall { name, input, .. } => {
            // summarize_tool_call already wraps its output in parens.
            let inner_summary = summarize_tool_call(name, input);
            lines.push(Line::from(vec![
                Span::styled("  ⎿  ", dim),
                Span::styled(format!("{name}{inner_summary}"), dim),
            ]));
        }
        TranscriptItem::Thinking { text, .. } => {
            lines.push(Line::from(vec![
                Span::styled("  ⎿  ", dim),
                Span::styled("Thinking:", dim),
            ]));
            for body in text.lines() {
                lines.push(Line::from(vec![
                    Span::styled("       ", dim),
                    Span::styled(body.to_string(), dim),
                ]));
            }
        }
        TranscriptItem::System { .. } => {
            // Skip — system events inside subagents are noise in the drill-in.
        }
    }
}

fn summarize_tool_call(name: &str, input: &serde_json::Value) -> String {
    // Match Claude's "Read 1 file" style where it makes sense; otherwise show
    // a single key argument.
    match name {
        "Task" => {
            let agent = input.get("subagent_type").and_then(|v| v.as_str()).unwrap_or("?");
            let desc = input.get("description").and_then(|v| v.as_str()).unwrap_or("");
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

/// Compute how many terminal rows a single `Line` occupies when wrapped at `width` columns.
/// Uses char count as a proxy for display width (accurate for ASCII; close enough for most TUI text).
fn visual_rows(line: &Line, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }
    let char_count: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    if char_count == 0 {
        return 1;
    }
    ((char_count + width as usize - 1) / width as usize) as u16
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::protocol::{ContentBlockFinal, UserPayload};
    use uuid::Uuid;

    #[test]
    fn legacy_push_appends_to_messages() {
        let mut sa = ScrollArea::new();
        sa.push(Message::System("hi".into()));
        assert_eq!(sa.messages.len(), 1);
        assert_eq!(sa.events.len(), 0);
    }

    #[test]
    fn push_event_appends_to_events() {
        let mut sa = ScrollArea::new();
        sa.push_event(BusMessage::User {
            message: UserPayload {
                role: "user".into(),
                content: vec![ContentBlockFinal::Text { text: "hi".into() }],
            },
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        });
        assert_eq!(sa.events.len(), 1);
        assert_eq!(sa.messages.len(), 0);
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
    fn task_done_summary_computes_from_child_events() {
        let tu = "tu_task".to_string();
        let events = vec![
            BusMessage::Result {
                stop_reason: Some("end_turn".into()),
                usage: crate::sdk::protocol::AnthropicUsage {
                    input_tokens: 1000,
                    output_tokens: 500,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
                total_cost_usd: 0.0,
                duration_ms: 11_000,
                num_turns: 1,
                parent_tool_use_id: Some(tu.clone()),
                uuid: Uuid::new_v4(),
                session_id: "agent-1".into(),
            },
        ];
        let summary = task_done_summary(&events, &tu).expect("returns summary");
        assert!(summary.contains("0 tool uses"), "got: {summary}");
        assert!(summary.contains("1.5k tokens"), "got: {summary}");
        assert!(summary.contains("11s"), "got: {summary}");
    }

    #[test]
    fn task_done_summary_returns_none_when_no_result() {
        let events: Vec<BusMessage> = vec![];
        assert!(task_done_summary(&events, "tu_x").is_none());
    }

    #[test]
    fn clear_wipes_both_paths() {
        let mut sa = ScrollArea::new();
        sa.push(Message::System("a".into()));
        sa.push_event(BusMessage::Result {
            stop_reason: None,
            usage: crate::sdk::protocol::AnthropicUsage::default(),
            total_cost_usd: 0.0, duration_ms: 0, num_turns: 0,
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(), session_id: "s1".into(),
        });
        sa.clear();
        assert_eq!(sa.messages.len(), 0);
        assert_eq!(sa.events.len(), 0);
    }
}
