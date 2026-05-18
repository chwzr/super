use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::commands::registry::CommandRegistry;

const MAX_VISIBLE: usize = 8;

pub struct SlashMenu {
    registry: CommandRegistry,
    pub selected: usize,
}

#[derive(Clone)]
pub struct MenuEntry {
    pub name: String,
    pub description: String,
}

impl Default for SlashMenu {
    fn default() -> Self {
        Self::new()
    }
}

impl SlashMenu {
    pub fn new() -> Self {
        Self {
            registry: CommandRegistry::new(),
            selected: 0,
        }
    }

    pub fn filter(&self, input: &str) -> Vec<MenuEntry> {
        let query = input.trim_start_matches('/').to_lowercase();
        let mut entries: Vec<MenuEntry> = self
            .registry
            .list()
            .into_iter()
            .filter_map(|cmd| {
                let stripped = cmd.name.trim_start_matches('/');
                if query.is_empty() || stripped.to_lowercase().contains(&query) {
                    Some(MenuEntry {
                        name: cmd.name.clone(),
                        description: cmd.description.clone(),
                    })
                } else {
                    None
                }
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }

    pub fn cursor_up(&mut self, entries: &[MenuEntry]) {
        if entries.is_empty() {
            self.selected = 0;
            return;
        }
        if self.selected == 0 {
            self.selected = entries.len() - 1;
        } else {
            self.selected -= 1;
        }
    }

    pub fn cursor_down(&mut self, entries: &[MenuEntry]) {
        if entries.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected + 1) % entries.len();
    }

    pub fn clamp(&mut self, entries: &[MenuEntry]) {
        if entries.is_empty() {
            self.selected = 0;
        } else if self.selected >= entries.len() {
            self.selected = entries.len() - 1;
        }
    }

    /// Estimated height the menu wants to occupy.
    pub fn height(entries: &[MenuEntry]) -> u16 {
        entries.len().min(MAX_VISIBLE) as u16
    }

    pub fn render(&self, f: &mut Frame, area: Rect, entries: &[MenuEntry]) {
        if area.height == 0 || entries.is_empty() {
            return;
        }
        let name_col_width = entries
            .iter()
            .map(|e| e.name.chars().count())
            .max()
            .unwrap_or(0)
            .max(20);

        let visible: Vec<&MenuEntry> = entries.iter().take(MAX_VISIBLE).collect();
        let lines: Vec<Line> = visible
            .iter()
            .enumerate()
            .map(|(idx, e)| {
                let is_selected = idx == self.selected;
                let bg = if is_selected {
                    Style::default().bg(Color::DarkGray)
                } else {
                    Style::default()
                };
                let name_style = if is_selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                let desc_style = if is_selected {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::Gray)
                };
                let padded = format!("  {:<width$}  ", e.name, width = name_col_width);
                Line::from(vec![
                    Span::styled(padded, name_style.patch(bg)),
                    Span::styled(e.description.clone(), desc_style.patch(bg)),
                ])
            })
            .collect();

        let paragraph = Paragraph::new(lines);
        f.render_widget(paragraph, area);
    }
}
