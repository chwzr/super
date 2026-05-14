use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

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
    /// Persisted activity trail rendered as `◆ Verb for Xs`.
    Trail(String),
    Thinking,
}

pub struct ScrollArea {
    pub messages: Vec<Message>,
    scroll_offset: u16,
}

impl ScrollArea {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            scroll_offset: 0,
        }
    }

    pub fn push(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
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
        let user_prefix = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);
        let assistant_prefix = Style::default().fg(Color::Cyan);
        let dim = Style::default().fg(Color::DarkGray);
        let body_style = Style::default().fg(Color::White);

        let mut lines: Vec<Line> = Vec::new();

        for msg in &self.messages {
            match msg {
                Message::User(text) => {
                    lines.push(Line::from(""));
                    for (i, body_line) in text.lines().enumerate() {
                        if i == 0 {
                            lines.push(Line::from(vec![
                                Span::styled("❯ ", user_prefix),
                                Span::styled(body_line.to_string(), body_style),
                            ]));
                        } else {
                            lines.push(Line::from(vec![
                                Span::raw("  "),
                                Span::styled(body_line.to_string(), body_style),
                            ]));
                        }
                    }
                }
                Message::Assistant(text) => {
                    lines.push(Line::from(""));
                    for (i, body_line) in text.lines().enumerate() {
                        if i == 0 {
                            lines.push(Line::from(vec![
                                Span::styled("◆ ", assistant_prefix),
                                Span::styled(body_line.to_string(), body_style),
                            ]));
                        } else {
                            lines.push(Line::from(vec![
                                Span::raw("  "),
                                Span::styled(body_line.to_string(), body_style),
                            ]));
                        }
                    }
                }
                Message::ToolCall {
                    name,
                    input,
                    result,
                } => {
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        Span::styled("◆ ", assistant_prefix),
                        Span::styled(
                            name.clone(),
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(format!("({})", input), dim),
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
                        Span::styled("◈ ", Style::default().fg(Color::Cyan)),
                        Span::styled(text.clone(), dim),
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
        }

        // Auto-tail: clamp scroll so the latest line is visible.
        let height = area.height as usize;
        let total = lines.len();
        let max_offset = total.saturating_sub(height);
        let offset = (self.scroll_offset as usize).min(max_offset);

        let paragraph = Paragraph::new(lines).scroll((offset as u16, 0));
        f.render_widget(paragraph, area);
    }
}
