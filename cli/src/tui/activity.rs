use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

static GLYPHS: &[&str] = &["⟣", "⟡", "⟐", "◈", "⟢"];
const IDLE_GLYPH: &str = "♦";

pub enum ActivityState {
    Idle,
    Active { verb: String, glyph_idx: usize },
}

impl ActivityState {
    #[allow(dead_code)]
    pub fn idle() -> Self {
        Self::Idle
    }

    #[allow(dead_code)]
    pub fn active(verb: &str) -> Self {
        Self::Active {
            verb: verb.to_string(),
            glyph_idx: 0,
        }
    }

    pub fn tick(&mut self) {
        if let Self::Active { glyph_idx, .. } = self {
            *glyph_idx = (*glyph_idx + 1) % GLYPHS.len();
        }
    }

    pub fn render(&self) -> Line<'_> {
        match self {
            Self::Idle => Line::from(vec![Span::styled(
                IDLE_GLYPH,
                Style::default().fg(Color::White),
            )]),
            Self::Active { verb, glyph_idx } => Line::from(vec![
                Span::styled(GLYPHS[*glyph_idx], Style::default().fg(Color::Cyan)),
                Span::raw("  "),
                Span::styled(verb.as_str(), Style::default().fg(Color::White)),
            ]),
        }
    }
}
