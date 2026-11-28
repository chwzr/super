//! A session's agents through the SDK dispatcher, which `--mode rpc`, the
//! daemon and the bindings all call: `get_agents` lists them, also after the
//! session was loaded again, `get_agent_entries` reads an agent's
//! transcript, and each loaded agent session is a view that sends the
//! agent's events and answers reading commands only.

#![cfg(all(feature = "mock", feature = "native"))]

use serde_json::{Value, json};
use std::sync::Arc;
use super_sdk::mock::{MockProvider, MockScript, MockTurn};
use super_sdk::protocol::Command;
use super_sdk::{
    AGENT_VIEW_REFUSAL, AgentChange, Session, SessionOptions, SessionSource, ThinkingLevel,
};

fn options(
    directory: &std::path::Path,
    provider: &MockProvider,
    source: SessionSource,
) -> SessionOptions {
    let mut settings = super_coding::Settings::default();
    settings.subagents.enabled = true;
    SessionOptions {
        cwd: directory.to_path_buf(),
        model: Some("mock/mock-1".into()),
        models_file: Some(provider.catalog_path()),
        thinking_level: Some(ThinkingLevel::Off),
        settings: Some(settings),
        // An allowlist names the agent tools too.
        tools: Some(
            [
                "Read",
                "Agent",
                "SendMessage",
                "ListAgents",
                "TaskStop",
                "Workflow",
                "Skill",
                "ScheduleWakeup",
                "EnterWorktree",
                "ExitWorktree",
            ]
            .map(str::to_string)
            .to_vec(),
        ),
        trust_project_files: false,
        no_context_files: true,
        session: source,
        session_dir: Some(directory.join("sessions")),
        ..Default::default()
    }
}

/// The text of every message entry, as `role: text`.
fn message_texts(entries: &Value) -> Vec<String> {
    entries
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["type"] == "message")
        .map(|entry| {
            let message = &entry["message"];
            let content = &message["content"];
            let text = content.as_str().map(str::to_string).unwrap_or_else(|| {
                content
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|block| block["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("")
            });
            format!("{}: {text}", message["role"].as_str().unwrap())
        })
        .collect()
}

async fn execute(session: &Arc<Session>, command: Command) -> Value {
    let response = session.execute(command).await;
    assert!(response.success, "{:?}", response.error);
    response.data.unwrap_or(Value::Null)
}

#[tokio::test]
async fn a_sessions_agents_are_listed_read_and_sent_like_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let provider = MockProvider::start(
        directory.path().join("provider"),
        MockScript::default()
            .then(vec![MockTurn::ToolCall {
                id: "call_1".into(),
                name: "Agent".into(),
                arguments: json!({"description": "Count", "prompt": "count the files", "run_in_background": false}),
            }])
            .then(vec![MockTurn::Text("three files".into())])
            .then(vec![MockTurn::Text("done".into())]),
    )
    .await
    .unwrap();
    let session = Session::create(options(directory.path(), &provider, SessionSource::Create))
        .await
        .unwrap();
    let (loaded, mut changes) = session.subscribe_agents();
    assert!(loaded.is_empty());
    session
        .prompt("count the files with an agent")
        .await
        .unwrap();

    // The agent's view came before its run, with its events from the start.
    let Ok(AgentChange::Loaded {
        id,
        session: view,
        mut events,
    }) = changes.try_recv()
    else {
        panic!("the agent's session was announced");
    };
    let mut kinds = Vec::new();
    while let Some(event) = events.try_recv() {
        kinds.push(event.event_type().to_string());
    }
    assert_eq!(
        kinds.first().map(String::as_str),
        Some("agent_start"),
        "{kinds:?}"
    );
    assert!(kinds.iter().any(|kind| kind == "message_end"), "{kinds:?}");
    assert_eq!(
        kinds.last().map(String::as_str),
        Some("agent_settled"),
        "{kinds:?}"
    );
    assert!(view.is_agent_view());
    assert!(!session.is_agent_view());
    assert_eq!(session.agent_sessions().len(), 1);
    assert_eq!(session.agent_sessions()[0].0, id);

    // get_agents lists it; its file is in the main session's folder.
    let main_file = session.state().session_file.unwrap();
    let listed = execute(&session, Command::GetAgents { agent: None }).await;
    let agent = &listed["agents"][0];
    assert_eq!(agent["id"], json!(id));
    assert_eq!(agent["path"], json!(id));
    assert_eq!(agent["name"], "Count");
    assert_eq!(agent["type"], "general-purpose");
    assert_eq!(agent["status"], "completed");
    assert_eq!(agent["background"], false);
    assert_eq!(
        agent["sessionFile"],
        json!(
            main_file
                .with_extension("")
                .join("agents")
                .join(format!("{id}.jsonl"))
        )
    );
    // Its own agents: none.
    let inner = execute(
        &session,
        Command::GetAgents {
            agent: Some(id.clone()),
        },
    )
    .await;
    assert_eq!(inner["agents"], json!([]));

    // get_agent_entries reads its transcript.
    let read = execute(
        &session,
        Command::GetAgentEntries {
            agent: id.clone(),
            since: None,
        },
    )
    .await;
    assert_eq!(
        message_texts(&read["entries"]),
        ["user: count the files", "assistant: three files"]
    );
    assert_eq!(read["sessionId"], json!(view.state().session_id));
    let first = read["entries"][0]["id"].as_str().unwrap().to_string();
    let later = execute(
        &session,
        Command::GetAgentEntries {
            agent: id.clone(),
            since: Some(first),
        },
    )
    .await;
    assert_eq!(
        later["entries"].as_array().unwrap().len(),
        read["entries"].as_array().unwrap().len() - 1
    );
    let unknown = session
        .execute(Command::GetAgentEntries {
            agent: "nobody".into(),
            since: None,
        })
        .await;
    assert!(!unknown.success);
    assert_eq!(
        unknown.error.as_deref(),
        Some("No agent of this session has the id or name 'nobody'; ListAgents lists them.")
    );

    // The view answers reading commands and refuses the others.
    let state = execute(&view, Command::GetState {}).await;
    assert_eq!(state["sessionId"], read["sessionId"]);
    let own = execute(&view, Command::GetEntries { since: None }).await;
    assert_eq!(own["entries"], read["entries"]);
    let refused = view
        .execute(Command::Prompt {
            message: "do more".into(),
            images: Vec::new(),
            streaming_behavior: None,
        })
        .await;
    assert!(!refused.success);
    assert_eq!(refused.error.as_deref(), Some(AGENT_VIEW_REFUSAL));
    assert!(!view.execute(Command::Abort {}).await.success);

    // A session loaded again lists the stored agent, and reads it.
    session.close();
    drop((session, view));
    let again = Session::create(options(
        directory.path(),
        &provider,
        SessionSource::Open(main_file.clone()),
    ))
    .await
    .unwrap();
    let listed = execute(&again, Command::GetAgents { agent: None }).await;
    assert_eq!(listed["agents"][0]["id"], json!(id));
    assert_eq!(listed["agents"][0]["status"], "completed");
    assert!(again.agent_sessions().is_empty());
    let reread = execute(
        &again,
        Command::GetAgentEntries {
            agent: id.clone(),
            since: None,
        },
    )
    .await;
    assert_eq!(reread["entries"], read["entries"]);
    assert_eq!(again.agent_sessions().len(), 1, "reading it loaded it");
    // Only the main session is a stored session of its own.
    let stored =
        super_coding::SessionManager::list_all(&directory.path().join("sessions")).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].path, main_file);
}
