use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use serde::Deserialize;
use std::collections::HashSet;

use crate::tui::colors::{CC_BLUE, CC_DIM, CC_GREEN};

#[derive(Clone)]
pub struct McpServer {
    pub name: String,
    pub transport: String,
    pub enabled: bool,
}

#[derive(Deserialize, Default)]
struct McpConfig {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: std::collections::HashMap<String, serde_json::Value>,
}

fn load_mcp_servers() -> Vec<McpServer> {
    let disabled = load_disabled_set();
    let mut servers: Vec<McpServer> = Vec::new();

    for path in mcp_config_paths() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<McpConfig>(&content) {
                for (name, def) in cfg.mcp_servers {
                    let transport = def["type"]
                        .as_str()
                        .or_else(|| def["command"].as_str().map(|_| "stdio"))
                        .unwrap_or("stdio")
                        .to_string();
                    let enabled = !disabled.contains(&name);
                    if !servers.iter().any(|s| s.name == name) {
                        servers.push(McpServer { name, transport, enabled });
                    }
                }
            }
        }
    }
    servers
}

fn mcp_config_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".claude").join("claude_desktop_config.json"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join(".claude").join("claude_desktop_config.json"));
    }
    paths
}

fn load_disabled_set() -> HashSet<String> {
    let cfg = crate::config::load_config();
    cfg.settings
        .get("disabledMcpServers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn save_disabled_set(disabled: &HashSet<String>) {
    let mut cfg = crate::config::load_config();
    let arr: Vec<serde_json::Value> = disabled
        .iter()
        .map(|s| serde_json::Value::String(s.clone()))
        .collect();
    if !cfg.settings.is_object() {
        cfg.settings = serde_json::Value::Object(serde_json::Map::new());
    }
    if let Some(obj) = cfg.settings.as_object_mut() {
        obj.insert("disabledMcpServers".into(), serde_json::Value::Array(arr));
    }
    crate::config::save_config(&cfg);
}

pub struct McpList {
    pub servers: Vec<McpServer>,
    pub cursor: usize,
}

impl McpList {
    pub fn new() -> Self {
        Self {
            servers: load_mcp_servers(),
            cursor: 0,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> McpAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 { self.cursor -= 1; }
                McpAction::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor + 1 < self.servers.len() { self.cursor += 1; }
                McpAction::Continue
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(srv) = self.servers.get_mut(self.cursor) {
                    srv.enabled = !srv.enabled;
                    let disabled: HashSet<String> = self
                        .servers
                        .iter()
                        .filter(|s| !s.enabled)
                        .map(|s| s.name.clone())
                        .collect();
                    save_disabled_set(&disabled);
                }
                McpAction::Continue
            }
            KeyCode::Esc => McpAction::Cancel,
            _ => McpAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::raw(""));

        if self.servers.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No MCP servers configured.",
                Style::default().fg(CC_DIM),
            )));
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "  Add servers in ~/.claude/claude_desktop_config.json",
                Style::default().fg(CC_DIM),
            )));
        } else {
            for (i, srv) in self.servers.iter().enumerate() {
                let is_cursor = i == self.cursor;
                let prefix = if is_cursor { "❯ " } else { "  " };
                let (glyph, glyph_style) = if srv.enabled {
                    ("✔", Style::default().fg(CC_GREEN))
                } else {
                    ("◯", Style::default().fg(CC_DIM))
                };
                let name_style = if is_cursor {
                    Style::default().fg(CC_BLUE)
                } else {
                    Style::default().fg(CC_DIM)
                };
                lines.push(Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(glyph, glyph_style),
                    Span::raw("  "),
                    Span::styled(format!("{:<24}", srv.name), name_style),
                    Span::styled(srv.transport.clone(), Style::default().fg(CC_DIM)),
                ]));
            }
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  ✔ enabled  ◯ disabled   [enter/space to toggle] [esc to close]",
            Style::default().fg(CC_DIM),
        )));

        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum McpAction {
    Continue,
    Cancel,
}
