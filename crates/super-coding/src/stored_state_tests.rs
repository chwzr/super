//! What a session keeps besides its conversation comes back when it is
//! loaded again: the worktree it works in, its wake-up and self-paced loop,
//! the workflow runs that were cut off and what it had read.

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
        assert_eq!(
            moves(&events.take()),
            [(project.root.clone(), None, Some(worktree.clone()))]
        );
        drop(third);
        let fourth = loaded(&file, &project.root, &events, None);
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
