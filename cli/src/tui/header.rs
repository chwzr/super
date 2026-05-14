use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::splash::MASCOT;

pub struct Header {
    pub version: String,
    pub model_label: String,
    pub provider: String,
    pub user_handle: String,
    pub cwd: String,
}

impl Header {
    pub fn new(version: String, model: String, provider: String, cwd: String) -> Self {
        Self {
            version,
            model_label: model,
            provider,
            user_handle: "super".to_string(),
            cwd,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let mascot_style = Style::default().fg(Color::Cyan);
        let title_style = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);
        let meta_style = Style::default().fg(Color::Gray);

        let gap = "   ";
        let lines = vec![
            Line::from(Span::styled(MASCOT[0], mascot_style)),
            Line::from(vec![
                Span::styled(MASCOT[1], mascot_style),
                Span::raw(gap),
                Span::styled(format!("Super CLI v{}", self.version), title_style),
            ]),
            Line::from(vec![
                Span::styled(MASCOT[2], mascot_style),
                Span::raw(gap),
                Span::styled(
                    format!("{} · {}", self.model_label, self.provider),
                    meta_style,
                ),
            ]),
            Line::from(vec![
                Span::styled(MASCOT[3], mascot_style),
                Span::raw(gap),
                Span::styled(
                    format!("@{} · {}", self.user_handle, self.cwd),
                    meta_style,
                ),
            ]),
            Line::from(Span::styled(MASCOT[4], mascot_style)),
            Line::from(Span::styled(MASCOT[5], mascot_style)),
        ];

        let paragraph = Paragraph::new(lines);
        f.render_widget(paragraph, area);
    }
}
