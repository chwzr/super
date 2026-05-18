use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

const PROMPT: &str = "❯ ";
const PLACEHOLDER: &str = "ask, plan, or paste a task";

pub struct InputBar {
    pub content: String,
    cursor_position: usize,
}

impl Default for InputBar {
    fn default() -> Self {
        Self::new()
    }
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

    pub fn clear(&mut self) {
        self.content.clear();
        self.cursor_position = 0;
    }

    pub fn move_left(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor_position < self.content.len() {
            self.cursor_position += 1;
        }
    }

    pub fn move_home(&mut self) {
        self.cursor_position = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor_position = self.content.len();
    }

    pub fn submit(&mut self) -> String {
        let text = std::mem::take(&mut self.content);
        self.cursor_position = 0;
        text
    }

    /// Renders the three-line input bar:
    /// dashed top border, `❯ <content>`, dashed bottom border.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let width = area.width as usize;
        let divider: String = "─".repeat(width);
        let dim = Style::default().fg(Color::DarkGray);

        let body = if self.content.is_empty() {
            Line::from(vec![
                Span::styled(PROMPT, Style::default().fg(Color::Cyan)),
                Span::styled(PLACEHOLDER, dim),
            ])
        } else {
            Line::from(vec![
                Span::styled(PROMPT, Style::default().fg(Color::Cyan)),
                Span::styled(self.content.as_str(), Style::default().fg(Color::White)),
            ])
        };

        let mut lines = Vec::with_capacity(3);
        lines.push(Line::styled(divider.clone(), dim));
        lines.push(body);
        if area.height >= 3 {
            lines.push(Line::styled(divider, dim));
        }
        let paragraph = Paragraph::new(lines);
        f.render_widget(paragraph, area);

        // Position the cursor on the input row (second line of the input bar).
        if area.height >= 2 {
            let cursor_x = area.x + PROMPT.chars().count() as u16 + self.cursor_position as u16;
            let cursor_y = area.y + 1;
            f.set_cursor_position((
                cursor_x.min(area.x + area.width.saturating_sub(1)),
                cursor_y,
            ));
        }
    }
}
