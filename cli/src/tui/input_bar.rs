use ratatui::{
    layout::Rect,
    style::{Color, Style},
    widgets::Paragraph,
    Frame,
};

pub struct InputBar {
    pub content: String,
    cursor_position: usize,
}

impl InputBar {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            cursor_position: 0,
        }
    }

    pub fn push_char(&mut self, c: char) {
        self.content.insert(self.cursor_position, c);
        self.cursor_position += 1;
    }

    pub fn delete_prev(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
            self.content.remove(self.cursor_position);
        }
    }

    pub fn submit(&mut self) -> String {
        let text = std::mem::take(&mut self.content);
        self.cursor_position = 0;
        text
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let display = format!("> {}", self.content);
        let paragraph = Paragraph::new(display).style(Style::default().fg(Color::White));
        f.render_widget(paragraph, area);
    }
}
