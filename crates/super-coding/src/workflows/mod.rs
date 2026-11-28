//! Dynamic workflows: many child agents orchestrated by a script the model
//! writes.
//!
//! A workflow is built on the same child sessions the `Agent` tool uses, so it
//! is available only when subagents are on. The difference is where the plan
//! lives: with `Agent` the model decides what to start next on every turn,
//! and every result lands in its context. With a workflow the plan is a
//! script, and only the final answer comes back.
//!
//! Every run has an id (`wf_` and eight letters or digits). Its script is
//! saved in the session's folder of workflows
//! ([`workflows_dir`](crate::session::manager::workflows_dir)) as
//! `<run id>.js`, and when it ends its journal of finished agent calls as
//! `<run id>.journal.json`, so that a later run can start from the file
//! (`scriptPath`) and replay the calls that still match (`resumeFromRunId`).

mod journal;
mod prompt;
mod runner;
mod store;
mod tool;

pub use prompt::{authoring_prompt, workflow_trigger};
pub use store::{SaveLocation, SavedWorkflow, discover, save};

// Re-exported so the terminal renders workflow state without depending on
// `super-workflow` directly.
pub use super_workflow::{
    AgentId, AgentOutcome, AgentSnapshot, AgentStatus, Journal, PhaseSnapshot, RunSnapshot,
    RunStatus, Script,
};

use crate::session_runner::{AgentSession, WorkflowTurnStatus};
use anyhow::Context as _;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use super_agent::DynTool;
use super_workflow::{Limits, Workflow};

/// Identifies one run within a session.
pub type RunId = u64;

/// How many finished runs to keep, so `/workflows` can still show and save a
/// run the user is only now getting back to.
const MAX_RETAINED_RUNS: usize = 20;

/// One run, with the script that produced it.
pub struct RunRecord {
    pub id: RunId,
    /// The id the model and the task registry know the run by: `wf_` and
    /// eight letters or digits. A run the `Workflow` tool starts is the
    /// background task with this id.
    pub run_id: String,
    pub name: String,
    pub description: String,
    /// The script text, kept so the user can read it and save it afterward.
    pub source: Arc<str>,
    /// Where the script was saved, for a later `scriptPath`; `None` when it
    /// could not be written.
    pub script_path: Option<PathBuf>,
    /// The session's folder of workflows when the run started: its journal
    /// goes there when it ends.
    directory: Option<PathBuf>,
    workflow: Arc<Workflow>,
}

impl RunRecord {
    pub fn snapshot(&self) -> Arc<RunSnapshot> {
        self.workflow.snapshot()
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.workflow.subscribe()
    }

    pub fn pause(&self) {
        self.workflow.pause();
    }

    pub fn resume(&self) {
        self.workflow.resume();
    }

    pub fn is_paused(&self) -> bool {
        self.workflow.is_paused()
    }

    pub fn stop(&self) {
        self.workflow.stop();
    }

    pub fn stop_agent(&self, agent: super_workflow::AgentId) {
        self.workflow.stop_agent(agent);
    }

    pub fn restart_agent(&self, agent: super_workflow::AgentId) {
        self.workflow.restart_agent(agent);
    }

    /// Results worth reusing if this run is started again.
    pub fn journal(&self) -> Journal {
        self.workflow.journal()
    }
}

/// A run as listed in `/workflows`.
#[derive(Debug, Clone)]
pub struct RunSummary {
    pub id: RunId,
    pub name: String,
    pub description: String,
    pub status: RunStatus,
    pub total_agents: usize,
    pub finished_agents: usize,
    pub tokens: u64,
    pub elapsed: Duration,
}

/// What the user decided when asked to approve a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Cancel,
}

/// What the user is shown before a run starts.
#[derive(Debug, Clone)]
pub struct WorkflowPlan {
    pub name: String,
    pub description: String,
    pub phases: Vec<String>,
    /// `None` when the agent count depends on data the run has not fetched yet.
    pub estimated_agents: Option<u32>,
    pub source: Arc<str>,
}

impl WorkflowPlan {
    pub fn from_script(script: &Script) -> WorkflowPlan {
        WorkflowPlan {
            name: script.meta().name.clone(),
            description: script.meta().description.clone(),
            phases: script.declared_phases().to_vec(),
            estimated_agents: script.estimated_agents(),
            source: Arc::from(script.source()),
        }
    }

    /// The agent count as shown to the user.
    ///
    /// A count is only stated when the script fixes it. Otherwise this says so,
    /// because a number in an approval prompt has to be one the user can rely
    /// on.
    pub fn agent_estimate(&self) -> String {
        match self.estimated_agents {
            Some(1) => "1 agent".into(),
            Some(count) => format!("{count} agents"),
            None => "an unbounded number of agents".into(),
        }
    }
}

/// Asks the user to approve a run. Interactive mode installs one. Other modes
/// leave it unset and runs start without asking, since nothing can answer.
pub type WorkflowApprover = Arc<
    dyn Fn(WorkflowPlan) -> futures::future::BoxFuture<'static, ApprovalDecision> + Send + Sync,
>;

/// The workflow runs belonging to one session.
pub struct WorkflowRuntime {
    parent: Weak<AgentSession>,
    runs: Mutex<Vec<Arc<RunRecord>>>,
    next_id: AtomicU64,
}

impl WorkflowRuntime {
    pub(crate) fn new(parent: Weak<AgentSession>) -> Arc<WorkflowRuntime> {
        Arc::new(WorkflowRuntime {
            parent,
            runs: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        })
    }

    /// The tool the model calls to run a workflow.
    pub(crate) fn tool(self: &Arc<Self>) -> DynTool {
        Arc::new(tool::WorkflowTool::new(self.clone()))
    }

    pub(crate) fn parent(&self) -> Option<Arc<AgentSession>> {
        self.parent.upgrade()
    }

    /// The folder that keeps the session's workflow scripts and journals:
    /// the folder of workflows of its session file, or for a session in
    /// memory `<temp>/super-workflows/<session id>`.
    pub fn directory(&self) -> Option<PathBuf> {
        let parent = self.parent.upgrade()?;
        let manager = parent.manager.lock().ok()?;
        Some(match manager.session_file() {
            Some(file) => crate::session::manager::workflows_dir(file),
            None => std::env::temp_dir()
                .join("super-workflows")
                .join(manager.session_id()),
        })
    }

    /// A run id that no run of this session and no saved script has.
    fn new_run_id(&self, directory: Option<&Path>) -> String {
        const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        use rand::Rng as _;
        let mut rng = rand::rng();
        loop {
            let mut id = String::from("wf_");
            for _ in 0..8 {
                id.push(ALPHABET[rng.random_range(0..ALPHABET.len())] as char);
            }
            let known = self.find(&id).is_some()
                || directory.is_some_and(|directory| directory.join(format!("{id}.js")).exists());
            if !known {
                return id;
            }
        }
    }

    /// The run with the id `run_id`, while the session remembers it.
    pub fn find(&self, run_id: &str) -> Option<Arc<RunRecord>> {
        self.runs
            .lock()
            .ok()?
            .iter()
            .find(|run| run.run_id == run_id)
            .cloned()
    }

    /// The finished agent calls of the run `run_id`, for a run that resumes
    /// it: from the run itself while the session remembers it, else from its
    /// journal in the session's folder of workflows. A run that still works
    /// cannot be resumed yet.
    pub fn journal_for(&self, run_id: &str) -> anyhow::Result<Journal> {
        if let Some(record) = self.find(run_id) {
            if !record.snapshot().status.is_finished() {
                anyhow::bail!(
                    "Workflow run {run_id} is still running; wait for its notice, or stop it with TaskStop, before you resume it."
                );
            }
            return Ok(record.journal());
        }
        let path = self
            .directory()
            .map(|directory| journal::path_in(&directory, run_id))
            .filter(|path| path.is_file())
            .with_context(|| {
                format!(
                    "No workflow run of this session has the id '{run_id}'; give the id a Workflow call returned."
                )
            })?;
        journal::load(&path).with_context(|| {
            format!(
                "Could not read the journal of workflow run {run_id} at {}",
                path.display()
            )
        })
    }

    pub(crate) fn emit_outcome(
        &self,
        run: Option<RunId>,
        name: String,
        status: WorkflowTurnStatus,
    ) {
        if let Some(parent) = self.parent.upgrade() {
            parent.emit_workflow_outcome(run, name, status);
        }
    }

    pub(crate) fn limits(&self) -> Limits {
        Limits::default()
    }

    /// Ask the user to approve a run.
    ///
    /// This is a cost gate rather than a permission gate: one run can start
    /// hundreds of child agents, and the user should see that before it is
    /// spent. Modes with no way to answer, such as `-p`, start the run.
    pub async fn approve(&self, plan: WorkflowPlan) -> ApprovalDecision {
        let Some(parent) = self.parent.upgrade() else {
            return ApprovalDecision::Cancel;
        };
        if !parent.settings().workflows.confirm {
            return ApprovalDecision::Approve;
        }
        match parent.workflow_approver() {
            Some(approver) => approver(plan).await,
            None => ApprovalDecision::Approve,
        }
    }

    /// Prepare a run and add it to the list, without starting it. The run
    /// gets its id, and its script is saved in the session's folder of
    /// workflows as `<run id>.js`.
    ///
    /// Registering before running is what lets the progress view find the run
    /// as soon as it is approved, rather than after its first agent answers.
    pub fn prepare(
        &self,
        script: Script,
        args: Value,
        journal: Journal,
    ) -> anyhow::Result<Arc<RunRecord>> {
        let parent = self
            .parent
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("the parent session has closed"))?;
        let limits = self.limits();
        let runner = Arc::new(runner::SessionAgentRunner::new(
            self.parent.clone(),
            limits.max_concurrency,
        ));
        let cwd = parent.working_folder().display().to_string();

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let directory = self.directory();
        let run_id = self.new_run_id(directory.as_deref());
        let name = script.meta().name.clone();
        let description = script.meta().description.clone();
        let source: Arc<str> = Arc::from(script.source());
        // A script that cannot be saved still runs; it just cannot be named
        // by its path later.
        let script_path = directory.as_deref().and_then(|directory| {
            let path = directory.join(format!("{run_id}.js"));
            std::fs::create_dir_all(directory)
                .and_then(|()| std::fs::write(&path, source.as_bytes()))
                .ok()
                .map(|()| path)
        });
        let workflow = Arc::new(Workflow::with_journal(
            script, args, cwd, runner, limits, journal,
        ));
        let record = Arc::new(RunRecord {
            id,
            run_id,
            name,
            description,
            source,
            script_path,
            directory,
            workflow,
        });

        // Forward the workflow's watch channel into the normal session event
        // stream. The terminal then sleeps until state changes instead of
        // polling every frame.
        let mut updates = record.subscribe();
        let parent_for_updates = self.parent.clone();
        let record_for_updates = record.clone();
        tokio::spawn(async move {
            while updates.changed().await.is_ok() {
                let version = *updates.borrow_and_update();
                let Some(parent) = parent_for_updates.upgrade() else {
                    break;
                };
                parent.emit_workflow(id, version);
                if record_for_updates.snapshot().status.is_finished() {
                    break;
                }
            }
        });

        let mut runs = self
            .runs
            .lock()
            .map_err(|_| anyhow::anyhow!("the workflow list is unavailable"))?;
        runs.push(record.clone());
        // Keep the newest runs, but never drop one that is still working.
        while runs.len() > MAX_RETAINED_RUNS {
            let Some(position) = runs
                .iter()
                .position(|run| run.snapshot().status.is_finished())
            else {
                break;
            };
            runs.remove(position);
        }
        Ok(record)
    }

    /// Run a prepared workflow to completion, and save its journal of
    /// finished agent calls beside its script for a run that resumes it.
    pub async fn run(&self, record: &Arc<RunRecord>) -> Result<Value, String> {
        let result = record
            .workflow
            .run()
            .await
            .map_err(|error| error.to_string());
        // Without the file a resume still finds the run while the session
        // remembers it.
        if let Some(directory) = &record.directory {
            let path = journal::path_in(directory, &record.run_id);
            let _ = journal::save(&path, &record.run_id, &record.journal());
        }
        result
    }

    pub fn get(&self, id: RunId) -> Option<Arc<RunRecord>> {
        self.runs
            .lock()
            .ok()?
            .iter()
            .find(|run| run.id == id)
            .cloned()
    }

    /// Every run in this session, oldest first.
    pub fn summaries(&self) -> Vec<RunSummary> {
        let Ok(runs) = self.runs.lock() else {
            return Vec::new();
        };
        runs.iter()
            .map(|run| {
                let snapshot = run.snapshot();
                RunSummary {
                    id: run.id,
                    name: run.name.clone(),
                    description: run.description.clone(),
                    status: snapshot.status,
                    total_agents: snapshot.total_agents(),
                    finished_agents: snapshot.finished_agents(),
                    tokens: snapshot.tokens,
                    elapsed: snapshot.elapsed,
                }
            })
            .collect()
    }

    /// The run still working, if any.
    pub fn active(&self) -> Option<Arc<RunRecord>> {
        let runs = self.runs.lock().ok()?;
        runs.iter()
            .rev()
            .find(|run| !run.snapshot().status.is_finished())
            .cloned()
    }

    /// The most recent run, working or finished.
    pub fn latest(&self) -> Option<Arc<RunRecord>> {
        self.runs.lock().ok()?.last().cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.lock().map(|runs| runs.is_empty()).unwrap_or(true)
    }

    /// Stop every run still working. Called when the session closes or when the
    /// setting is turned off. A run the `Workflow` tool started is stopped
    /// through its background task, so no notice announces it.
    pub(crate) fn stop_all(&self) {
        let working: Vec<Arc<RunRecord>> = match self.runs.lock() {
            Ok(runs) => runs
                .iter()
                .filter(|run| !run.snapshot().status.is_finished())
                .cloned()
                .collect(),
            Err(_) => return,
        };
        let parent = self.parent.upgrade();
        for run in working {
            if let Some(parent) = &parent {
                let _ = parent.background_tasks().stop(&run.run_id);
            }
            run.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_estimate_never_guesses_a_number_it_does_not_know() {
        let plan = |estimated_agents| WorkflowPlan {
            name: "x".into(),
            description: "y".into(),
            phases: Vec::new(),
            estimated_agents,
            source: Arc::from(""),
        };
        assert_eq!(plan(Some(1)).agent_estimate(), "1 agent");
        assert_eq!(plan(Some(12)).agent_estimate(), "12 agents");
        assert_eq!(plan(None).agent_estimate(), "an unbounded number of agents");
    }
}
