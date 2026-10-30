//! The terminal as a client of the daemon, end to end: a real `super daemon
//! run` process on a temporary socket with the mock provider, and in this
//! process the views the terminal renders from, attached through
//! `super_daemon::attached::RemoteSession` exactly as `super` attaches them.

#![cfg(unix)]

use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use super_agent::{AgentEvent, AgentMessage};
use super_coding::session_runner::{AgentSession, PromptMode, SessionEvent};
use super_daemon::attached::{AttachedNotice, Attachment, Connector, RemoteSession};
use super_daemon::client::{ClientInfo, DaemonClient, NewSession};
use super_sdk::daemon::ClientKind;
use super_sdk::mock::{MockProvider, MockScript, MockTurn};
use super_sdk::{SessionOptions, SessionSource};
use tokio::process::{Child, Command};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("ae")
            .tempdir()
            .expect("tempdir");
        for sub in ["home", "work", "sessions"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        // Workflows run on subagents, which are off by default; the
        // approval below is the workflow cost gate.
        let settings = dir.path().join("home/.super/agent/settings.json");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(&settings, r#"{"subagents":{"enabled":true}}"#).unwrap();
        Self { dir }
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub)
    }

    fn socket(&self) -> PathBuf {
        self.path("d.sock")
    }

    async fn start_daemon(&self, provider: &MockProvider) -> Child {
        let socket = self.socket();
        let child = Command::new(env!("CARGO_BIN_EXE_super"))
            .args(["daemon", "run", "--socket", socket.to_str().unwrap()])
            .current_dir(self.path("work"))
            .env("HOME", self.path("home"))
            .env("SUPER_SESSION_DIR", self.path("sessions"))
            .env("SUPER_MODELS_FILE", provider.catalog_path())
            .env("SUPER_PLATFORM_URL", "http://127.0.0.1:9")
            .env_remove("SHELL")
            .env_remove("SUPER_LOG")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn the daemon");
        let start = Instant::now();
        while DaemonClient::connect_to(&socket, &info("probe"))
            .await
            .is_err()
        {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "the daemon did not answer on {}",
                socket.display()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        child
    }
}

fn info(name: &str) -> ClientInfo {
    ClientInfo::new(ClientKind::Tui, name)
}

/// What one terminal sees: the view it renders and everything it was told.
struct Terminal {
    view: Arc<AgentSession>,
    remote: Arc<RemoteSession>,
    events: UnboundedReceiver<SessionEvent>,
    notices: UnboundedReceiver<AttachedNotice>,
    _cwd: PathBuf,
}

/// Build a view the way the terminal does (its own tools and settings, an
/// in-memory session) and attach it to `session_id`.
async fn terminal(
    sandbox: &Sandbox,
    provider: &MockProvider,
    name: &str,
    session_id: Option<&str>,
) -> Terminal {
    let (events_tx, events) = unbounded_channel();
    let sink: super_coding::SessionEventSink = Arc::new(move |event| {
        let _ = events_tx.send(event);
    });
    let built = SessionOptions {
        cwd: sandbox.path("work"),
        model: Some("mock/mock-1".into()),
        models_file: Some(provider.catalog_path()),
        settings: Some(super_coding::Settings::default()),
        session: SessionSource::InMemory,
        no_context_files: true,
        ..Default::default()
    }
    .build(sink.clone())
    .expect("the view builds");
    let mut client = DaemonClient::connect_to(&sandbox.socket(), &info(name))
        .await
        .expect("connect");
    let client_events = client.take_events().expect("events");
    let session_id = match session_id {
        Some(id) => id.to_string(),
        None => {
            client
                .create_session(NewSession {
                    cwd: sandbox.path("work"),
                    model: Some("mock/mock-1".into()),
                    ..Default::default()
                })
                .await
                .expect("create")
                .session_id
        }
    };
    let attached = client.attach(&session_id).await.expect("attach");
    let (remote, notices) = RemoteSession::attach(
        Attachment {
            client,
            events: client_events,
            attached,
            info: info(name),
            connector: Connector::Socket(sandbox.socket()),
        },
        &built.session,
        sink,
    )
    .await
    .expect("the view attaches");
    Terminal {
        view: built.session,
        remote,
        events,
        notices,
        _cwd: sandbox.path("work"),
    }
}

async fn next_notice(
    notices: &mut UnboundedReceiver<AttachedNotice>,
    mut wanted: impl FnMut(&AttachedNotice) -> bool,
) -> AttachedNotice {
    loop {
        let notice = tokio::time::timeout(Duration::from_secs(30), notices.recv())
            .await
            .expect("a notice in time")
            .expect("notices keep coming");
        if wanted(&notice) {
            return notice;
        }
    }
}

/// The events seen so far, until `done` matches one (included).
async fn events_until(
    events: &mut UnboundedReceiver<SessionEvent>,
    mut done: impl FnMut(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let mut seen = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(30), events.recv())
            .await
            .expect("an event in time")
            .expect("events keep coming");
        let finished = done(&event);
        seen.push(event);
        if finished {
            return seen;
        }
    }
}

fn agent_end(event: &SessionEvent) -> bool {
    matches!(event, SessionEvent::Agent(inner) if matches!(**inner, AgentEvent::AgentEnd { .. }))
}

fn streamed_text(events: &[SessionEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Agent(inner) => match &**inner {
                AgentEvent::MessageUpdate { assistant_event } => match &**assistant_event {
                    super_ai::AssistantEvent::TextDelta { delta, .. } => Some(delta.clone()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn user_messages(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Agent(inner) => match &**inner {
                AgentEvent::MessageStart {
                    message: AgentMessage::User(user),
                } => Some(user.content.as_text()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// The entry ids of a view's mirror, in order.
fn entry_ids(view: &AgentSession) -> Vec<String> {
    view.manager
        .lock()
        .unwrap()
        .entries()
        .iter()
        .map(|entry| entry.id().to_string())
        .collect()
}

/// The text of the conversation in a view's mirror.
fn transcript(view: &AgentSession) -> Vec<String> {
    view.manager
        .lock()
        .unwrap()
        .build_session_context()
        .messages
        .iter()
        .map(|message| match message {
            AgentMessage::User(user) => format!("user: {}", user.content.as_text()),
            AgentMessage::Assistant(assistant) => format!("assistant: {}", assistant.text()),
            AgentMessage::ToolResult(result) => format!("tool: {}", result.tool_name),
            other => other.role().to_string(),
        })
        .collect()
}

/// Wait until `check` holds, polling the views the pumps keep up to date.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < Duration::from_secs(20), "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

const WORKFLOW: &str = "export const meta = { name: 'greet', description: 'Say hi' }\nreturn 1\n";

fn script() -> MockScript {
    MockScript::text("Hello to both of you.")
        .then(vec![MockTurn::ToolCall {
            id: "call_wf".into(),
            name: "run_workflow".into(),
            arguments: json!({"script": WORKFLOW, "name": "greet"}),
        }])
        .then(vec![MockTurn::Text("Nothing ran.".into())])
        .then(vec![MockTurn::Text("Still here.".into())])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_terminals_share_a_session_answer_its_approvals_and_reattach_with_history() {
    let sandbox = Sandbox::new();
    let provider = MockProvider::start(sandbox.path("mock"), script())
        .await
        .expect("mock provider");
    let mut daemon = sandbox.start_daemon(&provider).await;

    let mut first = terminal(&sandbox, &provider, "Terminal A", None).await;
    let session_id = first.remote.session_id();
    let mut second = terminal(&sandbox, &provider, "Terminal B", Some(&session_id)).await;
    assert_eq!(second.remote.session_id(), session_id);

    // Fan-out: a prompt from the first terminal runs in the daemon, and both
    // views receive the same events, decoded into what they render.
    first
        .view
        .prompt_with_mode(vec![AgentMessage::user("say hi")], PromptMode::Ordinary)
        .await;
    let seen_first = events_until(&mut first.events, agent_end).await;
    let seen_second = events_until(&mut second.events, agent_end).await;
    assert_eq!(streamed_text(&seen_first), "Hello to both of you.");
    assert_eq!(streamed_text(&seen_second), "Hello to both of you.");
    // A prompt that starts a run is announced with its text and origin; a
    // steer shows up as a user message when the run takes it.
    for (terminal, own) in [(&mut first, true), (&mut second, false)] {
        let notice = next_notice(&mut terminal.notices, |notice| {
            matches!(notice, AttachedNotice::CommandApplied(_))
        })
        .await;
        let AttachedNotice::CommandApplied(applied) = notice else {
            unreachable!()
        };
        assert_eq!(applied.origin, "Terminal A");
        assert_eq!(applied.own, own);
        assert_eq!(applied.command["type"], "prompt");
        assert_eq!(applied.command["message"], "say hi");
        assert_eq!(applied.data, Some(json!({"disposition": "started"})));
    }
    assert!(user_messages(&seen_second).is_empty());
    // Both mirrors hold what the daemon wrote, under the daemon's ids.
    eventually("both mirrors catch up", || {
        transcript(&first.view) == ["user: say hi", "assistant: Hello to both of you."]
            && entry_ids(&first.view) == entry_ids(&second.view)
    })
    .await;
    assert!(!first.view.is_running());

    // A command from either terminal reaches both, marked as whose it is.
    second
        .remote
        .set_session_name("shared work")
        .await
        .expect("rename");
    for (terminal, own) in [(&mut first, false), (&mut second, true)] {
        let notice = next_notice(&mut terminal.notices, |notice| {
            matches!(notice, AttachedNotice::CommandApplied(_))
        })
        .await;
        let AttachedNotice::CommandApplied(applied) = notice else {
            unreachable!()
        };
        assert_eq!(applied.origin, "Terminal B");
        assert_eq!(applied.own, own);
        assert_eq!(
            applied.command,
            json!({"type": "set_session_name", "name": "shared work"})
        );
    }
    eventually("the name reaches the other mirror", || {
        first.view.manager.lock().unwrap().session_name().as_deref() == Some("shared work")
    })
    .await;

    // Approvals: the second terminal asks for a workflow, the daemon's gate
    // asks every terminal, the first one answers, and the daemon names it.
    let asking = {
        let view = second.view.clone();
        tokio::spawn(async move {
            view.prompt_with_mode(
                vec![AgentMessage::user("run a workflow to greet")],
                PromptMode::Ordinary,
            )
            .await;
        })
    };
    let mut asked = Vec::new();
    for terminal in [&mut first, &mut second] {
        let notice = next_notice(&mut terminal.notices, |notice| {
            matches!(notice, AttachedNotice::ApprovalRequested { .. })
        })
        .await;
        let AttachedNotice::ApprovalRequested {
            approval_id,
            request,
        } = notice
        else {
            unreachable!()
        };
        assert_eq!(request.title, "Run workflow greet?");
        assert!(request.choices.iter().any(|choice| choice.id == "cancel"));
        asked.push(approval_id);
    }
    assert_eq!(asked[0], asked[1], "one question, asked of both");
    assert!(second.view.is_running());
    first
        .remote
        .answer_approval(&asked[0], "cancel")
        .await
        .expect("answer");
    for terminal in [&mut first, &mut second] {
        let notice = next_notice(&mut terminal.notices, |notice| {
            matches!(notice, AttachedNotice::ApprovalResolved { .. })
        })
        .await;
        assert_eq!(
            notice,
            AttachedNotice::ApprovalResolved {
                approval_id: asked[0].clone(),
                choice: "cancel".into(),
                by: "Terminal A".into(),
            }
        );
    }
    tokio::time::timeout(Duration::from_secs(30), asking)
        .await
        .expect("the asking prompt settles")
        .expect("no panic");
    let after = events_until(&mut first.events, agent_end).await;
    assert_eq!(streamed_text(&after), "Nothing ran.");

    // The first terminal goes away; the session keeps running in the
    // daemon and the second terminal keeps working with it.
    first.remote.detach().await;
    drop(first);
    let status = DaemonClient::connect_to(&sandbox.socket(), &info("status"))
        .await
        .expect("connect")
        .list_sessions(None)
        .await
        .expect("list");
    let hosted = status
        .iter()
        .find(|session| session.session_id == session_id)
        .expect("listed");
    assert!(hosted.hosted, "still running in the daemon");
    assert_eq!(hosted.attached_clients, 1);

    // Coming back: a new terminal attaches and its view has the whole
    // transcript, entry for entry what the other one has.
    let again = terminal(&sandbox, &provider, "Terminal A", Some(&session_id)).await;
    let expected = [
        "user: say hi",
        "assistant: Hello to both of you.",
        "user: run a workflow to greet",
        "assistant: ",
        "tool: run_workflow",
        "assistant: Nothing ran.",
    ];
    assert_eq!(transcript(&again.view), expected);
    eventually("the remaining mirror has everything", || {
        transcript(&second.view) == expected
    })
    .await;
    assert_eq!(entry_ids(&again.view), entry_ids(&second.view));
    assert_eq!(
        again.view.manager.lock().unwrap().session_name().as_deref(),
        Some("shared work")
    );
    assert_eq!(again.view.manager.lock().unwrap().session_id(), session_id);
    assert!(!again.view.manager.lock().unwrap().is_persisted());

    daemon.kill().await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_daemon_is_reported_reconnected_and_finally_handed_back() {
    let sandbox = Sandbox::new();
    let provider = MockProvider::start(
        sandbox.path("mock"),
        MockScript::text("Before the restart."),
    )
    .await
    .expect("mock provider");
    let mut daemon = sandbox.start_daemon(&provider).await;
    let mut terminal = terminal(&sandbox, &provider, "Terminal", None).await;
    terminal
        .view
        .prompt_with_mode(vec![AgentMessage::user("hello")], PromptMode::Ordinary)
        .await;
    events_until(&mut terminal.events, agent_end).await;
    eventually("the mirror has the turn", || {
        transcript(&terminal.view).len() == 2
    })
    .await;
    let before = entry_ids(&terminal.view);

    // The daemon dies; a new one comes up on the same socket in time.
    daemon.kill().await.ok();
    let notice = next_notice(&mut terminal.notices, |notice| {
        matches!(notice, AttachedNotice::Disconnected(_))
    })
    .await;
    assert!(matches!(notice, AttachedNotice::Disconnected(_)));
    assert!(!terminal.remote.is_connected());
    let mut daemon = sandbox.start_daemon(&provider).await;
    next_notice(&mut terminal.notices, |notice| {
        *notice == AttachedNotice::Reconnected
    })
    .await;
    assert!(terminal.remote.is_connected());
    assert_eq!(entry_ids(&terminal.view), before, "the mirror is intact");
    // And the session works again: the restarted daemon loaded it.
    terminal
        .view
        .prompt_with_mode(
            vec![AgentMessage::user("still there?")],
            PromptMode::Ordinary,
        )
        .await;
    events_until(&mut terminal.events, agent_end).await;
    eventually("the second turn lands", || {
        transcript(&terminal.view).len() == 4
    })
    .await;

    // This time nothing comes back: the terminal is told, and the session
    // goes on in this process from its file.
    daemon.kill().await.ok();
    let lost = next_notice(&mut terminal.notices, |notice| {
        matches!(notice, AttachedNotice::Lost(_))
    })
    .await;
    assert!(matches!(lost, AttachedNotice::Lost(_)));
    let file = terminal.remote.release();
    assert!(!terminal.view.is_remote());
    let local = super_coding::SessionManager::open(Path::new(&file)).expect("the file opens");
    assert_eq!(local.session_id(), terminal.remote.session_id());
    assert_eq!(local.build_session_context().messages.len(), 4);
}
