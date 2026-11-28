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

/// The transcript line for a monitor's event: which monitor, and the first
/// line of the event.
pub(crate) fn event_line(task: &TaskInfo, event: &str) -> String {
    let first = event.lines().next().unwrap_or_default();
    let first: String = first.chars().take(100).collect();
    let more = event.lines().count().saturating_sub(1);
    let mut line = format!(
        "monitor {} · {first}",
        task.name.as_deref().unwrap_or(&task.id)
    );
    if more > 0 {
        line.push_str(&format!(" (+{more} lines)"));
    }
    line
}

/// The transcript line for a message an agent sent with `SendMessage`: who
/// sent it, and its summary or else its first line.
pub(crate) fn message_line(from: &str, summary: Option<&str>, message: &str) -> String {
    let text = summary.unwrap_or_else(|| message.lines().next().unwrap_or_default());
    let text: String = text.chars().take(100).collect();
    format!("message from {from} · {text}")
}

/// The first line of `text`, at most 100 characters.
fn first_line(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(100)
        .collect()
}

/// The transcript line for a wake-up that came due: its reason, else the
/// first line of the prompt it brings back.
pub(crate) fn wakeup_line(wakeup: &super_coding::WakeupInfo) -> String {
    let shown = wakeup
        .reason
        .as_deref()
        .or(wakeup.prompt.as_deref())
        .unwrap_or_default();
    let mut line = format!("wake-up · {}", first_line(shown));
    if wakeup.noop {
        line.push_str(" · noop");
    }
    line
}

/// The transcript line for a round of a self-paced loop: which round, and
/// the reason of the wake-up that started it, else the loop's prompt.
pub(crate) fn round_line(
    round: u32,
    max_rounds: u32,
    prompt: &str,
    woken_by: Option<&super_coding::WakeupInfo>,
) -> String {
    let shown = woken_by
        .and_then(|wakeup| wakeup.reason.as_deref())
        .unwrap_or(prompt);
    let mut line = format!("loop round {round}/{max_rounds} · {}", first_line(shown));
    if woken_by.is_some_and(|wakeup| wakeup.noop) {
        line.push_str(" · noop");
    }
    line
}

/// The transcript line for a custom message that reached the agent from
/// outside a tool call: a task's notice, a monitor's event, a message of
/// an agent, a wake-up or a loop's round; `None` for any other message.
pub(crate) fn background_line(message: &super_agent::AgentMessage) -> Option<String> {
    if let Some(task) = super_coding::notifications::notified_task(message) {
        return Some(notice_line(&task));
    }
    if let Some((from, summary, text)) = super_coding::notifications::sent_message(message) {
        return Some(message_line(&from, summary.as_deref(), &text));
    }
    if let Some(wakeup) = super_coding::notifications::woken(message) {
        return Some(wakeup_line(&wakeup));
    }
    if let Some((round, max_rounds, prompt, woken_by)) =
        super_coding::notifications::looped(message)
    {
        return Some(round_line(round, max_rounds, &prompt, woken_by.as_ref()));
    }
    super_coding::notifications::monitored_event(message)
        .map(|(task, event)| event_line(&task, &event))
}

/// The transcript line for an entry of the session's own state
/// ([`super_coding::stored_state`]): a worktree whose folder was gone when
/// the session was loaded again, so that it works in `folder`, its own
/// folder; `None` for any other entry.
pub(crate) fn state_line(
    entry: &super_coding::SessionEntry,
    folder: &std::path::Path,
) -> Option<String> {
    let super_coding::SessionEntry::Custom {
        custom_type,
        data: Some(data),
        ..
    } = entry
    else {
        return None;
    };
    if custom_type != super_coding::stored_state::WORKTREE_ENTRY {
        return None;
    }
    let missing = data.get("missing")?.as_str()?;
    Some(format!(
        "worktree {missing} is gone · the session works in {}",
        folder.display()
    ))
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

    #[test]
    fn notices_and_monitor_events_have_a_transcript_line() {
        let session = session();
        let handle = session
            .background_tasks()
            .register(TaskSpec::new(TaskKind::Monitor, "watch the log"))
            .unwrap();
        let task = session.tasks().pop().unwrap();
        let event = super_coding::monitor_event(&task, "error: disk full\nretrying", 0);
        assert_eq!(
            background_line(&event).unwrap(),
            format!("monitor {} · error: disk full (+1 lines)", handle.id())
        );
        let mut ended = task.clone();
        ended.status = TaskStatus::Completed;
        let outcome = super_coding::TaskOutcome::completed("done");
        let notice = super_coding::task_notification(&ended, &outcome);
        assert_eq!(
            background_line(&notice).unwrap(),
            format!("task {} completed · watch the log", handle.id())
        );
        assert!(background_line(&super_agent::AgentMessage::user("hi")).is_none());

        let sent = super_coding::agent_message("a1234abcd", "main", None, "found it\nin line 4");
        assert_eq!(
            background_line(&sent).unwrap(),
            "message from a1234abcd · found it"
        );
        let summarized =
            super_coding::agent_message("a1234abcd", "main", Some("progress"), "long text");
        assert_eq!(
            background_line(&summarized).unwrap(),
            "message from a1234abcd · progress"
        );
    }

    #[test]
    fn a_worktree_gone_at_load_has_a_transcript_line_and_other_state_has_none() {
        let entry =
            |custom_type: &str, data: serde_json::Value| super_coding::SessionEntry::Custom {
                base: super_coding::session::entry::EntryBase {
                    id: "e1".into(),
                    parent_id: None,
                    timestamp: "t".into(),
                },
                custom_type: custom_type.into(),
                data: Some(data),
                extra: Default::default(),
            };
        let folder = std::path::Path::new("/work/project");
        let gone = entry(
            super_coding::stored_state::WORKTREE_ENTRY,
            serde_json::json!({"worktree": null, "missing": "/work/project/.super/worktrees/feature"}),
        );
        assert_eq!(
            state_line(&gone, folder).unwrap(),
            "worktree /work/project/.super/worktrees/feature is gone · the session works in /work/project"
        );
        let left = entry(
            super_coding::stored_state::WORKTREE_ENTRY,
            serde_json::json!({"worktree": null}),
        );
        assert_eq!(state_line(&left, folder), None);
        let wakeup = entry(
            super_coding::stored_state::WAKEUP_ENTRY,
            serde_json::json!({"wakeup": null, "loop": null}),
        );
        assert_eq!(state_line(&wakeup, folder), None);
    }

    #[test]
    fn wake_ups_and_loop_rounds_have_a_transcript_line_with_their_reason() {
        let mut wakeup = super_coding::WakeupInfo {
            id: 1,
            delay_seconds: 300,
            due_at: 0,
            reason: Some("CI runs for five minutes".into()),
            noop: false,
            prompt: Some("check CI\nand report".into()),
            job: None,
            round: None,
        };
        let message = super_coding::notifications::wakeup_message(&wakeup);
        assert_eq!(
            background_line(&message).unwrap(),
            "wake-up · CI runs for five minutes"
        );
        wakeup.reason = None;
        wakeup.noop = true;
        let message = super_coding::notifications::wakeup_message(&wakeup);
        assert_eq!(
            background_line(&message).unwrap(),
            "wake-up · check CI · noop"
        );

        let first = super_coding::notifications::loop_round(1, "tidy the docs", 1, 100, None);
        assert_eq!(
            background_line(&first).unwrap(),
            "loop round 1/100 · tidy the docs"
        );
        wakeup.reason = Some("nothing changed".into());
        let next =
            super_coding::notifications::loop_round(1, "tidy the docs", 2, 100, Some(&wakeup));
        assert_eq!(
            background_line(&next).unwrap(),
            "loop round 2/100 · nothing changed · noop"
        );
    }
}
