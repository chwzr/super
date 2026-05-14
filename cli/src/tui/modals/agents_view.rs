use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::{CC_DIM, CC_TAB_BG, CC_TAB_FG};

#[derive(Clone, Copy, PartialEq)]
pub enum AgentsTab {
    Agents,
    Running,
    Library,
}

impl AgentsTab {
    fn all() -> &'static [AgentsTab] {
        &[AgentsTab::Agents, AgentsTab::Running, AgentsTab::Library]
    }
    fn label(self) -> &'static str {
        match self {
            AgentsTab::Agents  => "Agents",
            AgentsTab::Running => "Running",
            AgentsTab::Library => "Library",
        }
    }
    fn next(self) -> AgentsTab {
        let all = Self::all();
        let idx = all.iter().position(|&t| t == self).unwrap_or(0);
        all[(idx + 1) % all.len()]
    }
    fn prev(self) -> AgentsTab {
        let all = Self::all();
        let idx = all.iter().position(|&t| t == self).unwrap_or(0);
        all[idx.checked_sub(1).unwrap_or(all.len() - 1)]
    }
}

fn scan_agents(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().into_string().ok()?;
            name.strip_suffix(".md").map(|s| s.to_string())
        })
        .collect()
}

pub struct AgentsView {
    pub tab: AgentsTab,
    user_agents: Vec<String>,
    proj_agents: Vec<String>,
}

impl AgentsView {
    pub fn new() -> Self {
        let user_agents = dirs::home_dir()
            .map(|h| scan_agents(&h.join(".claude").join("agents")))
            .unwrap_or_default();
        let proj_agents = std::env::current_dir()
            .map(|cwd| scan_agents(&cwd.join(".claude").join("agents")))
            .unwrap_or_default();
        Self {
            tab: AgentsTab::Agents,
            user_agents,
            proj_agents,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> AgentsAction {
        match key.code {
            KeyCode::Right | KeyCode::Tab => {
                self.tab = self.tab.next();
                AgentsAction::Continue
            }
            KeyCode::Left | KeyCode::BackTab => {
                self.tab = self.tab.prev();
                AgentsAction::Continue
            }
            KeyCode::Esc | KeyCode::Char('q') => AgentsAction::Cancel,
            _ => AgentsAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();

        let mut spans: Vec<Span> = vec![Span::raw(" ")];
        for &tab in AgentsTab::all() {
            if tab == self.tab {
                spans.push(Span::styled(
                    format!(" {} ", tab.label()),
                    Style::default()
                        .fg(CC_TAB_FG)
                        .bg(CC_TAB_BG)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    format!(" {} ", tab.label()),
                    Style::default().fg(CC_DIM),
                ));
            }
            spans.push(Span::raw(" │ "));
        }
        lines.push(Line::from(spans));
        lines.push(Line::raw(""));

        match self.tab {
            AgentsTab::Agents  => self.render_agents(&mut lines),
            AgentsTab::Running => self.render_running(&mut lines),
            AgentsTab::Library => self.render_library(&mut lines),
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  [← → or Tab to switch] [esc to close]",
            Style::default().fg(CC_DIM),
        )));

        f.render_widget(Paragraph::new(lines), area);
    }

    fn render_agents(&self, lines: &mut Vec<Line>) {
        lines.push(Line::from(Span::styled(
            "  User agents (~/.claude/agents/)",
            Style::default().fg(CC_DIM).add_modifier(Modifier::BOLD),
        )));
        if self.user_agents.is_empty() {
            lines.push(Line::from(Span::styled(
                "    (no custom agents)",
                Style::default().fg(CC_DIM),
            )));
        } else {
            for name in &self.user_agents {
                lines.push(Line::from(Span::styled(
                    format!("    {name}"),
                    Style::default().fg(CC_DIM),
                )));
            }
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  Project agents (.claude/agents/)",
            Style::default().fg(CC_DIM).add_modifier(Modifier::BOLD),
        )));
        if self.proj_agents.is_empty() {
            lines.push(Line::from(Span::styled(
                "    (no project agents)",
                Style::default().fg(CC_DIM),
            )));
        } else {
            for name in &self.proj_agents {
                lines.push(Line::from(Span::styled(
                    format!("    {name}"),
                    Style::default().fg(CC_DIM),
                )));
            }
        }
    }

    fn render_running(&self, lines: &mut Vec<Line>) {
        lines.push(Line::from(Span::styled(
            "  No running agents.",
            Style::default().fg(CC_DIM),
        )));
    }

    fn render_library(&self, lines: &mut Vec<Line>) {
        lines.push(Line::from(Span::styled(
            "  Agent library not yet available.",
            Style::default().fg(CC_DIM),
        )));
    }
}

pub enum AgentsAction {
    Continue,
    Cancel,
}
