//! Bash in the background, driven through sessions with the session's own
//! queue and task registry.

use crate::background_tests::{model, scripted, text, tool_call, user_text};
use crate::session::manager::SessionManager;
use crate::session_runner::AgentSession;
use crate::settings::Settings;
use crate::subagents::ForkTurns;
use crate::tasks::{TaskKind, TaskStatus};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use super_agent::tools::bash::BashTool;
use super_agent::{AgentMessage, DynTool};
use super_ai::{Registry, ThinkingLevel};

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

#[tokio::test]
async fn only_the_main_session_keeps_a_cd() {
    let directory = tempfile::tempdir().unwrap();
    let session = session_in(directory.path(), test_settings());
    assert!(session.tool_session().shell.is_some());
    assert!(session.tool_session().tasks.is_some());
    for fork in [ForkTurns::None, ForkTurns::All] {
        let child = session
            .create_subagent_session("child", "/root/child", fork, None, None)
            .unwrap();
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
