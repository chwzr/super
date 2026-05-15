use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::{CC_BLUE, CC_DIM, CC_GREEN};
use crate::providers::PROVIDERS;

pub struct ProviderPicker {
    pub cursor: usize,
    pub current_provider: String,
}

impl ProviderPicker {
    pub fn new(current_provider: String) -> Self {
        let cursor = PROVIDERS
            .iter()
            .position(|p| p.id == current_provider)
            .unwrap_or(0);
        Self { cursor, current_provider }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ProviderAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 { self.cursor -= 1; }
                ProviderAction::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor + 1 < PROVIDERS.len() { self.cursor += 1; }
                ProviderAction::Continue
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let idx = (c as usize).wrapping_sub('1' as usize);
                if idx < PROVIDERS.len() { self.cursor = idx; }
                ProviderAction::Continue
            }
            KeyCode::Enter => ProviderAction::Select(PROVIDERS[self.cursor].id),
            KeyCode::Esc   => ProviderAction::Cancel,
            _ => ProviderAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let descriptions: &[&str] = &[
            "Claude models (default)",
            "GLM models",
            "Kimi models",
            "Deepseek V4 models",
            "OpenRouter free tier",
        ];

        let mut lines: Vec<Line> = Vec::new();

        for (i, entry) in PROVIDERS.iter().enumerate() {
            let is_cursor  = i == self.cursor;
            let is_current = entry.id == self.current_provider;

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
            let desc = descriptions.get(i).copied().unwrap_or("");

            lines.push(Line::from(vec![
                Span::raw(prefix),
                Span::styled(number, name_style),
                Span::styled(entry.display_name, name_style),
                Span::styled(format!("  {desc}"), desc_style),
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

pub enum ProviderAction {
    Continue,
    Select(&'static str),
    Cancel,
}
