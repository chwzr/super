use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::providers;
use crate::tui::colors::{CC_BLUE, CC_DIM, CC_GREEN};

pub struct ClassEntry {
    pub class: &'static str,
    pub description: &'static str,
}

const CLASSES: &[ClassEntry] = &[
    ClassEntry {
        class: "haiku",
        description: "Fastest model for simple tasks",
    },
    ClassEntry {
        class: "sonnet",
        description: "Balanced intelligence and speed",
    },
    ClassEntry {
        class: "opus",
        description: "Most capable for complex tasks",
    },
];

pub struct ModelPicker {
    pub cursor: usize,
    pub current_class: String,
    pub current_provider: String,
}

impl ModelPicker {
    pub fn new(current_provider: String, current_class: String) -> Self {
        let cursor = CLASSES
            .iter()
            .position(|c| c.class == current_class)
            .unwrap_or(1); // default to sonnet
        Self {
            cursor,
            current_class,
            current_provider,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ModelAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                ModelAction::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor + 1 < CLASSES.len() {
                    self.cursor += 1;
                }
                ModelAction::Continue
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let idx = (c as usize).wrapping_sub('1' as usize);
                if idx < CLASSES.len() {
                    self.cursor = idx;
                }
                ModelAction::Continue
            }
            KeyCode::Enter => ModelAction::Select(CLASSES[self.cursor].class),
            KeyCode::Esc => ModelAction::Cancel,
            _ => ModelAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();

        for (i, entry) in CLASSES.iter().enumerate() {
            let is_cursor = i == self.cursor;
            let is_current = entry.class == self.current_class;

            let prefix = if is_cursor { "❯ " } else { "  " };
            let number = format!("{}. ", i + 1);

            let name_style = if is_cursor {
                Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(CC_DIM)
            };
            let check_style = Style::default().fg(CC_GREEN);
            let desc_style = Style::default().fg(CC_DIM);
            let checkmark = if is_current { " ✔" } else { "" };

            let slug = providers::resolve_slug(&self.current_provider, entry.class);
            let label_with_slug = format!("{:<8}  {}", entry.class, slug);

            lines.push(Line::from(vec![
                Span::raw(prefix),
                Span::styled(number, name_style),
                Span::styled(label_with_slug, name_style),
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
