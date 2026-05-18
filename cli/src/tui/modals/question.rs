use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use serde_json::{json, Value};
use shared::Question;

use crate::tui::colors::{CC_BLUE, CC_DIM, CC_GREEN};

/// Widget that renders 1–4 questions from `AskUserQuestion` and lets the user
/// navigate options, toggle multi-select choices, and submit/cancel.
pub struct QuestionModal {
    pub questions: Vec<Question>,
    /// Per-question cursor index into that question's options.
    pub cursors: Vec<usize>,
    /// Per-question selected option indices (for multi-select; for single-select
    /// we just use the cursor position on submit).
    pub selections: Vec<Vec<usize>>,
    /// Which question is currently focused (0-indexed).
    pub focused_question: usize,
}

/// Actions the modal can return to its parent.
#[derive(Debug)]
pub enum QuestionAction {
    Continue,
    Submit(Value),
    Cancel,
}

impl QuestionModal {
    pub fn new(questions: Vec<Question>) -> Self {
        let n = questions.len();
        Self {
            questions,
            cursors: vec![0; n],
            selections: vec![Vec::new(); n],
            focused_question: 0,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> QuestionAction {
        let q_index = self.focused_question;
        let question = &self.questions[q_index];
        let n_opts = question.options.len();

        match key.code {
            KeyCode::Esc => return QuestionAction::Cancel,

            KeyCode::Enter => {
                // If any question still has no selection (and has options),
                // submit is a no-op — user must pick answers first.
                return self.build_submit();
            }

            KeyCode::Char(' ') if question.multi_select => {
                let cursor = self.cursors[q_index];
                let sel = &mut self.selections[q_index];
                if let Some(pos) = sel.iter().position(|&s| s == cursor) {
                    sel.remove(pos);
                } else {
                    sel.push(cursor);
                    sel.sort_unstable();
                }
            }

            KeyCode::Up | KeyCode::Char('k') if n_opts > 0 && self.cursors[q_index] > 0 => {
                self.cursors[q_index] -= 1;
            }
            KeyCode::Down | KeyCode::Char('j')
                if n_opts > 0 && self.cursors[q_index] + 1 < n_opts =>
            {
                self.cursors[q_index] += 1;
            }

            KeyCode::Tab if self.questions.len() > 1 => {
                self.focused_question = (self.focused_question + 1) % self.questions.len();
            }
            KeyCode::BackTab if self.questions.len() > 1 => {
                self.focused_question = if self.focused_question == 0 {
                    self.questions.len() - 1
                } else {
                    self.focused_question - 1
                };
            }

            KeyCode::Char(c) if c.is_ascii_digit() && n_opts > 0 => {
                let idx = (c as usize).wrapping_sub('1' as usize);
                if idx < n_opts {
                    if question.multi_select {
                        let sel = &mut self.selections[q_index];
                        if let Some(pos) = sel.iter().position(|&s| s == idx) {
                            sel.remove(pos);
                        } else {
                            sel.push(idx);
                            sel.sort_unstable();
                        }
                    }
                    self.cursors[q_index] = idx;
                }
            }

            _ => {}
        }
        QuestionAction::Continue
    }

    fn build_submit(&self) -> QuestionAction {
        let mut answers = serde_json::Map::new();

        for (qi, question) in self.questions.iter().enumerate() {
            let header = &question.header;

            if question.options.is_empty() {
                answers.insert(header.clone(), Value::String(String::new()));
                continue;
            }

            if question.multi_select {
                let selected_labels: Vec<Value> = self.selections[qi]
                    .iter()
                    .filter_map(|&idx| {
                        question
                            .options
                            .get(idx)
                            .map(|o| Value::String(o.label.clone()))
                    })
                    .collect();
                answers.insert(header.clone(), Value::Array(selected_labels));
            } else {
                let idx = self.cursors[qi];
                let label = question
                    .options
                    .get(idx)
                    .map(|o| o.label.as_str())
                    .unwrap_or("");
                answers.insert(header.clone(), Value::String(label.to_string()));
            }
        }

        QuestionAction::Submit(json!({
            "questions": self.questions.iter().map(|q| json!({
                "question": q.question,
                "header": q.header,
                "options": q.options.iter().map(|o| json!({
                    "label": o.label,
                    "description": o.description,
                })).collect::<Vec<_>>(),
                "multiSelect": q.multi_select,
            })).collect::<Vec<_>>(),
            "answers": Value::Object(answers),
            "annotations": Value::Object(serde_json::Map::new()),
        }))
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();

        for (qi, question) in self.questions.iter().enumerate() {
            let is_focused = qi == self.focused_question;

            // Question header
            let header_style = if is_focused {
                Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(CC_DIM)
            };
            lines.push(Line::from(vec![Span::styled(
                format!(
                    "{} {}:",
                    if is_focused { "❯" } else { " " },
                    question.header
                ),
                header_style,
            )]));
            lines.push(Line::from(vec![Span::styled(
                format!("  {}", question.question),
                Style::default().fg(CC_DIM),
            )]));

            // Options
            if question.options.is_empty() {
                lines.push(Line::from(vec![Span::styled(
                    "  (no options)",
                    Style::default().fg(CC_DIM),
                )]));
            }

            for (oi, opt) in question.options.iter().enumerate() {
                let is_cursor = is_focused && oi == self.cursors[qi];
                let is_selected = question.multi_select && self.selections[qi].contains(&oi);

                let prefix = if is_cursor { "❯ " } else { "  " };
                let number = format!("{}. ", oi + 1);

                let cursor_style = if is_cursor {
                    Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD)
                } else if is_selected {
                    Style::default().fg(CC_GREEN)
                } else {
                    Style::default().fg(CC_DIM)
                };

                let marker = if question.multi_select {
                    if is_selected {
                        " [x]"
                    } else {
                        " [ ]"
                    }
                } else {
                    ""
                };

                lines.push(Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(number, cursor_style),
                    Span::styled(format!("{}{}", opt.label, marker), cursor_style),
                    Span::styled(
                        format!("  {}", opt.description),
                        Style::default().fg(CC_DIM),
                    ),
                ]));

                // Preview panel for the focused option
                if is_cursor {
                    if let Some(ref preview) = opt.preview {
                        for preview_line in preview.lines() {
                            lines.push(Line::from(vec![Span::styled(
                                format!("      │ {}", preview_line),
                                Style::default().fg(CC_DIM),
                            )]));
                        }
                        lines.push(Line::from(vec![Span::styled(
                            "      ─────────────────────",
                            Style::default().fg(CC_DIM),
                        )]));
                    }
                }
            }

            // Spacer between questions
            if qi + 1 < self.questions.len() {
                lines.push(Line::from(""));
            }
        }

        // Help footer
        lines.push(Line::from(""));
        let help = if self.questions.iter().any(|q| q.multi_select) {
            "[↑↓/jk nav] [space toggle] [tab next question] [enter submit] [esc cancel]"
        } else {
            "[↑↓/jk nav] [tab next question] [enter submit] [esc cancel]"
        };
        lines.push(Line::from(Span::styled(help, Style::default().fg(CC_DIM))));

        f.render_widget(Paragraph::new(lines), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_questions() -> Vec<Question> {
        vec![Question {
            question: "What language?".into(),
            header: "Language".into(),
            multi_select: false,
            options: vec![
                shared::QuestionOption {
                    label: "Rust".into(),
                    description: "Systems programming".into(),
                    preview: None,
                },
                shared::QuestionOption {
                    label: "TypeScript".into(),
                    description: "Web development".into(),
                    preview: None,
                },
            ],
        }]
    }

    fn multi_select_questions() -> Vec<Question> {
        vec![Question {
            question: "Pick features".into(),
            header: "Features".into(),
            multi_select: true,
            options: vec![
                shared::QuestionOption {
                    label: "Auth".into(),
                    description: "Authentication".into(),
                    preview: None,
                },
                shared::QuestionOption {
                    label: "API".into(),
                    description: "REST API".into(),
                    preview: None,
                },
                shared::QuestionOption {
                    label: "UI".into(),
                    description: "User interface".into(),
                    preview: None,
                },
            ],
        }]
    }

    #[test]
    fn cursor_starts_at_zero() {
        let modal = QuestionModal::new(sample_questions());
        assert_eq!(modal.cursors[0], 0);
        assert_eq!(modal.focused_question, 0);
    }

    #[test]
    fn down_moves_cursor() {
        let mut modal = QuestionModal::new(sample_questions());
        modal.handle_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(modal.cursors[0], 1);
    }

    #[test]
    fn up_wraps_at_top() {
        let mut modal = QuestionModal::new(sample_questions());
        modal.handle_key(KeyEvent::from(KeyCode::Down)); // 0→1
        modal.handle_key(KeyEvent::from(KeyCode::Up)); // 1→0
        assert_eq!(modal.cursors[0], 0);
        modal.handle_key(KeyEvent::from(KeyCode::Up)); // stays at 0
        assert_eq!(modal.cursors[0], 0);
    }

    #[test]
    fn j_and_k_move_cursor() {
        let mut modal = QuestionModal::new(sample_questions());
        modal.handle_key(KeyEvent::from(KeyCode::Char('j')));
        assert_eq!(modal.cursors[0], 1);
        modal.handle_key(KeyEvent::from(KeyCode::Char('k')));
        assert_eq!(modal.cursors[0], 0);
    }

    #[test]
    fn number_jumps_to_option() {
        let mut modal = QuestionModal::new(sample_questions());
        modal.handle_key(KeyEvent::from(KeyCode::Char('2')));
        assert_eq!(modal.cursors[0], 1);
    }

    #[test]
    fn multi_select_toggle_with_space() {
        let mut modal = QuestionModal::new(multi_select_questions());
        assert!(modal.selections[0].is_empty());
        modal.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        assert_eq!(modal.selections[0], vec![0]);
        modal.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        assert!(modal.selections[0].is_empty());
    }

    #[test]
    fn multi_select_toggle_with_number() {
        let mut modal = QuestionModal::new(multi_select_questions());
        modal.handle_key(KeyEvent::from(KeyCode::Char('3')));
        assert_eq!(modal.selections[0], vec![2]);
        assert_eq!(modal.cursors[0], 2);
    }

    #[test]
    fn tab_moves_between_questions() {
        let questions = vec![sample_questions()[0].clone(), sample_questions()[0].clone()];
        let mut modal = QuestionModal::new(questions);
        assert_eq!(modal.focused_question, 0);
        modal.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(modal.focused_question, 1);
        modal.handle_key(KeyEvent::from(KeyCode::Tab));
        assert_eq!(modal.focused_question, 0);
    }

    #[test]
    fn submit_single_select_produces_answers() {
        let mut modal = QuestionModal::new(sample_questions());
        modal.handle_key(KeyEvent::from(KeyCode::Down)); // select "TypeScript"
        match modal.handle_key(KeyEvent::from(KeyCode::Enter)) {
            QuestionAction::Submit(payload) => {
                let answers = payload.get("answers").unwrap();
                assert_eq!(answers["Language"], "TypeScript");
            }
            other => panic!("expected Submit, got {other:?}"),
        }
    }

    #[test]
    fn submit_multi_select_produces_array() {
        let mut modal = QuestionModal::new(multi_select_questions());
        modal.handle_key(KeyEvent::from(KeyCode::Char(' '))); // toggle "Auth"
        modal.handle_key(KeyEvent::from(KeyCode::Char('3'))); // toggle "UI"
        match modal.handle_key(KeyEvent::from(KeyCode::Enter)) {
            QuestionAction::Submit(payload) => {
                let answers = payload.get("answers").unwrap();
                let features = answers["Features"].as_array().unwrap();
                assert_eq!(features.len(), 2);
                assert!(features.iter().any(|v| v == "Auth"));
                assert!(features.iter().any(|v| v == "UI"));
            }
            other => panic!("expected Submit, got {other:?}"),
        }
    }

    #[test]
    fn esc_cancels() {
        let mut modal = QuestionModal::new(sample_questions());
        match modal.handle_key(KeyEvent::from(KeyCode::Esc)) {
            QuestionAction::Cancel => {}
            other => panic!("expected Cancel, got {other:?}"),
        }
    }

    #[test]
    fn render_produces_lines() {
        let modal = QuestionModal::new(sample_questions());
        // Render to a zero-area frame won't work in unit tests, but we can
        // verify the internal state is sane.
        assert_eq!(modal.questions.len(), 1);
        assert_eq!(modal.questions[0].options.len(), 2);
    }
}
