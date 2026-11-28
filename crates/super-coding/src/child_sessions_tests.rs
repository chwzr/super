//! Agents' sessions as stored sessions: an agent's session lives in the
//! folder of agents of its parent's session file, a session loaded again
//! rebuilds its agents from there and resumes one from its transcript, and
//! an agent with `isolation: "worktree"` works in a worktree of its own that
//! is kept only when the agent changed something in it.

use crate::agent_tools_tests::{
    core_tools, lanes, last_tool_result, notice_text, queued, request, settings, texts, user_text,
};
use crate::agents::registry::AGENT_RUN_ENTRY;
use crate::agents::tools::agent_tools;
use crate::agents::{AgentObserver, AgentRegistry};
use crate::background_tests::{eventually, model, text, tool_call};
use crate::session::manager::{SUBAGENT_ENTRY, SessionManager, agents_dir};
use crate::session_runner::{AgentSession, SessionEvent};
use crate::tasks::TaskStatus;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use super_agent::placeholder::prompt_todo;
use super_agent::{AgentEvent, AgentMessage, DynTool, StreamFn};
use super_ai::{Message, Registry, ThinkingLevel};
use tokio_util::sync::CancellationToken;

/// A main session on `manager` and the scripted `stream`, whose queued
/// entries start no turn: the test reads them.
fn main_session(
    manager: SessionManager,
    stream: StreamFn,
    tools: Vec<DynTool>,
) -> Arc<AgentSession> {
    let session = AgentSession::new(
        manager,
        tools,
        Registry::from_builtin(),
        settings(),
        "main prompt".into(),
        model(),
        ThinkingLevel::Off,
        None,
        Arc::new(|_| {}),
    );
    session.set_stream_fn(Some(stream));
    session.set_turn_starter(Some(Arc::new(|_turn| {})));
    session
}

/// A stored main session in `sessions` that has something said in it, so
/// its file exists.
fn stored_manager(cwd: &Path, sessions: &Path) -> SessionManager {
    let mut manager = SessionManager::create(cwd, Some(sessions.to_path_buf())).unwrap();
    manager.append_message(AgentMessage::user("main")).unwrap();
    manager
}

fn tool(session: &Arc<AgentSession>, name: &str) -> DynTool {
    agent_tools(Arc::downgrade(session))
        .into_iter()
        .find(|tool| tool.name() == name)
        .unwrap()
}

fn registry(session: &AgentSession) -> Arc<AgentRegistry> {
    session
        .agent_registry()
        .expect("the main session has agents")
}

/// The next notice queued in `session` after the ones in `collected`.
async fn next_notice(session: &AgentSession, collected: &Mutex<Vec<AgentMessage>>) -> String {
    let before = collected
        .lock()
        .unwrap()
        .iter()
        .filter(|message| notice_text(message).is_some())
        .count();
    for _ in 0..2_000 {
        let notices: Vec<String> = queued(session, collected)
            .iter()
            .filter_map(notice_text)
            .collect();
        if notices.len() > before {
            return notices[before].clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("no notice came");
}

fn session_file(session: &AgentSession) -> PathBuf {
    session
        .manager
        .lock()
        .unwrap()
        .session_file()
        .unwrap()
        .to_path_buf()
}

#[tokio::test]
async fn a_session_loaded_again_rebuilds_its_agents_and_resumes_one_from_its_transcript() {
    let directory = tempfile::tempdir().unwrap();
    let sessions = directory.path().join("sessions");
    let (stream, seen) = lanes(vec![
        (
            "child task",
            vec![Some(text("first report")), Some(text("second report"))],
        ),
        ("hold", vec![None]),
    ]);
    let first = main_session(
        stored_manager(directory.path(), &sessions),
        stream.clone(),
        Vec::new(),
    );
    registry(&first)
        .start(
            request("Work", "child task", false),
            "call_1",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    registry(&first)
        .start(
            request("Held", "hold", true),
            "call_2",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let held_asks = seen.clone();
    eventually("the held agent asks", || held_asks.count("hold") == 1).await;
    let agents = registry(&first).list();
    let (work, held) = (agents[0].clone(), agents[1].clone());
    let main_file = session_file(&first);
    // Each agent is stored in the main session's folder of agents, by id,
    // and only the main session is listed.
    assert_eq!(
        work.session_file,
        Some(agents_dir(&main_file).join(format!("{}.jsonl", work.id)))
    );
    assert_eq!(
        held.session_file,
        Some(agents_dir(&main_file).join(format!("{}.jsonl", held.id)))
    );
    let listed = SessionManager::list_all(&sessions).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, main_file);
    let work_session_id = registry(&first)
        .session_of(&work.id)
        .unwrap()
        .manager
        .lock()
        .unwrap()
        .session_id()
        .to_string();

    // The process ends while Held runs: a new one loads the stored session.
    let second = main_session(
        SessionManager::open(&main_file).unwrap(),
        stream.clone(),
        Vec::new(),
    );
    let rebuilt = registry(&second).list();
    assert_eq!(rebuilt.len(), 2, "{rebuilt:?}");
    assert_eq!(rebuilt[0].id, work.id);
    assert_eq!(rebuilt[0].name, "Work");
    assert_eq!(rebuilt[0].agent_type, "general-purpose");
    assert_eq!(rebuilt[0].status, TaskStatus::Completed);
    assert!(!rebuilt[0].background);
    assert_eq!(rebuilt[0].session_file, work.session_file);
    // Held's run left no end behind: it counts as stopped.
    assert_eq!(rebuilt[1].id, held.id);
    assert_eq!(rebuilt[1].status, TaskStatus::Stopped);
    // Nothing of them is loaded yet.
    assert!(registry(&second).built_sessions().is_empty());
    first.stop_background_tasks();
    drop(first);

    // ListAgents lists them.
    let listing = tool(&second, "ListAgents")
        .execute("list", json!({}), CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(listing.details["agents"][0]["id"], json!(work.id));
    assert_eq!(listing.details["agents"][0]["status"], json!("completed"));
    assert_eq!(listing.details["agents"][1]["status"], json!("stopped"));

    // A message to the finished agent resumes it from its transcript.
    let sent = tool(&second, "SendMessage")
        .execute(
            "call_3",
            json!({"to": work.id, "message": "now the tests"}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        sent.output_text(),
        format!(
            "Agent {} had ended; the message resumes it in the background.",
            work.id
        )
    );
    let collected = Mutex::new(Vec::new());
    let notice = next_notice(&second, &collected).await;
    assert!(notice.contains(&format!("<task-id>{}</task-id>", work.id)));
    assert!(notice.contains("<tool-use-id>call_3</tool-use-id>"));
    assert!(notice.contains("<result>second report</result>"));
    let resumed = &seen.of("child task")[1];
    let messages = &resumed.context.messages;
    assert_eq!(messages.len(), 3);
    assert!(
        matches!(&messages[1], Message::Assistant(assistant) if assistant.text() == "first report")
    );
    assert!(
        user_text(&messages[2])
            .unwrap()
            .contains("<message>now the tests</message>")
    );
    let system = resumed.context.system_prompt.clone().unwrap();
    assert!(
        system.starts_with(&format!(
            "{}\nCurrent working directory: {}",
            prompt_todo("AgentType.general-purpose.instructions"),
            directory.path().display()
        )),
        "{system}"
    );
    // The same session went on in the same file.
    let loaded = registry(&second).session_of(&work.id).unwrap();
    assert_eq!(loaded.manager.lock().unwrap().session_id(), work_session_id);
    let transcript = SessionManager::open(work.session_file.as_ref().unwrap()).unwrap();
    assert_eq!(transcript.session_id(), work_session_id);
    assert_eq!(transcript.build_session_context().messages.len(), 4);
    let runs = transcript
        .entries()
        .iter()
        .filter(|entry| {
            matches!(entry, crate::SessionEntry::Custom { custom_type, .. } if custom_type == AGENT_RUN_ENTRY)
        })
        .count();
    assert_eq!(runs, 2, "every run's end is recorded");

    // A third load knows the resumed run's end.
    let third = main_session(
        SessionManager::open(&main_file).unwrap(),
        stream,
        Vec::new(),
    );
    let again = registry(&third).list();
    assert_eq!(again[0].status, TaskStatus::Completed);
    assert!(again[0].background);
    // A session moved to another conversation forgets them, and one moved
    // back rebuilds them.
    third.replace_manager(SessionManager::in_memory(directory.path()));
    assert!(registry(&third).list().is_empty());
    third.replace_manager(SessionManager::open(&main_file).unwrap());
    assert_eq!(registry(&third).list().len(), 2);
}

#[tokio::test]
async fn an_agents_agents_are_stored_and_rebuilt_one_folder_further() {
    let directory = tempfile::tempdir().unwrap();
    let sessions = directory.path().join("sessions");
    let (stream, _seen) = lanes(vec![
        (
            "outer task",
            vec![
                Some(tool_call(
                    "inner_call",
                    "Agent",
                    json!({"description": "Inner", "prompt": "inner task", "run_in_background": false}),
                )),
                Some(text("outer report")),
            ],
        ),
        ("inner task", vec![Some(text("inner report"))]),
    ]);
    let first = main_session(
        stored_manager(directory.path(), &sessions),
        stream.clone(),
        Vec::new(),
    );
    registry(&first)
        .start(
            request("Outer", "outer task", false),
            "call_1",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let outer = registry(&first).list()[0].clone();
    let outer_file = outer.session_file.clone().unwrap();
    let inner = registry(&first)
        .session_of(&outer.id)
        .unwrap()
        .agent_registry()
        .unwrap()
        .list()[0]
        .clone();
    assert_eq!(
        inner.session_file,
        Some(agents_dir(&outer_file).join(format!("{}.jsonl", inner.id)))
    );

    let second = main_session(
        SessionManager::open(&session_file(&first)).unwrap(),
        stream,
        Vec::new(),
    );
    // The inner agent is reached through the outer one, which is loaded to
    // get there.
    let path = format!("{}/{}", outer.id, inner.id);
    let loaded = second.agent_at(&path).unwrap();
    assert_eq!(
        loaded.manager.lock().unwrap().session_file(),
        inner.session_file.as_deref()
    );
    assert_eq!(loaded.agent_link().unwrap().depth, 2);
    assert_eq!(
        loaded.agent_link().unwrap().path,
        format!("/root/{}/{}", outer.id, inner.id)
    );
    let rebuilt = registry(&second)
        .session_of(&outer.id)
        .unwrap()
        .agent_registry()
        .unwrap()
        .list();
    assert_eq!(rebuilt.len(), 1);
    assert_eq!(rebuilt[0].status, TaskStatus::Completed);
    assert!(second.agent_at("nobody").is_err());
    assert!(second.agent_at(&format!("{}/nobody", outer.id)).is_err());
    assert!(second.agent_at("").is_err());
}

/// What an observer heard, in order.
#[derive(Default)]
struct Heard {
    lines: Mutex<Vec<String>>,
    events: Arc<Mutex<Vec<String>>>,
}

impl AgentObserver for Heard {
    fn agent_session(&self, agent_id: &str, session: &Arc<AgentSession>) {
        self.lines
            .lock()
            .unwrap()
            .push(format!("session {agent_id}"));
        let events = self.events.clone();
        let id = agent_id.to_string();
        assert!(session.forward_events(Some(Arc::new(move |event| {
            if let SessionEvent::Agent(event) = &event
                && let AgentEvent::AgentEnd { .. } = event.as_ref()
            {
                events.lock().unwrap().push(format!("agent_end {id}"));
            }
        }))));
    }

    fn agent_settled(&self, agent_id: &str) {
        self.lines
            .lock()
            .unwrap()
            .push(format!("settled {agent_id}"));
    }

    fn agents_reset(&self) {
        self.lines.lock().unwrap().push("reset".into());
    }
}

#[tokio::test]
async fn an_observer_hears_of_agent_sessions_and_gets_their_events() {
    let directory = tempfile::tempdir().unwrap();
    let sessions = directory.path().join("sessions");
    let (stream, _seen) = lanes(vec![(
        "child task",
        vec![Some(text("first")), Some(text("second"))],
    )]);
    let session = main_session(
        stored_manager(directory.path(), &sessions),
        stream.clone(),
        Vec::new(),
    );
    let heard = Arc::new(Heard::default());
    session.set_agent_observer(Some(heard.clone()));
    // The main session's own sink stays the one it was built with.
    assert!(!session.forward_events(None));
    registry(&session)
        .start(
            request("Work", "child task", false),
            "call_1",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let id = registry(&session).list()[0].id.clone();
    assert_eq!(
        *heard.lines.lock().unwrap(),
        [format!("session {id}"), format!("settled {id}")]
    );
    assert_eq!(*heard.events.lock().unwrap(), [format!("agent_end {id}")]);

    // An observer installed later hears of the sessions there are.
    let late = Arc::new(Heard::default());
    session.set_agent_observer(Some(late.clone()));
    assert_eq!(*late.lines.lock().unwrap(), [format!("session {id}")]);

    // A session loaded again tells of a stored agent when it is loaded.
    let file = session_file(&session);
    let loaded = main_session(SessionManager::open(&file).unwrap(), stream, Vec::new());
    let quiet = Arc::new(Heard::default());
    loaded.set_agent_observer(Some(quiet.clone()));
    assert!(quiet.lines.lock().unwrap().is_empty());
    loaded.agent_at(&id).unwrap();
    assert_eq!(*quiet.lines.lock().unwrap(), [format!("session {id}")]);
    loaded.replace_manager(SessionManager::in_memory(directory.path()));
    assert_eq!(quiet.lines.lock().unwrap().last().unwrap(), "reset");
}

#[test]
fn a_stored_agent_is_read_from_its_subagent_entry_and_its_last_run() {
    let directory = tempfile::tempdir().unwrap();
    let parent = stored_manager(directory.path(), &directory.path().join("sessions"));
    let mut child = parent
        .create_child_named(Some("a1b2c3d4e"), directory.path())
        .unwrap();
    child
        .append_custom(
            SUBAGENT_ENTRY,
            Some(json!({
                "taskName": "Review",
                "agentId": "a1b2c3d4e",
                "agentType": "Explore",
                "model": {"provider": "openai", "id": "gpt-test"},
                "thinkingLevel": "high",
            })),
        )
        .unwrap();
    let file = child.session_file().unwrap().to_path_buf();
    let read = || crate::agents::registry::read_stored_agent(&file).unwrap();
    // No run ended yet: it was cut off.
    assert_eq!(read().status, TaskStatus::Stopped);
    assert_eq!(read().name, "Review");
    assert_eq!(read().agent_type, "Explore");
    assert_eq!(read().model, Some(("openai".into(), "gpt-test".into())));
    assert_eq!(read().thinking, Some(ThinkingLevel::High));
    child.append_message(AgentMessage::user("task")).unwrap();
    child
        .append_custom(
            AGENT_RUN_ENTRY,
            Some(json!({"status": "failed", "background": true, "worktree": null})),
        )
        .unwrap();
    assert_eq!(read().status, TaskStatus::Failed);
    assert!(read().background);
    // A message after the last end is a run that was cut off.
    child.append_message(AgentMessage::user("again")).unwrap();
    assert_eq!(read().status, TaskStatus::Stopped);
    // A workflow's child has no agent id and is no agent.
    let mut workflow = parent.create_child().unwrap();
    workflow
        .append_custom(SUBAGENT_ENTRY, Some(json!({"taskName": "step"})))
        .unwrap();
    assert!(crate::agents::registry::read_stored_agent(workflow.session_file().unwrap()).is_none());
}

/// The branches of the repository at `root`.
fn branches(root: &Path) -> String {
    crate::worktree::tests::branches(root)
}

#[cfg(unix)]
#[tokio::test]
async fn an_agent_with_a_worktree_works_in_it_and_keeps_it_only_when_it_changed_something() {
    let directory = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    crate::worktree::tests::init_repository(directory.path());
    let root = directory.path().canonicalize().unwrap();
    let (stream, seen) = lanes(vec![
        (
            "edit task",
            vec![
                Some(tool_call(
                    "b1",
                    "Bash",
                    json!({"command": "pwd && echo work > note.txt"}),
                )),
                Some(text("wrote the note")),
            ],
        ),
        (
            "look task",
            vec![
                Some(text("nothing to change")),
                Some(tool_call("b2", "Bash", json!({"command": "pwd"}))),
                Some(text("looked again")),
            ],
        ),
    ]);
    let session = main_session(
        stored_manager(directory.path(), store.path()),
        stream,
        core_tools(directory.path()),
    );

    // Waited for, it writes a file: its worktree stays, and the result
    // names its folder and branch.
    let result = tool(&session, "Agent")
        .execute(
            "call_1",
            json!({"description": "Edit", "prompt": "edit task", "run_in_background": false, "isolation": "worktree"}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert!(!result.is_error, "{}", result.output_text());
    let id = result.details["agent"]["id"].as_str().unwrap().to_string();
    let path = root.join(".super/worktrees").join(format!("agent-{id}"));
    let branch = format!("worktree-agent-{id}");
    assert_eq!(
        result.details["agent"]["worktree"]["path"],
        json!(path.display().to_string())
    );
    assert_eq!(result.details["agent"]["worktree"]["branch"], json!(branch));
    assert!(path.join("note.txt").exists());
    assert!(!root.join("note.txt").exists());
    assert!(branches(&root).contains(&branch));
    let blocks: Vec<String> = result
        .content
        .iter()
        .filter_map(|block| match block {
            super_ai::ContentBlock::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(blocks[0], "wrote the note");
    assert!(
        blocks[1].ends_with(&format!(
            "<worktree-path>{}</worktree-path>\n<worktree-branch>{branch}</worktree-branch>\n</agent-result>",
            path.display()
        )),
        "{}",
        blocks[1]
    );
    // It worked there: its shell started in the worktree, and its prompt
    // names that folder.
    let edit = seen.of("edit task");
    let system = edit[0].context.system_prompt.clone().unwrap();
    assert!(
        system.contains(&format!("Current working directory: {}", path.display())),
        "{system}"
    );
    assert!(!system.contains(&format!(
        "Current working directory: {}\n",
        directory.path().display()
    )));
    assert_eq!(
        texts(&last_tool_result(&edit[1].context))[0],
        path.display().to_string()
    );
    let stored = registry(&session).session_of(&id).unwrap();
    assert_eq!(stored.manager.lock().unwrap().cwd(), path);

    // In the background it changes nothing: its worktree goes, branch and
    // all, and the notice names none.
    let started = tool(&session, "Agent")
        .execute(
            "call_2",
            json!({"description": "Look", "prompt": "look task", "isolation": "worktree"}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let look = started.details["agent"]["id"].as_str().unwrap().to_string();
    let look_path = root.join(".super/worktrees").join(format!("agent-{look}"));
    let look_branch = format!("worktree-agent-{look}");
    assert!(
        started.output_text().ends_with(&format!(
            " It works in the worktree {} on the branch {look_branch}.",
            look_path.display()
        )),
        "{}",
        started.output_text()
    );
    let collected = Mutex::new(Vec::new());
    let notice = next_notice(&session, &collected).await;
    assert!(notice.contains("<result>nothing to change</result>"));
    assert!(!notice.contains("<worktree-path>"));
    assert!(!look_path.exists());
    assert!(!branches(&root).contains(&look_branch));
    assert!(registry(&session).find(&look).unwrap().worktree.is_none());

    // Resumed, it gets its worktree back for the run, and loses it again.
    tool(&session, "SendMessage")
        .execute(
            "call_3",
            json!({"to": look, "message": "look again"}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let notice = next_notice(&session, &collected).await;
    assert!(notice.contains("<result>looked again</result>"), "{notice}");
    let again = seen.of("look task");
    let pwd = last_tool_result(&again[2].context);
    assert!(!pwd.is_error, "{:?}", texts(&pwd));
    assert_eq!(texts(&pwd)[0], look_path.display().to_string());
    assert!(!look_path.exists());
    // The kept one is still there.
    assert!(path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn an_agent_whose_folder_is_gone_is_loaded_and_resumed_in_its_parents_folder() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let sessions = root.join("sessions");
    let (stream, seen) = lanes(vec![(
        "child task",
        vec![
            Some(text("first report")),
            Some(tool_call("b1", "Bash", json!({"command": "pwd"}))),
            Some(text("second report")),
        ],
    )]);
    let first = main_session(
        stored_manager(&root, &sessions),
        stream.clone(),
        core_tools(&root),
    );
    // The session works in another folder, as in a worktree it entered,
    // while the agent starts and runs there.
    let elsewhere = root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    first.enter_worktree(crate::worktree::EnteredWorktree {
        worktree: crate::worktree::Worktree {
            path: elsewhere.clone(),
            branch: "worktree-elsewhere".into(),
            base: "0000000".into(),
            project: root.clone(),
        },
        created: true,
        original: root.clone(),
        entered_at: 0,
    });
    registry(&first)
        .start(
            request("Work", "child task", false),
            "call_1",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let work = registry(&first).list().pop().unwrap();
    let stored = SessionManager::open(work.session_file.as_ref().unwrap()).unwrap();
    assert_eq!(stored.cwd(), elsewhere.as_path());
    let main_file = session_file(&first);
    first.leave_worktree();
    drop(first);
    // The folder is removed later.
    std::fs::remove_dir_all(&elsewhere).unwrap();

    // Loading the session and the agent does not fail.
    let second = main_session(
        SessionManager::open(&main_file).unwrap(),
        stream,
        core_tools(&root),
    );
    assert_eq!(registry(&second).list()[0].id, work.id);
    let loaded = registry(&second).session_of(&work.id).unwrap();
    // The agent works in its parent's folder instead.
    assert_eq!(loaded.working_folder(), root);
    tool(&second, "SendMessage")
        .execute(
            "call_2",
            json!({"to": work.id, "message": "go on"}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let collected = Mutex::new(Vec::new());
    let notice = next_notice(&second, &collected).await;
    assert!(
        notice.contains("<result>second report</result>"),
        "{notice}"
    );
    let resumed = seen.of("child task");
    let pwd = last_tool_result(&resumed[2].context);
    assert!(!pwd.is_error, "{:?}", texts(&pwd));
    assert_eq!(texts(&pwd)[0], root.display().to_string());
    let system = resumed[1].context.system_prompt.clone().unwrap();
    assert!(
        system.contains(&format!("Current working directory: {}", root.display())),
        "{system}"
    );
}
