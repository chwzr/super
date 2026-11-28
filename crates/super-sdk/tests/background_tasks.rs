//! Background tasks through the SDK dispatcher, which `--mode rpc`, the
//! daemon and the bindings all call: an interrupt in both client modes, the
//! stop command, and a notice that starts a turn of its own.

#![cfg(all(feature = "mock", feature = "native"))]

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_ai::{AssistantEvent, AssistantMessage, ContentBlock, EventStream, StopReason};
use super_coding::{TaskHandle, TaskKind, TaskOutcome, TaskSpec};
use super_sdk::mock::{MockProvider, MockScript};
use super_sdk::protocol::Command;
use super_sdk::{CommandSender, PromptArgs, Session, SessionOptions, StreamFn, ThinkingLevel};

/// A scripted provider: request `n` is answered with `replies[n]`; `None`
/// holds the request until the turn is interrupted.
fn scripted(replies: Vec<Option<&'static str>>) -> (StreamFn, Arc<Mutex<usize>>) {
    let requests = Arc::new(Mutex::new(0usize));
    let seen = requests.clone();
    let stream: StreamFn = Arc::new(move |_, _, options| {
        let step = {
            let mut count = seen.lock().unwrap();
            *count += 1;
            *count - 1
        };
        let (sink, stream) = EventStream::channel();
        let mut message = AssistantMessage::empty("openai-completions", "mock", "mock-1");
        match replies.get(step).copied().flatten() {
            Some(text) => {
                message.content.push(ContentBlock::text(text));
                message.stop_reason = StopReason::Stop;
                sink.send(AssistantEvent::Start {
                    partial: message.clone(),
                });
                sink.done(message);
            }
            None => {
                let cancel = options.cancel.clone();
                tokio::spawn(async move {
                    cancel.cancelled().await;
                    message.stop_reason = StopReason::Aborted;
                    sink.send(AssistantEvent::Start {
                        partial: message.clone(),
                    });
                    sink.error(message);
                });
            }
        }
        stream
    });
    (stream, requests)
}

async fn session(
    directory: &std::path::Path,
    replies: Vec<Option<&'static str>>,
) -> (MockProvider, Arc<Session>, Arc<Mutex<usize>>) {
    // The mock provider only supplies the catalog; the scripted stream
    // answers every request.
    let provider = MockProvider::start(directory, MockScript::text("unused"))
        .await
        .expect("mock provider starts");
    let (stream, requests) = scripted(replies);
    let options = SessionOptions {
        cwd: directory.to_path_buf(),
        model: Some("mock/mock-1".into()),
        models_file: Some(provider.catalog_path()),
        thinking_level: Some(ThinkingLevel::Off),
        settings: Some(super_coding::Settings::default()),
        no_tools: true,
        no_context_files: true,
        stream_fn: Some(stream),
        ..Default::default()
    };
    let session = Session::create(options).await.expect("session builds");
    (provider, session, requests)
}

fn collect_events(session: &Arc<Session>) -> Arc<Mutex<Vec<Value>>> {
    let collected: Arc<Mutex<Vec<Value>>> = Default::default();
    let sink = collected.clone();
    let mut events = session.events();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            sink.lock().unwrap().push(event.0);
        }
    });
    collected
}

async fn eventually(what: &str, check: impl Fn() -> bool) {
    for _ in 0..400 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting until {what}");
}

fn register(session: &Session, spec: TaskSpec) -> TaskHandle {
    session
        .agent_session()
        .background_tasks()
        .register(spec)
        .expect("task registers")
}

/// Start a prompt whose model request waits until it is interrupted.
async fn start_waiting_turn(session: &Arc<Session>, requests: &Arc<Mutex<usize>>) {
    session
        .prompt_detached(PromptArgs::new("wait for the build"))
        .expect("prompt accepted");
    eventually("the turn waits for the model", || {
        *requests.lock().unwrap() == 1
    })
    .await;
}

#[tokio::test]
async fn an_abort_from_a_client_without_stop_controls_also_stops_the_background_tasks() {
    let directory = tempfile::tempdir().unwrap();
    let (_provider, session, requests) = session(directory.path(), vec![None]).await;
    let events = collect_events(&session);
    let handle = register(&session, TaskSpec::new(TaskKind::Shell, "cargo build"));
    start_waiting_turn(&session, &requests).await;

    let response = session.execute(Command::Abort {}).await;
    assert!(response.success, "{response:?}");
    assert!(!session.state().is_streaming);
    assert!(handle.is_stopped());
    assert_eq!(
        session.tasks()[0].status,
        super_sdk::TaskStatus::Stopped,
        "stopped, not just cancelled"
    );
    eventually("the end is announced", || {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "task_ended" && event["task"]["status"] == "stopped")
    })
    .await;

    // A remote client that never said otherwise is one without controls.
    let handle = register(&session, TaskSpec::new(TaskKind::Monitor, "tail the log"));
    let response = session
        .execute_from(Command::Abort {}, None, Some("iPhone".into()))
        .await;
    assert!(response.success);
    assert!(handle.is_stopped());
}

#[tokio::test]
async fn an_abort_from_a_client_with_stop_controls_cancels_the_turn_only() {
    let directory = tempfile::tempdir().unwrap();
    let (_provider, session, requests) = session(directory.path(), vec![None, Some("noted")]).await;
    let response = session
        .execute(Command::Initialize {
            task_controls: true,
        })
        .await;
    assert!(response.success, "{response:?}");
    let handle = register(
        &session,
        TaskSpec::new(TaskKind::Shell, "cargo build").with_name("build"),
    );
    start_waiting_turn(&session, &requests).await;

    assert!(session.execute(Command::Abort {}).await.success);
    assert!(!session.state().is_streaming);
    assert!(!handle.is_stopped());
    let listed = session.execute(Command::GetTasks {}).await;
    assert_eq!(listed.data.unwrap()["tasks"][0]["status"], "running");

    // The task's stop control: by name, then once more, then a wrong id.
    let stopped = session
        .execute(Command::StopTask {
            task_id: "build".into(),
        })
        .await;
    assert!(stopped.success, "{stopped:?}");
    assert_eq!(stopped.data.unwrap()["task"]["status"], "stopped");
    assert!(handle.is_stopped());
    let again = session
        .execute(Command::StopTask {
            task_id: handle.id().into(),
        })
        .await;
    assert!(!again.success);
    assert!(again.error.unwrap().contains("is not running"));
    let unknown = session
        .execute(Command::StopTask {
            task_id: "b00000000".into(),
        })
        .await;
    assert!(!unknown.success);

    // A daemon client says it in hello; the daemon passes that on.
    let spared = register(&session, TaskSpec::new(TaskKind::Agent, "review"));
    let response = session
        .execute_by(
            Command::Abort {},
            None,
            CommandSender {
                origin: Some("Terminal (ttys003)".into()),
                task_controls: true,
            },
        )
        .await;
    assert!(response.success);
    assert!(!spared.is_stopped());
    drop(spared);
}

#[tokio::test]
async fn a_task_that_ends_while_idle_starts_a_turn_that_settles() {
    let directory = tempfile::tempdir().unwrap();
    let (_provider, session, requests) = session(directory.path(), vec![Some("read it")]).await;
    let events = collect_events(&session);
    let handle = register(
        &session,
        TaskSpec::new(TaskKind::Shell, "npm test").with_tool_use_id("call_1"),
    );
    eventually("the start is announced", || {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "task_started")
    })
    .await;
    assert!(handle.finish(TaskOutcome::completed("npm test ended")));
    eventually("the notice's turn settled", || {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "agent_settled")
    })
    .await;
    session.wait_idle().await;
    assert_eq!(*requests.lock().unwrap(), 1);

    let events = events.lock().unwrap().clone();
    let position = |kind: &str| {
        events
            .iter()
            .position(|event| event["type"] == kind)
            .unwrap_or_else(|| panic!("no {kind} in {events:?}"))
    };
    assert!(position("task_ended") < position("agent_start"));
    assert!(position("agent_start") < position("agent_settled"));
    // The notice is a message clients show as a notice.
    let notice = events
        .iter()
        .find(|event| event["type"] == "message_end" && event["message"]["role"] == "custom")
        .expect("the notice is delivered");
    assert_eq!(notice["message"]["customType"], "task_notification");
    assert_eq!(notice["message"]["details"]["task"]["toolUseId"], "call_1");
    assert_eq!(
        notice["message"]["details"]["outcome"],
        json!({"status": "completed", "summary": "npm test ended"})
    );
}
