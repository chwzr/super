//! The tools of the agent system: `Agent` starts an agent, `SendMessage`
//! writes to one (or, from an agent, to the session that started it), and
//! `ListAgents` lists the agents of the session. `TaskStop` stops a
//! background agent like any other background task.

use super::registry::{AgentRequest, Delivery, Isolation, Started};
use crate::child_turn::{AgentStatus, ChildTurnOutcome};
use crate::notifications::{Priority, agent_message};
use crate::session_runner::AgentSession;
use anyhow::Context as _;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Weak};
use super_agent::placeholder::{tool_description, tool_parameter};
use super_agent::tools::arguments::{boolean, drop_nulls};
use super_agent::{AgentTool, ToolResult, ToolUpdateSink};
use super_ai::ContentBlock;
use tokio_util::sync::CancellationToken;

/// The address of the session that started an agent, for `SendMessage`.
pub const MAIN: &str = "main";

/// How many agents one page of `ListAgents` holds.
pub const PAGE_SIZE: usize = 50;

/// `Agent`, `SendMessage` and `ListAgents`, bound to `session`.
pub(crate) fn agent_tools(session: Weak<AgentSession>) -> Vec<super_agent::DynTool> {
    vec![
        Arc::new(LaunchAgentTool {
            session: session.clone(),
        }),
        Arc::new(SendMessageTool {
            session: session.clone(),
        }),
        Arc::new(ListAgentsTool { session }),
    ]
}

fn session_of(session: &Weak<AgentSession>) -> anyhow::Result<Arc<AgentSession>> {
    session
        .upgrade()
        .context("The session has ended, so it has no agents.")
}

fn parameter(tool: &str, name: &str, schema: Value) -> Value {
    let mut schema = schema;
    schema["description"] = Value::String(tool_parameter(tool, name));
    schema
}

pub struct LaunchAgentTool {
    session: Weak<AgentSession>,
}

#[derive(Deserialize)]
struct AgentArgs {
    description: String,
    prompt: String,
    #[serde(default)]
    subagent_type: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    run_in_background: Option<bool>,
    #[serde(default)]
    isolation: Option<String>,
}

#[async_trait::async_trait]
impl AgentTool for LaunchAgentTool {
    fn name(&self) -> &str {
        "Agent"
    }

    fn advanced_only(&self) -> bool {
        true
    }

    fn description(&self) -> String {
        tool_description("Agent")
    }

    fn parameters(&self) -> Value {
        let p = |name, schema| parameter("Agent", name, schema);
        json!({
            "type": "object",
            "properties": {
                "description": p("description", json!({"type": "string"})),
                "prompt": p("prompt", json!({"type": "string"})),
                "subagent_type": p("subagent_type", json!({"type": "string"})),
                "model": p("model", json!({"type": "string"})),
                "effort": p("effort", json!({"type": "string"})),
                "run_in_background": p("run_in_background", json!({"type": "boolean"})),
                "isolation": p("isolation", json!({"type": "string", "enum": ["worktree", "remote"]})),
            },
            "required": ["description", "prompt"],
            "additionalProperties": false,
        })
    }

    fn prepare_arguments(&self, mut args: Value) -> Value {
        if let Some(object) = args.as_object_mut() {
            drop_nulls(
                object,
                &[
                    "subagent_type",
                    "model",
                    "effort",
                    "run_in_background",
                    "isolation",
                ],
            );
            boolean(object, "run_in_background");
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
        let args: AgentArgs = serde_json::from_value(args)?;
        let session = session_of(&self.session)?;
        let registry = session
            .agent_registry()
            .context("This session cannot start agents.")?;
        let isolation = match args.isolation.as_deref() {
            None => None,
            Some("worktree") => Some(Isolation::Worktree),
            Some("remote") => Some(Isolation::Remote),
            Some(other) => anyhow::bail!(
                "isolation '{other}' is not one of worktree and remote; leave it out to run the agent in this folder."
            ),
        };
        let request = AgentRequest {
            description: args.description,
            prompt: args.prompt,
            agent_type: args.subagent_type,
            model: args.model,
            effort: args.effort,
            background: args.run_in_background.unwrap_or(true),
            isolation,
        };
        match registry.start(request, tool_call_id, cancel).await? {
            Started::Background(agent) => {
                let mut text = format!("Agent {} runs in the background.", agent.id);
                if let Some(file) = &agent.session_file {
                    text.push_str(&format!(
                        " Its transcript is written to {}.",
                        file.display()
                    ));
                }
                Ok(ToolResult {
                    content: vec![ContentBlock::text(text)],
                    details: json!({"agent": agent}),
                    ..Default::default()
                })
            }
            Started::Finished(agent, outcome) => Ok(finished_result(
                &agent.id,
                agent_details(&agent, &outcome),
                &outcome,
            )),
        }
    }
}

fn agent_details(agent: &super::AgentInfo, outcome: &ChildTurnOutcome) -> Value {
    json!({
        "agent": agent,
        "usage": {
            "subagentTokens": outcome.usage.total_tokens,
            "toolUses": outcome.tool_uses,
            "durationMs": u64::try_from(outcome.duration.as_millis()).unwrap_or(u64::MAX),
        },
    })
}

/// The result of an agent that ran in the caller's turn: its report, then
/// its id and what it used.
fn finished_result(id: &str, details: Value, outcome: &ChildTurnOutcome) -> ToolResult {
    let usage = format!(
        "<agent-result>\n<agent-id>{id}</agent-id>\n<usage><subagent_tokens>{}</subagent_tokens><tool_uses>{}</tool_uses><duration_ms>{}</duration_ms></usage>\n</agent-result>",
        details["usage"]["subagentTokens"],
        details["usage"]["toolUses"],
        details["usage"]["durationMs"],
    );
    let (report, is_error) = match outcome.status {
        AgentStatus::Completed => (outcome.result.clone().unwrap_or_default(), false),
        AgentStatus::Failed => (
            format!(
                "Agent {id} failed: {}",
                outcome.error.as_deref().unwrap_or("the agent failed")
            ),
            true,
        ),
        AgentStatus::Interrupted => (format!("Agent {id} was stopped."), true),
    };
    ToolResult {
        content: vec![ContentBlock::text(report), ContentBlock::text(usage)],
        details,
        is_error,
        ..Default::default()
    }
}

pub struct SendMessageTool {
    session: Weak<AgentSession>,
}

#[derive(Deserialize)]
struct SendArgs {
    to: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    notify_when_idle: Option<bool>,
}

#[async_trait::async_trait]
impl AgentTool for SendMessageTool {
    fn name(&self) -> &str {
        "SendMessage"
    }

    fn advanced_only(&self) -> bool {
        true
    }

    fn description(&self) -> String {
        tool_description("SendMessage")
    }

    fn parameters(&self) -> Value {
        let p = |name, schema| parameter("SendMessage", name, schema);
        json!({
            "type": "object",
            "properties": {
                "to": p("to", json!({
                    "type": "string",
                    "allOf": [
                        {"pattern": "^[^\\n\\r]*$"},
                        {"pattern": "^[\\s\\S]{0,300}$"},
                    ],
                })),
                "summary": p("summary", json!({"type": "string", "maxLength": 200})),
                "message": p("message", json!({"type": "string", "default": ""})),
                "notify_when_idle": p("notify_when_idle", json!({"type": "boolean"})),
            },
            "required": ["to", "message"],
            "additionalProperties": false,
        })
    }

    fn prepare_arguments(&self, mut args: Value) -> Value {
        if let Some(object) = args.as_object_mut() {
            drop_nulls(object, &["summary", "notify_when_idle"]);
            boolean(object, "notify_when_idle");
        }
        args
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        args: Value,
        _cancel: CancellationToken,
        _on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        let args: SendArgs = serde_json::from_value(args)?;
        if args.notify_when_idle == Some(true) {
            anyhow::bail!("notify_when_idle is not available; leave it out.");
        }
        let to = args.to.trim();
        if to.is_empty() {
            anyhow::bail!("to is empty; give the id or name of an agent, or main.");
        }
        if args.message.trim().is_empty() {
            anyhow::bail!("message is empty; give the text to send.");
        }
        let summary = args
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|summary| !summary.is_empty());
        let session = session_of(&self.session)?;

        if to == MAIN {
            let link = session
                .agent_link()
                .context("This is the main session; give the id or name of one of its agents.")?;
            let parent = link.parent.upgrade().context(
                "The session that started this agent has ended, so the message cannot reach it.",
            )?;
            parent.queue_entry(
                agent_message(&link.id, MAIN, summary, &args.message),
                Priority::Next,
            );
            return Ok(ToolResult {
                content: vec![ContentBlock::text("Sent to main.")],
                details: json!({"to": MAIN, "from": link.id}),
                ..Default::default()
            });
        }

        let registry = session
            .agent_registry()
            .context("This agent can send messages to main only.")?;
        let (text, agent, resumed) =
            match registry.send(to, summary, &args.message, tool_call_id)? {
                Delivery::Queued(agent) => (
                    format!(
                        "Agent {} runs; the message reaches it at its next step.",
                        agent.id
                    ),
                    agent,
                    false,
                ),
                Delivery::Resumed(agent) => (
                    format!(
                        "Agent {} had ended; the message resumes it in the background.",
                        agent.id
                    ),
                    agent,
                    true,
                ),
            };
        Ok(ToolResult {
            content: vec![ContentBlock::text(text)],
            details: json!({"to": agent.id, "agent": agent, "resumed": resumed}),
            ..Default::default()
        })
    }
}

pub struct ListAgentsTool {
    session: Weak<AgentSession>,
}

#[async_trait::async_trait]
impl AgentTool for ListAgentsTool {
    fn name(&self) -> &str {
        "ListAgents"
    }

    fn advanced_only(&self) -> bool {
        true
    }

    fn description(&self) -> String {
        tool_description("ListAgents")
    }

    fn parameters(&self) -> Value {
        let p = |name, schema| parameter("ListAgents", name, schema);
        json!({
            "type": "object",
            "properties": {
                "channel": p("channel", json!({"type": "string", "maxLength": 256})),
                "q": p("q", json!({"type": "string", "maxLength": 256})),
                "cursor": p("cursor", json!({"type": "string", "maxLength": 256})),
            },
            "additionalProperties": false,
        })
    }

    fn prepare_arguments(&self, mut args: Value) -> Value {
        if let Some(object) = args.as_object_mut() {
            drop_nulls(object, &["channel", "q", "cursor"]);
        }
        args
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        args: Value,
        _cancel: CancellationToken,
        _on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        // `channel` and `q` are accepted and not used: the list is this
        // session's agents.
        let session = session_of(&self.session)?;
        let registry = session
            .agent_registry()
            .context("This session has no agents to list.")?;
        let start = match args["cursor"].as_str().map(str::trim) {
            None | Some("") => 0,
            Some(cursor) => cursor.parse::<usize>().ok().with_context(|| {
                format!("cursor '{cursor}' is not one ListAgents gave; leave it out to list from the start.")
            })?,
        };
        let agents = registry.list();
        let page: Vec<Value> = agents
            .iter()
            .skip(start)
            .take(PAGE_SIZE)
            .map(|agent| {
                json!({
                    "name": agent.name,
                    "id": agent.id,
                    "type": agent.agent_type,
                    "status": agent.status,
                })
            })
            .collect();
        let mut listing = json!({"agents": page});
        let next = start.saturating_add(PAGE_SIZE);
        if next < agents.len() {
            listing["nextCursor"] = Value::String(next.to_string());
        }
        Ok(ToolResult {
            content: vec![ContentBlock::text(serde_json::to_string_pretty(&listing)?)],
            details: listing,
            ..Default::default()
        })
    }
}
