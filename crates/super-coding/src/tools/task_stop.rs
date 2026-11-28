//! TaskStop tool: stop a background task of the session by id or by name: a
//! shell command, a monitor, an agent or a workflow run. Jobs of `/loop` and
//! `/autoresearch` are not background tasks, and the user manages them.

use crate::session_runner::AgentSession;
use anyhow::Context as _;
use serde_json::{Value, json};
use std::sync::Weak;
use super_agent::placeholder;
use super_agent::{AgentTool, ExecutionMode, ToolResult, ToolUpdateSink};
use tokio_util::sync::CancellationToken;

/// How long `TaskStop` waits for a stopped agent's run to close.
const AGENT_STOP_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

pub struct TaskStopTool {
    session: Weak<AgentSession>,
}

impl TaskStopTool {
    pub(crate) fn new(session: Weak<AgentSession>) -> Self {
        TaskStopTool { session }
    }
}

#[async_trait::async_trait]
impl AgentTool for TaskStopTool {
    fn name(&self) -> &str {
        "TaskStop"
    }

    fn advanced_only(&self) -> bool {
        true
    }

    /// Alone in its place among the calls of one answer, so that it finds
    /// the task a call before it started.
    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Sequential
    }

    fn description(&self) -> String {
        placeholder::tool_description("TaskStop")
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": placeholder::tool_parameter("TaskStop", "task_id"),
                },
                "shell_id": {
                    "type": "string",
                    "description": placeholder::tool_parameter("TaskStop", "shell_id"),
                },
            },
            "additionalProperties": false,
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        args: Value,
        cancel: CancellationToken,
        _on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        // `shell_id` is the old name of `task_id`.
        let target = ["task_id", "shell_id"]
            .iter()
            .filter_map(|name| args[*name].as_str())
            .map(str::trim)
            .find(|target| !target.is_empty())
            .context("Give the task_id of the background task to stop.")?;
        let session = self
            .session
            .upgrade()
            .context("The session has ended, so it has no background tasks to stop.")?;
        let task = session
            .stop_task(target)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        // A stopped agent's run closes a moment later: wait for it, so that
        // the agent is stopped when the result says so and a message sent
        // after this call resumes it.
        if task.kind == crate::tasks::TaskKind::Agent
            && let Some(registry) = session.agent_registry()
        {
            registry
                .wait_until_stopped(&task.id, AGENT_STOP_LIMIT, &cancel)
                .await;
        }
        Ok(ToolResult {
            content: vec![super_ai::ContentBlock::text(format!(
                "Stopped the {} task {} ({}).",
                task.kind.as_str(),
                task.id,
                task.description
            ))],
            details: json!({"task": task}),
            ..Default::default()
        })
    }
}
