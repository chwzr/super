//! `Agent`, `SendMessage` and `ListAgents`, driven through sessions with a
//! scripted provider. Parent and agents run at the same time, so the
//! provider answers each request from the lane of the conversation it
//! belongs to: the first user text of the conversation, or `fork` for a
//! fork.

use crate::agents::registry::{AgentRegistry, AgentRequest, MAX_DEPTH, MAX_RUNNING};
use crate::agents::tools::{PAGE_SIZE, agent_tools};
use crate::agents::types::{AgentTypeSource, parse_definition};
use crate::agents::{AgentResources, AgentType};
use crate::background_tests::{eventually, model, text, tool_call};
use crate::notifications::{AGENT_MESSAGE, sent_message};
use crate::session::manager::SessionManager;
use crate::session_runner::{AgentChild, AgentSession};
use crate::settings::{Mode, Settings};
use crate::tasks::{TaskKind, TaskStatus};
use crate::tools::task_stop::TaskStopTool;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use super_agent::placeholder::prompt_todo;
use super_agent::{AgentMessage, AgentTool, DynTool, StreamFn, ToolResult, ToolUpdateSink};
use super_ai::{
    AssistantEvent, AssistantMessage, Context, EventStream, Message, Model, Registry, StopReason,
    ThinkingLevel,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// One request the provider saw.
#[derive(Clone)]
struct Request {
    lane: String,
    model: String,
    context: Context,
}

#[derive(Default)]
struct Seen {
    requests: Mutex<Vec<Request>>,
}

impl Seen {
    fn of(&self, lane: &str) -> Vec<Request> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.lane == lane)
            .cloned()
            .collect()
    }

    fn count(&self, lane: &str) -> usize {
        self.of(lane).len()
    }
}

fn user_text(message: &Message) -> Option<String> {
    match message {
        Message::User(user) => Some(user.content.as_text()),
        _ => None,
    }
}

/// The lane of a request: `fork` when the conversation has a fork's first
/// message, else its first user text.
fn lane_of(context: &Context) -> String {
    let fork = prompt_todo("AgentType.fork.instructions");
    let users: Vec<String> = context.messages.iter().filter_map(user_text).collect();
    if users.iter().any(|text| text.starts_with(&fork)) {
        return "fork".into();
    }
    users.first().cloned().unwrap_or_default()
}

/// A provider that answers each lane's requests with its replies in order;
/// `None` holds the request until it is cancelled.
fn lanes(script: Vec<(&str, Vec<Option<AssistantMessage>>)>) -> (StreamFn, Arc<Seen>) {
    let replies: Arc<Mutex<HashMap<String, VecDeque<Option<AssistantMessage>>>>> =
        Arc::new(Mutex::new(
            script
                .into_iter()
                .map(|(lane, replies)| (lane.to_string(), replies.into()))
                .collect(),
        ));
    let seen = Arc::new(Seen::default());
    let record = seen.clone();
    let stream: StreamFn = Arc::new(move |model: &Model, context, options| {
        let lane = lane_of(context);
        record.requests.lock().unwrap().push(Request {
            lane: lane.clone(),
            model: model.id.clone(),
            context: context.clone(),
        });
        let reply = replies
            .lock()
            .unwrap()
            .get_mut(&lane)
            .and_then(VecDeque::pop_front);
        let (sink, stream) = EventStream::channel();
        match reply {
            Some(Some(message)) => {
                sink.send(AssistantEvent::Start {
                    partial: message.clone(),
                });
                sink.done(message);
            }
            Some(None) => {
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
                message.error_message = Some(format!("script exhausted for lane {lane:?}"));
                sink.send(AssistantEvent::Start {
                    partial: message.clone(),
                });
                sink.error(message);
            }
        }
        stream
    });
    (stream, seen)
}

/// `Gate` waits until the test opens it and says it was entered; `Probe`
/// answers at once.
struct TestTool {
    name: &'static str,
    gate: Arc<Notify>,
    entered: Arc<AtomicBool>,
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
        _tool_call_id: &str,
        _args: Value,
        _cancel: CancellationToken,
        _on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        if self.name == "Gate" {
            self.entered.store(true, Ordering::SeqCst);
            self.gate.notified().await;
        }
        Ok(ToolResult::text(format!("{} done", self.name)))
    }
}

struct Fixture {
    session: Arc<AgentSession>,
    seen: Arc<Seen>,
    gate: Arc<Notify>,
    entered: Arc<AtomicBool>,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn registry(&self) -> Arc<AgentRegistry> {
        self.session
            .agent_registry()
            .expect("the main session has agents")
    }

    /// The session's agent tool `name`, bound to it.
    fn tool(&self, name: &str) -> DynTool {
        agent_tools(Arc::downgrade(&self.session))
            .into_iter()
            .find(|tool| tool.name() == name)
            .unwrap()
    }

    /// Queued entries start no turn: the test reads them.
    fn quiet(self) -> Self {
        self.session.set_turn_starter(Some(Arc::new(|_turn| {})));
        self
    }
}

fn settings() -> Settings {
    let mut settings = Settings {
        retry: crate::settings::RetrySettings {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    settings.subagents.enabled = true;
    settings
}

fn fixture_with(
    tools: impl FnOnce(&Path) -> Vec<DynTool>,
    script: Vec<(&str, Vec<Option<AssistantMessage>>)>,
) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let manager =
        SessionManager::create(directory.path(), Some(directory.path().join("sessions"))).unwrap();
    let gate = Arc::new(Notify::new());
    let entered = Arc::new(AtomicBool::new(false));
    let mut base = tools(directory.path());
    for name in ["Gate", "Probe"] {
        base.push(Arc::new(TestTool {
            name,
            gate: gate.clone(),
            entered: entered.clone(),
        }));
    }
    let session = AgentSession::new(
        manager,
        base,
        Registry::from_builtin(),
        settings(),
        "main prompt".into(),
        model(),
        ThinkingLevel::Off,
        None,
        Arc::new(|_| {}),
    );
    let (stream, seen) = lanes(script);
    session.set_stream_fn(Some(stream));
    Fixture {
        session,
        seen,
        gate,
        entered,
        directory,
    }
}

fn fixture(script: Vec<(&str, Vec<Option<AssistantMessage>>)>) -> Fixture {
    fixture_with(|_| Vec::new(), script)
}

fn request(description: &str, prompt: &str, background: bool) -> AgentRequest {
    AgentRequest {
        description: description.into(),
        prompt: prompt.into(),
        background,
        ..Default::default()
    }
}

fn last_tool_result(context: &Context) -> super_ai::ToolResultMessage {
    context
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::ToolResult(result) => Some(result.clone()),
            _ => None,
        })
        .expect("a tool result")
}

fn texts(result: &super_ai::ToolResultMessage) -> Vec<String> {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            super_ai::ContentBlock::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn notice_header() -> String {
    prompt_todo("Notification.header")
}

/// Every queued entry of a quiet session so far.
fn queued(session: &AgentSession, seen: &Mutex<Vec<AgentMessage>>) -> Vec<AgentMessage> {
    let mut seen = seen.lock().unwrap();
    seen.extend(session.take_queued_for_test());
    seen.clone()
}

fn notice_text(message: &AgentMessage) -> Option<String> {
    match message {
        AgentMessage::Custom(custom)
            if custom.custom_type == crate::notifications::TASK_NOTIFICATION =>
        {
            Some(custom.content.as_text())
        }
        _ => None,
    }
}

#[tokio::test]
async fn a_background_agent_is_announced_with_its_report_and_wakes_the_idle_parent() {
    let fixture = fixture(vec![
        (
            "main",
            vec![
                Some(tool_call(
                    "call_1",
                    "Agent",
                    json!({"description": "Review the diff", "prompt": "child task"}),
                )),
                Some(text("started it")),
                Some(text("got the report")),
            ],
        ),
        (
            "child task",
            vec![
                Some(tool_call("g1", "Gate", json!({}))),
                Some(text("the report")),
            ],
        ),
    ]);
    let session = fixture.session.clone();
    session.prompt(vec![AgentMessage::user("main")]).await;
    // The call returned at once and the turn ended while the agent works.
    assert_eq!(fixture.seen.count("main"), 2);
    let result = last_tool_result(&fixture.seen.of("main")[1].context);
    assert!(!result.is_error);
    let agents = fixture.registry().list();
    assert_eq!(agents.len(), 1);
    let agent = agents[0].clone();
    assert!(agent.id.starts_with('a') && agent.id.len() == 9);
    assert_eq!(agent.name, "Review the diff");
    assert_eq!(agent.agent_type, "general-purpose");
    assert!(agent.background);
    let transcript = agent.session_file.clone().expect("a stored transcript");
    assert_eq!(
        texts(&result),
        [format!(
            "Agent {} runs in the background. Its transcript is written to {}.",
            agent.id,
            transcript.display()
        )]
    );
    // Its run is the background task with the agent's id.
    let task = session.background_tasks().find(&agent.id).unwrap();
    assert_eq!(task.kind, TaskKind::Agent);
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.tool_use_id.as_deref(), Some("call_1"));
    assert_eq!(task.output_file.as_deref(), Some(transcript.as_path()));

    fixture.gate.notify_one();
    session.wait_for_background_work().await;
    let main = fixture.seen.of("main");
    assert_eq!(main.len(), 3, "the notice started a turn");
    let notice = user_text(main[2].context.messages.last().unwrap()).unwrap();
    assert!(notice.starts_with(&notice_header()));
    assert!(notice.contains(&format!("<task-id>{}</task-id>", agent.id)));
    assert!(notice.contains("<tool-use-id>call_1</tool-use-id>"));
    assert!(notice.contains(&format!(
        "<output-file>{}</output-file>",
        transcript.display()
    )));
    assert!(notice.contains("<status>completed</status>"));
    assert!(notice.contains("<summary>Review the diff (completed)</summary>"));
    assert!(notice.contains("<result>the report</result>"));
    assert!(notice.contains("<usage><subagent_tokens>"));
    assert!(notice.contains("<tool_uses>1</tool_uses>"));
    assert_eq!(fixture.registry().list()[0].status, TaskStatus::Completed);
    assert_eq!(
        session.background_tasks().find(&agent.id).unwrap().status,
        TaskStatus::Completed
    );

    // The agent started from the instructions of its type and the context
    // of the parent's prompt, not from the parent's prompt.
    let child = &fixture.seen.of("child task")[0];
    // It may start agents itself, so the agent sections follow.
    let system = child.context.system_prompt.clone().unwrap();
    assert!(
        system.starts_with(&format!(
            "{}\nCurrent working directory: {}\n\n{}\n\n{}",
            prompt_todo("AgentType.general-purpose.instructions"),
            fixture.directory.path().display(),
            prompt_todo("Agent.system_section"),
            prompt_todo("Agent.type_list"),
        )),
        "{system}"
    );
    assert!(!system.contains("main prompt"));
    assert!(child.context.messages.len() == 1);
}

#[tokio::test]
async fn an_agent_the_parent_waits_for_returns_its_report_as_the_result() {
    let fixture = fixture(vec![
        (
            "main",
            vec![
                Some(tool_call(
                    "call_1",
                    "Agent",
                    json!({"description": "Count", "prompt": "child task", "run_in_background": false}),
                )),
                Some(text("done")),
            ],
        ),
        (
            "child task",
            vec![
                Some(tool_call("p1", "Probe", json!({}))),
                Some(text("the report")),
            ],
        ),
    ]);
    let session = fixture.session.clone();
    session.prompt(vec![AgentMessage::user("main")]).await;
    let main = fixture.seen.of("main");
    assert_eq!(main.len(), 2);
    let result = last_tool_result(&main[1].context);
    assert!(!result.is_error);
    let agent = fixture.registry().list().pop().unwrap();
    let texts = texts(&result);
    assert_eq!(texts[0], "the report");
    assert!(texts[1].starts_with(&format!(
        "<agent-result>\n<agent-id>{}</agent-id>\n<usage><subagent_tokens>",
        agent.id
    )));
    assert!(texts[1].contains("<tool_uses>1</tool_uses>"));
    assert_eq!(agent.status, TaskStatus::Completed);
    assert!(!agent.background);
    // A waited run is no background task, and nothing is announced.
    assert!(session.tasks().is_empty());
    assert!(!session.has_queued_entries());
}

#[tokio::test]
async fn an_agent_can_start_an_agent_of_its_own() {
    let fixture = fixture(vec![
        (
            "main",
            vec![
                Some(tool_call(
                    "call_1",
                    "Agent",
                    json!({"description": "Outer", "prompt": "child task", "run_in_background": false}),
                )),
                Some(text("done")),
            ],
        ),
        (
            "child task",
            vec![
                Some(tool_call(
                    "inner",
                    "Agent",
                    json!({"description": "Inner", "prompt": "grandchild task", "run_in_background": false}),
                )),
                Some(text("outer report")),
            ],
        ),
        ("grandchild task", vec![Some(text("inner report"))]),
    ]);
    fixture
        .session
        .prompt(vec![AgentMessage::user("main")])
        .await;
    let child = fixture.seen.of("child task");
    assert_eq!(
        texts(&last_tool_result(&child[1].context))[0],
        "inner report"
    );
    let main = fixture.seen.of("main");
    assert_eq!(
        texts(&last_tool_result(&main[1].context))[0],
        "outer report"
    );
    let outer = fixture.registry().list().pop().unwrap();
    let inner = fixture
        .registry()
        .session_of(&outer.id)
        .unwrap()
        .agent_registry()
        .unwrap()
        .list();
    assert_eq!(inner.len(), 1);
    assert_eq!(inner[0].name, "Inner");
    assert_eq!(inner[0].status, TaskStatus::Completed);
}

fn core_tools(directory: &Path) -> Vec<DynTool> {
    let cwd = directory.to_path_buf();
    vec![
        Arc::new(super_agent::tools::read::ReadTool::new(cwd.clone())),
        Arc::new(super_agent::tools::write::WriteTool::new(cwd.clone())),
        Arc::new(super_agent::tools::edit::EditTool::new(cwd.clone())),
        Arc::new(super_agent::tools::bash::BashTool::new(cwd)),
    ]
}

fn child_of(parent: &Arc<AgentSession>, agent_type: &AgentType, depth: usize) -> Arc<AgentSession> {
    parent
        .create_agent_session(AgentChild {
            id: &parent.background_tasks().new_id(TaskKind::Agent),
            agent_type,
            description: "test",
            model: parent.model(),
            thinking: parent.thinking_level(),
            depth,
            tool_use_id: "call",
        })
        .unwrap()
}

fn type_named(session: &AgentSession, name: &str) -> AgentType {
    session
        .agent_types()
        .into_iter()
        .find(|known| known.name == name)
        .unwrap()
}

#[tokio::test]
async fn each_agent_type_gets_its_tools() {
    let fixture = fixture_with(core_tools, Vec::new());
    let session = fixture.session.clone();
    assert_eq!(
        session.available_tool_names(),
        [
            "Read",
            "Write",
            "Edit",
            "Bash",
            "Gate",
            "Probe",
            "Agent",
            "SendMessage",
            "ListAgents",
            "Monitor",
            "TaskStop"
        ]
    );
    let names = |agent_type: &str| {
        child_of(&session, &type_named(&session, agent_type), 1).available_tool_names()
    };
    assert_eq!(
        names("general-purpose"),
        [
            "Read",
            "Write",
            "Edit",
            "Bash",
            "Gate",
            "Probe",
            "Agent",
            "SendMessage",
            "ListAgents",
            "Monitor",
            "TaskStop"
        ]
    );
    for read_only in ["Explore", "Plan"] {
        assert_eq!(
            names(read_only),
            [
                "Read",
                "Bash",
                "Gate",
                "Probe",
                "SendMessage",
                "Monitor",
                "TaskStop"
            ],
            "{read_only}"
        );
    }
    assert_eq!(
        names("fork"),
        [
            "Read",
            "Write",
            "Edit",
            "Bash",
            "Gate",
            "Probe",
            "SendMessage",
            "Monitor",
            "TaskStop"
        ]
    );
    let definition = |text: &str| {
        parse_definition(
            Path::new("/u/custom.md"),
            text,
            AgentTypeSource::User("/u/custom.md".into()),
        )
        .unwrap()
    };
    let reader = definition("---\nname: reader\ntools: Read\n---\nRead.");
    assert_eq!(
        child_of(&session, &reader, 1).available_tool_names(),
        ["Read", "SendMessage"]
    );
    let delegator = definition("---\nname: delegator\ntools: [Read, Agent]\n---\nDelegate.");
    assert_eq!(
        child_of(&session, &delegator, 1).available_tool_names(),
        ["Read", "Agent", "SendMessage", "ListAgents", "TaskStop"]
    );

    // The instructions of a type start its agent's system prompt; a fork
    // keeps the parent's.
    let explore = child_of(&session, &type_named(&session, "Explore"), 1);
    assert!(
        explore
            .system_prompt()
            .starts_with(&prompt_todo("AgentType.Explore.instructions"))
    );
    let fork = child_of(&session, &type_named(&session, "fork"), 1);
    assert_eq!(fork.system_prompt(), "main prompt");

    // The basic mode and the Subagents switch take the agent tools away.
    let mut basic = session.settings();
    basic.mode = Mode::Basic;
    session.update_settings(basic);
    assert_eq!(
        session.available_tool_names(),
        ["Read", "Write", "Edit", "Bash", "Gate", "Probe"]
    );
    let mut off = settings();
    off.subagents.enabled = false;
    session.update_settings(off);
    assert_eq!(
        session.available_tool_names(),
        [
            "Read", "Write", "Edit", "Bash", "Gate", "Probe", "Monitor", "TaskStop"
        ]
    );
}

#[tokio::test]
async fn a_fork_starts_with_an_exact_copy_of_the_conversation_on_the_parent_model() {
    let fixture = fixture(vec![
        (
            "main",
            vec![
                Some(tool_call("p1", "Probe", json!({}))),
                Some(tool_call(
                    "call_fork",
                    "Agent",
                    json!({"description": "Try it", "prompt": "fork task", "subagent_type": "fork", "run_in_background": false}),
                )),
                Some(text("done")),
            ],
        ),
        ("fork", vec![Some(text("fork report"))]),
    ]);
    let session = fixture.session.clone();
    session.prompt(vec![AgentMessage::user("main")]).await;
    let fork = &fixture.seen.of("fork")[0];
    assert_eq!(fork.model, "scripted");
    assert_eq!(fork.context.system_prompt.as_deref(), Some("main prompt"));
    let messages = &fork.context.messages;
    assert_eq!(messages.len(), 6, "{messages:#?}");
    assert_eq!(user_text(&messages[0]).as_deref(), Some("main"));
    assert!(
        matches!(&messages[1], Message::Assistant(assistant) if assistant.tool_calls().any(|call| call.id == "p1"))
    );
    assert!(
        matches!(&messages[2], Message::ToolResult(result) if result.tool_call_id == "p1" && texts(result) == ["Probe done"])
    );
    assert!(
        matches!(&messages[3], Message::Assistant(assistant) if assistant.tool_calls().any(|call| call.id == "call_fork"))
    );
    // The call that started the fork has no result yet: a stand-in.
    assert!(
        matches!(&messages[4], Message::ToolResult(result) if result.tool_call_id == "call_fork"
            && texts(result) == [prompt_todo("AgentType.fork.pending_result")])
    );
    assert_eq!(
        user_text(&messages[5]).unwrap(),
        format!("{}\nfork task", prompt_todo("AgentType.fork.instructions"))
    );
    let main = fixture.seen.of("main");
    assert_eq!(texts(&last_tool_result(&main[2].context))[0], "fork report");
    assert_eq!(fixture.registry().list()[0].agent_type, "fork");

    // A fork runs on the parent's model and effort, whatever the call says.
    let refused = fixture
        .registry()
        .start(
            AgentRequest {
                agent_type: Some("fork".into()),
                model: Some("scripted".into()),
                ..request("Try", "fork task", false)
            },
            "call",
            CancellationToken::new(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        refused.to_string(),
        "A fork runs on the parent's model and effort; leave out model and effort."
    );
}

/// A built-in model that thinks at some levels and not at `max`.
fn thinking_model() -> Model {
    Registry::from_builtin()
        .all()
        .iter()
        .find(|known| {
            let levels = known.supported_thinking_levels();
            known.reasoning && levels.len() >= 3 && !levels.contains(&ThinkingLevel::Max)
        })
        .expect("a built-in reasoning model")
        .clone()
}

#[tokio::test]
async fn definition_files_add_types_with_their_instructions_model_and_effort() {
    let thinking = thinking_model();
    let fixture = fixture(vec![
        (
            "main",
            vec![
                Some(tool_call(
                    "call_1",
                    "Agent",
                    json!({"description": "Review", "prompt": "review task", "subagent_type": "reviewer", "run_in_background": false}),
                )),
                Some(text("done")),
            ],
        ),
        ("review task", vec![Some(text("looks fine"))]),
    ]);
    let folder = fixture.directory.path().join("agents");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("reviewer.md"),
        format!(
            "---\nname: reviewer\ndescription: Reviews a diff\nmodel: {}/{}\neffort: low\ntools: Probe\n---\nLook at the diff.\n",
            thinking.provider, thinking.id
        ),
    )
    .unwrap();
    let session = fixture.session.clone();
    session.set_agent_resources(AgentResources {
        definitions: crate::agents::types::discover_in(Some(&folder), None),
        prompt_context: Some("\n\nThe context.".into()),
    });
    let list = session.agent_type_list();
    let names: Vec<&str> = list.iter().map(|summary| summary.name.as_str()).collect();
    assert_eq!(
        names,
        ["general-purpose", "Explore", "Plan", "fork", "reviewer"]
    );
    assert_eq!(list[4].description, "Reviews a diff");

    session.prompt(vec![AgentMessage::user("main")]).await;
    // The model is told of the types in the slot of the type list.
    let main = fixture.seen.of("main");
    let system = main[0].context.system_prompt.clone().unwrap();
    assert!(system.contains(&crate::agents::type_list_section(&list)));
    assert!(system.contains(&prompt_todo("Agent.system_section")));

    let review = &fixture.seen.of("review task")[0];
    assert_eq!(review.model, thinking.id);
    assert_eq!(
        review.context.system_prompt.as_deref(),
        Some("Look at the diff.\n\nThe context.")
    );
    let tools: Vec<&str> = review
        .context
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(tools, ["Probe", "SendMessage"]);
    let agent = fixture.registry().list().pop().unwrap();
    assert_eq!(agent.agent_type, "reviewer");
    let child = fixture.registry().session_of(&agent.id).unwrap();
    assert_eq!(child.thinking_level(), ThinkingLevel::Low);
}

async fn refusal(fixture: &Fixture, request: AgentRequest) -> String {
    fixture
        .registry()
        .start(request, "call", CancellationToken::new())
        .await
        .err()
        .expect("the call fails")
        .to_string()
}

#[tokio::test]
async fn a_model_or_effort_that_does_not_fit_fails_with_what_would() {
    let fixture = fixture(Vec::new());
    assert_eq!(
        refusal(
            &fixture,
            AgentRequest {
                model: Some("no-such-model-xyz".into()),
                ..request("Try", "task", false)
            }
        )
        .await,
        "No model of the registry matches 'no-such-model-xyz'; give one it has, such as scripted/scripted."
    );
    assert_eq!(
        refusal(
            &fixture,
            AgentRequest {
                effort: Some("turbo".into()),
                ..request("Try", "task", false)
            }
        )
        .await,
        "effort 'turbo' is not a thinking level; give one of off, minimal, low, medium, high, xhigh or max."
    );
    // The parent's model does not think.
    assert_eq!(
        refusal(
            &fixture,
            AgentRequest {
                effort: Some("high".into()),
                ..request("Try", "task", false)
            }
        )
        .await,
        "scripted/scripted does not accept the effort 'high'; it accepts off."
    );
    let thinking = thinking_model();
    let accepted: Vec<&str> = thinking
        .supported_thinking_levels()
        .iter()
        .map(ThinkingLevel::as_str)
        .collect();
    assert_eq!(
        refusal(
            &fixture,
            AgentRequest {
                model: Some(format!("{}/{}", thinking.provider, thinking.id)),
                effort: Some("max".into()),
                ..request("Try", "task", false)
            }
        )
        .await,
        format!(
            "{}/{} does not accept the effort 'max'; it accepts {}.",
            thinking.provider,
            thinking.id,
            accepted.join(", ")
        )
    );
    assert_eq!(
        refusal(
            &fixture,
            AgentRequest {
                agent_type: Some("nobody".into()),
                ..request("Try", "task", false)
            }
        )
        .await,
        "No agent type is named 'nobody'; the types are general-purpose, Explore, Plan, fork."
    );
    assert_eq!(
        refusal(
            &fixture,
            AgentRequest {
                isolation: Some(crate::agents::Isolation::Remote),
                ..request("Try", "task", false)
            }
        )
        .await,
        "isolation 'remote' is not available: agents run on this machine only. Leave isolation out."
    );
    assert_eq!(
        refusal(
            &fixture,
            AgentRequest {
                isolation: Some(crate::agents::Isolation::Worktree),
                ..request("Try", "task", false)
            }
        )
        .await,
        "isolation 'worktree' is not available yet; leave isolation out to run the agent in this folder."
    );
    // No refused call left an agent behind.
    assert!(fixture.registry().list().is_empty());
    // A type's name in another case is found.
    let explore = fixture
        .registry()
        .start(
            AgentRequest {
                agent_type: Some("explore".into()),
                ..request("Look", "task", true)
            },
            "call",
            CancellationToken::new(),
        )
        .await;
    assert!(explore.is_ok());
    assert_eq!(fixture.registry().list()[0].agent_type, "Explore");
    fixture.session.stop_background_tasks();
}

#[tokio::test]
async fn at_most_twenty_agents_run_at_once_and_agents_nest_three_levels_deep() {
    let fixture = fixture(vec![
        ("hold", vec![None; MAX_RUNNING]),
        ("quick", vec![Some(text("ok"))]),
    ]);
    let registry = fixture.registry();
    for index in 0..MAX_RUNNING {
        assert!(
            registry
                .start(
                    request(&format!("agent {index}"), "hold", true),
                    "call",
                    CancellationToken::new()
                )
                .await
                .is_ok()
        );
    }
    let seen = fixture.seen.clone();
    eventually("every agent asks", || seen.count("hold") == MAX_RUNNING).await;
    assert_eq!(
        refusal(&fixture, request("one more", "quick", true)).await,
        "20 agents of this session run already, the most at once; wait for one to end or stop one with TaskStop."
    );
    assert_eq!(fixture.session.stop_background_tasks(), MAX_RUNNING);
    let stopped = registry.clone();
    eventually("every agent stopped", || {
        stopped
            .list()
            .iter()
            .all(|agent| agent.status == TaskStatus::Stopped)
    })
    .await;
    let started = registry
        .start(
            request("one more", "quick", false),
            "call",
            CancellationToken::new(),
        )
        .await;
    assert!(started.is_ok());

    // Depth: the main session's agents are at 1; one at 3 starts none.
    let session = fixture.session.clone();
    let general = type_named(&session, "general-purpose");
    let first = child_of(&session, &general, 1);
    assert_eq!(first.agent_registry().unwrap().depth(), 1);
    let second = child_of(&first, &general, 2);
    let third = child_of(&second, &general, MAX_DEPTH);
    assert!(second.available_tool_names().contains(&"Agent".to_string()));
    assert!(third.agent_registry().is_none());
    let names = third.available_tool_names();
    assert!(!names.contains(&"Agent".to_string()));
    assert!(!names.contains(&"ListAgents".to_string()));
    assert!(names.contains(&"SendMessage".to_string()));
    assert_eq!(third.agent_link().unwrap().depth, MAX_DEPTH);
    let deepest = AgentRegistry::new(Arc::downgrade(&third), MAX_DEPTH);
    let refused = deepest
        .start(
            request("deeper", "task", false),
            "call",
            CancellationToken::new(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        refused.to_string(),
        "Agents nest at most 3 levels deep, and this agent is at the deepest level; do the task here."
    );
}

#[tokio::test]
async fn a_message_to_a_running_agent_arrives_at_its_next_step() {
    let fixture = fixture(vec![(
        "child task",
        vec![
            Some(tool_call("g1", "Gate", json!({}))),
            Some(text("child saw it")),
        ],
    )])
    .quiet();
    let registry = fixture.registry();
    registry
        .start(
            request("Work", "child task", true),
            "call_1",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let id = registry.list()[0].id.clone();
    let entered = fixture.entered.clone();
    eventually("the agent is in its tool", || {
        entered.load(Ordering::SeqCst)
    })
    .await;

    let sent = fixture
        .tool("SendMessage")
        .execute(
            "call_2",
            json!({"to": id, "summary": "a hint", "message": "use the cache"}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        sent.output_text(),
        format!("Agent {id} runs; the message reaches it at its next step.")
    );
    fixture.gate.notify_one();
    let seen = fixture.seen.clone();
    eventually("the agent asks again", || seen.count("child task") == 2).await;
    let second = &fixture.seen.of("child task")[1];
    let delivered = user_text(second.context.messages.last().unwrap()).unwrap();
    assert_eq!(
        delivered,
        format!(
            "{}\n<agent-message>\n<from>main</from>\n<summary>a hint</summary>\n<message>use the cache</message>\n</agent-message>",
            prompt_todo("Notification.agent_message")
        )
    );
    let session = fixture.session.clone();
    eventually("the agent ended", || {
        session.tasks()[0].status == TaskStatus::Completed
    })
    .await;
}

#[tokio::test]
async fn a_message_to_a_finished_agent_resumes_it_in_the_background() {
    let fixture = fixture(vec![(
        "child task",
        vec![Some(text("first report")), Some(text("second report"))],
    )])
    .quiet();
    let registry = fixture.registry();
    registry
        .start(
            request("Work", "child task", false),
            "call_1",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let agent = registry.list().pop().unwrap();
    assert_eq!(agent.status, TaskStatus::Completed);

    let sent = fixture
        .tool("SendMessage")
        .execute(
            "call_2",
            json!({"to": "Work", "message": "now the tests"}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        sent.output_text(),
        format!(
            "Agent {} had ended; the message resumes it in the background.",
            agent.id
        )
    );
    let session = fixture.session.clone();
    let collected = Mutex::new(Vec::new());
    for _ in 0..1_000 {
        if queued(&session, &collected)
            .iter()
            .any(|message| notice_text(message).is_some())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let messages = queued(&session, &collected);
    let notice = messages.iter().find_map(notice_text).expect("a notice");
    assert!(notice.contains(&format!("<task-id>{}</task-id>", agent.id)));
    assert!(notice.contains("<tool-use-id>call_2</tool-use-id>"));
    assert!(notice.contains("<result>second report</result>"));
    // The resumed run carried on the conversation it had.
    let second = &fixture.seen.of("child task")[1];
    let messages = &second.context.messages;
    assert_eq!(messages.len(), 3);
    assert!(
        matches!(&messages[1], Message::Assistant(assistant) if assistant.text() == "first report")
    );
    assert!(
        user_text(&messages[2])
            .unwrap()
            .contains("<message>now the tests</message>")
    );
    // Same agent, same id, now a finished background task.
    let agents = registry.list();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].id, agent.id);
    assert_eq!(agents[0].status, TaskStatus::Completed);
    assert!(agents[0].background);
    assert_eq!(
        session.background_tasks().find(&agent.id).unwrap().status,
        TaskStatus::Completed
    );
}

#[tokio::test]
async fn an_agent_writes_to_main_through_the_queue_of_its_parent() {
    let fixture = fixture(vec![(
        "child task",
        vec![
            Some(tool_call(
                "s1",
                "SendMessage",
                json!({"to": "main", "summary": "progress", "message": "half done"}),
            )),
            Some(text("report")),
        ],
    )])
    .quiet();
    let registry = fixture.registry();
    registry
        .start(
            request("Work", "child task", true),
            "call_1",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let id = registry.list()[0].id.clone();
    let session = fixture.session.clone();
    let collected = Mutex::new(Vec::new());
    for _ in 0..1_000 {
        if queued(&session, &collected).len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let messages = queued(&session, &collected);
    assert_eq!(messages.len(), 2, "{messages:#?}");
    assert!(
        matches!(&messages[0], AgentMessage::Custom(custom) if custom.custom_type == AGENT_MESSAGE)
    );
    assert_eq!(
        sent_message(&messages[0]),
        Some((id.clone(), Some("progress".into()), "half done".into()))
    );
    assert!(notice_text(&messages[1]).is_some());
    let child = &fixture.seen.of("child task")[1];
    assert_eq!(texts(&last_tool_result(&child.context)), ["Sent to main."]);

    // The main session itself has no main to write to.
    let refused = fixture
        .tool("SendMessage")
        .execute(
            "call",
            json!({"to": "main", "message": "hello"}),
            CancellationToken::new(),
            None,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        refused.to_string(),
        "This is the main session; give the id or name of one of its agents."
    );
}

#[tokio::test]
async fn list_agents_lists_name_id_type_and_status_in_pages() {
    let count = PAGE_SIZE + 5;
    let fixture = fixture(vec![("quick", vec![Some(text("ok")); count])]);
    let registry = fixture.registry();
    for index in 0..count {
        registry
            .start(
                request(&format!("agent {index}"), "quick", false),
                "call",
                CancellationToken::new(),
            )
            .await
            .unwrap();
    }
    let tool = fixture.tool("ListAgents");
    let first = tool
        .execute(
            "call",
            json!({"channel": "anything", "q": "ignored"}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let page: Value = serde_json::from_str(&first.output_text()).unwrap();
    let agents = page["agents"].as_array().unwrap();
    assert_eq!(agents.len(), PAGE_SIZE);
    let all = registry.list();
    assert_eq!(
        agents[0],
        json!({"name": "agent 0", "id": all[0].id, "type": "general-purpose", "status": "completed"})
    );
    assert_eq!(page["nextCursor"], PAGE_SIZE.to_string());
    let second = tool
        .execute(
            "call",
            json!({"cursor": page["nextCursor"]}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let page: Value = serde_json::from_str(&second.output_text()).unwrap();
    let agents = page["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 5);
    assert_eq!(agents[4]["name"], format!("agent {}", count - 1));
    assert!(page.get("nextCursor").is_none());
    let bad = tool
        .execute(
            "call",
            json!({"cursor": "later"}),
            CancellationToken::new(),
            None,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        bad.to_string(),
        "cursor 'later' is not one ListAgents gave; leave it out to list from the start."
    );
}

#[tokio::test]
async fn an_interrupt_stops_the_agent_the_turn_waits_for_and_spares_background_ones() {
    let fixture = fixture(vec![
        (
            "main",
            vec![
                Some(tool_call(
                    "bg",
                    "Agent",
                    json!({"description": "Background", "prompt": "bg task"}),
                )),
                Some(tool_call(
                    "fg",
                    "Agent",
                    json!({"description": "Waited", "prompt": "fg task", "run_in_background": false}),
                )),
            ],
        ),
        ("bg task", vec![None]),
        ("fg task", vec![None]),
    ]);
    let session = fixture.session.clone();
    let turn = tokio::spawn({
        let session = session.clone();
        async move { session.prompt(vec![AgentMessage::user("main")]).await }
    });
    let seen = fixture.seen.clone();
    eventually("both agents ask", || {
        seen.count("bg task") == 1 && seen.count("fg task") == 1
    })
    .await;
    session.abort();
    tokio::time::timeout(std::time::Duration::from_secs(5), turn)
        .await
        .expect("the turn ends")
        .unwrap();

    let registry = fixture.registry();
    let find = |name: &str| {
        registry
            .list()
            .into_iter()
            .find(|agent| agent.name == name)
            .unwrap()
    };
    assert_eq!(find("Waited").status, TaskStatus::Stopped);
    let background = find("Background");
    assert_eq!(background.status, TaskStatus::Running);
    assert!(registry.session_of(&background.id).unwrap().is_running());
    assert_eq!(
        session
            .background_tasks()
            .find(&background.id)
            .unwrap()
            .status,
        TaskStatus::Running
    );

    // TaskStop stops it through the task registry, without a notice.
    let stop = TaskStopTool::new(Arc::downgrade(&session));
    let stopped = stop
        .execute(
            "call",
            json!({"task_id": background.id}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        stopped.output_text(),
        format!("Stopped the agent task {} (Background).", background.id)
    );
    assert_eq!(find("Background").status, TaskStatus::Stopped);
    let child = registry.session_of(&background.id).unwrap();
    eventually("the agent stopped", || !child.is_running()).await;
    assert!(!session.has_queued_entries());
}

#[test]
fn the_agent_tools_have_the_interface_schemas_and_placeholder_descriptions() {
    let tools = agent_tools(std::sync::Weak::new());
    let expected = [
        (
            "Agent",
            vec![
                "description",
                "prompt",
                "subagent_type",
                "model",
                "effort",
                "run_in_background",
                "isolation",
            ],
            json!(["description", "prompt"]),
        ),
        (
            "SendMessage",
            vec!["to", "summary", "message", "notify_when_idle"],
            json!(["to", "message"]),
        ),
        ("ListAgents", vec!["channel", "q", "cursor"], Value::Null),
    ];
    for (tool, (name, parameters, required)) in tools.iter().zip(expected) {
        assert_eq!(tool.name(), name);
        assert!(tool.advanced_only());
        assert_eq!(
            tool.description(),
            prompt_todo(&format!("{name}.description"))
        );
        let schema = tool.parameters();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], required);
        let properties = schema["properties"].as_object().unwrap();
        let names: Vec<&str> = properties.keys().map(String::as_str).collect();
        let mut sorted = parameters.clone();
        sorted.sort_unstable();
        let mut found = names.clone();
        found.sort_unstable();
        assert_eq!(found, sorted, "{name}");
        for parameter in parameters {
            assert_eq!(
                properties[parameter]["description"],
                prompt_todo(&format!("{name}.param.{parameter}"))
            );
        }
    }
    let agent = tools[0].parameters();
    assert_eq!(
        agent["properties"]["isolation"]["enum"],
        json!(["worktree", "remote"])
    );
    let send = tools[1].parameters();
    assert_eq!(send["properties"]["summary"]["maxLength"], 200);
    assert_eq!(send["properties"]["message"]["default"], "");
    assert_eq!(
        send["properties"]["to"]["allOf"].as_array().unwrap().len(),
        2
    );
}

#[tokio::test]
async fn send_message_refuses_what_it_cannot_do() {
    let fixture = fixture(Vec::new());
    let send = fixture.tool("SendMessage");
    let refuse = |args: Value| {
        let send = send.clone();
        async move {
            send.execute("call", args, CancellationToken::new(), None)
                .await
                .err()
                .unwrap()
                .to_string()
        }
    };
    assert_eq!(
        refuse(json!({"to": "x", "message": "m", "notify_when_idle": true})).await,
        "notify_when_idle is not available; leave it out."
    );
    assert_eq!(
        refuse(json!({"to": "x", "message": " "})).await,
        "message is empty; give the text to send."
    );
    assert_eq!(
        refuse(json!({"to": "nobody", "message": "m"})).await,
        "No agent of this session has the id or name 'nobody'; ListAgents lists them."
    );
    // Two agents with one name are told apart by id.
    let fixture = fixture_with(|_| Vec::new(), vec![("quick", vec![Some(text("ok")); 2])]);
    for _ in 0..2 {
        fixture
            .registry()
            .start(
                request("Same", "quick", false),
                "call",
                CancellationToken::new(),
            )
            .await
            .unwrap();
    }
    let refused = fixture
        .tool("SendMessage")
        .execute(
            "call",
            json!({"to": "Same", "message": "m"}),
            CancellationToken::new(),
            None,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        refused.to_string(),
        "Several agents of this session are named 'Same'; give the id of one of them."
    );
}

#[tokio::test]
async fn a_new_conversation_forgets_the_agents_and_stops_the_running_ones() {
    let fixture = fixture(vec![("hold", vec![None])]);
    let registry = fixture.registry();
    registry
        .start(
            request("Held", "hold", true),
            "call",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let child = registry.session_of("Held").unwrap();
    let seen = fixture.seen.clone();
    eventually("the agent asks", || seen.count("hold") == 1).await;
    fixture
        .session
        .replace_manager(SessionManager::in_memory(fixture.directory.path()));
    assert!(registry.list().is_empty());
    eventually("the agent stopped", || !child.is_running()).await;
}

#[cfg(unix)]
#[tokio::test]
async fn the_notice_of_an_agents_own_background_command_belongs_to_its_run() {
    let fixture = fixture_with(
        core_tools,
        vec![
            (
                "main",
                vec![
                    Some(tool_call(
                        "call_1",
                        "Agent",
                        json!({"description": "Build", "prompt": "child task", "run_in_background": false}),
                    )),
                    Some(text("done")),
                ],
            ),
            (
                "child task",
                vec![
                    Some(tool_call(
                        "b1",
                        "Bash",
                        json!({"command": "sleep 0.2; echo built", "run_in_background": true}),
                    )),
                    Some(text("started the build")),
                    Some(text("the build finished")),
                ],
            ),
        ],
    );
    fixture
        .session
        .prompt(vec![AgentMessage::user("main")])
        .await;
    // The child's turn ended while its command ran; the command's notice
    // started the child's next turn, and that turn's answer is the report.
    let child = fixture.seen.of("child task");
    assert_eq!(child.len(), 3);
    let notice = user_text(child[2].context.messages.last().unwrap()).unwrap();
    assert!(notice.starts_with(&notice_header()));
    let main = fixture.seen.of("main");
    assert_eq!(
        texts(&last_tool_result(&main[1].context))[0],
        "the build finished"
    );
    // The command was the child's task, not the parent's.
    assert!(fixture.session.tasks().is_empty());
}

#[tokio::test]
async fn turning_subagents_off_stops_the_running_agents() {
    let fixture = fixture(vec![("hold", vec![None])]);
    let registry = fixture.registry();
    registry
        .start(
            request("Held", "hold", true),
            "call",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let child = registry.session_of("Held").unwrap();
    let seen = fixture.seen.clone();
    eventually("the agent asks", || seen.count("hold") == 1).await;
    let mut off = fixture.session.settings();
    off.subagents.enabled = false;
    fixture.session.update_settings(off);
    assert_eq!(registry.list()[0].status, TaskStatus::Stopped);
    eventually("the agent stopped", || !child.is_running()).await;
    assert!(!fixture.session.has_queued_entries());
}
