use std::time::Instant;

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Cycling glyph set used while the agent is working.
/// Mirrors Claude Code's cycling sparkle but uses the Super diamond family
/// declared in the project plan.
static GLYPHS: &[&str] = &["⟣", "⟡", "⟐", "◈", "⟢"];
/// Idle state glyph shown when the agent is awaiting input.
const IDLE_GLYPH: &str = "♦";

/// Verb rotation matching Claude Code's whimsical set. Picked once when the
/// activity becomes Active and persists until completion (so the trail line
/// reads back consistently).
static WORKING_VERBS: &[&str] = &[
    "Thinking",
    "Cogitating",
    "Churning",
    "Pondering",
    "Shenaniganing",
    "Jitterbugging",
    "Ruminating",
    "Hatching",
    "Deliberating",
    "Brewing",
];

/// Random-ish tips shown below the spinner.
static TIPS: &[&str] = &[
    "Tip: Use /clear to wipe the conversation history",
    "Tip: Use /model to switch model",
    "Tip: Ctrl+L clears the visible scrollback",
    "Tip: Press ↑ to recall a previous prompt",
    "Tip: Use /help to see all commands",
];

fn pick<T: Copy>(slice: &[T]) -> T {
    use std::time::SystemTime;
    let n = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as usize)
        .unwrap_or(0);
    slice[n % slice.len()]
}

pub enum ActivityState {
    Idle,
    Active {
        verb: String,
        tip: String,
        started: Instant,
        tokens: u64,
        glyph_idx: usize,
    },
}

impl ActivityState {
    pub fn idle() -> Self {
        Self::Idle
    }

    pub fn active(verb: &str) -> Self {
        let verb = if verb.is_empty() {
            pick(WORKING_VERBS).to_string()
        } else {
            verb.to_string()
        };
        Self::Active {
            verb,
            tip: pick(TIPS).to_string(),
            started: Instant::now(),
            tokens: 0,
            glyph_idx: 0,
        }
    }

    pub fn tick(&mut self) {
        if let Self::Active { glyph_idx, .. } = self {
            *glyph_idx = (*glyph_idx + 1) % GLYPHS.len();
        }
    }

    pub fn verb(&self) -> Option<&str> {
        match self {
            Self::Active { verb, .. } => Some(verb),
            Self::Idle => None,
        }
    }

    /// Total height (rows) this widget renders.
    pub fn height(&self) -> u16 {
        match self {
            Self::Idle => 0,
            Self::Active { .. } => 2,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        match self {
            Self::Idle => {
                let line = Line::from(vec![Span::styled(
                    IDLE_GLYPH,
                    Style::default().fg(Color::DarkGray),
                )]);
                f.render_widget(Paragraph::new(line), area);
            }
            Self::Active {
                verb,
                tip,
                started,
                tokens,
                glyph_idx,
            } => {
                let elapsed = started.elapsed().as_secs();
                let meta = if *tokens > 0 {
                    format!(" ({}s · ↓ {} tokens)", elapsed, tokens)
                } else {
                    format!(" ({}s)", elapsed)
                };
                let spinner = Line::from(vec![
                    Span::styled(GLYPHS[*glyph_idx], Style::default().fg(Color::Cyan)),
                    Span::raw(" "),
                    Span::styled(verb.clone(), Style::default().fg(Color::White)),
                    Span::styled("…", Style::default().fg(Color::White)),
                    Span::styled(meta, Style::default().fg(Color::DarkGray)),
                ]);
                let tip_line = Line::from(vec![
                    Span::styled("  ⎿  ", Style::default().fg(Color::DarkGray)),
                    Span::styled(tip.clone(), Style::default().fg(Color::DarkGray)),
                ]);
                let paragraph = Paragraph::new(vec![spinner, tip_line]);
                f.render_widget(paragraph, area);
            }
        }
    }
}
