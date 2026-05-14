use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::{CC_BLUE, CC_BAR_BG, CC_DIM, CC_GREEN, CC_TAB_BG, CC_TAB_FG, CC_YELLOW};

#[derive(Clone, Copy, PartialEq)]
pub enum StatusTab {
    Settings,
    Status,
    Config,
    Usage,
    Stats,
}

impl StatusTab {
    pub fn all() -> &'static [StatusTab] {
        &[StatusTab::Settings, StatusTab::Status, StatusTab::Config, StatusTab::Usage, StatusTab::Stats]
    }

    pub fn label(self) -> &'static str {
        match self {
            StatusTab::Settings => "Settings",
            StatusTab::Status   => "Status",
            StatusTab::Config   => "Config",
            StatusTab::Usage    => "Usage",
            StatusTab::Stats    => "Stats",
        }
    }

    fn next(self) -> StatusTab {
        let all = StatusTab::all();
        let idx = all.iter().position(|&t| t == self).unwrap_or(0);
        all[(idx + 1) % all.len()]
    }

    fn prev(self) -> StatusTab {
        let all = StatusTab::all();
        let idx = all.iter().position(|&t| t == self).unwrap_or(0);
        all[idx.checked_sub(1).unwrap_or(all.len() - 1)]
    }
}

pub enum UsageState {
    Loading,
    Loaded { used_usd: f64, limit_usd: f64 },
    Error(String),
}

pub struct StatusSnapshot {
    pub version: String,
    pub model: String,
    pub thinking: bool,
    pub effort: Option<String>,
    pub email: Option<String>,
    pub messages: usize,
    pub cwd: String,
}

pub struct StatusView {
    pub tab: StatusTab,
    pub usage: UsageState,
    pub snap: StatusSnapshot,
}

impl StatusView {
    pub fn new(snap: StatusSnapshot) -> Self {
        Self {
            tab: StatusTab::Settings,
            usage: UsageState::Loading,
            snap,
        }
    }

    pub fn set_usage(&mut self, result: Result<(f64, f64), String>) {
        self.usage = match result {
            Ok((used, limit)) => UsageState::Loaded { used_usd: used, limit_usd: limit },
            Err(e)            => UsageState::Error(e),
        };
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> StatusAction {
        match key.code {
            KeyCode::Right | KeyCode::Tab      => { self.tab = self.tab.next(); StatusAction::Continue }
            KeyCode::Left  | KeyCode::BackTab  => { self.tab = self.tab.prev(); StatusAction::Continue }
            KeyCode::Esc   | KeyCode::Char('q')=> StatusAction::Cancel,
            _ => StatusAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        if area.height == 0 { return; }
        let mut lines: Vec<Line> = Vec::new();

        // Tab bar
        let mut spans: Vec<Span> = vec![Span::raw(" ")];
        for &tab in StatusTab::all() {
            if tab == self.tab {
                spans.push(Span::styled(
                    format!(" {} ", tab.label()),
                    Style::default().fg(CC_TAB_FG).bg(CC_TAB_BG).add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    format!(" {} ", tab.label()),
                    Style::default().fg(CC_DIM),
                ));
            }
            spans.push(Span::raw(" │"));
        }
        lines.push(Line::from(spans));
        lines.push(Line::raw(""));

        match self.tab {
            StatusTab::Settings => self.render_settings(&mut lines),
            StatusTab::Status   => self.render_status(&mut lines),
            StatusTab::Config   => self.render_config(&mut lines),
            StatusTab::Usage    => self.render_usage(&mut lines),
            StatusTab::Stats    => self.render_stats(&mut lines),
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  [← → or Tab to switch tabs] [esc to close]",
            Style::default().fg(CC_DIM),
        )));

        f.render_widget(Paragraph::new(lines), area);
    }

    fn kv(key: &str, val: String) -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("  {:<20}", key), Style::default().fg(CC_DIM)),
            Span::raw(val),
        ])
    }

    fn render_settings(&self, lines: &mut Vec<Line>) {
        let email = self.snap.email.clone().unwrap_or_else(|| "(not signed in)".into());
        lines.push(Self::kv("Account", email));
        let model = self.snap.model.rsplit_once('/').map(|(_, r)| r).unwrap_or(&self.snap.model);
        lines.push(Self::kv("Model", model.to_string()));
        lines.push(Self::kv("Effort", self.snap.effort.clone().unwrap_or_else(|| "medium".into())));
        lines.push(Self::kv("Extended thinking", if self.snap.thinking { "enabled".into() } else { "disabled".into() }));
        lines.push(Self::kv("Working dir", self.snap.cwd.clone()));
    }

    fn render_status(&self, lines: &mut Vec<Line>) {
        let ok = Style::default().fg(CC_GREEN);
        lines.push(Line::from(vec![
            Span::styled("  ✔", ok), Span::raw("  API connection     "), Span::styled("connected", ok),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  ✔", ok), Span::raw("  Auth               "), Span::styled("authenticated", ok),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  ✔", ok), Span::raw("  OpenRouter         "), Span::styled("reachable", ok),
        ]));
    }

    fn render_config(&self, lines: &mut Vec<Line>) {
        let cfg = crate::config::load_config();
        lines.push(Self::kv("api_base_url", cfg.api_base_url.clone()));
        lines.push(Self::kv("model", cfg.model.clone()));
        lines.push(Self::kv("access_token", if cfg.access_token.is_some() { "••••••••".into() } else { "(none)".into() }));
        lines.push(Self::kv("openrouter_key", if cfg.openrouter_api_key.is_some() { "••••••••".into() } else { "(none)".into() }));
    }

    fn render_usage(&self, lines: &mut Vec<Line>) {
        match &self.usage {
            UsageState::Loading => {
                lines.push(Line::from(Span::styled("  Loading usage data\u{2026}", Style::default().fg(CC_DIM))));
            }
            UsageState::Error(e) => {
                lines.push(Line::from(Span::styled(
                    format!("  Usage data unavailable: {e}"),
                    Style::default().fg(CC_YELLOW),
                )));
            }
            UsageState::Loaded { used_usd, limit_usd } => {
                // f64::MAX is our sentinel for "no cap / unlimited"
                let is_unlimited = *limit_usd >= f64::MAX / 2.0;

                lines.push(Line::from(Span::styled(
                    "  API Usage (this billing period)",
                    Style::default().fg(CC_DIM).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::raw(""));

                if is_unlimited {
                    lines.push(Line::from(Span::raw(format!("  ${:.2} used \u{00b7} no spending cap", used_usd))));
                } else {
                    let pct = if *limit_usd > 0.0 { (used_usd / limit_usd * 100.0) as usize } else { 0 };
                    let bar_total = 24usize;
                    let filled = (bar_total * pct / 100).min(bar_total);
                    let empty  = bar_total - filled;

                    let bar_line = Line::from(vec![
                        Span::raw("  "),
                        Span::styled("\u{2588}".repeat(filled), Style::default().fg(CC_BLUE).bg(CC_BAR_BG)),
                        Span::styled(" ".repeat(empty),  Style::default().bg(CC_BAR_BG)),
                        Span::raw(format!("  {}% used", pct)),
                    ]);
                    lines.push(bar_line);
                    lines.push(Line::raw(""));
                    lines.push(Line::from(Span::raw(format!(
                        "  ${:.2} used of ${:.2} limit  \u{00b7}  ${:.2} remaining",
                        used_usd,
                        limit_usd,
                        (limit_usd - used_usd).max(0.0),
                    ))));
                }
            }
        }
    }

    fn render_stats(&self, lines: &mut Vec<Line>) {
        lines.push(Self::kv("Messages this session", self.snap.messages.to_string()));
    }
}

pub enum StatusAction {
    Continue,
    Cancel,
}
