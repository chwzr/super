//! A session's record of what was read, driven through whole agent runs
//! with a scripted provider: Read records, Edit and Write check, a child
//! agent reads for itself and a fork starts from its parent's reads.

use crate::child_turn::ForkTurns;
use crate::session::manager::SessionManager;
use crate::session_runner::AgentSession;
use crate::settings::{Mode, Settings};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use super_agent::{AgentMessage, DynTool, StreamFn};
use super_ai::{
    AssistantEvent, AssistantMessage, ContentBlock, EventStream, Model, Registry, StopReason,
    ThinkingLevel, ToolCall,
};

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

fn call(id: &str, name: &str, arguments: Value) -> AssistantMessage {
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

/// A provider that answers its requests with `replies`, in order.
fn scripted(replies: Vec<AssistantMessage>) -> StreamFn {
    let replies = Arc::new(Mutex::new(replies.into_iter()));
    Arc::new(move |_, _, _| {
        let (sink, stream) = EventStream::channel();
        let reply = replies.lock().unwrap().next().unwrap_or_else(|| {
            let mut message = AssistantMessage::empty("openai-responses", "scripted", "scripted");
            message.stop_reason = StopReason::Error;
            message.error_message = Some("script exhausted".into());
            message
        });
        sink.send(AssistantEvent::Start {
            partial: reply.clone(),
        });
        if reply.stop_reason == StopReason::Error {
            sink.error(reply);
        } else {
            sink.done(reply);
        }
        stream
    })
}

fn file_tools(cwd: &Path) -> Vec<DynTool> {
    // Each tool comes with a record of its own; the session binds all three
    // to its record.
    vec![
        Arc::new(super_agent::tools::read::ReadTool::new(cwd.to_path_buf())),
        Arc::new(super_agent::tools::edit::EditTool::new(cwd.to_path_buf())),
        Arc::new(super_agent::tools::write::WriteTool::new(cwd.to_path_buf())),
    ]
}

struct Fixture {
    session: Arc<AgentSession>,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new(mode: Mode) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let manager =
            SessionManager::create(directory.path(), Some(directory.path().join("sessions")))
                .unwrap();
        let settings = Settings {
            mode,
            retry: crate::settings::RetrySettings {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let session = AgentSession::new(
            manager,
            file_tools(directory.path()),
            Registry::load(None),
            settings,
            "test".into(),
            model(),
            ThinkingLevel::Off,
            None,
            Arc::new(|_| {}),
        );
        Self { session, directory }
    }

    fn file(&self, name: &str, content: &str) -> String {
        let path = self.directory.path().join(name);
        std::fs::write(&path, content).unwrap();
        path.display().to_string()
    }

    fn content(&self, name: &str) -> String {
        std::fs::read_to_string(self.directory.path().join(name)).unwrap()
    }
}

/// Run one prompt on `session` against `replies`.
async fn run(session: &Arc<AgentSession>, replies: Vec<AssistantMessage>) {
    session.set_stream_fn(Some(scripted(replies)));
    session.prompt(vec![AgentMessage::user("go")]).await;
}

/// Every tool result in the session's history: tool name, whether it failed,
/// and its text.
fn results(session: &AgentSession) -> Vec<(String, bool, String)> {
    session
        .manager
        .lock()
        .unwrap()
        .build_session_context()
        .messages
        .into_iter()
        .filter_map(|message| match message {
            AgentMessage::ToolResult(result) => Some((
                result.tool_name.clone(),
                result.is_error,
                result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            )),
            _ => None,
        })
        .collect()
}

fn details(session: &AgentSession) -> Vec<Value> {
    session
        .manager
        .lock()
        .unwrap()
        .build_session_context()
        .messages
        .into_iter()
        .filter_map(|message| match message {
            AgentMessage::ToolResult(result) => Some(result.details.unwrap_or(Value::Null)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn edit_and_write_need_a_read_in_both_modes() {
    for mode in [Mode::Basic, Mode::Advanced] {
        let fixture = Fixture::new(mode);
        let path = fixture.file("f.txt", "alpha\n");
        run(
            &fixture.session,
            vec![
                call(
                    "e1",
                    "Edit",
                    json!({"file_path": path, "old_string": "alpha", "new_string": "beta"}),
                ),
                call("w1", "Write", json!({"file_path": path, "content": "x"})),
                call("r1", "Read", json!({"file_path": path})),
                call(
                    "e2",
                    "Edit",
                    json!({"file_path": path, "old_string": "alpha", "new_string": "beta"}),
                ),
                // The session's own edit keeps the record whole and current.
                call(
                    "w2",
                    "Write",
                    json!({"file_path": path, "content": "gamma\n"}),
                ),
                text("done"),
            ],
        )
        .await;
        let results = results(&fixture.session);
        let outcome: Vec<(&str, bool)> = results
            .iter()
            .map(|(name, failed, _)| (name.as_str(), *failed))
            .collect();
        assert_eq!(
            outcome,
            [
                ("Edit", true),
                ("Write", true),
                ("Read", false),
                ("Edit", false),
                ("Write", false)
            ],
            "{mode:?}: {results:?}"
        );
        assert!(
            results[0].2.contains("has not been read yet"),
            "{results:?}"
        );
        assert!(
            results[1].2.contains("has not been read whole"),
            "{results:?}"
        );
        assert_eq!(results[2].2, "     1\talpha");
        assert_eq!(fixture.content("f.txt"), "gamma\n");
    }
}

#[tokio::test]
async fn a_child_reads_for_itself_and_a_fork_starts_from_its_parents_reads() {
    let fixture = Fixture::new(Mode::Advanced);
    let path = fixture.file("f.txt", "one\n");
    let parent = &fixture.session;
    run(
        parent,
        vec![call("r1", "Read", json!({"file_path": path})), text("read")],
    )
    .await;
    assert_eq!(parent.file_state().len(), 1);

    let child = parent.test_child("child", ForkTurns::None);
    assert!(child.file_state().is_empty());
    let edit = |id: &str, old: &str, new: &str| {
        call(
            id,
            "Edit",
            json!({"file_path": path, "old_string": old, "new_string": new}),
        )
    };
    run(&child, vec![edit("c1", "one", "two"), text("refused")]).await;
    assert!(results(&child)[0].1, "{:?}", results(&child));
    assert_eq!(fixture.content("f.txt"), "one\n");
    assert!(child.file_state().is_empty());

    let fork = parent.test_child("fork", ForkTurns::All);
    assert_eq!(fork.file_state().len(), 1);
    run(&fork, vec![edit("f1", "one", "two"), text("edited")]).await;
    let fork_results = results(&fork);
    assert!(!fork_results.last().unwrap().1, "{fork_results:?}");
    assert_eq!(fixture.content("f.txt"), "two\n");

    // The fork's edit brought only its own record up to date: to the parent
    // the file changed on disk since its read.
    run(parent, vec![edit("p1", "two", "three"), text("refused")]).await;
    let parent_results = results(parent);
    let (_, failed, message) = parent_results.last().unwrap();
    assert!(*failed && message.contains("changed on disk"), "{message}");
    assert_eq!(fixture.content("f.txt"), "two\n");
}

#[tokio::test]
async fn a_new_conversation_forgets_what_was_read_and_a_reload_keeps_it() {
    let fixture = Fixture::new(Mode::Advanced);
    let path = fixture.file("f.txt", "one\n");
    let session = &fixture.session;
    run(
        session,
        vec![call("r1", "Read", json!({"file_path": path})), text("read")],
    )
    .await;

    // New tools after a reload are bound to the same record.
    session.reload_runtime(
        session.settings(),
        "test".into(),
        file_tools(fixture.directory.path()),
    );
    assert_eq!(session.file_state().len(), 1);
    run(
        session,
        vec![
            call(
                "e1",
                "Edit",
                json!({"file_path": path, "old_string": "one", "new_string": "two"}),
            ),
            text("edited"),
        ],
    )
    .await;
    assert_eq!(fixture.content("f.txt"), "two\n");

    session.replace_manager(SessionManager::in_memory(fixture.directory.path()));
    assert!(session.file_state().is_empty());
}

#[tokio::test]
async fn a_side_question_reads_without_touching_the_sessions_record() {
    let fixture = Fixture::new(Mode::Advanced);
    let path = fixture.file("f.txt", "one\n");
    fixture.session.set_stream_fn(Some(scripted(vec![
        call("r1", "Read", json!({"file_path": path})),
        text("it says one"),
    ])));
    let answer = fixture
        .session
        .answer_btw(
            "what does f.txt say?",
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(answer.text, "it says one");
    assert!(fixture.session.file_state().is_empty());
}

#[tokio::test]
async fn allow_large_reads_into_the_free_room_of_the_sessions_context() {
    let fixture = Fixture::new(Mode::Advanced);
    // 1,500 lines of 100 bytes: about 40,000 tokens with the line numbers,
    // more than 25,000 but less than 70% of the scripted model's 100,000.
    let line = format!("{}\n", "c".repeat(99));
    let path = fixture.file("large.txt", &line.repeat(1500));
    run(
        &fixture.session,
        vec![
            call("r1", "Read", json!({"file_path": path})),
            call(
                "r2",
                "Read",
                json!({"file_path": path, "allow_large": true}),
            ),
            text("read"),
        ],
    )
    .await;
    let details = details(&fixture.session);
    assert_eq!(details[0]["truncatedBy"], "tokens", "{details:?}");
    assert_eq!(details[0]["wholeFile"], false);
    assert_eq!(details[1]["truncatedBy"], Value::Null, "{details:?}");
    assert_eq!(details[1]["wholeFile"], true);
    assert_eq!(details[1]["lines"]["last"], 1500);
}
