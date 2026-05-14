use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::CC_DIM;

pub struct ResumePicker {
    pub query: String,
}

impl ResumePicker {
    pub fn new() -> Self {
        Self { query: String::new() }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ResumeAction {
        match key.code {
            KeyCode::Esc => ResumeAction::Cancel,
            KeyCode::Backspace => {
                self.query.pop();
                ResumeAction::Continue
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                ResumeAction::Continue
            }
            _ => ResumeAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let search_display = format!("  Search: [{}]", self.query);
        let lines = vec![
            Line::raw(""),
            Line::from(Span::styled(search_display, Style::default().fg(CC_DIM))),
            Line::raw(""),
            Line::from(Span::styled(
                "  No previous sessions found.",
                Style::default().fg(CC_DIM),
            )),
            Line::from(Span::styled(
                "  Sessions will appear here once conversation persistence is implemented.",
                Style::default().fg(CC_DIM),
            )),
            Line::raw(""),
            Line::from(Span::styled("  [esc to cancel]", Style::default().fg(CC_DIM))),
        ];
        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum ResumeAction {
    Continue,
    Cancel,
}
