use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::conversation::transcript::SessionMeta;
use crate::tui::colors::{CC_BLUE, CC_DIM};

pub struct ResumePicker {
    pub query: String,
    pub sessions: Vec<SessionMeta>,
    pub selected_idx: usize,
}

impl ResumePicker {
    pub fn new() -> Self {
        let sessions = crate::conversation::transcript::list_sessions();
        Self {
            query: String::new(),
            sessions,
            selected_idx: 0,
        }
    }

    fn filtered_sessions(&self) -> Vec<&SessionMeta> {
        if self.query.is_empty() {
            self.sessions.iter().collect()
        } else {
            let q = self.query.to_lowercase();
            self.sessions
                .iter()
                .filter(|s| {
                    s.first_prompt.to_lowercase().contains(&q)
                        || s.custom_title
                            .as_ref()
                            .map(|t| t.to_lowercase().contains(&q))
                            .unwrap_or(false)
                        || s.session_id.to_lowercase().contains(&q)
                })
                .collect()
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ResumeAction {
        match key.code {
            KeyCode::Esc => ResumeAction::Cancel,
            KeyCode::Up | KeyCode::Char('k') => {
                let filtered = self.filtered_sessions();
                if !filtered.is_empty() {
                    self.selected_idx = self.selected_idx.saturating_sub(1);
                }
                ResumeAction::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let filtered = self.filtered_sessions();
                if !filtered.is_empty() {
                    let max = filtered.len().saturating_sub(1);
                    if self.selected_idx < max {
                        self.selected_idx += 1;
                    }
                }
                ResumeAction::Continue
            }
            KeyCode::Enter => {
                let filtered = self.filtered_sessions();
                if let Some(s) = filtered.get(self.selected_idx) {
                    ResumeAction::Select(s.session_id.clone())
                } else {
                    ResumeAction::Continue
                }
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.selected_idx = 0;
                ResumeAction::Continue
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.selected_idx = 0;
                ResumeAction::Continue
            }
            _ => ResumeAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let filtered = self.filtered_sessions();
        let max_items = area.height.saturating_sub(3) as usize;

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("  Search: [{}]", self.query),
            Style::default().fg(CC_DIM),
        )));
        lines.push(Line::raw(""));

        if filtered.is_empty() {
            if self.sessions.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  No previous sessions found.",
                    Style::default().fg(CC_DIM),
                )));
                lines.push(Line::from(Span::styled(
                    "  Start a conversation to create a session.",
                    Style::default().fg(CC_DIM),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "  No sessions match your search.",
                    Style::default().fg(CC_DIM),
                )));
            }
        } else {
            let start = self.selected_idx.saturating_sub(max_items.saturating_sub(1));
            let end = (start + max_items).min(filtered.len());

            for (i, session) in filtered.iter().enumerate().skip(start).take(end - start) {
                let is_selected = i == self.selected_idx;
                let prefix = if is_selected { "> " } else { "  " };

                let title = session
                    .custom_title
                    .as_deref()
                    .unwrap_or(&session.first_prompt)
                    .to_string();
                let display_title = if title.len() > 60 {
                    format!("{}…", &title[..59])
                } else {
                    title
                };

                let line = if is_selected {
                    Line::from(vec![
                        Span::styled(
                            format!("{}{}", prefix, display_title),
                            Style::default()
                                .fg(CC_BLUE)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ])
                } else {
                    Line::from(vec![Span::raw(format!("{}{}", prefix, display_title))])
                };
                lines.push(line);
            }
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  [esc to cancel] [↑↓ to navigate] [enter to select]",
            Style::default().fg(CC_DIM),
        )));

        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum ResumeAction {
    Continue,
    Cancel,
    Select(String),
}
