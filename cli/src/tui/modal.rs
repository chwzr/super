use crossterm::event::KeyEvent;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::CC_BLUE;
use crate::tui::modals::effort_picker::{EffortAction, EffortPicker};
use crate::tui::modals::model_picker::{ModelAction, ModelPicker};
use crate::tui::modals::resume_picker::{ResumeAction, ResumePicker};
use crate::tui::modals::status_view::{StatusAction, StatusView};

pub enum ModalAction {
    Continue,
    Close,
    SetModel(String),
    SetEffort(String),
}

pub enum Modal {
    Model(ModelPicker),
    Effort(EffortPicker),
    Resume(ResumePicker),
    Status(StatusView),
}

impl Modal {
    pub fn title(&self) -> &str {
        match self {
            Modal::Model(_) => "Set Model",
            Modal::Effort(_) => "Effort Level",
            Modal::Resume(_) => "Resume Session",
            Modal::Status(_) => "Super",
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ModalAction {
        match self {
            Modal::Model(p) => match p.handle_key(key) {
                ModelAction::Continue      => ModalAction::Continue,
                ModelAction::Select(id)    => ModalAction::SetModel(id.to_string()),
                ModelAction::Cancel        => ModalAction::Close,
            },
            Modal::Effort(p) => match p.handle_key(key) {
                EffortAction::Continue   => ModalAction::Continue,
                EffortAction::Select(v)  => ModalAction::SetEffort(v.to_string()),
                EffortAction::Cancel     => ModalAction::Close,
            },
            Modal::Resume(p) => match p.handle_key(key) {
                ResumeAction::Continue => ModalAction::Continue,
                ResumeAction::Cancel   => ModalAction::Close,
            },
            Modal::Status(v) => match v.handle_key(key) {
                StatusAction::Continue => ModalAction::Continue,
                StatusAction::Cancel   => ModalAction::Close,
            },
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        if area.height < 2 { return; }
        let sep_area = Rect { height: 1, ..area };
        let content_area = Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            ..area
        };
        render_separator(f, sep_area, self.title());
        match self {
            Modal::Model(p) => p.render(f, content_area),
            Modal::Effort(p) => p.render(f, content_area),
            Modal::Resume(p) => p.render(f, content_area),
            Modal::Status(v) => v.render(f, content_area),
        }
    }
}

fn render_separator(f: &mut Frame, area: Rect, title: &str) {
    let total_w = area.width as usize;
    let title_with_spaces = format!(" {} ", title);
    let title_len = title_with_spaces.len();
    let dashes = total_w.saturating_sub(title_len);
    let left  = dashes / 2;
    let right = dashes - left;

    let sep_line = Line::from(vec![
        Span::styled("─".repeat(left),  Style::default().fg(CC_BLUE)),
        Span::styled(title_with_spaces, Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD)),
        Span::styled("─".repeat(right), Style::default().fg(CC_BLUE)),
    ]);
    f.render_widget(Paragraph::new(sep_line), area);
}
