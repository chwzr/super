//! The queue for entries from outside a tool call and the background task
//! registry, driven through whole agent runs with a scripted provider.

use crate::notifications::{Priority, TASK_NOTIFICATION, is_task_notification};
use crate::session::manager::SessionManager;
use crate::session_runner::{AgentSession, SessionEvent};
use crate::settings::Settings;
use crate::tasks::{TaskHandle, TaskKind, TaskOutcome, TaskSpec, TaskStatus, TaskStopError};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use super_agent::{AgentMessage, AgentTool, StreamFn, ToolResult, ToolUpdateSink};
use super_ai::{
    AssistantEvent, AssistantMessage, ContentBlock, EventStream, Message, Model, Registry,
    StopReason, ThinkingLevel, ToolCall,
};
use tokio_util::sync::CancellationToken;

fn model() -> Model {
    Model {
        id: "scripted".into(),
        name: "Scripted".into(),
        api: "openai-responses".into(),
        provider: "scripted".into(),
        base_url: "http://localhost:0".into(),
        reasoning: false,
        input: vec!["text".into()],
        cost: Default::default(),
        prompt_cache: None,
        context_window: 100_000,
        max_tokens: 1_000,
        compat: None,
        thinking_level_map: BTreeMap::new(),
        headers: BTreeMap::new(),
        sampling_params: Default::default(),
    }
}

fn text(reply: &str) -> AssistantMessage {
    let mut message = AssistantMessage::empty("openai-responses", "scripted", "scripted");
    message.content.push(ContentBlock::text(reply));
    message.stop_reason = StopReason::Stop;
    message
}

fn tool_call(id: &str, name: &str, arguments: Value) -> AssistantMessage {
    let mut message = AssistantMessage::empty("openai-responses", "scripted", "scripted");
    message.content.push(ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
        thought_signature: None,
    }));
    message.stop_reason = StopReason::ToolUse;
    message
}

/// What the scripted provider saw and what the test can do while it runs.
#[derive(Default)]
struct Script {
    /// The messages of every request, in order.
    requests: Mutex<Vec<Vec<Message>>>,
}

impl Script {
    fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

/// A provider that answers request `n` with `replies[n]`, and fails once
/// the script runs out. `hold` makes a request wait until it is cancelled,
/// for a turn that must still run when the test interrupts it.
fn scripted(replies: Vec<Option<AssistantMessage>>) -> (StreamFn, Arc<Script>) {
    let script = Arc::new(Script::default());
    let seen = script.clone();
    let stream: StreamFn = Arc::new(move |_, context, options| {
        let step = {
            let mut requests = seen.requests.lock().unwrap();
            requests.push(context.messages.clone());
            requests.len() - 1
        };
        let (sink, stream) = EventStream::channel();
        match replies.get(step) {
            Some(Some(message)) => {
                sink.send(AssistantEvent::Start {
                    partial: message.clone(),
                });
                sink.done(message.clone());
            }
            Some(None) => {
                // Held until the turn is interrupted.
                let cancel = options.cancel.clone();
                tokio::spawn(async move {
                    cancel.cancelled().await;
                    let mut message =
                        AssistantMessage::empty("openai-responses", "scripted", "scripted");
                    message.stop_reason = StopReason::Aborted;
                    sink.send(AssistantEvent::Start {
                        partial: message.clone(),
                    });
                    sink.error(message);
                });
            }
            None => {
                let mut message =
                    AssistantMessage::empty("openai-responses", "scripted", "scripted");
                message.stop_reason = StopReason::Error;
                message.error_message = Some("script exhausted".into());
                sink.send(AssistantEvent::Start {
                    partial: message.clone(),
                });
                sink.error(message);
            }
        }
        stream
    });
    (stream, script)
}

/// The test double for a background task tool. `StartTask` registers a
/// task, `FinishTask` ends one (while the agent is busy), `QueueLater`
/// queues an entry at the later priority.
struct TestTool {
    name: &'static str,
    session: Arc<OnceLock<Weak<AgentSession>>>,
    handles: Arc<Mutex<Vec<TaskHandle>>>,
}

#[async_trait::async_trait]
impl AgentTool for TestTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> String {
        String::new()
    }

    fn parameters(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        _args: Value,
        _cancel: CancellationToken,
        _on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        let session = self.session.get().unwrap().upgrade().unwrap();
        match self.name {
            "StartTask" => {
                let handle = session
                    .background_tasks()
                    .register(
                        TaskSpec::new(TaskKind::Shell, "sleep 5").with_tool_use_id(tool_call_id),
                    )
                    .unwrap();
                let id = handle.id().to_string();
                self.handles.lock().unwrap().push(handle);
                Ok(ToolResult::text(id))
            }
            "FinishTask" => {
                let handle = self.handles.lock().unwrap().remove(0);
                assert!(
                    handle.finish(TaskOutcome::completed("sleep 5 ended").with_result("slept"))
                );
                Ok(ToolResult::text("finished"))
            }
            "QueueLater" => {
                session.queue_entry(AgentMessage::user("later entry"), Priority::Later);
                Ok(ToolResult::text("queued"))
            }
            other => unreachable!("{other}"),
        }
    }
}

struct Fixture {
    session: Arc<AgentSession>,
    script: Arc<Script>,
    events: Arc<Mutex<Vec<SessionEvent>>>,
    handles: Arc<Mutex<Vec<TaskHandle>>>,
    _directory: tempfile::TempDir,
}

fn fixture(replies: Vec<Option<AssistantMessage>>) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let manager =
        SessionManager::create(directory.path(), Some(directory.path().join("sessions"))).unwrap();
    let settings = Settings {
        retry: crate::settings::RetrySettings {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let slot: Arc<OnceLock<Weak<AgentSession>>> = Arc::default();
    let handles: Arc<Mutex<Vec<TaskHandle>>> = Arc::default();
    let tools = ["StartTask", "FinishTask", "QueueLater"]
        .into_iter()
        .map(|name| {
            Arc::new(TestTool {
                name,
                session: slot.clone(),
                handles: handles.clone(),
            }) as super_agent::DynTool
        })
        .collect();
    let events: Arc<Mutex<Vec<SessionEvent>>> = Arc::default();
    let recorded = events.clone();
    let session = AgentSession::new(
        manager,
        tools,
        Registry::load(None),
        settings,
        "test".into(),
        model(),
        ThinkingLevel::Off,
        None,
        Arc::new(move |event| recorded.lock().unwrap().push(event)),
    );
    slot.set(Arc::downgrade(&session)).unwrap();
    let (stream, script) = scripted(replies);
    session.set_stream_fn(Some(stream));
    Fixture {
        session,
        script,
        events,
        handles,
        _directory: directory,
    }
}

fn user_text(message: &Message) -> Option<String> {
    match message {
        Message::User(user) => Some(user.content.as_text()),
        _ => None,
    }
}

fn is_notice_text(text: &str, task_id: &str) -> bool {
    text.starts_with(&super_agent::placeholder::prompt_todo(
        "Notification.header",
    )) && text.contains(&format!("<task-id>{task_id}</task-id>"))
}

async fn eventually(what: &str, check: impl Fn() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting until {what}");
}

#[tokio::test]
async fn a_task_that_ends_while_the_agent_is_idle_starts_a_turn() {
    let fixture = fixture(vec![Some(text("noted"))]);
    let tasks = fixture.session.background_tasks();
    let handle = tasks
        .register(
            TaskSpec::new(TaskKind::Shell, "npm test")
                .with_name("tests")
                .with_tool_use_id("call_9"),
        )
        .unwrap();
    let id = handle.id().to_string();
    assert!(fixture.session.has_background_work());
    assert_eq!(fixture.script.count(), 0);

    assert!(handle.finish(TaskOutcome::completed("npm test ended").with_result("all green")));
    fixture.session.wait_for_background_work().await;

    // One turn ran, and the notice was the last thing the model read.
    let requests = fixture.script.requests();
    assert_eq!(requests.len(), 1);
    let last = user_text(requests[0].last().unwrap()).unwrap();
    assert!(is_notice_text(&last, &id), "{last}");
    assert!(last.contains("<tool-use-id>call_9</tool-use-id>"));
    assert!(last.contains("<status>completed</status>"));
    assert!(last.contains("<result>all green</result>"));

    // The history records it as a notice, and a reload still has it.
    let file = fixture
        .session
        .manager
        .lock()
        .unwrap()
        .session_file()
        .unwrap()
        .to_path_buf();
    let reloaded = SessionManager::open(&file).unwrap().build_session_context();
    let kinds: Vec<String> = reloaded
        .messages
        .iter()
        .map(|message| match message {
            AgentMessage::Custom(custom) => custom.custom_type.clone(),
            other => other.role().to_string(),
        })
        .collect();
    assert_eq!(kinds, [TASK_NOTIFICATION, "assistant"]);
    assert!(!fixture.session.has_background_work());

    // Clients heard the task start and end, and saw the notice delivered.
    let events = fixture.events.lock().unwrap();
    assert!(events.iter().any(
        |event| matches!(event, SessionEvent::TaskStarted(task) if task.id == id && task.name.as_deref() == Some("tests"))
    ));
    assert!(events.iter().any(|event| matches!(event, SessionEvent::TaskEnded(task) if task.id == id && task.status == TaskStatus::Completed)));
    assert!(events.iter().any(|event| matches!(event, SessionEvent::Agent(agent) if matches!(agent.as_ref(), super_agent::AgentEvent::MessageEnd { message } if is_task_notification(message)))));
}

#[tokio::test]
async fn a_notice_reaches_a_busy_agent_between_two_tool_rounds() {
    let fixture = fixture(vec![
        Some(tool_call("call_start", "StartTask", json!({}))),
        Some(tool_call("call_finish", "FinishTask", json!({}))),
        Some(text("both rounds done")),
    ]);
    fixture
        .session
        .prompt(vec![AgentMessage::user("start and finish a task")])
        .await;
    fixture.session.wait_for_background_work().await;

    let requests = fixture.script.requests();
    assert_eq!(requests.len(), 3, "no extra turn after the run");
    let id = fixture.session.background_tasks().list()[0].id.clone();
    // Not before the task ended ...
    assert!(
        !requests[1]
            .iter()
            .filter_map(user_text)
            .any(|text| is_notice_text(&text, &id))
    );
    // ... and right after the tool round in which it ended.
    let after = &requests[2];
    assert!(
        matches!(&after[after.len() - 2], Message::ToolResult(result) if result.tool_call_id == "call_finish")
    );
    assert!(is_notice_text(
        &user_text(after.last().unwrap()).unwrap(),
        &id
    ));
    assert!(fixture.handles.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_later_entry_waits_until_the_turn_is_over() {
    let fixture = fixture(vec![
        Some(tool_call("call_later", "QueueLater", json!({}))),
        Some(text("first turn done")),
        Some(text("read the later entry")),
    ]);
    fixture
        .session
        .prompt(vec![AgentMessage::user("queue something for later")])
        .await;
    fixture.session.wait_for_background_work().await;

    let requests = fixture.script.requests();
    assert_eq!(requests.len(), 3);
    // The step after the tool round does not have it yet.
    assert!(
        !requests[1]
            .iter()
            .filter_map(user_text)
            .any(|text| text == "later entry")
    );
    // It comes after the answer that ended the turn.
    let last = &requests[2];
    assert_eq!(
        user_text(last.last().unwrap()).as_deref(),
        Some("later entry")
    );
    assert!(
        matches!(&last[last.len() - 2], Message::Assistant(answer) if answer.text() == "first turn done")
    );
}

#[tokio::test]
async fn a_task_stops_by_id_or_by_name_without_a_notice() {
    let fixture = fixture(Vec::new());
    let tasks = fixture.session.background_tasks();
    let by_id = tasks
        .register(TaskSpec::new(TaskKind::Monitor, "tail the log"))
        .unwrap();
    let by_name = tasks
        .register(TaskSpec::new(TaskKind::Agent, "review the diff").with_name("reviewer"))
        .unwrap();
    assert!(by_id.id().starts_with('m'));
    assert!(by_name.id().starts_with('a'));
    assert_eq!(
        tasks
            .register(TaskSpec::new(TaskKind::Agent, "again").with_name("reviewer"))
            .err(),
        Some(crate::tasks::TaskRegisterError::DuplicateName(
            "reviewer".into()
        ))
    );
    assert_eq!(tasks.find("reviewer").unwrap().id, by_name.id());
    assert_eq!(tasks.running().len(), 2);

    let stopped = tasks.stop(by_id.id()).unwrap();
    assert_eq!(stopped.status, TaskStatus::Stopped);
    assert!(by_id.is_stopped());
    let stopped = fixture.session.stop_task("reviewer").unwrap();
    assert_eq!(stopped.id, by_name.id());
    assert!(by_name.stop_signal().is_cancelled());
    assert!(!tasks.has_running());

    assert_eq!(
        tasks.stop("reviewer"),
        Err(TaskStopError::NotRunning {
            id: by_name.id().to_string(),
            status: TaskStatus::Stopped,
        })
    );
    assert_eq!(
        tasks.stop("nothing"),
        Err(TaskStopError::Unknown("nothing".into()))
    );
    // The owner's late finish changes nothing and queues no notice.
    assert!(!by_name.finish(TaskOutcome::completed("done anyway")));
    drop(by_id);
    fixture.session.wait_for_background_work().await;
    assert_eq!(fixture.script.count(), 0, "a stopped task starts no turn");
    assert!(!fixture.session.has_queued_entries());
    let ended = fixture
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| matches!(event, SessionEvent::TaskEnded(task) if task.status == TaskStatus::Stopped))
        .count();
    assert_eq!(ended, 2);
}

#[tokio::test]
async fn an_interrupt_cancels_the_turn_and_spares_the_tasks() {
    // The turn holds its model request until it is interrupted; the task
    // registered before it keeps running and announces itself afterwards.
    let fixture = fixture(vec![None, Some(text("the task is done"))]);
    let handle = fixture
        .session
        .background_tasks()
        .register(TaskSpec::new(TaskKind::Shell, "cargo build"))
        .unwrap();
    let session = fixture.session.clone();
    let run = tokio::spawn(async move {
        session
            .prompt(vec![AgentMessage::user("wait for the build")])
            .await
    });
    eventually("the turn is waiting for the model", || {
        fixture.script.count() == 1
    })
    .await;
    fixture.session.abort();
    run.await.unwrap();
    assert!(fixture.session.background_tasks().has_running());
    assert!(!handle.is_stopped());

    assert!(handle.finish(TaskOutcome::completed("cargo build ended")));
    fixture.session.wait_for_background_work().await;
    assert_eq!(fixture.script.count(), 2, "the notice started a new turn");
}

#[tokio::test]
async fn an_entry_queued_during_an_interrupted_turn_waits_for_the_next_turn() {
    let fixture = fixture(vec![
        None,
        Some(text("answer to the next prompt")),
        Some(text("read the notice")),
    ]);
    let handle = fixture
        .session
        .background_tasks()
        .register(TaskSpec::new(TaskKind::Shell, "make"))
        .unwrap();
    let id = handle.id().to_string();
    let session = fixture.session.clone();
    let run = tokio::spawn(async move { session.prompt(vec![AgentMessage::user("go")]).await });
    eventually("the turn is waiting for the model", || {
        fixture.script.count() == 1
    })
    .await;
    // The task ends while the turn runs, then the user interrupts it.
    assert!(handle.finish(TaskOutcome::failed("make failed")));
    fixture.session.abort();
    run.await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(fixture.script.count(), 1, "no turn follows the interrupt");
    assert!(fixture.session.has_queued_entries());
    assert!(fixture.session.has_background_work());
    assert!(fixture.session.reclaim_queued().is_empty());

    // The next turn delivers it at its next step.
    fixture
        .session
        .prompt(vec![AgentMessage::user("next prompt")])
        .await;
    let requests = fixture.script.requests();
    assert_eq!(requests.len(), 3);
    assert!(is_notice_text(
        &user_text(requests[2].last().unwrap()).unwrap(),
        &id
    ));
    assert!(!fixture.session.has_queued_entries());
}

#[tokio::test]
async fn a_now_entry_goes_ahead_of_what_waits() {
    let fixture = fixture(Vec::new());
    // Busy, so nothing is delivered yet.
    *fixture.session_running() = true;
    fixture
        .session
        .queue_steering(AgentMessage::user("typed steer"));
    fixture
        .session
        .queue_entry(AgentMessage::user("next entry"), Priority::Next);
    fixture
        .session
        .queue_entry(AgentMessage::user("now entry"), Priority::Now);
    fixture
        .session
        .queue_entry(AgentMessage::user("later entry"), Priority::Later);
    let order: Vec<String> = fixture
        .session
        .take_queued_for_test()
        .into_iter()
        .map(|message| match message {
            AgentMessage::User(user) => user.content.as_text(),
            other => other.role().to_string(),
        })
        .collect();
    assert_eq!(
        order,
        ["now entry", "typed steer", "next entry", "later entry"]
    );
    *fixture.session_running() = false;
}

#[tokio::test]
async fn the_turn_starter_runs_the_turns_a_session_starts_itself() {
    let fixture = fixture(vec![Some(text("ok"))]);
    let started: Arc<Mutex<Vec<bool>>> = Arc::default();
    let seen = started.clone();
    fixture.session.set_turn_starter(Some(Arc::new(move |turn| {
        let seen = seen.clone();
        tokio::spawn(async move {
            let ran = turn.await;
            seen.lock().unwrap().push(ran);
        });
    })));
    fixture
        .session
        .queue_entry(AgentMessage::user("wake up"), Priority::Next);
    eventually("the starter ran the turn", || {
        started.lock().unwrap().as_slice() == [true]
    })
    .await;
    assert_eq!(fixture.script.count(), 1);
}

#[tokio::test]
async fn a_held_wake_up_keeps_the_session_busy_and_a_new_conversation_drops_its_tasks() {
    let fixture = fixture(Vec::new());
    let tasks = fixture.session.background_tasks();
    let hold = tasks.hold();
    assert!(fixture.session.has_background_work());
    drop(hold);
    assert!(!fixture.session.has_background_work());

    let handle = tasks
        .register(TaskSpec::new(TaskKind::Workflow, "audit"))
        .unwrap();
    assert!(handle.id().starts_with('w'));
    let output = tasks.output_file_for(handle.id());
    assert!(output.parent().unwrap().is_dir());
    assert!(output.ends_with(format!("{}.output", handle.id())));
    fixture
        .session
        .replace_manager(SessionManager::in_memory(std::path::Path::new("/other")));
    assert!(handle.is_stopped());
    assert!(tasks.list().is_empty());
}

#[tokio::test]
async fn the_tasks_of_a_session_that_goes_away_are_told_to_stop() {
    let Fixture {
        session,
        _directory: directory,
        ..
    } = fixture(Vec::new());
    let handle = session
        .background_tasks()
        .register(TaskSpec::new(TaskKind::Shell, "sleep 60"))
        .unwrap();
    session.set_stream_fn(None);
    drop(session);
    assert!(handle.is_stopped());
    // Its owner's finish has nobody to tell.
    assert!(!handle.finish(TaskOutcome::completed("too late")));
    drop(directory);
}

impl Fixture {
    fn session_running(&self) -> std::sync::MutexGuard<'_, bool> {
        self.session.running_flag_for_test()
    }
}
