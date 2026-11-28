//! What a session keeps besides its conversation comes back when it is
//! loaded again: the worktree it works in, its wake-up and self-paced loop,
//! the workflow runs that were cut off and what it had read.

// Most of these tests need a POSIX shell or git paths and run on Unix
// only; elsewhere their helpers and imports stay unused.
#![cfg_attr(not(unix), allow(unused_imports, dead_code))]

use crate::agent_tools_tests::{core_tools, lanes, last_tool_result, texts};
use crate::background_tests::{model, text, tool_call};
use crate::session::manager::SessionManager;
use crate::session_runner::{AgentSession, SessionEvent, SessionEventSink};
use crate::settings::Settings;
use crate::stored_state;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use super_agent::{AgentMessage, AgentTool, StreamFn, ToolResult};
use super_ai::{Registry, ThinkingLevel};
use tokio_util::sync::CancellationToken;

pub(crate) fn quiet_settings() -> Settings {
    Settings {
        retry: crate::settings::RetrySettings {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Every event a session emits, for a test to look at.
#[derive(Clone, Default)]
pub(crate) struct Events(Arc<Mutex<Vec<SessionEvent>>>);

impl Events {
    pub(crate) fn sink(&self) -> SessionEventSink {
        let events = self.0.clone();
        Arc::new(move |event| events.lock().unwrap().push(event))
    }

    pub(crate) fn take(&self) -> Vec<SessionEvent> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// A session of `manager` working in `root` with the four core tools.
pub(crate) fn session_of(
    manager: SessionManager,
    root: &Path,
    settings: Settings,
    events: &Events,
    stream: Option<StreamFn>,
) -> Arc<AgentSession> {
    let session = AgentSession::new(
        manager,
        core_tools(root),
        Registry::from_builtin(),
        settings,
        format!(
            "main prompt\n\nCurrent working directory: {}",
            root.display()
        ),
        model(),
        ThinkingLevel::Off,
        None,
        events.sink(),
    );
    session.set_stream_fn(stream);
    session
}

/// The session stored in `file`, loaded again.
pub(crate) fn loaded(
    file: &Path,
    root: &Path,
    events: &Events,
    stream: Option<StreamFn>,
) -> Arc<AgentSession> {
    session_of(
        SessionManager::open(file).unwrap(),
        root,
        quiet_settings(),
        events,
        stream,
    )
}

pub(crate) fn file_of(session: &AgentSession) -> PathBuf {
    session
        .manager
        .lock()
        .unwrap()
        .session_file()
        .unwrap()
        .to_path_buf()
}

pub(crate) async fn call(tool: &dyn AgentTool, args: Value) -> Result<ToolResult, String> {
    tool.execute(
        "call",
        tool.prepare_arguments(args),
        CancellationToken::new(),
        None,
    )
    .await
    .map_err(|error| format!("{error:#}"))
}

fn state_entries(file: &Path, custom_type: &str) -> Vec<Value> {
    let manager = SessionManager::open(file).unwrap();
    stored_state::all(&manager, custom_type)
        .into_iter()
        .cloned()
        .collect()
}

#[cfg(unix)]
mod worktrees {
    use super::*;
    use crate::tools::worktree::{EnterWorktreeTool, ExitWorktreeTool};
    use crate::worktree::tests::init_repository;

    struct Project {
        root: PathBuf,
        sessions: PathBuf,
        _directory: tempfile::TempDir,
    }

    fn project() -> Project {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        init_repository(&root);
        let sessions = directory.path().join("sessions");
        Project {
            root,
            sessions,
            _directory: directory,
        }
    }

    fn new_session(project: &Project, events: &Events) -> Arc<AgentSession> {
        session_of(
            SessionManager::create(&project.root, Some(project.sessions.clone())).unwrap(),
            &project.root,
            quiet_settings(),
            events,
            None,
        )
    }

    async fn enter(session: &Arc<AgentSession>, name: &str) -> PathBuf {
        call(
            &EnterWorktreeTool::new(Arc::downgrade(session)),
            json!({"name": name}),
        )
        .await
        .unwrap();
        session.worktree().unwrap().worktree.path
    }

    fn moves(events: &[SessionEvent]) -> Vec<(PathBuf, Option<PathBuf>, Option<PathBuf>)> {
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::WorktreeChanged {
                    folder,
                    worktree,
                    missing,
                } => Some((
                    folder.clone(),
                    worktree
                        .as_ref()
                        .map(|entered| entered.worktree.path.clone()),
                    missing.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_session_in_a_worktree_works_there_again_when_it_is_loaded() {
        let project = project();
        let events = Events::default();
        let session = new_session(&project, &events);
        let worktree = enter(&session, "feature").await;
        assert_eq!(
            moves(&events.take()),
            [(worktree.clone(), Some(worktree.clone()), None)],
            "clients hear of the move"
        );
        let file = file_of(&session);
        drop(session);

        // Loaded again, its folder, its tools and its prompt are the
        // worktree's.
        let (stream, seen) = lanes(vec![(
            "where",
            vec![
                Some(tool_call("c1", "Bash", json!({"command": "pwd -P"}))),
                Some(text("there")),
            ],
        )]);
        let again = loaded(&file, &project.root, &events, Some(stream));
        let entered = again.worktree().expect("in the worktree again");
        assert_eq!(entered.worktree.path, worktree);
        assert!(entered.created);
        assert_eq!(again.working_folder(), worktree);
        again.prompt(vec![AgentMessage::user("where")]).await;
        let requests = seen.of("where");
        assert_eq!(
            texts(&last_tool_result(&requests[1].context)),
            [worktree.display().to_string()]
        );
        assert!(
            requests[0]
                .context
                .system_prompt
                .as_deref()
                .unwrap()
                .ends_with(&format!(
                    "Current working directory: {}",
                    worktree.display()
                ))
        );
        events.take();
        drop(again);

        // The worktree's folder is gone: the session works in its own
        // folder and says so once.
        std::fs::remove_dir_all(&worktree).unwrap();
        let third = loaded(&file, &project.root, &events, None);
        assert_eq!(third.worktree(), None);
        assert_eq!(third.working_folder(), project.root);
        // Written down and said once its host holds the file.
        assert!(moves(&events.take()).is_empty());
        assert_eq!(state_entries(&file, stored_state::WORKTREE_ENTRY).len(), 1);
        third.resume_stored_work();
        assert_eq!(
            moves(&events.take()),
            [(project.root.clone(), None, Some(worktree.clone()))]
        );
        drop(third);
        let fourth = loaded(&file, &project.root, &events, None);
        fourth.resume_stored_work();
        assert_eq!(fourth.worktree(), None);
        assert!(moves(&events.take()).is_empty(), "said once");
        let written = state_entries(&file, stored_state::WORKTREE_ENTRY);
        assert_eq!(written.len(), 2);
        assert_eq!(written[1]["worktree"], Value::Null);
        assert_eq!(written[1]["missing"], json!(worktree));
    }

    #[tokio::test]
    async fn leaving_a_worktree_is_kept_and_another_conversation_leaves_it_for_the_first() {
        let project = project();
        let events = Events::default();
        let session = new_session(&project, &events);
        enter(&session, "first").await;
        call(
            &ExitWorktreeTool::new(Arc::downgrade(&session)),
            json!({"action": "keep"}),
        )
        .await
        .unwrap();
        let file = file_of(&session);
        let again = loaded(&file, &project.root, &events, None);
        assert_eq!(again.worktree(), None, "it left the worktree");
        drop(again);

        // A new conversation leaves the worktree for this session only: the
        // conversation it left is in it again when it is loaded.
        let worktree = enter(&session, "second").await;
        session.replace_manager(
            SessionManager::create(&project.root, Some(project.sessions.clone())).unwrap(),
        );
        assert_eq!(session.worktree(), None);
        assert_eq!(session.working_folder(), project.root);
        let first = loaded(&file, &project.root, &events, None);
        assert_eq!(first.worktree().unwrap().worktree.path, worktree);

        // And switching back to it brings it back too.
        session.replace_manager(SessionManager::open(&file).unwrap());
        assert_eq!(session.worktree().unwrap().worktree.path, worktree);
    }
}

mod wakeups {
    use super::*;
    use crate::notifications::{LOOP_ROUND, WAKEUP, looped, woken};
    use std::time::Duration;
    use super_agent::placeholder::prompt_todo;

    /// Let the paused clock run `seconds` forward, and every task that is
    /// due run with it.
    async fn pass(seconds: u64) {
        tokio::time::sleep(Duration::from_secs(seconds)).await;
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
    }

    fn custom(session: &AgentSession, kind: &str) -> Vec<AgentMessage> {
        session
            .manager
            .lock()
            .unwrap()
            .build_session_context()
            .messages
            .into_iter()
            .filter(|message| {
                matches!(message, AgentMessage::Custom(custom) if custom.custom_type == kind)
            })
            .collect()
    }

    fn round_text(goal: &str) -> String {
        format!("{goal}\n\n{}", prompt_todo("Loop.self_paced_instructions"))
    }

    struct Stored {
        root: PathBuf,
        sessions: PathBuf,
        _directory: tempfile::TempDir,
    }

    fn stored() -> Stored {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        std::fs::create_dir(&root).unwrap();
        Stored {
            sessions: directory.path().join("sessions"),
            root,
            _directory: directory,
        }
    }

    impl Stored {
        fn session(&self, stream: Option<StreamFn>) -> Arc<AgentSession> {
            session_of(
                SessionManager::create(&self.root, Some(self.sessions.clone())).unwrap(),
                &self.root,
                quiet_settings(),
                &Events::default(),
                stream,
            )
        }

        /// A stored session whose last state is `wakeups`.
        fn with_state(&self, wakeups: Value) -> PathBuf {
            let mut manager =
                SessionManager::create(&self.root, Some(self.sessions.clone())).unwrap();
            manager.append_message(AgentMessage::user("go")).unwrap();
            stored_state::record(&mut manager, stored_state::WAKEUP_ENTRY, wakeups);
            manager.session_file().unwrap().to_path_buf()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_pending_wake_up_comes_back_when_the_session_is_loaded_and_fires_at_its_time() {
        let stored = stored();
        let (stream, _) = lanes(vec![(
            "go",
            vec![
                Some(tool_call(
                    "w1",
                    "ScheduleWakeup",
                    json!({"delaySeconds": 600, "prompt": "check the build", "reason": "the build is slow"}),
                )),
                Some(text("waiting")),
            ],
        )]);
        let session = stored.session(Some(stream));
        session.prompt(vec![AgentMessage::user("go")]).await;
        let pending = session.wakeups().pending().unwrap();
        let file = file_of(&session);
        let written = state_entries(&file, stored_state::WAKEUP_ENTRY);
        assert_eq!(
            written.last().unwrap()["wakeup"]["prompt"],
            "check the build"
        );
        assert_eq!(written.last().unwrap()["loop"], Value::Null);
        drop(session);

        let (stream, seen) = lanes(vec![("go", vec![Some(text("the build passed"))])]);
        let again = loaded(&file, &stored.root, &Events::default(), Some(stream));
        pass(700).await;
        assert_eq!(
            seen.count("go"),
            0,
            "nothing starts before its host is ready"
        );
        assert_eq!(again.wakeups().pending(), None);

        let (stream, seen) = lanes(vec![("go", vec![Some(text("the build passed"))])]);
        let again = loaded(&file, &stored.root, &Events::default(), Some(stream));
        again.resume_stored_work();
        let restored = again.wakeups().pending().expect("pending again");
        assert_eq!(restored, pending);
        assert!(again.has_background_work(), "it keeps the session loaded");
        // The clock started when it was scheduled, not when it was loaded:
        // the first session waited 700 of the paused seconds, which the
        // wall clock did not, so it is still about 600 seconds away.
        pass(590).await;
        assert_eq!(seen.count("go"), 0, "not due yet");
        pass(20).await;
        again.wait_for_background_work().await;
        assert_eq!(seen.count("go"), 1);
        let wakeup = woken(&custom(&again, WAKEUP)[0]).unwrap();
        assert_eq!(wakeup.prompt.as_deref(), Some("check the build"));
        assert_eq!(again.wakeups().pending(), None);
        let written = state_entries(&file, stored_state::WAKEUP_ENTRY);
        assert_eq!(written.last().unwrap()["wakeup"], Value::Null);
    }

    /// A move to another conversation leaves what the session queued with
    /// the conversation that is left: a wake-up that came due is pending
    /// there again and comes at once when it is loaded, a notice is in its
    /// history, and what a client queued is dropped. The other conversation
    /// gets none of it.
    #[tokio::test(start_paused = true)]
    async fn what_waits_in_the_queue_stays_with_the_conversation_that_is_left() {
        let stored = stored();
        let session = stored.session(None);
        session.set_turn_starter(Some(Arc::new(|_turn| {})));
        session
            .manager
            .lock()
            .unwrap()
            .append_message(AgentMessage::user("go"))
            .unwrap();
        let file = file_of(&session);
        let due: crate::wakeup::WakeupInfo = serde_json::from_value(json!({
            "id": 1,
            "delaySeconds": 60,
            "dueAt": super_ai::now_ms() - 1_000,
            "reason": "came due",
            "noop": false,
            "prompt": "WAKE-A",
            "job": null,
            "round": null,
        }))
        .unwrap();
        session.queue_entry(
            crate::notifications::wakeup_message(&due),
            crate::notifications::Priority::Later,
        );
        let notice = AgentMessage::Custom(super_agent::CustomMessage {
            custom_type: crate::notifications::TASK_NOTIFICATION.into(),
            content: super_ai::UserContent::Text("NOTICE-A".into()),
            display: true,
            details: None,
            timestamp: super_ai::now_ms(),
        });
        session.queue_entry(notice, crate::notifications::Priority::Next);
        session.queue_steering(AgentMessage::user("STEER-A"));
        session.queue_follow_up(AgentMessage::user("FOLLOWUP-A"));

        session.replace_manager(SessionManager::in_memory(&stored.root));
        assert!(!session.has_queued_entries());
        assert!(session.reclaim_queued().is_empty());
        assert!(
            session
                .manager
                .lock()
                .unwrap()
                .build_session_context()
                .messages
                .is_empty()
        );

        let texts: Vec<String> = SessionManager::open(&file)
            .unwrap()
            .build_session_context()
            .messages
            .iter()
            .map(|message| serde_json::to_string(message).unwrap())
            .collect();
        assert!(
            texts.iter().any(|text| text.contains("NOTICE-A")),
            "{texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|text| text.contains("STEER-A") || text.contains("FOLLOWUP-A")),
            "{texts:?}"
        );
        let written = state_entries(&file, stored_state::WAKEUP_ENTRY);
        assert_eq!(written.last().unwrap()["wakeup"]["prompt"], "WAKE-A");

        let (stream, seen) = lanes(vec![("go", vec![Some(text("woke"))])]);
        let again = loaded(&file, &stored.root, &Events::default(), Some(stream));
        again.resume_stored_work();
        pass(1).await;
        again.wait_for_background_work().await;
        assert_eq!(seen.count("go"), 1, "the wake-up came at once");
        let wakeup = woken(&custom(&again, WAKEUP)[0]).unwrap();
        assert_eq!(wakeup.prompt.as_deref(), Some("WAKE-A"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_wake_up_whose_time_has_passed_fires_at_once_when_the_session_is_loaded() {
        let stored = stored();
        let file = stored.with_state(json!({
            "wakeup": {
                "id": 3,
                "delaySeconds": 60,
                "dueAt": super_ai::now_ms() - 3_600_000,
                "reason": "an hour ago",
                "noop": false,
                "prompt": "look again",
                "job": null,
                "round": null,
            },
            "loop": null,
        }));
        let (stream, seen) = lanes(vec![("go", vec![Some(text("looked"))])]);
        let session = loaded(&file, &stored.root, &Events::default(), Some(stream));
        session.resume_stored_work();
        pass(1).await;
        session.wait_for_background_work().await;
        assert_eq!(seen.count("go"), 1);
        let wakeup = woken(&custom(&session, WAKEUP)[0]).unwrap();
        assert_eq!(wakeup.reason.as_deref(), Some("an hour ago"));
        // Started once only.
        session.resume_stored_work();
        pass(4_000).await;
        assert_eq!(seen.count("go"), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_self_paced_loop_goes_on_from_its_round_when_the_session_is_loaded() {
        let stored = stored();
        let goal = "tidy the docs";
        let lane = round_text(goal);
        let (stream, _) = lanes(vec![(
            &lane,
            vec![
                Some(tool_call(
                    "w1",
                    "ScheduleWakeup",
                    json!({"delaySeconds": 300, "reason": "wait for CI"}),
                )),
                Some(text("round 1 done")),
            ],
        )]);
        let session = stored.session(Some(stream));
        let job = session
            .iterative_jobs()
            .unwrap()
            .start_self_paced(goal, Some(5))
            .unwrap();
        pass(1).await;
        assert_eq!(job.snapshot().iteration, 1);
        let file = file_of(&session);
        let written = state_entries(&file, stored_state::WAKEUP_ENTRY);
        let last = written.last().unwrap();
        assert_eq!(last["loop"]["goal"], goal);
        assert_eq!(last["loop"]["round"], 1);
        assert_eq!(last["loop"]["maxRounds"], 5);
        assert_eq!(last["wakeup"]["round"], 2);
        drop(job);
        drop(session);

        // Loaded again, round 2 comes when the wake-up is due.
        let (stream, seen) = lanes(vec![(&lane, vec![Some(text("round 2 done"))])]);
        let again = loaded(&file, &stored.root, &Events::default(), Some(stream));
        again.resume_stored_work();
        let (job_id, round, max_rounds) = again.wakeups().running_loop().unwrap();
        assert_eq!((round, max_rounds), (1, 5));
        let job = again.iterative_jobs().unwrap().get(job_id).unwrap();
        assert!(job.is_self_paced());
        pass(290).await;
        assert_eq!(seen.count(&lane), 0);
        pass(20).await;
        let mut updates = job.subscribe();
        while !job.snapshot().status.is_finished() {
            updates.changed().await.unwrap();
        }
        assert_eq!(seen.count(&lane), 1);
        assert_eq!(job.snapshot().iteration, 2);
        let rounds = custom(&again, LOOP_ROUND);
        let (round, max_rounds, prompt, woken_by) = looped(rounds.last().unwrap()).unwrap();
        assert_eq!((round, max_rounds, prompt.as_str()), (2, 5, goal));
        assert_eq!(woken_by.unwrap().reason.as_deref(), Some("wait for CI"));
        assert_eq!(again.wakeups().running_loop(), None);
        let written = state_entries(&file, stored_state::WAKEUP_ENTRY);
        assert_eq!(written.last().unwrap()["loop"], Value::Null);
    }

    #[tokio::test(start_paused = true)]
    async fn a_loop_cut_off_in_its_round_runs_that_round_again() {
        let stored = stored();
        let goal = "watch the deploy";
        let lane = round_text(goal);
        let file = stored.with_state(json!({
            "wakeup": null,
            "loop": {"job": 7, "goal": goal, "round": 2, "maxRounds": 4},
        }));
        let (stream, seen) = lanes(vec![("go", vec![Some(text("deployed"))])]);
        let session = loaded(&file, &stored.root, &Events::default(), Some(stream));
        session.resume_stored_work();
        pass(1).await;
        session.wait_for_background_work().await;
        pass(1).await;
        assert_eq!(seen.count("go"), 1, "{:?}", seen.count(&lane));
        let rounds = custom(&session, LOOP_ROUND);
        assert_eq!(rounds.len(), 1);
        let (round, max_rounds, prompt, woken_by) = looped(&rounds[0]).unwrap();
        assert_eq!((round, max_rounds, prompt.as_str()), (2, 4, goal));
        assert!(woken_by.is_none());
        assert_eq!(
            session.wakeups().running_loop(),
            None,
            "nothing continued it"
        );
    }
}

mod reads {
    use super::*;

    fn tool(session: &AgentSession, name: &str) -> super_agent::DynTool {
        session
            .offered_tools()
            .into_iter()
            .find(|tool| tool.name() == name)
            .unwrap()
    }

    fn key(path: &Path) -> PathBuf {
        path.canonicalize().unwrap()
    }

    #[tokio::test]
    async fn a_session_loaded_again_counts_what_it_read_of_files_nobody_changed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let file = |name: &str, text: &str| {
            let path = root.join(name);
            std::fs::write(&path, text).unwrap();
            path.display().to_string()
        };
        let (a, b, c) = (
            file("a.txt", "alpha\n"),
            file("b.txt", "beta\n"),
            file("c.txt", "gamma\n"),
        );
        let d = root.join("d.txt").display().to_string();
        let (stream, _) = lanes(vec![(
            "work",
            vec![
                Some(tool_call("r1", "Read", json!({"file_path": a}))),
                Some(tool_call("r2", "Read", json!({"file_path": b}))),
                Some(tool_call("r3", "Read", json!({"file_path": c}))),
                Some(tool_call(
                    "e1",
                    "Edit",
                    json!({"file_path": c, "old_string": "gamma", "new_string": "delta"}),
                )),
                Some(tool_call(
                    "w1",
                    "Write",
                    json!({"file_path": d, "content": "new\n"}),
                )),
                Some(text("done")),
            ],
        )]);
        let session = session_of(
            SessionManager::create(&root, Some(root.join("sessions"))).unwrap(),
            &root,
            quiet_settings(),
            &Events::default(),
            Some(stream),
        );
        session.prompt(vec![AgentMessage::user("work")]).await;
        let stored = file_of(&session);
        let before = session.file_state().get(&key(&root.join("c.txt"))).unwrap();
        drop(session);

        // Someone changes b.txt while the session is not loaded.
        std::fs::write(root.join("b.txt"), "beta, changed\n").unwrap();
        let again = loaded(&stored, &root, &Events::default(), None);
        let files = again.file_state();
        assert!(files.get(&key(&root.join("a.txt"))).unwrap().whole);
        // A file changed since keeps the read the session made, with the
        // stamp the file had then.
        let changed = files.get(&key(&root.join("b.txt"))).unwrap();
        let now = std::fs::metadata(root.join("b.txt")).unwrap();
        assert!(!changed.same_stamp(now.modified().ok(), now.len()));
        // Its own edit counts, with the stamp the edit left (the time of the
        // read is kept to the millisecond).
        let edited = files.get(&key(&root.join("c.txt"))).unwrap();
        assert_eq!(
            super_agent::FileRead {
                at: before.at,
                ..edited
            },
            before
        );
        assert!(files.get(&key(&root.join("d.txt"))).unwrap().whole);
        assert_eq!(files.len(), 4);

        // So Edit takes a.txt without a new read, and refuses b.txt as
        // changed on disk, as a session that kept running would.
        let edit = tool(&again, "Edit");
        call(
            edit.as_ref(),
            json!({"file_path": a, "old_string": "alpha", "new_string": "omega"}),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "omega\n"
        );
        let refused = call(
            edit.as_ref(),
            json!({"file_path": b, "old_string": "beta", "new_string": "zeta"}),
        )
        .await
        .unwrap_err();
        assert!(
            refused.contains("has changed on disk since it was read"),
            "{refused}"
        );
        // Write overwrites d.txt, read whole by its own Write.
        call(
            tool(&again, "Write").as_ref(),
            json!({"file_path": d, "content": "newer\n"}),
        )
        .await
        .unwrap();
    }
    /// A whole read and then a read of a part of the same unchanged file
    /// leave the whole read in the record also when the session is loaded
    /// again, as while it ran: `Write` may overwrite the file.
    #[tokio::test]
    async fn a_whole_read_before_a_part_read_still_counts_after_a_reload() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let lines: String = (1..=10).map(|n| format!("line {n}\n")).collect();
        std::fs::write(root.join("f.txt"), lines).unwrap();
        let f = root.join("f.txt").display().to_string();
        let (stream, _) = lanes(vec![(
            "read",
            vec![
                Some(tool_call("r1", "Read", json!({"file_path": f}))),
                Some(tool_call("r2", "Read", json!({"file_path": f, "limit": 2}))),
                Some(text("read")),
            ],
        )]);
        let session = session_of(
            SessionManager::create(&root, Some(root.join("sessions"))).unwrap(),
            &root,
            quiet_settings(),
            &Events::default(),
            Some(stream),
        );
        session.prompt(vec![AgentMessage::user("read")]).await;
        assert!(
            session
                .file_state()
                .get(&key(&root.join("f.txt")))
                .unwrap()
                .whole
        );
        let stored = file_of(&session);
        drop(session);

        let again = loaded(&stored, &root, &Events::default(), None);
        assert!(
            again
                .file_state()
                .get(&key(&root.join("f.txt")))
                .unwrap()
                .whole
        );
        call(
            tool(&again, "Write").as_ref(),
            json!({"file_path": f, "content": "new\n"}),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("f.txt")).unwrap(),
            "new\n"
        );
    }

    /// Moving back to before a read, as `fork` over RPC and `/tree` do,
    /// leaves the read out of the record, in the session and when it is
    /// loaded again: the conversation the model gets has no such read.
    #[tokio::test]
    async fn a_read_on_a_branch_the_session_moved_away_from_does_not_count() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(root.join("a.txt"), "alpha\n").unwrap();
        let a = root.join("a.txt").display().to_string();
        // Every request of the session is in the lane of its first prompt.
        let (stream, _) = lanes(vec![(
            "first",
            vec![
                Some(text("one")),
                Some(tool_call("r1", "Read", json!({"file_path": a}))),
                Some(text("read")),
                Some(text("three")),
            ],
        )]);
        let session = session_of(
            SessionManager::create(&root, Some(root.join("sessions"))).unwrap(),
            &root,
            quiet_settings(),
            &Events::default(),
            Some(stream),
        );
        session.prompt(vec![AgentMessage::user("first")]).await;
        session.prompt(vec![AgentMessage::user("read")]).await;
        assert!(
            session
                .file_state()
                .get(&key(&root.join("a.txt")))
                .is_some()
        );
        let read_prompt = session
            .manager
            .lock()
            .unwrap()
            .entries()
            .iter()
            .find(|entry| {
                matches!(
                    entry,
                    crate::session::entry::SessionEntry::Message {
                        message: AgentMessage::User(user),
                        ..
                    } if user.content.as_text() == "read"
                )
            })
            .map(|entry| entry.id().to_string())
            .unwrap();
        session
            .navigate_tree(&read_prompt, false, None, CancellationToken::new())
            .await
            .unwrap();
        assert!(
            session
                .file_state()
                .get(&key(&root.join("a.txt")))
                .is_none()
        );
        let refused = call(
            tool(&session, "Edit").as_ref(),
            json!({"file_path": a, "old_string": "alpha", "new_string": "omega"}),
        )
        .await
        .unwrap_err();
        assert!(refused.contains("has not been read yet"), "{refused}");

        // The new branch goes on, and the session is loaded again.
        session.prompt(vec![AgentMessage::user("after")]).await;
        let stored = file_of(&session);
        drop(session);
        let again = loaded(&stored, &root, &Events::default(), None);
        assert!(again.file_state().get(&key(&root.join("a.txt"))).is_none());
    }
}
