use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::{CC_BLUE, CC_DIM, CC_GREEN};

pub struct ModelEntry {
    pub label: &'static str,
    pub or_id: &'static str,
    pub description: &'static str,
}

const MODELS: &[ModelEntry] = &[
    ModelEntry { label: "claude-opus-4-7",    or_id: "anthropic/claude-opus-4-7",            description: "Most capable model for complex tasks" },
    ModelEntry { label: "claude-opus-4-6",    or_id: "anthropic/claude-opus-4-6",            description: "Previous opus generation" },
    ModelEntry { label: "claude-sonnet-4-6",  or_id: "anthropic/claude-sonnet-4-6",          description: "Balanced intelligence and speed" },
    ModelEntry { label: "claude-haiku-4-5",   or_id: "anthropic/claude-haiku-4-5-20251001",  description: "Fastest model for simple tasks" },
];

pub struct ModelPicker {
    pub cursor: usize,
    pub current_or_id: String,
}

impl ModelPicker {
    pub fn new(current_or_id: String) -> Self {
        let cursor = MODELS
            .iter()
            .position(|m| m.or_id == current_or_id)
            .unwrap_or(0);
        Self { cursor, current_or_id }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ModelAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 { self.cursor -= 1; }
                ModelAction::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor + 1 < MODELS.len() { self.cursor += 1; }
                ModelAction::Continue
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let idx = (c as usize).wrapping_sub('1' as usize);
                if idx < MODELS.len() { self.cursor = idx; }
                ModelAction::Continue
            }
            KeyCode::Enter => ModelAction::Select(MODELS[self.cursor].or_id),
            KeyCode::Esc   => ModelAction::Cancel,
            _ => ModelAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();

        for (i, entry) in MODELS.iter().enumerate() {
            let is_cursor  = i == self.cursor;
            let is_current = entry.or_id == self.current_or_id;

            let prefix = if is_cursor { "❯ " } else { "  " };
            let number = format!("{}. ", i + 1);

            let name_style = if is_cursor {
                Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(CC_DIM)
            };
            let check_style = Style::default().fg(CC_GREEN);
            let desc_style  = Style::default().fg(CC_DIM);
            let checkmark   = if is_current { " ✔" } else { "" };

            lines.push(Line::from(vec![
                Span::raw(prefix),
                Span::styled(number, name_style),
                Span::styled(entry.label, name_style),
                Span::styled(format!("  {}", entry.description), desc_style),
                Span::styled(checkmark, check_style),
            ]));
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  [enter to select] [esc to cancel]",
            Style::default().fg(CC_DIM),
        )));

        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum ModelAction {
    Continue,
    Select(&'static str),
    Cancel,
}
