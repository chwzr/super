use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::{CC_BLUE, CC_DIM};

const SENSITIVE_KEYS: &[&str] = &["access_token", "refresh_token", "openrouter_api_key"];

pub struct ConfigEntry {
    pub key: String,
    pub value: String,
}

pub struct ConfigView {
    pub query: String,
    pub entries: Vec<ConfigEntry>,
    pub scroll: usize,
}

impl Default for ConfigView {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigView {
    pub fn new() -> Self {
        let entries = Self::load_entries();
        Self {
            query: String::new(),
            entries,
            scroll: 0,
        }
    }

    fn load_entries() -> Vec<ConfigEntry> {
        let cfg = crate::config::load_config();
        let mut entries = Vec::new();

        let mut push = |key: &str, val: String| {
            let display = if SENSITIVE_KEYS.contains(&key) && !val.is_empty() && val != "(not set)"
            {
                "••••••••".to_string()
            } else {
                val
            };
            entries.push(ConfigEntry {
                key: key.to_string(),
                value: display,
            });
        };

        push(
            "access_token",
            cfg.access_token
                .as_deref()
                .unwrap_or("(not set)")
                .to_string(),
        );
        push(
            "refresh_token",
            cfg.refresh_token
                .as_deref()
                .unwrap_or("(not set)")
                .to_string(),
        );
        push(
            "openrouter_api_key",
            cfg.openrouter_api_key
                .as_deref()
                .unwrap_or("(not set)")
                .to_string(),
        );
        push("api_base_url", cfg.api_base_url.clone());
        push("model", cfg.model.clone());
        push("permissions", cfg.permissions.to_string());
        push("settings", cfg.settings.to_string());

        // Also show the config file path as a meta entry.
        let config_path = crate::config::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(unknown — HOME not set)".to_string());
        entries.push(ConfigEntry {
            key: "config_file".to_string(),
            value: config_path,
        });

        entries.sort_by(|a, b| a.key.cmp(&b.key));
        entries
    }

    fn filtered(&self) -> Vec<&ConfigEntry> {
        let q = self.query.to_lowercase();
        self.entries
            .iter()
            .filter(|e| q.is_empty() || e.key.to_lowercase().contains(&q))
            .collect()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ConfigAction {
        match key.code {
            KeyCode::Esc => ConfigAction::Cancel,
            KeyCode::Backspace => {
                self.query.pop();
                self.scroll = 0;
                ConfigAction::Continue
            }
            KeyCode::Up => {
                self.scroll = self.scroll.saturating_sub(1);
                ConfigAction::Continue
            }
            KeyCode::Down => {
                let max = self.filtered().len().saturating_sub(1);
                if self.scroll < max {
                    self.scroll += 1;
                }
                ConfigAction::Continue
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.scroll = 0;
                ConfigAction::Continue
            }
            _ => ConfigAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let filtered = self.filtered();
        let mut lines: Vec<Line> = Vec::new();

        lines.push(Line::from(vec![
            Span::styled("  Search: ", Style::default().fg(CC_DIM)),
            Span::raw(format!("[{}]", self.query)),
        ]));
        lines.push(Line::raw(""));

        let visible_start = self.scroll.min(filtered.len().saturating_sub(1));
        for entry in filtered.iter().skip(visible_start) {
            let val_display = if entry.value.chars().count() > 60 {
                let truncated: String = entry.value.chars().take(57).collect();
                format!("{truncated}…")
            } else {
                entry.value.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<28}", entry.key), Style::default().fg(CC_BLUE)),
                Span::styled(val_display, Style::default().fg(CC_DIM)),
            ]));
        }

        if filtered.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (no matching keys)",
                Style::default().fg(CC_DIM),
            )));
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  [type to filter] [↑↓ to scroll] [esc to close]",
            Style::default().fg(CC_DIM),
        )));

        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum ConfigAction {
    Continue,
    Cancel,
}
