//! `Skill` and `Workflow`, driven through sessions with a scripted provider
//! that answers each conversation from its own lane (see
//! `agent_tools_tests`): the main session's lane is `main`, an agent's or a
//! workflow agent's is the first text it was given.
//!
//! Where a workflow ends while the main session works, the main session's
//! model waits at a `Gate` until the workflow's notice is queued, so that the
//! notice reaches the turn at its next step and the requests come in a fixed
//! order.

use crate::agent_tools_tests::{
    Seen, core_tools, lanes, last_tool_result, notice_header, notice_text, queued, settings, texts,
    user_text,
};
use crate::background_tests::{eventually, model, text, tool_call};
use crate::session::manager::{SessionManager, workflows_dir};
use crate::settings::{Mode, ProjectTrustDefault, Settings};
use crate::skills::{Skill, skill_text};
use crate::tasks::{TaskKind, TaskStatus};
use crate::tools::task_stop::TaskStopTool;
use crate::workflows::{AgentStatus as WorkflowAgentStatus, RunRecord, RunStatus};
use crate::{AgentSession, PromptMode};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_agent::placeholder::prompt_todo;
use super_agent::{AgentMessage, AgentTool, DynTool, ToolResult, ToolUpdateSink};
use super_ai::{AssistantMessage, Registry, ThinkingLevel};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Waits until the test opens it, and says it was entered.
struct Gate {
    open: Arc<Notify>,
    entered: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl AgentTool for Gate {
    fn name(&self) -> &str {
        "Gate"
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
        cancel: CancellationToken,
        _on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        self.entered.store(true, Ordering::SeqCst);
        tokio::select! {
            () = self.open.notified() => Ok(ToolResult::text("gate opened")),
            () = cancel.cancelled() => anyhow::bail!("gate cancelled"),
        }
    }
}

struct Fixture {
    session: Arc<AgentSession>,
    seen: Arc<Seen>,
    open: Arc<Notify>,
    entered: Arc<AtomicBool>,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn folder(&self) -> &Path {
        self.directory.path()
    }

    /// Queued entries start no turn: the test reads them.
    fn quiet(self) -> Self {
        self.session.set_turn_starter(Some(Arc::new(|_turn| {})));
        self
    }

    /// Answer from these lanes from now on.
    fn answer(&mut self, script: Vec<(&str, Vec<Option<AssistantMessage>>)>) {
        let (stream, seen) = lanes(script);
        self.session.set_stream_fn(Some(stream));
        self.seen = seen;
    }

    /// Write a skill file under the folder and hand it to the session.
    fn with_skill(self, name: &str, text: &str) -> Self {
        let file = self.skill_file(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        let mut resources = self.session.agent_resources();
        resources.skills.push(Skill {
            name: name.into(),
            description: format!("Use {name}"),
            file_path: file,
            disable_model_invocation: text.contains("disable-model-invocation: true"),
        });
        self.session.set_agent_resources(resources);
        self
    }

    fn skill_file(&self, name: &str) -> PathBuf {
        self.folder().join("skills").join(name).join("SKILL.md")
    }

    fn latest_run(&self) -> Arc<RunRecord> {
        self.session
            .workflows()
            .and_then(|runtime| runtime.latest())
            .expect("a workflow ran")
    }

    fn session_file(&self) -> PathBuf {
        self.session
            .manager
            .lock()
            .unwrap()
            .session_file()
            .unwrap()
            .to_path_buf()
    }

    /// Run a main turn whose model starts a workflow and then waits at the
    /// gate until the workflow's notice is queued; returns the notice the
    /// turn's last request carried.
    async fn workflow_turn(&self, prompt: &str) -> String {
        self.entered.store(false, Ordering::SeqCst);
        let session = self.session.clone();
        let prompt = prompt.to_string();
        let turn = tokio::spawn(async move {
            session.prompt(vec![AgentMessage::user(prompt)]).await;
        });
        eventually("the main session waits at the gate", || {
            self.entered.load(Ordering::SeqCst)
        })
        .await;
        eventually("the workflow's notice is queued", || {
            self.session.has_queued_entries()
        })
        .await;
        self.open.notify_one();
        turn.await.unwrap();
        last_notice(&self.seen)
    }
}

fn session_on(
    manager: SessionManager,
    directory: &Path,
    settings: Settings,
    open: &Arc<Notify>,
    entered: &Arc<AtomicBool>,
) -> Arc<AgentSession> {
    let mut tools: Vec<DynTool> = core_tools(directory);
    tools.push(Arc::new(Gate {
        open: open.clone(),
        entered: entered.clone(),
    }));
    AgentSession::new(
        manager,
        tools,
        Registry::from_builtin(),
        settings,
        "main prompt".into(),
        model(),
        ThinkingLevel::Off,
        None,
        Arc::new(|_| {}),
    )
}

fn fixture_with(settings: Settings, script: Vec<(&str, Vec<Option<AssistantMessage>>)>) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let open = Arc::new(Notify::new());
    let entered = Arc::new(AtomicBool::new(false));
    let manager =
        SessionManager::create(directory.path(), Some(directory.path().join("sessions"))).unwrap();
    let session = session_on(manager, directory.path(), settings, &open, &entered);
    let (stream, seen) = lanes(script);
    session.set_stream_fn(Some(stream));
    Fixture {
        session,
        seen,
        open,
        entered,
        directory,
    }
}

fn fixture(script: Vec<(&str, Vec<Option<AssistantMessage>>)>) -> Fixture {
    fixture_with(settings(), script)
}

fn tool_names(seen: &Seen, request: usize) -> Vec<String> {
    seen.of("main")[request]
        .context
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

fn system_prompt(seen: &Seen, request: usize) -> String {
    seen.of("main")[request]
        .context
        .system_prompt
        .clone()
        .unwrap_or_default()
}

/// The latest notice the main session's model saw.
fn last_notice(seen: &Seen) -> String {
    seen.of("main")
        .last()
        .expect("a request")
        .context
        .messages
        .iter()
        .rev()
        .filter_map(user_text)
        .find(|text| text.starts_with(&notice_header()))
        .expect("a notice")
}

fn result_text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            super_ai::ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn statuses(run: &RunRecord) -> Vec<WorkflowAgentStatus> {
    run.snapshot().phases[0]
        .agents
        .iter()
        .map(|agent| agent.status)
        .collect()
}

/// A Workflow call, then the gate, then an answer: one main turn of
/// [`Fixture::workflow_turn`].
fn workflow_calls(id: &str, args: Value) -> Vec<Option<AssistantMessage>> {
    vec![
        Some(tool_call(id, "Workflow", args)),
        Some(tool_call(&format!("{id}_gate"), "Gate", json!({}))),
        Some(text("noted")),
    ]
}

const REVIEW: &str =
    "---\nname: review\ndescription: Review a change\n---\nReview $ARGUMENTS carefully.\n";

// ----- Skill ---------------------------------------------------------------

#[tokio::test]
async fn the_model_loads_a_skill_and_its_text_comes_back_into_the_turn() {
    let fixture = fixture(vec![(
        "main",
        vec![
            Some(tool_call(
                "s1",
                "Skill",
                json!({"skill": "review", "args": "src/a.rs"}),
            )),
            Some(tool_call("s2", "Skill", json!({"skill": "/review"}))),
            Some(tool_call("s3", "Skill", json!({"skill": "mine"}))),
            Some(tool_call("s4", "Skill", json!({"skill": "missing"}))),
            Some(text("reviewed")),
        ],
    )])
    .with_skill("review", REVIEW)
    .with_skill(
        "mine",
        "---\nname: mine\ndisable-model-invocation: true\n---\nOnly for me.",
    );
    let session = fixture.session.clone();
    session.prompt(vec![AgentMessage::user("main")]).await;

    let main = fixture.seen.of("main");
    assert_eq!(main.len(), 5);
    let loaded = last_tool_result(&main[1].context);
    assert!(!loaded.is_error, "{:?}", texts(&loaded));
    assert_eq!(
        texts(&loaded),
        [skill_text(
            "Skill.loaded",
            "review",
            Some(&fixture.skill_file("review")),
            "Review $ARGUMENTS carefully.\n",
            Some("src/a.rs"),
        )]
    );
    assert!(texts(&loaded)[0].contains("<content>\nReview src/a.rs carefully.\n</content>"));
    // `/review`, as the user types it, is the same skill.
    let again = last_tool_result(&main[2].context);
    assert!(!again.is_error);
    assert!(texts(&again)[0].contains("<name>review</name>"));
    // A skill only the user may start is refused, and an unknown one names
    // the skills there are; the built-in guide comes first.
    let mine = last_tool_result(&main[3].context);
    assert!(mine.is_error);
    assert_eq!(
        texts(&mine),
        ["The skill mine is one the user starts with /mine; it cannot be loaded with Skill."]
    );
    let missing = last_tool_result(&main[4].context);
    assert!(missing.is_error);
    assert_eq!(
        texts(&missing),
        ["No skill is named 'missing'; the skills are workflow-authoring, review."]
    );
    // Nothing ran in an agent.
    assert!(session.agent_registry().unwrap().list().is_empty());
}

#[tokio::test]
async fn the_advanced_mode_lists_skills_for_the_tool_and_the_basic_mode_does_not_offer_it() {
    let fixture = fixture(vec![("main", vec![Some(text("one")), Some(text("two"))])])
        .with_skill("review", REVIEW)
        .with_skill(
            "mine",
            "---\nname: mine\ndisable-model-invocation: true\n---\nOnly for me.",
        );
    let session = fixture.session.clone();
    session.prompt(vec![AgentMessage::user("main")]).await;
    assert!(tool_names(&fixture.seen, 0).contains(&"Skill".to_string()));
    let prompt = system_prompt(&fixture.seen, 0);
    let list = format!(
        "{}\n<available_skills>\n  <skill>\n    <name>workflow-authoring</name>\n    <description>{}</description>\n  </skill>\n  <skill>\n    <name>review</name>\n    <description>Use review</description>\n  </skill>\n</available_skills>",
        prompt_todo("Skill.list"),
        prompt_todo("Skill.workflow-authoring.description"),
    );
    assert!(prompt.contains(&list), "{prompt}");
    // No file, no word on reading it, and no skill only the user starts.
    assert!(!prompt.contains("SKILL.md"));
    assert!(!prompt.contains("<location>"));
    assert!(!prompt.contains("<name>mine</name>"));

    // The basic mode keeps today's way: no tool and no such list; the
    // system prompt the host builds lists the skills with their files.
    let mut basic = session.settings();
    basic.mode = Mode::Basic;
    session.update_settings(basic);
    session.prompt(vec![AgentMessage::user("again")]).await;
    assert!(!tool_names(&fixture.seen, 1).contains(&"Skill".to_string()));
    assert!(!system_prompt(&fixture.seen, 1).contains(&prompt_todo("Skill.list")));
    let basic_prompt =
        crate::system_prompt::build_system_prompt(&crate::system_prompt::SystemPromptOptions {
            mode: Mode::Basic,
            custom_prompt: None,
            append: None,
            selected_tools: &["Read".to_string()],
            cwd: "/work",
            context_files: &[],
            skills: &session.agent_resources().skills,
        });
    assert!(basic_prompt.contains(&format!(
        "<location>{}</location>",
        fixture.skill_file("review").display()
    )));
    assert!(basic_prompt.contains("Use Read to load a skill's file"));
}

#[tokio::test]
async fn the_workflow_guide_is_a_built_in_skill_while_workflows_are_on() {
    let fixture = fixture(vec![(
        "main",
        vec![
            Some(tool_call(
                "s1",
                "Skill",
                json!({"skill": "workflow-authoring"}),
            )),
            Some(text("read it")),
        ],
    )]);
    fixture
        .session
        .prompt(vec![AgentMessage::user("main")])
        .await;
    let result = last_tool_result(&fixture.seen.of("main")[1].context);
    assert!(!result.is_error);
    let guide =
        crate::workflows::authoring_prompt(fixture.session.settings().workflows.size, 1000, 4096);
    assert_eq!(
        texts(&result),
        [skill_text(
            "Skill.loaded",
            "workflow-authoring",
            None,
            &guide,
            None
        )]
    );
    // Its text is the guide the prompt had before, with the tool's new name.
    assert!(guide.contains("Call the `Workflow` tool"));
    assert!(guide.contains("Give `Workflow` a short kebab-case `name`"));
    assert!(!guide.contains("run_workflow"));

    // Without dynamic workflows there is no guide to load.
    let mut off = fixture.session.settings();
    off.workflows.enabled = false;
    fixture.session.update_settings(off);
    assert!(crate::tools::skill::summaries(&fixture.session).is_empty());
}

#[tokio::test]
async fn a_skill_that_asks_for_it_runs_in_an_agent_the_turn_waits_for() {
    let body = "Audit the tools.\n";
    let mut fixture = fixture(Vec::new()).with_skill(
        "audit",
        &format!("---\nname: audit\ncontext: fork\n---\n{body}"),
    );
    let prompt = skill_text(
        "Skill.agent_prompt",
        "audit",
        Some(&fixture.skill_file("audit")),
        body,
        Some("src/tools"),
    );
    fixture.answer(vec![
        (
            "main",
            vec![
                Some(tool_call(
                    "s1",
                    "Skill",
                    json!({"skill": "audit", "args": "src/tools"}),
                )),
                Some(text("done")),
            ],
        ),
        (prompt.as_str(), vec![Some(text("the audit report"))]),
    ]);
    let session = fixture.session.clone();
    session.prompt(vec![AgentMessage::user("main")]).await;

    assert_eq!(
        fixture.seen.count(&prompt),
        1,
        "the agent got the skill as its task"
    );
    let main = fixture.seen.of("main");
    assert_eq!(main.len(), 2);
    let result = last_tool_result(&main[1].context);
    assert!(!result.is_error, "{:?}", texts(&result));
    let agents = session.agent_registry().unwrap().list();
    assert_eq!(agents.len(), 1);
    let agent = &agents[0];
    assert_eq!(agent.name, "audit");
    assert_eq!(agent.agent_type, "general-purpose");
    assert!(!agent.background);
    assert_eq!(agent.status, TaskStatus::Completed);
    let texts = texts(&result);
    assert_eq!(texts[0], "the audit report");
    assert!(texts[1].starts_with(&format!(
        "<agent-result>\n<agent-id>{}</agent-id>",
        agent.id
    )));
    // A waited run is no background task.
    assert!(session.tasks().is_empty());
}

#[tokio::test]
async fn a_background_skill_answers_with_its_agent_and_is_announced_with_a_notice() {
    let body = "Look around.\n";
    let mut fixture = fixture(Vec::new()).with_skill(
        "survey",
        &format!("---\nname: survey\ncontext: fork\nagent: Explore\nbackground: true\n---\n{body}"),
    );
    let prompt = skill_text(
        "Skill.agent_prompt",
        "survey",
        Some(&fixture.skill_file("survey")),
        body,
        None,
    );
    fixture.answer(vec![
        (
            "main",
            vec![
                Some(tool_call("s1", "Skill", json!({"skill": "survey"}))),
                Some(text("started it")),
                Some(text("got the survey")),
            ],
        ),
        (
            prompt.as_str(),
            vec![
                Some(tool_call("g1", "Gate", json!({}))),
                Some(text("the survey")),
            ],
        ),
    ]);
    let session = fixture.session.clone();
    session.prompt(vec![AgentMessage::user("main")]).await;

    // The call returned at once with the agent's name and id.
    assert_eq!(fixture.seen.count("main"), 2);
    let agent = session.agent_registry().unwrap().list().pop().unwrap();
    assert_eq!(agent.name, "survey");
    assert_eq!(agent.agent_type, "Explore");
    assert!(agent.background);
    let result = last_tool_result(&fixture.seen.of("main")[1].context);
    assert!(!result.is_error, "{:?}", texts(&result));
    assert_eq!(
        texts(&result),
        [format!(
            "Skill survey runs in the background as the agent survey ({}). Its transcript is written to {}.",
            agent.id,
            agent.session_file.as_ref().unwrap().display()
        )]
    );
    let task = session.background_tasks().find(&agent.id).unwrap();
    assert_eq!(task.kind, TaskKind::Agent);
    assert_eq!(task.tool_use_id.as_deref(), Some("s1"));

    eventually("the agent waits at the gate", || {
        fixture.entered.load(Ordering::SeqCst)
    })
    .await;
    fixture.open.notify_one();
    session.wait_for_background_work().await;
    let main = fixture.seen.of("main");
    assert_eq!(main.len(), 3, "the notice started a turn");
    let notice = user_text(main[2].context.messages.last().unwrap()).unwrap();
    assert!(notice.starts_with(&notice_header()));
    assert!(notice.contains(&format!("<task-id>{}</task-id>", agent.id)));
    assert!(notice.contains("<tool-use-id>s1</tool-use-id>"));
    assert!(notice.contains("<summary>survey (completed)</summary>"));
    assert!(notice.contains("<result>the survey</result>"));
}

#[tokio::test]
async fn a_skill_that_needs_an_agent_fails_where_agents_are_off() {
    let mut off = settings();
    off.subagents.enabled = false;
    let fixture = fixture_with(
        off,
        vec![(
            "main",
            vec![
                Some(tool_call("s1", "Skill", json!({"skill": "audit"}))),
                Some(text("done")),
            ],
        )],
    )
    .with_skill("audit", "---\nname: audit\ncontext: fork\n---\nAudit.");
    fixture
        .session
        .prompt(vec![AgentMessage::user("main")])
        .await;
    let result = last_tool_result(&fixture.seen.of("main")[1].context);
    assert!(result.is_error);
    assert_eq!(
        texts(&result),
        ["The skill audit runs in an agent of its own, and this session cannot start agents."]
    );
}

// ----- Workflow ------------------------------------------------------------

const GREET: &str = "export const meta = { name: 'greet', description: 'Greet twice' }\n\
const a = await agent('first agent')\n\
const b = await agent(`second agent ${args.who}`)\n\
return `${a} and ${b}`\n";

#[tokio::test]
async fn a_workflow_runs_in_the_background_and_its_notice_carries_the_answer() {
    let fixture = fixture(vec![
        (
            "main",
            vec![
                Some(tool_call(
                    "w1",
                    "Workflow",
                    json!({
                        "script": GREET,
                        "description": "accepted, not used",
                        "title": "accepted, not used",
                        "args": {"who": "Ada"},
                    }),
                )),
                Some(text("started it")),
                Some(text("got the answer")),
            ],
        ),
        ("first agent", vec![Some(text("one"))]),
        (
            "second agent Ada",
            vec![Some(tool_call("g1", "Gate", json!({}))), Some(text("two"))],
        ),
    ]);
    let session = fixture.session.clone();
    // The tool is there on an ordinary turn.
    session.prompt(vec![AgentMessage::user("main")]).await;
    assert!(tool_names(&fixture.seen, 0).contains(&"Workflow".to_string()));

    // The call returned at once, with the run's id and its saved script.
    let main = fixture.seen.of("main");
    assert_eq!(main.len(), 2);
    let run = fixture.latest_run();
    assert!(run.run_id.starts_with("wf_") && run.run_id.len() == 11);
    let script_path = workflows_dir(&fixture.session_file()).join(format!("{}.js", run.run_id));
    assert_eq!(run.script_path.as_deref(), Some(script_path.as_path()));
    assert_eq!(std::fs::read_to_string(&script_path).unwrap(), GREET);
    let result = last_tool_result(&main[1].context);
    assert!(!result.is_error, "{:?}", texts(&result));
    assert_eq!(
        texts(&result),
        [format!(
            "Workflow greet runs in the background as task {}. Its script is saved at {}.",
            run.run_id,
            script_path.display()
        )]
    );
    let task = session.background_tasks().find(&run.run_id).unwrap();
    assert_eq!(task.kind, TaskKind::Workflow);
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.name.as_deref(), Some("greet"));
    assert_eq!(task.description, "Greet twice");
    assert_eq!(task.tool_use_id.as_deref(), Some("w1"));

    eventually("the second agent waits at the gate", || {
        fixture.entered.load(Ordering::SeqCst)
    })
    .await;
    fixture.open.notify_one();
    session.wait_for_background_work().await;

    // `args` reached the script: the second agent was asked with them.
    assert_eq!(fixture.seen.count("second agent Ada"), 2);
    assert_eq!(fixture.seen.count("main"), 3, "the notice started a turn");
    let notice = last_notice(&fixture.seen);
    assert!(notice.contains(&format!("<task-id>{}</task-id>", run.run_id)));
    assert!(notice.contains("<tool-use-id>w1</tool-use-id>"));
    assert!(notice.contains("<status>completed</status>"));
    assert!(notice.contains("<summary>greet (completed)</summary>"));
    assert!(
        notice.contains("<result>one and two\n\n[workflow: 2 agents across 1 phases"),
        "{notice}"
    );
    assert_eq!(
        session.background_tasks().find(&run.run_id).unwrap().status,
        TaskStatus::Completed
    );
    assert_eq!(run.snapshot().status, RunStatus::Completed);
    // Its journal is kept beside the script for a run that resumes it.
    assert!(
        workflows_dir(&fixture.session_file())
            .join(format!("{}.journal.jsonl", run.run_id))
            .is_file()
    );
}

#[tokio::test]
async fn task_stop_stops_a_workflow_without_a_notice() {
    let fixture = fixture(vec![
        (
            "main",
            vec![
                Some(tool_call(
                    "w1",
                    "Workflow",
                    json!({"script": GREET, "args": {"who": "Ada"}}),
                )),
                Some(text("started it")),
            ],
        ),
        (
            "first agent",
            vec![Some(tool_call("g1", "Gate", json!({}))), None],
        ),
    ])
    .quiet();
    let session = fixture.session.clone();
    session.prompt(vec![AgentMessage::user("main")]).await;
    let run = fixture.latest_run();
    eventually("the first agent waits at the gate", || {
        fixture.entered.load(Ordering::SeqCst)
    })
    .await;

    let stop = TaskStopTool::new(Arc::downgrade(&session));
    let stopped = stop
        .execute(
            "t1",
            json!({"task_id": run.run_id}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        result_text(&stopped),
        format!("Stopped the workflow task {} (Greet twice).", run.run_id)
    );
    eventually("the run ends", || run.snapshot().status.is_finished()).await;
    assert_eq!(run.snapshot().status, RunStatus::Stopped);
    assert_eq!(
        session.background_tasks().find(&run.run_id).unwrap().status,
        TaskStatus::Stopped
    );
    // The run stopped through its task is not announced.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let seen = Mutex::new(Vec::new());
    assert!(
        queued(&session, &seen)
            .iter()
            .all(|message| notice_text(message).is_none())
    );
    // The second agent never started.
    assert_eq!(fixture.seen.count("second agent Ada"), 0);
}

#[tokio::test]
async fn a_workflow_is_stopped_by_the_name_its_call_gave_it() {
    let fixture = fixture(vec![
        (
            "main",
            vec![
                Some(tool_call(
                    "w1",
                    "Workflow",
                    json!({"script": GREET, "name": "my-label", "args": {"who": "Ada"}}),
                )),
                Some(text("started it")),
            ],
        ),
        (
            "first agent",
            vec![Some(tool_call("g1", "Gate", json!({}))), None],
        ),
    ])
    .quiet();
    let session = fixture.session.clone();
    session.prompt(vec![AgentMessage::user("main")]).await;
    let run = fixture.latest_run();
    let task = session.background_tasks().find(&run.run_id).unwrap();
    assert_eq!(task.name.as_deref(), Some("my-label"));

    let stop = TaskStopTool::new(Arc::downgrade(&session));
    let stopped = stop
        .execute(
            "t1",
            json!({"task_id": "my-label"}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        result_text(&stopped),
        format!("Stopped the workflow task {} (Greet twice).", run.run_id)
    );
    eventually("the run ends", || run.snapshot().status.is_finished()).await;
    assert_eq!(run.snapshot().status, RunStatus::Stopped);
}

#[tokio::test]
async fn a_workflow_the_user_stops_is_announced_with_how_to_resume_it() {
    let fixture = fixture(vec![
        (
            "main",
            vec![
                Some(tool_call(
                    "w1",
                    "Workflow",
                    json!({"script": GREET, "args": {"who": "Ada"}}),
                )),
                Some(text("started it")),
            ],
        ),
        (
            "first agent",
            vec![Some(tool_call("g1", "Gate", json!({}))), None],
        ),
    ])
    .quiet();
    let session = fixture.session.clone();
    session.prompt(vec![AgentMessage::user("main")]).await;
    let run = fixture.latest_run();
    eventually("the first agent waits at the gate", || {
        fixture.entered.load(Ordering::SeqCst)
    })
    .await;
    // The progress view stops the run itself, not its task.
    run.stop();
    eventually("the notice is queued", || session.has_queued_entries()).await;
    let seen = Mutex::new(Vec::new());
    let notice = queued(&session, &seen)
        .iter()
        .find_map(notice_text)
        .expect("a notice");
    assert!(notice.contains("<status>stopped</status>"));
    // The task id is the run id resumeFromRunId takes; the summary says how
    // far the run came; the fixed text is a placeholder.
    assert!(notice.contains(&format!("<task-id>{}</task-id>", run.run_id)));
    assert!(
        notice.contains("<summary>greet (stopped after "),
        "{notice}"
    );
    assert!(notice.contains(&format!(
        "<result>{}</result>",
        prompt_todo("Workflow.notice.stopped")
    )));
}

#[tokio::test]
async fn a_saved_workflow_runs_by_its_name_with_its_args() {
    let mut trusting = settings();
    trusting.default_project_trust = ProjectTrustDefault::Always;
    let mut main = vec![Some(tool_call("w0", "Workflow", json!({"name": "nobody"})))];
    main.extend(workflow_calls(
        "w1",
        json!({"name": "hello", "args": "Ada"}),
    ));
    let fixture = fixture_with(
        trusting,
        vec![
            ("main", main),
            ("say hi to Ada", vec![Some(text("hi Ada"))]),
        ],
    );
    let saved = crate::workflows::SaveLocation::Project
        .directory(fixture.folder())
        .unwrap();
    std::fs::create_dir_all(&saved).unwrap();
    std::fs::write(
        saved.join("hello.js"),
        "export const meta = { name: 'hello', description: 'Say hi' }\n\
         return await agent(`say hi to ${args}`)\n",
    )
    .unwrap();
    let notice = fixture.workflow_turn("main").await;

    let unknown = last_tool_result(&fixture.seen.of("main")[1].context);
    assert!(unknown.is_error);
    // The user's own folder may hold saved workflows too.
    assert!(
        texts(&unknown)[0]
            .starts_with("No saved workflow is named 'nobody'; the saved workflows are "),
        "{:?}",
        texts(&unknown)
    );
    assert!(texts(&unknown)[0].contains("hello"));
    assert_eq!(fixture.seen.count("say hi to Ada"), 1);
    assert!(notice.contains("<summary>hello (completed)</summary>"));
    assert!(notice.contains("<result>hi Ada\n\n[workflow:"), "{notice}");
}

#[tokio::test]
async fn a_saved_workflow_comes_from_the_project_the_host_found_and_trusts() {
    // The session works elsewhere (a worktree), and the default trust would
    // not let it read a project's files: the host's project and trust count.
    let mut main = vec![];
    main.extend(workflow_calls("w1", json!({"name": "proj-wf"})));
    let fixture = fixture(vec![
        ("main", main),
        ("project agent", vec![Some(text("from the project"))]),
    ]);
    let project = fixture.folder().join("project");
    let saved = crate::workflows::SaveLocation::Project
        .directory(&project)
        .unwrap();
    std::fs::create_dir_all(&saved).unwrap();
    std::fs::write(
        saved.join("proj-wf.js"),
        "export const meta = { name: 'proj-wf', description: 'From the project' }\n\
         return await agent('project agent')\n",
    )
    .unwrap();
    let mut resources = fixture.session.agent_resources();
    resources.project = Some(crate::agents::ProjectFolder {
        folder: project,
        trusted: true,
    });
    fixture.session.set_agent_resources(resources);

    let notice = fixture.workflow_turn("main").await;
    assert_eq!(fixture.seen.count("project agent"), 1);
    assert!(
        notice.contains("<summary>proj-wf (completed)</summary>"),
        "{notice}"
    );
}

const STEPS: &str = "export const meta = { name: 'steps', description: 'Two steps' }\n\
const a = await agent('step one')\n\
const b = await agent('step two')\n\
return [a, b].join(' + ')\n";

#[tokio::test]
async fn a_saved_script_runs_again_from_its_path_and_a_resumed_run_replays_finished_calls() {
    let mut fixture = fixture(Vec::new());
    fixture.answer(vec![
        ("main", workflow_calls("w1", json!({"script": STEPS}))),
        ("step one", vec![Some(text("one"))]),
        ("step two", vec![Some(text("two"))]),
    ]);
    let notice = fixture.workflow_turn("main").await;
    assert!(notice.contains("<result>one + two\n\n"), "{notice}");
    let first = fixture.latest_run();
    assert_eq!(first.snapshot().status, RunStatus::Completed);

    // The same script from the file the first run saved, resumed from it:
    // nothing runs again.
    let saved = first.script_path.clone().unwrap();
    fixture.answer(vec![(
        "main",
        workflow_calls(
            "w2",
            json!({"scriptPath": saved, "resumeFromRunId": first.run_id}),
        ),
    )]);
    let notice = fixture.workflow_turn("again").await;
    let second = fixture.latest_run();
    assert_ne!(second.run_id, first.run_id);
    assert_eq!(second.source.as_ref(), STEPS);
    let result = last_tool_result(&fixture.seen.of("main")[1].context);
    assert_eq!(
        texts(&result),
        [format!(
            "Workflow steps runs in the background as task {}. Its script is saved at {}. It reuses the finished agent calls of run {} up to the first one whose prompt changed.",
            second.run_id,
            second.script_path.as_ref().unwrap().display(),
            first.run_id
        )]
    );
    assert_eq!(
        statuses(&second),
        [WorkflowAgentStatus::Reused, WorkflowAgentStatus::Reused]
    );
    assert_eq!(fixture.seen.count("step one"), 0);
    assert_eq!(fixture.seen.count("step two"), 0);
    assert!(notice.contains("<result>one + two\n\n"), "{notice}");

    // An edited copy replays the calls up to the first changed prompt.
    let edited = fixture.folder().join("edited.js");
    std::fs::write(&edited, STEPS.replace("'step two'", "'step two, revised'")).unwrap();
    fixture.answer(vec![
        (
            "main",
            workflow_calls(
                "w3",
                json!({"scriptPath": "edited.js", "resumeFromRunId": first.run_id}),
            ),
        ),
        ("step two, revised", vec![Some(text("two, revised"))]),
    ]);
    let notice = fixture.workflow_turn("again").await;
    let third = fixture.latest_run();
    assert_eq!(
        statuses(&third),
        [WorkflowAgentStatus::Reused, WorkflowAgentStatus::Completed]
    );
    assert_eq!(fixture.seen.count("step one"), 0);
    assert_eq!(fixture.seen.count("step two, revised"), 1);
    assert!(
        notice.contains("<result>one + two, revised\n\n"),
        "{notice}"
    );
}

#[tokio::test]
async fn a_run_resumes_from_its_journal_after_the_session_is_loaded_again() {
    let mut fixture = fixture(Vec::new());
    fixture.answer(vec![
        ("main", workflow_calls("w1", json!({"script": STEPS}))),
        ("step one", vec![Some(text("one"))]),
        ("step two", vec![Some(text("two"))]),
    ]);
    fixture.workflow_turn("main").await;
    let first = fixture.latest_run();

    // The same session loaded again: its runs are no longer in memory, and
    // its journal on disk is what the resumed run replays.
    let loaded = session_on(
        SessionManager::open(&fixture.session_file()).unwrap(),
        fixture.folder(),
        settings(),
        &fixture.open,
        &fixture.entered,
    );
    assert!(loaded.workflows().unwrap().find(&first.run_id).is_none());
    fixture.session = loaded;
    let mut main = vec![Some(tool_call(
        "w0",
        "Workflow",
        json!({"script": STEPS, "resumeFromRunId": "wf_unknown1"}),
    ))];
    main.extend(workflow_calls(
        "w2",
        json!({
            "scriptPath": first.script_path.as_ref().unwrap(),
            "resumeFromRunId": first.run_id,
        }),
    ));
    fixture.answer(vec![("main", main)]);
    let notice = fixture.workflow_turn("again").await;

    let unknown = last_tool_result(&fixture.seen.of("main")[1].context);
    assert!(unknown.is_error);
    assert_eq!(
        texts(&unknown),
        [
            "No workflow run of this session has the id 'wf_unknown1'; give the id a Workflow call returned."
        ]
    );
    assert_eq!(fixture.seen.count("step one"), 0);
    assert_eq!(fixture.seen.count("step two"), 0);
    assert_eq!(
        statuses(&fixture.latest_run()),
        [WorkflowAgentStatus::Reused, WorkflowAgentStatus::Reused]
    );
    assert!(notice.contains("<result>one + two\n\n"), "{notice}");
}

const THREE_STEPS: &str = "export const meta = { name: 'steps', description: 'Three steps' }\n\
const a = await agent('step one')\n\
const b = await agent('step two')\n\
const c = await agent('step three')\n\
return [a, b, c].join(' + ')\n";

/// The session's stored workflow run states, oldest first.
fn run_states(file: &Path) -> Vec<Value> {
    let manager = SessionManager::open(file).unwrap();
    crate::stored_state::all(&manager, crate::stored_state::WORKFLOW_RUN_ENTRY)
        .into_iter()
        .cloned()
        .collect()
}

#[tokio::test]
async fn a_run_cut_off_by_the_end_of_its_process_is_announced_once_and_resumes_from_its_journal() {
    let mut fixture = fixture(Vec::new()).quiet();
    fixture.answer(vec![
        (
            "main",
            vec![
                Some(tool_call("w1", "Workflow", json!({"script": THREE_STEPS}))),
                Some(text("started it")),
            ],
        ),
        ("step one", vec![Some(text("one"))]),
        ("step two", vec![Some(text("two"))]),
        // The third call is still out when the process ends.
        ("step three", vec![None]),
    ]);
    let session = fixture.session.clone();
    session.prompt(vec![AgentMessage::user("main")]).await;
    let run = fixture.latest_run();
    eventually("the third call is out", || {
        fixture.seen.count("step three") == 1
    })
    .await;
    // Each finished call is on disk as soon as it finished.
    let file = fixture.session_file();
    let journal = workflows_dir(&file).join(format!("{}.journal.jsonl", run.run_id));
    let written = std::fs::read_to_string(&journal).unwrap();
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines.len(), 3, "{written}");
    assert!(lines[1].contains("step one") && lines[2].contains("step two"));
    assert_eq!(run_states(&file).last().unwrap()["status"], "running");

    // The process ends: its host closes the session, which stops the run.
    session.set_closing();
    session.stop_background_tasks();
    eventually("the run ends", || run.snapshot().status.is_finished()).await;
    assert_eq!(
        run_states(&file).last().unwrap()["status"],
        "running",
        "a run the end of its process stopped is not recorded as stopped"
    );
    drop(session);

    // Loaded again, the run is announced once, as stopped.
    let quiet = |session: &Arc<AgentSession>| {
        session.set_turn_starter(Some(Arc::new(|_turn| {})));
    };
    let loaded = session_on(
        SessionManager::open(&file).unwrap(),
        fixture.folder(),
        settings(),
        &fixture.open,
        &fixture.entered,
    );
    quiet(&loaded);
    loaded.resume_stored_work();
    let seen = Mutex::new(Vec::new());
    let notices: Vec<String> = queued(&loaded, &seen)
        .iter()
        .filter_map(notice_text)
        .collect();
    assert_eq!(notices.len(), 1);
    let notice = &notices[0];
    assert!(notice.contains(&format!("<task-id>{}</task-id>", run.run_id)));
    assert!(notice.contains("<status>stopped</status>"));
    assert!(
        notice.contains(
            "<summary>steps (stopped when its process ended, after 2 finished agent calls)</summary>"
        ),
        "{notice}"
    );
    assert!(notice.contains(&format!(
        "<result>{}</result>",
        prompt_todo("Workflow.notice.cut_off")
    )));
    let last = run_states(&file).last().unwrap().clone();
    assert_eq!(last["status"], "stopped");
    assert_eq!(last["cutOff"], true);
    drop(loaded);

    let again = session_on(
        SessionManager::open(&file).unwrap(),
        fixture.folder(),
        settings(),
        &fixture.open,
        &fixture.entered,
    );
    quiet(&again);
    again.resume_stored_work();
    let seen = Mutex::new(Vec::new());
    assert!(
        queued(&again, &seen)
            .iter()
            .all(|message| notice_text(message).is_none()),
        "announced once"
    );

    // Resumed from its journal, only the call that was out runs.
    again.set_turn_starter(None);
    fixture.session = again;
    fixture.answer(vec![
        (
            "main",
            workflow_calls(
                "w2",
                json!({
                    "scriptPath": run.script_path.as_ref().unwrap(),
                    "resumeFromRunId": run.run_id,
                }),
            ),
        ),
        ("step three", vec![Some(text("three"))]),
    ]);
    let notice = fixture.workflow_turn("again").await;
    assert_eq!(fixture.seen.count("step one"), 0);
    assert_eq!(fixture.seen.count("step two"), 0);
    assert_eq!(fixture.seen.count("step three"), 1);
    assert!(notice.contains("<result>one + two + three\n\n"), "{notice}");
    let states = run_states(&file);
    assert_eq!(states.last().unwrap()["status"], "completed");
}

#[tokio::test]
async fn a_workflow_needs_a_script_a_path_or_a_name() {
    let fixture = fixture(vec![(
        "main",
        vec![
            Some(tool_call("w1", "Workflow", json!({}))),
            Some(tool_call(
                "w2",
                "Workflow",
                json!({"scriptPath": "/no/such/script.js"}),
            )),
            Some(text("done")),
        ],
    )]);
    fixture
        .session
        .prompt(vec![AgentMessage::user("main")])
        .await;
    let main = fixture.seen.of("main");
    let nothing = last_tool_result(&main[1].context);
    assert!(nothing.is_error);
    assert_eq!(
        texts(&nothing),
        [
            "Give the script to run, the scriptPath of a saved script, or the name of a saved workflow."
        ]
    );
    let missing = last_tool_result(&main[2].context);
    assert!(missing.is_error);
    assert!(
        texts(&missing)[0].starts_with("Could not read the workflow script /no/such/script.js: "),
        "{:?}",
        texts(&missing)
    );
    assert!(fixture.session.tasks().is_empty());
}

#[tokio::test]
async fn a_workflow_turn_still_says_the_user_asked_for_one() {
    let fixture = fixture(vec![("main", vec![Some(text("ok"))])]);
    fixture
        .session
        .prompt_with_mode(vec![AgentMessage::user("main")], PromptMode::Workflow)
        .await;
    assert!(
        system_prompt(&fixture.seen, 0)
            .ends_with(&format!("\n\n{}", prompt_todo("Workflow.opt_in")))
    );
}

const LABELLED: &str = "export const meta = { name: 'labelled', description: 'Two labelled steps' }\n\
const a = await agent('look one', { label: 'Look' })\n\
const b = await agent('look two')\n\
return [a, b].join(' + ')\n";

#[tokio::test]
async fn a_workflows_agents_are_agents_of_the_session_for_storage_and_clients() {
    use crate::agents::AgentOrigin;
    let mut fixture = fixture(Vec::new());
    fixture.answer(vec![
        ("main", workflow_calls("w1", json!({"script": LABELLED}))),
        ("look one", vec![Some(text("one"))]),
        ("look two", vec![Some(text("two"))]),
    ]);
    // Hosts hear of the agents' sessions, so they mirror them.
    let heard = Arc::new(Mutex::new(Vec::<String>::new()));
    struct Heard(Arc<Mutex<Vec<String>>>);
    impl crate::agents::AgentObserver for Heard {
        fn agent_session(&self, agent_id: &str, _session: &Arc<AgentSession>) {
            self.0.lock().unwrap().push(agent_id.to_string());
        }
    }
    fixture
        .session
        .set_agent_observer(Some(Arc::new(Heard(heard.clone()))));
    fixture.workflow_turn("main").await;
    let run = fixture.latest_run();

    let registry = fixture.session.agent_registry().unwrap();
    let agents = registry.list();
    assert_eq!(agents.len(), 2, "{agents:?}");
    let origin = AgentOrigin::Workflow {
        run_id: run.run_id.clone(),
    };
    for agent in &agents {
        assert!(
            agent.id.starts_with('a') && agent.id.len() == 9,
            "{agent:?}"
        );
        assert_eq!(agent.origin, origin);
        assert_eq!(agent.agent_type, "workflow");
        assert_eq!(agent.status, TaskStatus::Completed);
        assert!(!agent.background);
        // Stored in the session's folder of agents by its id.
        assert_eq!(
            agent.session_file,
            Some(
                crate::session::manager::agents_dir(&fixture.session_file())
                    .join(format!("{}.jsonl", agent.id))
            )
        );
    }
    // Named by the script's label, else by its task name.
    assert_eq!(agents[0].name, "Look");
    assert!(agents[1].name.starts_with("wf_"), "{}", agents[1].name);
    assert_eq!(
        serde_json::to_value(&agents[0]).unwrap()["origin"],
        json!({"kind": "workflow", "runId": run.run_id})
    );
    assert_eq!(
        *heard.lock().unwrap(),
        [agents[0].id.clone(), agents[1].id.clone()]
    );
    // Its transcript is the agent's session.
    let first = fixture.session.agent_at(&agents[0].id).unwrap();
    let messages = first
        .manager
        .lock()
        .unwrap()
        .build_session_context()
        .messages;
    assert!(
        matches!(&messages[0], AgentMessage::User(user) if user.content.as_text() == "look one")
    );

    // The model's ListAgents and SendMessage know the agents it started
    // only.
    let tools = crate::agents::tools::agent_tools(Arc::downgrade(&fixture.session));
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap()
            .clone()
    };
    let listing = tool("ListAgents")
        .execute("l1", json!({}), CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(listing.details["agents"], json!([]));
    let refused = tool("SendMessage")
        .execute(
            "s1",
            json!({"to": agents[0].id, "message": "again"}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(
        format!("{refused:#}"),
        format!(
            "No agent of this session has the id or name '{}'; ListAgents lists them.",
            agents[0].id
        )
    );

    // The session loaded again rebuilds them.
    let loaded = session_on(
        SessionManager::open(&fixture.session_file()).unwrap(),
        fixture.folder(),
        settings(),
        &fixture.open,
        &fixture.entered,
    );
    let rebuilt = loaded.agent_registry().unwrap().list();
    assert_eq!(rebuilt.len(), 2);
    assert_eq!(rebuilt[0].id, agents[0].id);
    assert_eq!(rebuilt[0].name, "Look");
    assert_eq!(rebuilt[0].origin, origin);
    assert_eq!(rebuilt[0].agent_type, "workflow");
    assert_eq!(rebuilt[0].status, TaskStatus::Completed);
    assert_eq!(rebuilt[1].id, agents[1].id);
    let read = loaded.agent_at(&agents[1].id).unwrap();
    assert_eq!(
        read.manager
            .lock()
            .unwrap()
            .build_session_context()
            .messages
            .len(),
        2
    );
}
