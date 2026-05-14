use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::{CC_BLUE, CC_DIM};

#[derive(Clone, Copy, PartialEq)]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    Max,
}

impl EffortLevel {
    pub fn all() -> &'static [EffortLevel] {
        &[EffortLevel::Low, EffortLevel::Medium, EffortLevel::High, EffortLevel::Max]
    }

    pub fn label(self) -> &'static str {
        match self {
            EffortLevel::Low    => "Low",
            EffortLevel::Medium => "Medium",
            EffortLevel::High   => "High",
            EffortLevel::Max    => "Max",
        }
    }

    pub fn value(self) -> &'static str {
        match self {
            EffortLevel::Low    => "low",
            EffortLevel::Medium => "medium",
            EffortLevel::High   => "high",
            EffortLevel::Max    => "max",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            EffortLevel::Low    => "Minimal processing, fastest response",
            EffortLevel::Medium => "Balanced performance and quality",
            EffortLevel::High   => "More thorough analysis",
            EffortLevel::Max    => "Maximum capability, slowest",
        }
    }

    fn from_str(s: &str) -> EffortLevel {
        match s {
            "low"    => EffortLevel::Low,
            "medium" => EffortLevel::Medium,
            "high"   => EffortLevel::High,
            "max"    => EffortLevel::Max,
            _        => EffortLevel::Medium,
        }
    }
}

pub struct EffortPicker {
    pub level: EffortLevel,
}

impl EffortPicker {
    pub fn new(current: Option<String>) -> Self {
        let level = current
            .as_deref()
            .map(EffortLevel::from_str)
            .unwrap_or(EffortLevel::Medium);
        Self { level }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> EffortAction {
        let all = EffortLevel::all();
        let idx = all.iter().position(|&l| l == self.level).unwrap_or(1);
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                if idx > 0 { self.level = all[idx - 1]; }
                EffortAction::Continue
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if idx + 1 < all.len() { self.level = all[idx + 1]; }
                EffortAction::Continue
            }
            KeyCode::Enter => EffortAction::Select(self.level.value()),
            KeyCode::Esc   => EffortAction::Cancel,
            _ => EffortAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let all = EffortLevel::all();
        let idx = all.iter().position(|&l| l == self.level).unwrap_or(1);

        let axis_line = Line::from(vec![
            Span::styled("  Speed ", Style::default().fg(CC_DIM)),
            Span::styled("←", Style::default().fg(CC_DIM)),
            Span::styled("────────────────────────────────────", Style::default().fg(CC_DIM)),
            Span::styled("→", Style::default().fg(CC_DIM)),
            Span::styled(" Intelligence", Style::default().fg(CC_DIM)),
        ]);

        let slot_w = 12usize;
        let mut labels_raw = "  ".to_string();
        for level in all.iter() {
            labels_raw.push_str(&format!("{:<width$}", level.label(), width = slot_w));
        }
        let labels_line = Line::from(Span::styled(labels_raw, Style::default().fg(CC_DIM)));

        let marker_offset = 2 + idx * slot_w + slot_w / 2;
        let mut marker_raw = " ".repeat(marker_offset);
        marker_raw.push('▲');
        let marker_line = Line::from(Span::styled(
            marker_raw,
            Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD),
        ));

        let desc_line = Line::from(vec![
            Span::styled("  Current: ", Style::default().fg(CC_DIM)),
            Span::styled(
                self.level.label(),
                Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" · {}", self.level.description()),
                Style::default().fg(CC_DIM),
            ),
        ]);

        let hint_line = Line::from(Span::styled(
            "  [← → to adjust] [enter to confirm] [esc to cancel]",
            Style::default().fg(CC_DIM),
        ));

        let lines = vec![
            Line::raw(""),
            axis_line,
            labels_line,
            marker_line,
            Line::raw(""),
            desc_line,
            Line::raw(""),
            hint_line,
        ];

        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum EffortAction {
    Continue,
    Select(&'static str),
    Cancel,
}
