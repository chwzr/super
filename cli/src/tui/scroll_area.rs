use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Text},
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

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let lines: Vec<Line> = self
            .messages
            .iter()
            .flat_map(|msg| match msg {
                Message::User(text) => vec![
                    Line::from(format!("╭─ User ─")),
                    Line::from(text.as_str()),
                    Line::from(""),
                ],
                Message::Assistant(text) => vec![
                    Line::from(format!("╭─ Super ─")),
                    Line::from(text.as_str()),
                    Line::from(""),
                ],
                Message::ToolCall { name, input, result } => {
                    let mut lines = vec![
                        Line::from(format!("╭─ Tool: {name} ─")),
                        Line::from(input.as_str()),
                    ];
                    if let Some(r) = result {
                        lines.push(Line::from(r.as_str()));
                    }
                    lines.push(Line::from(""));
                    lines
                }
                Message::System(text) => vec![Line::from(text.as_str())],
                Message::Thinking => vec![Line::styled(
                    "thinking...",
                    Style::default().fg(Color::DarkGray),
                )],
            })
            .collect();

        let text = Text::from(lines);
        let paragraph = Paragraph::new(text).scroll((self.scroll_offset, 0));
        f.render_widget(paragraph, area);
    }
}
