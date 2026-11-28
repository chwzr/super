//! The question before a workflow run, as `--mode rpc` and the daemon ask
//! it: it is withdrawn when the turn is stopped or the switches are turned
//! off while it waits, and no answer can start the run after that.

#![cfg(all(feature = "mock", feature = "native"))]

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_ai::{AssistantEvent, AssistantMessage, ContentBlock, EventStream, StopReason, ToolCall};
use super_sdk::mock::{MockProvider, MockScript};
use super_sdk::protocol::Command;
use super_sdk::{PromptArgs, Session, SessionOptions, StreamFn, ThinkingLevel};

const SCRIPT: &str = "export const meta = { name: 'asked-wf', description: 'Asked' }\nreturn 1\n";

/// Request 0 calls `Workflow`; every later request is answered with text.
fn scripted() -> StreamFn {
    let count = Arc::new(Mutex::new(0usize));
    Arc::new(move |_, _, _| {
        let step = {
            let mut count = count.lock().unwrap();
            *count += 1;
            *count - 1
        };
        let (sink, stream) = EventStream::channel();
        let mut message = AssistantMessage::empty("openai-completions", "mock", "mock-1");
        if step == 0 {
            message.content.push(ContentBlock::ToolCall(ToolCall {
                id: "w1".into(),
                name: "Workflow".into(),
                arguments: json!({"script": SCRIPT}),
                thought_signature: None,
            }));
            message.stop_reason = StopReason::ToolUse;
        } else {
            message.content.push(ContentBlock::text("done"));
            message.stop_reason = StopReason::Stop;
        }
        sink.send(AssistantEvent::Start {
            partial: message.clone(),
        });
        sink.done(message);
        stream
    })
}

async fn session(directory: &std::path::Path) -> (MockProvider, Arc<Session>) {
    let provider = MockProvider::start(directory, MockScript::text("unused"))
        .await
        .expect("mock provider starts");
    let mut settings = super_coding::Settings::default();
    settings.subagents.enabled = true;
    settings.workflows.enabled = true;
    settings.workflows.confirm = true;
    let session = Session::create(SessionOptions {
        cwd: directory.to_path_buf(),
        model: Some("mock/mock-1".into()),
        models_file: Some(provider.catalog_path()),
        thinking_level: Some(ThinkingLevel::Off),
        settings: Some(settings),
        tools: Some(vec!["Read".into(), "Workflow".into()]),
        no_context_files: true,
        stream_fn: Some(scripted()),
        ..Default::default()
    })
    .await
    .expect("session builds");
    super_sdk::install_workflow_approval_gate(&session);
    (provider, session)
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
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting until {what}");
}

/// Prompt, and wait until the question before the run is open; its id.
async fn asked(session: &Arc<Session>, events: &Arc<Mutex<Vec<Value>>>) -> String {
    session
        .prompt_detached(PromptArgs::new("run it"))
        .expect("prompt accepted");
    eventually("the question comes", || {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "approval_request")
    })
    .await;
    let events = events.lock().unwrap();
    let request = events
        .iter()
        .find(|event| event["type"] == "approval_request")
        .unwrap();
    assert_eq!(request["kind"], "workflow");
    request["approvalId"].as_str().unwrap().to_string()
}

/// After the question was withdrawn: nobody can answer it, it is listed no
/// more, every surface was told, and no run started.
async fn withdrawn(session: &Arc<Session>, events: &Arc<Mutex<Vec<Value>>>, id: &str) {
    let resolved = events
        .lock()
        .unwrap()
        .iter()
        .find(|event| event["type"] == "approval_resolved")
        .cloned()
        .expect("approval_resolved");
    assert_eq!(resolved["approvalId"], id);
    assert_eq!(resolved["choice"], "cancel");
    let open = session.execute(Command::GetApprovals {}).await;
    assert_eq!(open.data.unwrap()["approvals"], json!([]));
    let late = session
        .execute(Command::AnswerApproval {
            approval_id: id.to_string(),
            choice: "approve".into(),
        })
        .await;
    assert!(!late.success, "{late:?}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(session.tasks().is_empty(), "{:?}", session.tasks());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abort_while_the_question_waits_stops_the_turn_and_withdraws_it() {
    let directory = tempfile::tempdir().unwrap();
    let (_provider, session) = session(directory.path()).await;
    let events = collect_events(&session);
    let id = asked(&session, &events).await;

    let response = tokio::time::timeout(Duration::from_secs(5), session.execute(Command::Abort {}))
        .await
        .expect("abort answers while the question waits");
    assert!(response.success, "{response:?}");
    assert!(!session.state().is_streaming);
    withdrawn(&session, &events, &id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_workflows_off_while_the_question_waits_withdraws_it() {
    let directory = tempfile::tempdir().unwrap();
    let (_provider, session) = session(directory.path()).await;
    let events = collect_events(&session);
    let id = asked(&session, &events).await;

    let response = session
        .execute(Command::SetMode {
            mode: None,
            subagents: None,
            workflows: Some(false),
        })
        .await;
    assert!(response.success, "{response:?}");
    eventually("the turn ends", || !session.state().is_streaming).await;
    withdrawn(&session, &events, &id).await;
    let result = events
        .lock()
        .unwrap()
        .iter()
        .find(|event| event["type"] == "tool_execution_end")
        .cloned()
        .expect("the Workflow call ends");
    assert_eq!(result["isError"], true);
    assert!(
        result["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("so nothing ran"),
        "{result}"
    );
}
