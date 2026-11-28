//! The `Workflow` tool.
//!
//! The tool starts a run in the background and returns at once with the
//! run's id. The run is a background task of the session with that id, so
//! `TaskStop` stops it, and its end is announced with a notice that carries
//! the workflow's answer. The script comes from the call (`script`), from a
//! file (`scriptPath`, such as the copy every run saves) or from a saved
//! workflow (`name`); `resumeFromRunId` replays the finished agent calls of
//! an earlier run up to the first one whose prompt changed.

use super::{ApprovalDecision, RunRecord, WorkflowPlan, WorkflowRuntime};
use crate::session_runner::WorkflowTurnStatus;
use crate::tasks::{TaskHandle, TaskKind, TaskOutcome, TaskSpec};
use anyhow::Context as _;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use super_agent::placeholder::{tool_description, tool_parameter};
use super_agent::tools::arguments::drop_nulls;
use super_agent::{AgentTool, ExecutionMode, ToolResult, ToolUpdateSink};
use super_workflow::{Journal, Script};
use tokio_util::sync::CancellationToken;

/// The longest script a call may carry, in bytes.
pub const MAX_SCRIPT_BYTES: usize = 524_288;

pub(crate) struct WorkflowTool(Arc<WorkflowRuntime>);

impl WorkflowTool {
    pub(crate) fn new(runtime: Arc<WorkflowRuntime>) -> WorkflowTool {
        WorkflowTool(runtime)
    }
}

/// `description` and `title` are accepted and not used.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowArgs {
    #[serde(default)]
    script: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    args: Option<Value>,
    #[serde(default)]
    script_path: Option<String>,
    #[serde(default)]
    resume_from_run_id: Option<String>,
}

fn parameter(name: &str, schema: Value) -> Value {
    let mut schema = schema;
    schema["description"] = Value::String(tool_parameter("Workflow", name));
    schema
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[async_trait::async_trait]
impl AgentTool for WorkflowTool {
    fn name(&self) -> &str {
        "Workflow"
    }

    fn advanced_only(&self) -> bool {
        true
    }

    fn description(&self) -> String {
        tool_description("Workflow")
    }

    /// A workflow can start hundreds of agents, and its start asks the user
    /// first. Starting it on its own, rather than beside other tool calls
    /// from the same turn, keeps one question on screen at a time.
    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Sequential
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "script": parameter("script", json!({"type": "string", "maxLength": MAX_SCRIPT_BYTES})),
                "name": parameter("name", json!({"type": "string"})),
                "description": parameter("description", json!({"type": "string"})),
                "title": parameter("title", json!({"type": "string"})),
                "args": parameter("args", json!({})),
                "scriptPath": parameter("scriptPath", json!({"type": "string"})),
                "resumeFromRunId": parameter(
                    "resumeFromRunId",
                    json!({"type": "string", "pattern": "^wf_[a-z0-9-]{6,}$"}),
                ),
            },
            "additionalProperties": false,
        })
    }

    fn prepare_arguments(&self, mut args: Value) -> Value {
        if let Some(object) = args.as_object_mut() {
            drop_nulls(
                object,
                &[
                    "script",
                    "name",
                    "description",
                    "title",
                    "args",
                    "scriptPath",
                    "resumeFromRunId",
                ],
            );
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
        let args: WorkflowArgs = serde_json::from_value(args)?;
        let runtime = &self.0;
        let session = runtime
            .parent()
            .context("The session has ended, so it cannot run a workflow.")?;
        if !session.workflows_enabled() {
            anyhow::bail!(
                "Dynamic workflows are off in this session, so no workflow can run here."
            );
        }
        let cwd = session.working_folder();
        let label = non_empty(args.name.as_deref());

        let source = if let Some(script) = args.script.filter(|script| !script.trim().is_empty()) {
            script
        } else if let Some(path) = non_empty(args.script_path.as_deref()) {
            read_script(&absolute(&cwd, path))?
        } else if let Some(name) = label {
            let trusted = crate::trust::resolve_non_interactive(
                &cwd,
                None,
                session.settings().default_project_trust,
            );
            let saved = super::discover(&cwd, trusted);
            let found = saved
                .iter()
                .find(|workflow| workflow.name == name)
                .ok_or_else(|| unknown_saved(name, &saved))?;
            read_script(&found.path)?
        } else {
            anyhow::bail!(
                "Give the script to run, the scriptPath of a saved script, or the name of a saved workflow."
            );
        };

        // A script that does not parse comes back as a tool error carrying the
        // rendered diagnostic, so the model can correct it and call again.
        let script = Script::parse(&source)
            .map_err(|diagnostic| anyhow::anyhow!("{}", diagnostic.render(&source)))?;

        let resumed = non_empty(args.resume_from_run_id.as_deref());
        let journal = match resumed {
            Some(run_id) => runtime.journal_for(run_id)?,
            None => Journal::default(),
        };

        let mut plan = WorkflowPlan::from_script(&script);
        if let Some(label) = label {
            plan.name = label.to_string();
        }
        let workflow_name = plan.name.clone();

        if runtime.approve(plan).await == ApprovalDecision::Cancel {
            runtime.emit_outcome(None, workflow_name, WorkflowTurnStatus::Cancelled);
            return Ok(ToolResult::text(
                "The user did not approve this workflow, so nothing ran. Ask what they would \
                 like to change, or carry out the task directly instead.",
            ));
        }

        let record = runtime.prepare(script, args.args.unwrap_or(Value::Null), journal)?;
        let handle = match register(&session, &record, tool_call_id) {
            Ok(handle) => handle,
            Err(error) => {
                // Settle the prepared run so that nothing waits for it.
                record.stop();
                let _ = runtime.run(&record).await;
                return Err(error);
            }
        };
        spawn_run(
            runtime.clone(),
            record.clone(),
            workflow_name.clone(),
            handle,
        );

        let mut text = format!(
            "Workflow {workflow_name} runs in the background as task {}.",
            record.run_id
        );
        if let Some(path) = &record.script_path {
            text.push_str(&format!(" Its script is saved at {}.", path.display()));
        }
        if let Some(run_id) = resumed {
            text.push_str(&format!(
                " It reuses the finished agent calls of run {run_id} up to the first one whose prompt changed."
            ));
        }
        Ok(ToolResult {
            content: vec![super_ai::ContentBlock::text(text)],
            details: json!({
                "runId": record.run_id,
                "name": workflow_name,
                "scriptPath": record.script_path,
                "resumedFrom": resumed,
            }),
            ..Default::default()
        })
    }
}

/// `path` as given when it is absolute, else in the session folder.
fn absolute(cwd: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn read_script(path: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(path).map_err(|error| {
        anyhow::anyhow!(
            "Could not read the workflow script {}: {error}.",
            path.display()
        )
    })
}

fn unknown_saved(name: &str, saved: &[super::SavedWorkflow]) -> anyhow::Error {
    if saved.is_empty() {
        return anyhow::anyhow!(
            "No saved workflow is named '{name}', and none is saved in .super/workflows or ~/.super/agent/workflows."
        );
    }
    let names: Vec<&str> = saved
        .iter()
        .map(|workflow| workflow.name.as_str())
        .collect();
    anyhow::anyhow!(
        "No saved workflow is named '{name}'; the saved workflows are {}.",
        names.join(", ")
    )
}

/// Register the run as the session's background task with the run's id,
/// named after the workflow while no other running task has that name.
fn register(
    session: &crate::AgentSession,
    record: &RunRecord,
    tool_use_id: &str,
) -> anyhow::Result<TaskHandle> {
    let description = if record.description.trim().is_empty() {
        record.name.clone()
    } else {
        record.description.clone()
    };
    let mut spec = TaskSpec::new(TaskKind::Workflow, description).with_id(&record.run_id);
    if !tool_use_id.is_empty() {
        spec = spec.with_tool_use_id(tool_use_id);
    }
    let tasks = session.background_tasks();
    if !record.name.trim().is_empty()
        && let Ok(handle) = tasks.register(spec.clone().with_name(&record.name))
    {
        return Ok(handle);
    }
    tasks
        .register(spec)
        .map_err(|error| anyhow::anyhow!("{error}"))
}

/// Run `record` in the background. Its task's stop signal stops the run;
/// its end is the task's end, announced with a notice unless the task was
/// stopped through the registry.
fn spawn_run(
    runtime: Arc<WorkflowRuntime>,
    record: Arc<RunRecord>,
    workflow_name: String,
    handle: TaskHandle,
) {
    tokio::spawn(async move {
        let stop = handle.stop_signal();
        let stopping = record.clone();
        let guard = tokio::spawn(async move {
            stop.cancelled().await;
            stopping.stop();
        });
        let outcome = runtime.run(&record).await;
        guard.abort();

        let snapshot = record.snapshot();
        let summary = format!(
            "{} agents across {} phases, {} tokens",
            snapshot.total_agents(),
            snapshot
                .phases
                .iter()
                .filter(|phase| !phase.agents.is_empty())
                .count(),
            snapshot.tokens,
        );
        let (status, ended) = match outcome {
            Ok(Value::Null) if snapshot.status == super_workflow::RunStatus::Stopped => (
                WorkflowTurnStatus::Stopped,
                TaskOutcome::stopped(format!("{workflow_name} (stopped)")).with_result(format!(
                    "The user stopped this workflow after {summary}. Its finished agent calls are \
                     kept: a Workflow call with resumeFromRunId {} replays them, up to the first \
                     one whose prompt changed.",
                    record.run_id
                )),
            ),
            Ok(value) => {
                let report = match value {
                    Value::String(text) => text,
                    Value::Null => "The workflow returned nothing.".to_string(),
                    other => {
                        serde_json::to_string_pretty(&other).unwrap_or_else(|_| other.to_string())
                    }
                };
                (
                    WorkflowTurnStatus::Completed,
                    TaskOutcome::completed(format!("{workflow_name} (completed)"))
                        .with_result(format!("{report}\n\n[workflow: {summary}]")),
                )
            }
            Err(error) => (
                WorkflowTurnStatus::Failed,
                TaskOutcome::failed(format!("{workflow_name} (failed)"))
                    .with_result(format!("The workflow failed after {summary}.\n{error}")),
            ),
        };
        runtime.emit_outcome(Some(record.id), record.name.clone(), status);
        handle.finish(ended);
    });
}
