//! `Skill`, driven through sessions with a scripted provider that answers
//! each conversation from its own lane (see `agent_tools_tests`): the main
//! session's lane is `main`, an agent's is the first text it was given.

use crate::AgentSession;
use crate::agent_tools_tests::{
    Seen, core_tools, lanes, last_tool_result, notice_header, settings, texts, user_text,
};
use crate::background_tests::{eventually, model, text, tool_call};
use crate::session::manager::SessionManager;
use crate::settings::{Mode, Settings};
use crate::skills::{Skill, skill_text};
use crate::tasks::{TaskKind, TaskStatus};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
