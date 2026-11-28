//! Bash in the background, Monitor and TaskStop, driven through sessions
//! with the session's own queue and task registry.

// Most of these tests need a POSIX shell or git paths and run on Unix
// only; elsewhere their helpers and imports stay unused.
#![cfg_attr(not(unix), allow(unused_imports, dead_code))]

use crate::background_tests::{eventually, model, scripted, text, tool_call, user_text};
use crate::child_turn::ForkTurns;
use crate::notifications::{MONITOR_EVENT, TASK_NOTIFICATION, monitored_event, notified_task};
use crate::session::manager::SessionManager;
use crate::session_runner::AgentSession;
use crate::settings::{Mode, Settings};
use crate::tasks::{TaskKind, TaskSpec, TaskStatus};
use crate::tools::monitor::{Limits, MonitorTool};
use crate::tools::task_stop::TaskStopTool;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_agent::tools::bash::BashTool;
use super_agent::{AgentMessage, AgentTool, DynTool, ToolResult};
use super_ai::{Registry, ThinkingLevel};
use tokio_util::sync::CancellationToken;

fn session_in(directory: &Path, settings: Settings) -> Arc<AgentSession> {
    let manager = SessionManager::create(directory, Some(directory.join(".sessions"))).unwrap();
    let tools: Vec<DynTool> = vec![Arc::new(BashTool::new(directory.to_path_buf()))];
    AgentSession::new(
        manager,
        tools,
        Registry::load(None),
        settings,
        "test".into(),
        model(),
        ThinkingLevel::Off,
        None,
        Arc::new(|_| {}),
    )
}

fn test_settings() -> Settings {
    Settings {
        retry: crate::settings::RetrySettings {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A session whose queued entries start no turn: the test collects them.
fn quiet_session(directory: &Path) -> Arc<AgentSession> {
    let session = session_in(directory, test_settings());
    session.set_turn_starter(Some(Arc::new(|_turn| {})));
    session
}

/// Everything the session has queued so far, collected into `seen`.
fn collect(session: &AgentSession, seen: &Mutex<Vec<AgentMessage>>) -> Vec<AgentMessage> {
    let mut seen = seen.lock().unwrap();
    seen.extend(session.take_queued_for_test());
    seen.clone()
}

fn events_of(messages: &[AgentMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(monitored_event)
        .map(|(_, event)| event)
        .collect()
}

fn notices_of(messages: &[AgentMessage]) -> Vec<crate::TaskInfo> {
    messages.iter().filter_map(notified_task).collect()
}

fn summary_of(message: &AgentMessage) -> String {
    let AgentMessage::Custom(custom) = message else {
        panic!("not a notice: {message:?}");
    };
    custom.details.as_ref().unwrap()["outcome"]["summary"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn call(tool: &dyn AgentTool, id: &str, args: Value) -> anyhow::Result<ToolResult> {
    tool.execute(id, args, CancellationToken::new(), None).await
}

fn monitor(session: &Arc<AgentSession>, limits: Limits) -> MonitorTool {
    MonitorTool::with_limits(Arc::downgrade(session), limits)
}

fn quick_limits() -> Limits {
    Limits {
        group_window: Duration::from_millis(200),
        max_events: 60,
        event_window: Duration::from_secs(60),
    }
}

/// Wait until a notice of `task` is among the queued entries.
async fn notice_of(
    session: &AgentSession,
    seen: &Mutex<Vec<AgentMessage>>,
    task: &str,
) -> AgentMessage {
    for _ in 0..1_000 {
        if let Some(message) = collect(session, seen)
            .into_iter()
            .find(|message| notified_task(message).is_some_and(|info| info.id == task))
        {
            return message;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no notice of {task}");
}

#[tokio::test]
async fn monitor_and_task_stop_come_with_bash_in_the_advanced_mode() {
    let directory = tempfile::tempdir().unwrap();
    let session = session_in(directory.path(), test_settings());
    // `Skill`, `ScheduleWakeup` and the worktree tools come with Bash too.
    assert_eq!(
        session.available_tool_names(),
        [
            "Bash",
            "Monitor",
            "TaskStop",
            "Skill",
            "ScheduleWakeup",
            "EnterWorktree",
            "ExitWorktree"
        ]
    );

    let mut basic = session.settings();
    basic.mode = Mode::Basic;
    session.update_settings(basic);
    assert_eq!(session.available_tool_names(), ["Bash"]);

    let without_bash = AgentSession::new(
        SessionManager::in_memory(directory.path()),
        Vec::new(),
        Registry::load(None),
        test_settings(),
        "test".into(),
        model(),
        ThinkingLevel::Off,
        None,
        Arc::new(|_| {}),
    );
    assert!(without_bash.available_tool_names().is_empty());
}

#[tokio::test]
async fn only_the_main_session_keeps_a_cd() {
    let directory = tempfile::tempdir().unwrap();
    let session = session_in(directory.path(), test_settings());
    assert!(session.tool_session().shell.is_some());
    assert!(session.tool_session().tasks.is_some());
    for fork in [ForkTurns::None, ForkTurns::All] {
        let child = session.test_child("child", fork);
        assert!(child.tool_session().shell.is_none());
        assert!(child.tool_session().tasks.is_some());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_background_command_wakes_the_idle_agent_with_its_notice() {
    let directory = tempfile::tempdir().unwrap();
    let session = session_in(directory.path(), test_settings());
    let (stream, script) = scripted(vec![
        Some(tool_call(
            "call_bg",
            "Bash",
            json!({"command": "echo working; sleep 0.3; echo finished; exit 2", "run_in_background": true, "description": "Slow build"}),
        )),
        Some(text("started it")),
        Some(text("it failed")),
    ]);
    session.set_stream_fn(Some(stream));
    session
        .prompt(vec![AgentMessage::user("build in the background")])
        .await;
    // The call returned at once and the turn ended while the command runs.
    assert_eq!(script.count(), 2);
    let running = session.tasks();
    assert_eq!(running.len(), 1);
    let task = running[0].clone();
    assert_eq!(task.kind, TaskKind::Shell);
    assert_eq!(task.description, "Slow build");
    assert_eq!(task.tool_use_id.as_deref(), Some("call_bg"));
    assert!(task.id.starts_with('b'));
    let output = task.output_file.clone().unwrap();

    session.wait_for_background_work().await;
    let requests = script.requests();
    assert_eq!(requests.len(), 3, "the notice started a turn");
    let notice = user_text(requests[2].last().unwrap()).unwrap();
    assert!(notice.starts_with(&super_agent::placeholder::prompt_todo(
        "Notification.header"
    )));
    assert!(notice.contains(&format!("<task-id>{}</task-id>", task.id)));
    assert!(notice.contains("<tool-use-id>call_bg</tool-use-id>"));
    assert!(notice.contains(&format!("<output-file>{}</output-file>", output.display())));
    assert!(notice.contains("<status>failed</status>"));
    assert!(notice.contains("<summary>Slow build (exit code 2)</summary>"));
    let written = std::fs::read_to_string(&output).unwrap();
    assert_eq!(written, "working\nfinished\n");
    assert_eq!(session.tasks()[0].status, TaskStatus::Failed);
}

#[cfg(unix)]
#[tokio::test]
async fn monitor_lines_within_the_window_form_one_event() {
    let directory = tempfile::tempdir().unwrap();
    let session = quiet_session(directory.path());
    let seen = Mutex::new(Vec::new());
    let tool = monitor(&session, quick_limits());
    let result = call(
        &tool,
        "call_mon",
        json!({"description": "Watch the build", "timeout_ms": 10_000, "command": "echo one; echo two; echo hidden 1>&2; sleep 0.6; echo three; echo; sleep 0.6"}),
    )
    .await
    .unwrap();
    let id = result.details["taskId"].as_str().unwrap().to_string();
    assert!(id.starts_with('m'));
    assert_eq!(
        result.output_text(),
        format!(
            "The monitor runs as task {id} for up to 10000 ms. Its output is written to {}.",
            result.details["outputFile"].as_str().unwrap()
        )
    );
    let notice = notice_of(&session, &seen, &id).await;
    let messages = collect(&session, &seen);
    assert_eq!(events_of(&messages), ["one\ntwo", "three"]);
    // Every event, then the one notice, all through the queue.
    let kinds: Vec<String> = messages
        .iter()
        .map(|message| match message {
            AgentMessage::Custom(custom) => custom.custom_type.clone(),
            other => other.role().to_string(),
        })
        .collect();
    assert_eq!(kinds, [MONITOR_EVENT, MONITOR_EVENT, TASK_NOTIFICATION]);
    let (task, _) = monitored_event(&messages[0]).unwrap();
    assert_eq!(task.id, id);
    assert_eq!(task.tool_use_id.as_deref(), Some("call_mon"));
    let AgentMessage::Custom(first) = &messages[0] else {
        unreachable!()
    };
    let first = first.content.as_text();
    assert!(first.starts_with(&super_agent::placeholder::prompt_todo(
        "Notification.monitor_event"
    )));
    assert!(first.contains("<status>running</status>"));
    assert!(first.contains("<event>one\ntwo</event>"));
    assert_eq!(
        notified_task(&notice).unwrap().status,
        TaskStatus::Completed
    );
    assert_eq!(summary_of(&notice), "Watch the build (exit code 0)");
    // stderr went to the output file only, with stdout.
    let output = std::fs::read_to_string(result.details["outputFile"].as_str().unwrap()).unwrap();
    for line in ["one", "two", "hidden", "three"] {
        assert!(output.contains(line), "{output}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_monitor_with_too_many_events_is_stopped_with_a_notice() {
    let directory = tempfile::tempdir().unwrap();
    let session = quiet_session(directory.path());
    let seen = Mutex::new(Vec::new());
    let marker = directory.path().join("went-on");
    let tool = monitor(
        &session,
        Limits {
            group_window: Duration::from_millis(20),
            max_events: 3,
            event_window: Duration::from_secs(60),
        },
    );
    let result = call(
        &tool,
        "call_flood",
        json!({"description": "Flood", "timeout_ms": 20_000, "command": format!("for i in 1 2 3 4 5 6; do echo line $i; sleep 0.1; done; touch {}", marker.display())}),
    )
    .await
    .unwrap();
    let id = result.details["taskId"].as_str().unwrap().to_string();
    let notice = notice_of(&session, &seen, &id).await;
    let messages = collect(&session, &seen);
    assert_eq!(events_of(&messages), ["line 1", "line 2", "line 3"]);
    assert_eq!(notified_task(&notice).unwrap().status, TaskStatus::Stopped);
    assert_eq!(
        summary_of(&notice),
        "Flood (stopped after more than 3 events within 60 seconds)"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!marker.exists(), "the script was not stopped");
}

#[cfg(unix)]
#[tokio::test]
async fn a_monitor_ends_at_its_time_limit_with_one_notice() {
    let directory = tempfile::tempdir().unwrap();
    let session = quiet_session(directory.path());
    let seen = Mutex::new(Vec::new());
    let tool = monitor(&session, quick_limits());
    let started = std::time::Instant::now();
    let result = call(
        &tool,
        "call_wait",
        json!({"description": "Wait for a file", "timeout_ms": 1000, "command": "echo waiting; sleep 30"}),
    )
    .await
    .unwrap();
    let id = result.details["taskId"].as_str().unwrap().to_string();
    let notice = notice_of(&session, &seen, &id).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(started.elapsed() >= Duration::from_millis(1000));
    assert_eq!(notified_task(&notice).unwrap().status, TaskStatus::Stopped);
    assert_eq!(
        summary_of(&notice),
        "Wait for a file (stopped at its time limit of 1000 ms)"
    );
    let messages = collect(&session, &seen);
    assert_eq!(events_of(&messages), ["waiting"]);
    assert_eq!(notices_of(&messages).len(), 1);

    // timeout_ms is cut to ten minutes, and a source is needed.
    let result = call(
        &tool,
        "call_long",
        json!({"description": "Long", "timeout_ms": 3_600_000, "command": "true"}),
    )
    .await
    .unwrap();
    assert_eq!(result.details["timeoutMs"], 600_000);
    for args in [
        json!({"description": "Nothing", "timeout_ms": 5000}),
        json!({"description": "Both", "timeout_ms": 5000, "command": "true", "ws": {"url": "ws://localhost:1"}}),
        json!({"description": "Bad", "timeout_ms": 5000, "ws": {"url": "http://localhost:1"}}),
        json!({"description": "", "timeout_ms": 5000, "command": "true"}),
    ] {
        assert!(
            call(&tool, "call_bad", args.clone()).await.is_err(),
            "{args}"
        );
    }
    let schema = tool.parameters();
    assert!(
        super_agent::validate::validate_arguments(
            &schema,
            &json!({"description": "x", "timeout_ms": 500, "command": "true"})
        )
        .is_err()
    );
    assert!(
        super_agent::validate::validate_arguments(
            &schema,
            &json!({"description": "x", "timeout_ms": 5000, "ws": {"url": "ws://h", "protocols": ["bad protocol"]}})
        )
        .is_err()
    );
}

// The handshake callback's error type is the library's.
#[allow(clippy::result_large_err)]
#[tokio::test]
async fn a_websocket_source_turns_frames_into_events_until_it_closes() {
    use futures::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (asked_tx, asked_rx) = tokio::sync::oneshot::channel::<Option<String>>();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut asked_tx = Some(asked_tx);
        let mut socket = tokio_tungstenite::accept_hdr_async(
            stream,
            |request: &Request, mut response: Response| {
                let protocol = request
                    .headers()
                    .get("sec-websocket-protocol")
                    .map(|value| value.to_str().unwrap().to_string());
                if let Some(protocol) = &protocol {
                    let first = protocol.split(',').next().unwrap().trim().to_string();
                    response
                        .headers_mut()
                        .insert("sec-websocket-protocol", first.parse().unwrap());
                }
                let _ = asked_tx.take().unwrap().send(protocol);
                Ok(response)
            },
        )
        .await
        .unwrap();
        socket.send(Message::text("build started")).await.unwrap();
        socket.send(Message::text("step 1 of 2")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        socket.send(Message::binary(vec![1u8, 2, 3])).await.unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        socket
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::Normal,
                reason: "done".into(),
            })))
            .await
            .unwrap();
        // Read until the client answers the close.
        while let Some(Ok(_)) = socket.next().await {}
    });

    let directory = tempfile::tempdir().unwrap();
    let session = quiet_session(directory.path());
    let seen = Mutex::new(Vec::new());
    let tool = monitor(&session, quick_limits());
    let result = call(
        &tool,
        "call_ws",
        json!({"description": "Build feed", "timeout_ms": 10_000, "ws": {"url": format!("ws://{address}/feed"), "protocols": ["build.v1", "build.v0"]}}),
    )
    .await
    .unwrap();
    assert_eq!(
        asked_rx.await.unwrap().as_deref(),
        Some("build.v1, build.v0")
    );
    let id = result.details["taskId"].as_str().unwrap().to_string();
    let notice = notice_of(&session, &seen, &id).await;
    let messages = collect(&session, &seen);
    assert_eq!(
        events_of(&messages),
        ["build started\nstep 1 of 2", "[binary frame of 3 bytes]"]
    );
    assert_eq!(
        notified_task(&notice).unwrap().status,
        TaskStatus::Completed
    );
    assert_eq!(
        summary_of(&notice),
        "Build feed (the server closed the connection with code 1000: done)"
    );
    let output = std::fs::read_to_string(result.details["outputFile"].as_str().unwrap()).unwrap();
    assert_eq!(
        output,
        "build started\nstep 1 of 2\n[binary frame of 3 bytes]\n"
    );
    server.await.unwrap();

    // Nothing listens: the call fails and no task is left.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unused = closed.local_addr().unwrap();
    drop(closed);
    let before = session.tasks().len();
    let error = call(
        &tool,
        "call_none",
        json!({"description": "Nothing", "timeout_ms": 5_000, "ws": {"url": format!("ws://{unused}")}}),
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("Could not open the WebSocket"));
    assert_eq!(session.tasks().len(), before);
}

#[cfg(unix)]
#[tokio::test]
async fn task_stop_stops_a_task_by_id_or_by_name() {
    let directory = tempfile::tempdir().unwrap();
    let session = quiet_session(directory.path());
    let seen = Mutex::new(Vec::new());
    let stop = TaskStopTool::new(Arc::downgrade(&session));
    assert_eq!(
        stop.parameters()["properties"].as_object().unwrap().len(),
        2
    );
    let bash = BashTool::new(directory.path().to_path_buf())
        .for_session(session.tool_session())
        .unwrap();

    // A shell command, by its id.
    let marker = directory.path().join("went-on");
    let started = call(
        bash.as_ref(),
        "call_bg",
        json!({"command": format!("sleep 1; touch {}", marker.display()), "run_in_background": true, "description": "Touch later"}),
    )
    .await
    .unwrap();
    let id = started.details["backgroundTaskId"]
        .as_str()
        .unwrap()
        .to_string();
    let result = call(&stop, "call_stop", json!({"task_id": id}))
        .await
        .unwrap();
    assert_eq!(
        result.output_text(),
        format!("Stopped the shell task {id} (Touch later).")
    );
    assert_eq!(result.details["task"]["status"], "stopped");

    // A monitor, by its id under the old name shell_id.
    let watched = call(
        &monitor(&session, quick_limits()),
        "call_mon",
        json!({"description": "Watch", "timeout_ms": 30_000, "command": "sleep 30"}),
    )
    .await
    .unwrap();
    let monitor_id = watched.details["taskId"].as_str().unwrap().to_string();
    call(&stop, "call_stop", json!({"shell_id": monitor_id}))
        .await
        .unwrap();

    // Any task, an agent here, by its name.
    let handle = session
        .background_tasks()
        .register(TaskSpec::new(TaskKind::Agent, "review the diff").with_name("reviewer"))
        .unwrap();
    let result = call(&stop, "call_stop", json!({"task_id": "reviewer"}))
        .await
        .unwrap();
    assert_eq!(result.details["task"]["id"], handle.id());
    assert!(handle.is_stopped());

    let statuses: Vec<TaskStatus> = session.tasks().iter().map(|task| task.status).collect();
    assert_eq!(statuses, [TaskStatus::Stopped; 3]);

    // What cannot be stopped is a plain error.
    let error = call(&stop, "call_stop", json!({"task_id": id}))
        .await
        .unwrap_err();
    assert_eq!(
        format!("{error:#}"),
        format!("Background task {id} is not running: it stopped already.")
    );
    let error = call(&stop, "call_stop", json!({"task_id": "nope"}))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("No background task has the id or name 'nope'"));
    assert!(call(&stop, "call_stop", json!({})).await.is_err());

    // A stopped task ends without a notice, and its process with it.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!marker.exists(), "the stopped command went on");
    assert!(notices_of(&collect(&session, &seen)).is_empty());
    eventually("nothing runs", || !session.background_tasks().has_running()).await;
}

/// A monitor whose shell ends in its profile delivers no event of what the
/// profile printed, and its notice says that the command did not run.
#[cfg(unix)]
#[tokio::test]
async fn a_monitor_whose_shell_ends_in_its_profile_says_its_command_did_not_run() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join(".bash_profile"), "echo switching; exit 0\n").unwrap();
    let shell = directory.path().join("bin/bash");
    std::fs::create_dir_all(shell.parent().unwrap()).unwrap();
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\nHOME='{}' exec /bin/bash \"$@\"\n",
            home.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    let settings = Settings {
        shell_path: Some(shell.display().to_string()),
        ..test_settings()
    };
    let session = session_in(directory.path(), settings);
    session.set_turn_starter(Some(Arc::new(|_turn| {})));
    let seen = Mutex::new(Vec::new());
    let tool = monitor(&session, quick_limits());
    let result = call(
        &tool,
        "call_mon",
        json!({"description": "m", "timeout_ms": 10_000, "command": "echo EVENT"}),
    )
    .await
    .unwrap();
    let id = result.details["taskId"].as_str().unwrap().to_string();
    let notice = notice_of(&session, &seen, &id).await;
    let messages = collect(&session, &seen);
    assert!(events_of(&messages).is_empty(), "{messages:?}");
    assert_eq!(notified_task(&notice).unwrap().status, TaskStatus::Failed);
    assert_eq!(
        summary_of(&notice),
        "m (the shell ended while it ran the profile, so the command did not run)"
    );
    let output = std::fs::read_to_string(result.details["outputFile"].as_str().unwrap()).unwrap();
    assert_eq!(output, "switching\n");
}
