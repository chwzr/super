//! `/tasks`: the background tasks that are running, and a key to stop one.
//!
//! Background tasks keep running when a turn is interrupted, so this view is
//! how the terminal stops one. It reads the session's tasks each time it
//! draws: the registry of an in-process session, or what the daemon reported
//! for an attached one.

use crate::workflow_ui::format_elapsed;
use std::time::Duration;
use super_coding::{AgentSession, TaskInfo};
use super_tui::{Key, KeyEvent, Theme, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskViewAction {
    None,
    Close,
}

#[derive(Default)]
pub(crate) struct TaskView {
    selected: usize,
}

/// The tasks the view lists: the running ones, oldest first.
fn running(session: &AgentSession) -> Vec<TaskInfo> {
    session
        .tasks()
        .into_iter()
        .filter(|task| task.status.is_running())
        .collect()
}

impl TaskView {
    pub(crate) fn handle_key(&mut self, key: &KeyEvent, session: &AgentSession) -> TaskViewAction {
        let tasks = running(session);
        match key.key {
            Key::Escape | Key::Left => return TaskViewAction::Close,
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(tasks.len().saturating_sub(1)),
            Key::Char('x') => {
                if let Some(task) = tasks.get(self.selected.min(tasks.len().saturating_sub(1))) {
                    // Gone already or never there: the list redraws without it.
                    let _ = session.stop_task(&task.id);
                }
            }
            _ => {}
        }
        TaskViewAction::None
    }

    pub(crate) fn render(
        &mut self,
        width: usize,
        theme: &Theme,
        session: &AgentSession,
    ) -> Vec<String> {
        let tasks = running(session);
        let mut lines = vec![theme.fg("accent", &theme.bold("Tasks")), String::new()];
        if tasks.is_empty() {
            lines.push(theme.fg("muted", "No background tasks are running."));
        } else {
            self.selected = self.selected.min(tasks.len() - 1);
            let now = super_ai::now_ms();
            for (index, task) in tasks.iter().enumerate() {
                let marker = if index == self.selected { "▸" } else { " " };
                let elapsed =
                    Duration::from_millis(now.saturating_sub(task.started_at).max(0) as u64);
                let row = format!(
                    "{marker} {:<10} {:<9} {:>6}  {}",
                    task.name.as_deref().unwrap_or(&task.id),
                    task.kind.as_str(),
                    format_elapsed(elapsed),
                    task.description,
                );
                lines.push(if index == self.selected {
                    theme.fg("accent", &theme.bold(&text::truncate_to_width(&row, width)))
                } else {
                    theme.fg("toolOutput", &text::truncate_to_width(&row, width))
                });
                if index == self.selected
                    && let Some(progress) = &task.progress
                {
                    lines.push(theme.fg(
                        "muted",
                        &text::truncate_to_width(&format!("    {progress}"), width),
                    ));
                }
            }
        }
        lines.push(String::new());
        lines.push(theme.fg(
            "dim",
            &text::truncate_to_width("↑↓ select · x stop · esc close", width),
        ));
        lines
    }
}

/// The footer line while background tasks run, or `None`.
pub(crate) fn progress_line(session: &AgentSession, theme: &Theme, width: usize) -> Option<String> {
    let count = running(session).len();
    if count == 0 {
        return None;
    }
    let noun = if count == 1 { "task" } else { "tasks" };
    Some(theme.fg(
        "accent",
        &text::truncate_to_width(
            &format!("● {count} background {noun} running · /tasks opens the task view"),
            width,
        ),
    ))
}

/// The transcript line for a task's notice: which task ended and how.
pub(crate) fn notice_line(task: &TaskInfo) -> String {
    format!(
        "task {} {} · {}",
        task.name.as_deref().unwrap_or(&task.id),
        task.status.as_str(),
        task.description
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use super_ai::{Model, Registry, ThinkingLevel};
    use super_coding::{SessionManager, Settings, TaskKind, TaskSpec, TaskStatus};

    fn session() -> Arc<AgentSession> {
        let registry = Registry::from_builtin();
        let model: Model = registry.all().first().expect("built-in model").clone();
        AgentSession::new(
            SessionManager::in_memory(std::path::Path::new("/test")),
            Vec::new(),
            registry,
            Settings::default(),
            "test".into(),
            model,
            ThinkingLevel::Off,
            None,
            Arc::new(|_| {}),
        )
    }

    fn key(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            ..Default::default()
        }
    }

    fn plain(lines: &[String]) -> String {
        lines
            .iter()
            .map(|line| text::strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_view_lists_running_tasks_and_x_stops_the_selected_one() {
        let session = session();
        let theme = Theme::dark();
        let mut view = TaskView::default();
        assert!(
            plain(&view.render(80, &theme, &session)).contains("No background tasks are running.")
        );
        assert!(progress_line(&session, &theme, 80).is_none());

        let tasks = session.background_tasks();
        let first = tasks
            .register(TaskSpec::new(TaskKind::Shell, "npm test"))
            .unwrap();
        let second = tasks
            .register(TaskSpec::new(TaskKind::Agent, "review the diff").with_name("reviewer"))
            .unwrap();
        let shown = plain(&view.render(80, &theme, &session));
        assert!(shown.contains(first.id()), "{shown}");
        assert!(shown.contains("reviewer"), "{shown}");
        assert!(shown.contains("review the diff"), "{shown}");
        assert!(
            text::strip_ansi(&progress_line(&session, &theme, 80).unwrap())
                .contains("2 background tasks running")
        );

        view.handle_key(&key(Key::Down), &session);
        view.handle_key(&KeyEvent::char('x'), &session);
        assert!(second.is_stopped());
        assert!(!first.is_stopped());
        assert!(!plain(&view.render(80, &theme, &session)).contains("reviewer"));
        assert_eq!(
            view.handle_key(&key(Key::Escape), &session),
            TaskViewAction::Close
        );
        let stopped = session
            .tasks()
            .into_iter()
            .find(|task| task.status == TaskStatus::Stopped)
            .unwrap();
        assert_eq!(
            notice_line(&stopped),
            "task reviewer stopped · review the diff"
        );
    }
}
