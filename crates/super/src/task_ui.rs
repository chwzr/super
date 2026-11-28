//! `/tasks`: the background tasks that are running, and a key to stop one;
//! the footer line while they run, and the key that stops them all.
//!
//! Background tasks keep running when a turn is interrupted, so this view
//! and that key are how the terminal stops them. Both read the session's
//! tasks each time: the registry of an in-process session, or what the
//! daemon reported for an attached one.

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
                    task.id,
                    task.kind.as_str(),
                    format_elapsed(elapsed),
                    described(task),
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

/// The footer line while background tasks run, or `None`: how many run,
/// and `stop_key`, the key that stops them, when one is bound.
pub(crate) fn progress_line(
    session: &AgentSession,
    theme: &Theme,
    width: usize,
    stop_key: Option<&KeyEvent>,
) -> Option<String> {
    let count = running(session).len();
    if count == 0 {
        return None;
    }
    let noun = if count == 1 { "task" } else { "tasks" };
    let mut line = format!("● {count} background {noun} running");
    if let Some(key) = stop_key {
        line.push_str(&format!(" · {} to stop", key.label()));
    }
    Some(theme.fg("accent", &text::truncate_to_width(&line, width)))
}

/// Stop every running background task of the session: commands, monitors,
/// agents and workflows. An attached session asks the daemon for each.
/// Returns how many were stopped; `None` when none was running, so that the
/// key does what it does otherwise.
pub(crate) fn stop_all(session: &AgentSession) -> Option<usize> {
    let tasks = running(session);
    if tasks.is_empty() {
        return None;
    }
    Some(
        tasks
            .iter()
            .filter(|task| session.stop_task(&task.id).is_ok())
            .count(),
    )
}

/// The transcript line after the key stopped `count` background tasks.
pub(crate) fn stopped_line(count: usize) -> String {
    let noun = if count == 1 { "task" } else { "tasks" };
    format!("stopped {count} background {noun}")
}

/// What a task is: its description, and its name after it when that is
/// something else (a workflow run's name, say). An agent's name is its
/// description.
fn described(task: &TaskInfo) -> String {
    match task.name.as_deref() {
        Some(name) if name != task.description => format!("{} ({name})", task.description),
        _ => task.description.clone(),
    }
}

/// The transcript line for a task's notice: which task ended and how. A
/// task is named by its id, as the tool result that started it and an
/// agent's messages name it.
pub(crate) fn notice_line(task: &TaskInfo) -> String {
    format!(
        "task {} {} · {}",
        task.id,
        task.status.as_str(),
        described(task)
    )
}

/// The transcript line for a monitor's event: which monitor, and the first
/// line of the event.
pub(crate) fn event_line(task: &TaskInfo, event: &str) -> String {
    let first = event.lines().next().unwrap_or_default();
    let first: String = first.chars().take(100).collect();
    let more = event.lines().count().saturating_sub(1);
    let mut line = format!("monitor {} · {first}", task.id);
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
/// folder; `None` for any other entry. A terminal attached to the daemon
/// shows it from the transcript, since the daemon said it when it loaded
/// the session; the terminal that runs the session shows it as it happens.
#[cfg_attr(not(unix), allow(dead_code))]
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
    Some(worktree_gone_line(std::path::Path::new(missing), folder))
}

/// The line for a worktree whose folder was gone when the session was
/// loaded again, so that the session works in `folder`.
pub(crate) fn worktree_gone_line(missing: &std::path::Path, folder: &std::path::Path) -> String {
    format!(
        "worktree {} is gone · the session works in {}",
        missing.display(),
        folder.display()
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
        assert!(progress_line(&session, &theme, 80, None).is_none());

        let tasks = session.background_tasks();
        let first = tasks
            .register(TaskSpec::new(TaskKind::Shell, "npm test"))
            .unwrap();
        let second = tasks
            .register(TaskSpec::new(TaskKind::Agent, "review the diff").with_name("reviewer"))
            .unwrap();
        let shown = plain(&view.render(80, &theme, &session));
        assert!(shown.contains(first.id()), "{shown}");
        assert!(shown.contains(second.id()), "{shown}");
        assert!(shown.contains("review the diff (reviewer)"), "{shown}");
        // An agent's task is named by its description: the row names its
        // id, and the description once, in line with the others.
        let agent = tasks
            .register(
                TaskSpec::new(TaskKind::Agent, "Review the parser").with_name("Review the parser"),
            )
            .unwrap();
        let shown = plain(&view.render(120, &theme, &session));
        let row = shown
            .lines()
            .find(|line| line.contains(agent.id()))
            .unwrap();
        assert_eq!(row.matches("Review the parser").count(), 1, "{row}");
        let column = |id: &str| {
            let line = shown.lines().find(|line| line.contains(id)).unwrap();
            line[..line.find(id).unwrap()].chars().count()
        };
        assert_eq!(column(agent.id()), column(first.id()));
        session.stop_task(agent.id()).unwrap();
        assert!(
            text::strip_ansi(&progress_line(&session, &theme, 80, None).unwrap())
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
            format!("task {} stopped · review the diff (reviewer)", second.id())
        );
    }

    #[test]
    fn the_footer_names_the_key_that_stops_every_background_task() {
        let session = session();
        let theme = Theme::dark();
        let ctrl_x = KeyEvent::parse("ctrl+x").unwrap();
        let line = |key: Option<&KeyEvent>| {
            progress_line(&session, &theme, 80, key).map(|line| text::strip_ansi(&line))
        };
        assert_eq!(line(Some(&ctrl_x)), None);
        assert_eq!(stop_all(&session), None, "nothing runs, nothing to stop");

        let tasks = session.background_tasks();
        let command = tasks
            .register(TaskSpec::new(TaskKind::Shell, "npm test"))
            .unwrap();
        assert_eq!(
            line(Some(&ctrl_x)).unwrap(),
            "● 1 background task running · Ctrl+X to stop"
        );
        let monitor = tasks
            .register(TaskSpec::new(TaskKind::Monitor, "watch the log"))
            .unwrap();
        let agent = tasks
            .register(TaskSpec::new(TaskKind::Agent, "review the diff"))
            .unwrap();
        let workflow = tasks
            .register(TaskSpec::new(TaskKind::Workflow, "audit"))
            .unwrap();
        let ended = tasks
            .register(TaskSpec::new(TaskKind::Shell, "ls"))
            .unwrap();
        session.stop_task(ended.id()).unwrap();
        assert_eq!(
            line(Some(&ctrl_x)).unwrap(),
            "● 4 background tasks running · Ctrl+X to stop"
        );
        // A key the user bound instead is the one the line names.
        let ctrl_b = KeyEvent::parse("ctrl+b").unwrap();
        assert_eq!(
            line(Some(&ctrl_b)).unwrap(),
            "● 4 background tasks running · Ctrl+B to stop"
        );
        assert_eq!(line(None).unwrap(), "● 4 background tasks running");

        assert_eq!(stop_all(&session), Some(4));
        for handle in [&command, &monitor, &agent, &workflow] {
            assert!(handle.is_stopped());
        }
        assert_eq!(stopped_line(4), "stopped 4 background tasks");
        assert_eq!(stopped_line(1), "stopped 1 background task");
        assert_eq!(line(Some(&ctrl_x)), None);
        assert_eq!(stop_all(&session), None);
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
