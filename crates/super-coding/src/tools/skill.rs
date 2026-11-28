//! Skill tool of the advanced mode: the model loads a skill by its name, and
//! the skill's text comes back as the tool's result. A skill whose
//! frontmatter asks for it ([`crate::skills::SkillRun`]) runs in an agent of
//! its own instead, on the agent system of the `Agent` tool: waited for, its
//! report is the result; in the background, the call answers with the
//! agent's name and id, and a notice announces its report when it ends.
//!
//! The skills are the session's ([`crate::agents::AgentResources::skills`],
//! from `.super/skills`, `~/.super/agent/skills` and `.agents/skills`) and
//! the built-in ones: `workflow-authoring`, the guide to writing a workflow
//! script, while the session offers the `Workflow` tool. A built-in skill
//! hides a skill file of the same name.

use crate::agents::registry::{AgentRequest, Started};
use crate::agents::tools::{agent_details, finished_result};
use crate::session_runner::AgentSession;
use crate::skills::{self, Skill, SkillRun, SkillSummary};
use anyhow::Context as _;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Weak;
use super_agent::placeholder::{prompt_todo, tool_description, tool_parameter};
use super_agent::tools::arguments::drop_nulls;
use super_agent::{AgentTool, ToolResult, ToolUpdateSink};
use super_ai::ContentBlock;
use tokio_util::sync::CancellationToken;

/// The built-in skill with the guide to writing a workflow script.
pub const WORKFLOW_AUTHORING: &str = "workflow-authoring";

/// The names of the built-in skills `session` offers now.
fn built_in_names(session: &AgentSession) -> Vec<&'static str> {
    let mut names = Vec::new();
    if session.workflows_enabled() && session.workflows().is_some() {
        names.push(WORKFLOW_AUTHORING);
    }
    names
}

/// The text of the built-in skill `name`, while `session` offers it.
fn built_in_content(session: &AgentSession, name: &str) -> Option<String> {
    if !built_in_names(session).contains(&name) {
        return None;
    }
    match name {
        WORKFLOW_AUTHORING => {
            let limits = session.workflows()?.limits();
            let size = session.settings().workflows.size;
            Some(crate::workflows::authoring_prompt(
                size,
                limits.max_agents,
                limits.max_fanout,
            ))
        }
        _ => None,
    }
}

/// Every skill the model may load in `session`, as the list shows it: the
/// built-in ones first, then the session's skill files, without those only
/// the user may start and those a built-in one hides.
pub(crate) fn summaries(session: &AgentSession) -> Vec<SkillSummary> {
    let built_in = built_in_names(session);
    let mut out: Vec<SkillSummary> = built_in
        .iter()
        .map(|name| SkillSummary {
            name: name.to_string(),
            description: prompt_todo(&format!("Skill.{name}.description")),
        })
        .collect();
    out.extend(
        session
            .agent_resources()
            .skills
            .into_iter()
            .filter(|skill| {
                !skill.disable_model_invocation && !built_in.contains(&skill.name.as_str())
            })
            .map(|skill| SkillSummary {
                name: skill.name,
                description: skill.description,
            }),
    );
    out
}

pub struct SkillTool {
    session: Weak<AgentSession>,
}

impl SkillTool {
    pub(crate) fn new(session: Weak<AgentSession>) -> Self {
        SkillTool { session }
    }
}

#[derive(Deserialize)]
struct SkillArgs {
    skill: String,
    #[serde(default)]
    args: Option<String>,
}

#[async_trait::async_trait]
impl AgentTool for SkillTool {
    fn name(&self) -> &str {
        "Skill"
    }

    fn advanced_only(&self) -> bool {
        true
    }

    fn description(&self) -> String {
        tool_description("Skill")
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": tool_parameter("Skill", "skill"),
                },
                "args": {
                    "type": "string",
                    "description": tool_parameter("Skill", "args"),
                },
            },
            "required": ["skill"],
            "additionalProperties": false,
        })
    }

    fn prepare_arguments(&self, mut args: Value) -> Value {
        if let Some(object) = args.as_object_mut() {
            drop_nulls(object, &["args"]);
        }
        args
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        args: Value,
        cancel: CancellationToken,
        _on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        let args: SkillArgs = serde_json::from_value(args)?;
        let session = self
            .session
            .upgrade()
            .context("The session has ended, so it cannot load a skill.")?;
        // A name as the user would type it, `/review` or `$review`, is the
        // skill `review`.
        let name = args
            .skill
            .trim()
            .trim_start_matches(['/', '$'])
            .trim()
            .to_string();
        if name.is_empty() {
            anyhow::bail!("skill is empty; give the name of a skill from the list of skills.");
        }
        let arguments = args.args.as_deref();

        if let Some(content) = built_in_content(&session, &name) {
            return Ok(ToolResult {
                content: vec![ContentBlock::text(skills::skill_text(
                    "Skill.loaded",
                    &name,
                    None,
                    &content,
                    arguments,
                ))],
                details: json!({"skill": name, "builtIn": true}),
                ..Default::default()
            });
        }

        let skill = session
            .agent_resources()
            .skills
            .into_iter()
            .find(|skill| skill.name == name)
            .ok_or_else(|| unknown_skill(&name, &summaries(&session)))?;
        if skill.disable_model_invocation {
            anyhow::bail!(
                "The skill {name} is one the user starts with /{name}; it cannot be loaded with Skill."
            );
        }
        let loaded = skills::load(&skill)?;
        match loaded.run {
            None => Ok(ToolResult {
                content: vec![ContentBlock::text(skills::skill_text(
                    "Skill.loaded",
                    &name,
                    Some(&skill.file_path),
                    &loaded.body,
                    arguments,
                ))],
                details: json!({"skill": name, "path": skill.file_path}),
                ..Default::default()
            }),
            Some(run) => {
                run_in_agent(
                    &session,
                    &skill,
                    &loaded.body,
                    arguments,
                    run,
                    tool_call_id,
                    cancel,
                )
                .await
            }
        }
    }
}

fn unknown_skill(name: &str, known: &[SkillSummary]) -> anyhow::Error {
    if known.is_empty() {
        return anyhow::anyhow!("No skill is named '{name}', and this session has no skills.");
    }
    let names: Vec<&str> = known.iter().map(|skill| skill.name.as_str()).collect();
    anyhow::anyhow!(
        "No skill is named '{name}'; the skills are {}.",
        names.join(", ")
    )
}

/// Run `skill` in an agent of its own, named after the skill: waited for
/// within the caller's turn, or in the background when its frontmatter says
/// so.
async fn run_in_agent(
    session: &AgentSession,
    skill: &Skill,
    body: &str,
    arguments: Option<&str>,
    run: SkillRun,
    tool_call_id: &str,
    cancel: CancellationToken,
) -> anyhow::Result<ToolResult> {
    let name = skill.name.as_str();
    let registry = session
        .agent_registry()
        .filter(|_| session.subagents_enabled())
        .with_context(|| {
            format!(
                "The skill {name} runs in an agent of its own, and this session cannot start agents."
            )
        })?;
    let request = AgentRequest {
        description: name.to_string(),
        prompt: skills::skill_text(
            "Skill.agent_prompt",
            name,
            Some(&skill.file_path),
            body,
            arguments,
        ),
        agent_type: run.agent,
        model: run.model,
        effort: run.effort,
        background: run.background,
        isolation: None,
    };
    match registry.start(request, tool_call_id, cancel).await? {
        Started::Background(agent) => {
            let mut text = format!(
                "Skill {name} runs in the background as the agent {} ({}).",
                agent.name, agent.id
            );
            if let Some(file) = &agent.session_file {
                text.push_str(&format!(
                    " Its transcript is written to {}.",
                    file.display()
                ));
            }
            Ok(ToolResult {
                content: vec![ContentBlock::text(text)],
                details: json!({"skill": name, "agent": agent}),
                ..Default::default()
            })
        }
        Started::Finished(agent, outcome) => {
            let mut details = agent_details(&agent, &outcome);
            details["skill"] = Value::String(name.to_string());
            Ok(finished_result(&agent.id, details, &outcome))
        }
    }
}
