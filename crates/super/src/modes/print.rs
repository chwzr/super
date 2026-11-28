//! Print mode: run one prompt to completion, print the final assistant
//! text, exit non-zero on error.
//!
//! Completion includes the background tasks the run started: the mode waits
//! for them and lets their notices start the turns they start, and for a
//! pending wake-up and the turn it starts, then prints the last answer.
//! SIGINT or SIGTERM cancels the turn, stops the tasks and ends the wait.

use crate::args::Args;
use crate::setup::build_startup;
use anyhow::Result;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_agent::{AgentEvent, AgentMessage};
use super_coding::AgentSession;
use super_coding::session_runner::{PromptMode, SessionEvent, WorkflowTurnStatus};

pub async fn run(args: &Args) -> Result<i32> {
    let final_text: Arc<Mutex<Vec<String>>> = Default::default();
    let error: Arc<Mutex<Option<String>>> = Default::default();
    let workflow_outcomes: Arc<Mutex<Vec<String>>> = Default::default();
    let sink = {
        let final_text = final_text.clone();
        let error = error.clone();
        let workflow_outcomes = workflow_outcomes.clone();
        Arc::new(move |event: SessionEvent| match event {
            SessionEvent::Agent(agent_event) => {
                if let AgentEvent::MessageEnd {
                    message: AgentMessage::Assistant(a),
                } = *agent_event
                {
                    if a.stop_reason == super_ai::StopReason::Error {
                        *error.lock().unwrap() = Some(a.error_message.clone().unwrap_or_default());
                    } else {
                        let text = a.text();
                        if !text.is_empty() {
                            final_text.lock().unwrap().push(text);
                        }
                    }
                }
            }
            // A worktree the session worked in was gone when it was loaded.
            SessionEvent::WorktreeChanged {
                folder,
                missing: Some(missing),
                ..
            } => eprintln!(
                "worktree {} is gone · the session works in {}",
                missing.display(),
                folder.display()
            ),
            SessionEvent::WorkflowOutcome { run, name, status } => {
                let run = run.map(|id| format!(" run {id}")).unwrap_or_default();
                let result = match status {
                    WorkflowTurnStatus::Cancelled => "was cancelled. No agents ran",
                    WorkflowTurnStatus::Completed => "completed",
                    WorkflowTurnStatus::Failed => "failed",
                    WorkflowTurnStatus::Stopped => "was stopped",
                };
                workflow_outcomes
                    .lock()
                    .unwrap()
                    .push(format!("verified workflow{run} `{name}` {result}"));
            }
            _ => {}
        })
    };

    let startup = build_startup(args, false, sink).await?;
    let _session_lock = crate::setup::keep_session_lock(&startup.session)?;

    // Merge piped stdin into the prompt.
    let mut prompt = startup.initial_message.clone().unwrap_or_default();
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let mut piped = String::new();
        std::io::stdin().read_to_string(&mut piped)?;
        let piped = piped.trim();
        if !piped.is_empty() {
            prompt = if prompt.is_empty() {
                piped.to_string()
            } else {
                format!("{piped}\n\n{prompt}")
            };
        }
    }
    if prompt.trim().is_empty() {
        anyhow::bail!("no prompt provided (pass a message or pipe stdin)");
    }

    let prompt_mode = startup.session.prompt_mode_for(&prompt);
    let finished = run_one_prompt(
        &startup.session,
        AgentMessage::user(prompt),
        prompt_mode,
        told_to_end(),
    )
    .await;
    // Nobody can be asked at the end here: a worktree the session made
    // stays, and the session is in it when it is opened again.
    if let Some(note) = kept_worktree_note(&startup.session) {
        eprintln!("{note}");
    }
    if !finished {
        // Told to end: the turn was cancelled and the tasks stopped.
        return Ok(130);
    }

    if let Some(err) = error.lock().unwrap().as_ref() {
        eprintln!("error: {err}");
        return Ok(1);
    }
    let texts = final_text.lock().unwrap();
    if let Some(last) = texts.last() {
        println!("{last}");
    }
    for outcome in workflow_outcomes.lock().unwrap().iter() {
        println!("\n{outcome}");
    }
    Ok(0)
}

/// Run one prompt to its end in a session that gets no further input: the
/// prompt's run, then its background tasks and the turns their notices
/// start, and a pending wake-up (a stored one too) and the turns it starts.
/// When `end` resolves first the turn is cancelled, the background tasks are
/// stopped and the wait ends, and `false` comes back; a pending wake-up stays
/// stored with the session.
pub(crate) async fn run_one_prompt(
    session: &Arc<AgentSession>,
    prompt: AgentMessage,
    mode: PromptMode,
    end: impl std::future::Future<Output = ()>,
) -> bool {
    // What a session loaded with `--session` or `-c` stored starts too.
    session.resume_stored_work();
    let work = async {
        session.prompt_with_mode(vec![prompt], mode).await;
        session.wait_for_all_work().await;
    };
    tokio::select! {
        () = work => return true,
        () = end => {}
    }
    session.abort();
    // The run ends with the process: what it leaves unfinished was cut off.
    session.set_closing();
    session.stop_background_tasks();
    // The cancelled turn records its end; it does not take long.
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while session.is_running() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    false
}

/// The worktree the session made with `EnterWorktree` and works in, when
/// it ends where nobody can be asked whether to keep or remove it (`-p`,
/// `--mode json`, `--mode rpc`, ACP): it is kept.
pub(crate) fn kept_worktree(session: &AgentSession) -> Option<super_coding::EnteredWorktree> {
    session.worktree().filter(|entered| entered.created)
}

/// What such a session says at its end about the worktree it keeps.
pub(crate) fn kept_worktree_note(session: &AgentSession) -> Option<String> {
    let entered = kept_worktree(session)?;
    Some(format!(
        "The worktree {} and its branch {} are kept.",
        entered.worktree.path.display(),
        entered.worktree.branch
    ))
}

/// Resolves on SIGINT or, on Unix, SIGTERM.
pub(crate) async fn told_to_end() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super_ai::{AssistantEvent, AssistantMessage, ContentBlock, EventStream, StopReason};
    use super_coding::{TaskKind, TaskOutcome, TaskSpec};

    /// A session whose provider answers every request with one line, and
    /// counts the requests.
    fn session() -> (Arc<AgentSession>, Arc<Mutex<usize>>) {
        let registry = super_ai::Registry::from_builtin();
        let model = registry.all().first().expect("built-in model").clone();
        let session = AgentSession::new(
            super_coding::SessionManager::in_memory(std::path::Path::new("/test")),
            Vec::new(),
            registry,
            super_coding::Settings::default(),
            "test".into(),
            model,
            super_ai::ThinkingLevel::Off,
            None,
            Arc::new(|_| {}),
        );
        let requests = Arc::new(Mutex::new(0));
        let seen = requests.clone();
        session.set_stream_fn(Some(Arc::new(move |_, _, _| {
            *seen.lock().unwrap() += 1;
            let mut message = AssistantMessage::empty("scripted", "scripted", "scripted");
            message.content.push(ContentBlock::text("ok"));
            message.stop_reason = StopReason::Stop;
            let (sink, stream) = EventStream::channel();
            sink.send(AssistantEvent::Start {
                partial: message.clone(),
            });
            sink.done(message);
            stream
        })));
        (session, requests)
    }

    #[tokio::test]
    async fn a_print_run_waits_for_its_background_tasks_and_their_notices() {
        let (session, requests) = session();
        let task = session
            .background_tasks()
            .register(TaskSpec::new(TaskKind::Shell, "npm test"))
            .unwrap();
        let run = tokio::spawn({
            let session = session.clone();
            async move {
                run_one_prompt(
                    &session,
                    AgentMessage::user("start the tests"),
                    PromptMode::Ordinary,
                    std::future::pending(),
                )
                .await
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!run.is_finished(), "the task still runs");
        assert_eq!(*requests.lock().unwrap(), 1);

        assert!(task.finish(TaskOutcome::completed("npm test ended")));
        assert!(run.await.unwrap());
        assert_eq!(*requests.lock().unwrap(), 2, "the notice had its turn");
    }

    #[tokio::test(start_paused = true)]
    async fn a_print_run_waits_for_a_pending_wake_up_and_its_turn_until_told_to_end() {
        let (session, requests) = session();
        // As the model's ScheduleWakeup would during the prompt's turn.
        session
            .wakeups()
            .schedule(super_coding::wakeup::WakeupRequest {
                delay_seconds: 120.0,
                prompt: Some("look again".into()),
                ..Default::default()
            })
            .unwrap();
        let run = tokio::spawn({
            let session = session.clone();
            async move {
                run_one_prompt(
                    &session,
                    AgentMessage::user("check later"),
                    PromptMode::Ordinary,
                    std::future::pending(),
                )
                .await
            }
        });
        tokio::time::sleep(Duration::from_secs(100)).await;
        assert!(!run.is_finished(), "the wake-up is pending");
        assert_eq!(*requests.lock().unwrap(), 1);
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert!(run.await.unwrap());
        assert_eq!(*requests.lock().unwrap(), 2, "the wake-up had its turn");

        // Told to end, the wait ends and the wake-up stays pending.
        session
            .wakeups()
            .schedule(super_coding::wakeup::WakeupRequest {
                delay_seconds: 600.0,
                prompt: Some("much later".into()),
                ..Default::default()
            })
            .unwrap();
        let (end, ended) = tokio::sync::oneshot::channel::<()>();
        let run = tokio::spawn({
            let session = session.clone();
            async move {
                run_one_prompt(
                    &session,
                    AgentMessage::user("and now"),
                    PromptMode::Ordinary,
                    async {
                        let _ = ended.await;
                    },
                )
                .await
            }
        });
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(!run.is_finished());
        end.send(()).unwrap();
        assert!(!run.await.unwrap());
        assert_eq!(*requests.lock().unwrap(), 3);
        assert!(session.wakeups().pending().is_some());
    }

    #[test]
    fn a_session_that_ends_in_a_worktree_it_made_says_it_is_kept() {
        let (session, _) = session();
        assert_eq!(kept_worktree_note(&session), None);
        let entered = |created| super_coding::EnteredWorktree {
            worktree: super_coding::worktree::Worktree {
                path: "/work/project/.super/worktrees/feature".into(),
                branch: "worktree-feature".into(),
                base: "abc123".into(),
                project: "/work/project".into(),
            },
            created,
            original: "/work/project".into(),
            entered_at: 0,
        };
        session.enter_worktree(entered(false));
        assert_eq!(kept_worktree_note(&session), None, "not one it made");
        session.leave_worktree();
        session.enter_worktree(entered(true));
        assert_eq!(
            kept_worktree_note(&session).unwrap(),
            "The worktree /work/project/.super/worktrees/feature and its branch worktree-feature are kept."
        );
        let event = super_sdk::events::worktree_kept(&kept_worktree(&session).unwrap());
        assert_eq!(event["type"], "worktree_kept");
        assert_eq!(
            event["worktree"]["path"],
            "/work/project/.super/worktrees/feature"
        );
        assert_eq!(event["worktree"]["branch"], "worktree-feature");
        assert_eq!(event["worktree"]["created"], true);
    }

    #[tokio::test]
    async fn a_print_run_told_to_end_stops_its_background_tasks() {
        let (session, requests) = session();
        let task = session
            .background_tasks()
            .register(TaskSpec::new(TaskKind::Monitor, "tail the log"))
            .unwrap();
        let (end, ended) = tokio::sync::oneshot::channel::<()>();
        let run = tokio::spawn({
            let session = session.clone();
            async move {
                run_one_prompt(
                    &session,
                    AgentMessage::user("watch the log"),
                    PromptMode::Ordinary,
                    async {
                        let _ = ended.await;
                    },
                )
                .await
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        end.send(()).unwrap();
        assert!(!run.await.unwrap());
        assert!(task.is_stopped());
        assert_eq!(*requests.lock().unwrap(), 1, "no turn after the end");
    }
}
