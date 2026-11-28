//! The agents a session started with the `Agent` tool.
//!
//! Every agent is a child [`AgentSession`] with an id (`a` and eight letters
//! or digits) that stays the same for as long as the session remembers it.
//! A run of an agent in the background is a task of the parent's
//! [`BackgroundTasks`](crate::tasks::BackgroundTasks) with that same id, so
//! `TaskStop` and the clients' task lists stop and show it, and its end is
//! announced with a notice that carries the agent's report. A run in the
//! foreground is part of the parent's turn and ends with it.
//!
//! A finished agent keeps its session, so a message sent to it with
//! `SendMessage` resumes it as a background run.
//!
//! An agent's session is stored like any session, in the folder of agents
//! of its parent's session file
//! ([`agents_dir`](crate::session::manager::agents_dir)), as
//! `<agent id>.jsonl`. Every run's end is recorded in it (an
//! [`AGENT_RUN_ENTRY`] entry), so a session that is loaded again rebuilds
//! its registry from the stored agents ([`AgentRegistry::restore`]): they are
//! listed at once and their sessions are built the first time they are
//! needed, for a message that resumes them or a client that reads them.
//!
//! An agent with `isolation: "worktree"` works in a git worktree of its own
//! (see [`crate::worktree`]). When a run ends and nothing changed in it, the
//! worktree is removed; otherwise it is kept, and the run's result names its
//! folder and branch. A resumed agent gets its worktree back first.
//!
//! The child sessions of a workflow run and of a `/loop` or `/autoresearch`
//! job have a record too ([`AgentRegistry::adopt`]), so they are stored,
//! listed, mirrored and rebuilt like agents; their [`AgentOrigin`] says where
//! they come from. They run as their workflow or job says, not as the model
//! says: `ListAgents` and `SendMessage` know only the agents the session
//! started itself, and they do not count against [`MAX_RUNNING`].

use super::types::{AgentType, GENERAL_PURPOSE};
use crate::child_turn::{AgentStatus, ChildRun, ChildTurnOutcome};
use crate::notifications::{Priority, agent_message};
use crate::session::manager::SUBAGENT_ENTRY;
use crate::session_runner::AgentSession;
use crate::tasks::{TaskHandle, TaskKind, TaskOutcome, TaskSpec, TaskStatus, TaskUsage};
use crate::worktree::{self, Worktree};
use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use super_agent::AgentMessage;
use super_ai::{ContentBlock, Model, ThinkingLevel, UserContent, UserMessage};
use tokio_util::sync::CancellationToken;

/// `customType` of the entry an agent's session gets when a run of it ends:
/// how it ended, whether it ran in the background, and its worktree.
pub const AGENT_RUN_ENTRY: &str = "agent_run";

/// At most this many agents of one session run at once.
pub const MAX_RUNNING: usize = 20;
/// Agents nest at most this deep: the main session's agents are at depth 1.
pub const MAX_DEPTH: usize = 3;

/// Where an agent works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// In a git worktree of its own.
    Worktree,
    /// On another machine.
    Remote,
}

/// What an agent's session knows of the session that started it.
#[derive(Debug, Clone)]
pub struct AgentLink {
    /// The session that started the agent: `main` for `SendMessage`.
    pub parent: Weak<AgentSession>,
    /// The agent's id in its parent's registry.
    pub id: String,
    /// 1 for an agent of the main session.
    pub depth: usize,
    /// `/root/<id>`, with the ids of the agents above it in between.
    pub path: String,
}

/// Where an agent comes from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentOrigin {
    /// An `Agent` call of the session, or a skill that runs in an agent.
    #[default]
    Agent,
    /// An agent of the workflow run `run_id`.
    #[serde(rename_all = "camelCase")]
    Workflow { run_id: String },
    /// The child session of the job `job_id`: a `/loop` with an interval or
    /// an `/autoresearch` (`job_kind`).
    #[serde(rename_all = "camelCase")]
    Job { job_id: u64, job_kind: String },
}

impl AgentOrigin {
    /// Whether the session started the agent itself, so that the model's
    /// `ListAgents` and `SendMessage` reach it.
    pub fn is_agent_call(&self) -> bool {
        matches!(self, AgentOrigin::Agent)
    }

    /// The type a child of a workflow or a job has: `workflow`, `loop` or
    /// `autoresearch`.
    pub(crate) fn child_type(&self) -> &str {
        match self {
            AgentOrigin::Agent => GENERAL_PURPOSE,
            AgentOrigin::Workflow { .. } => "workflow",
            AgentOrigin::Job { job_kind, .. } => job_kind,
        }
    }
}

/// What a call of the `Agent` tool asks for.
#[derive(Debug, Clone, Default)]
pub struct AgentRequest {
    /// A few words on what the agent does; it is also the agent's name.
    pub description: String,
    pub prompt: String,
    /// The name of an agent type; `None` is `general-purpose`.
    pub agent_type: Option<String>,
    /// A model of the registry.
    pub model: Option<String>,
    /// A thinking level the model accepts.
    pub effort: Option<String>,
    /// Run as a background task instead of within the caller's turn.
    pub background: bool,
    pub isolation: Option<Isolation>,
}

/// An agent as `ListAgents` and the session's clients see it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub id: String,
    /// The description it was started with.
    pub name: String,
    #[serde(rename = "type")]
    pub agent_type: String,
    pub status: TaskStatus,
    /// Whether its latest run is, or was, a background task.
    pub background: bool,
    /// Where its transcript is stored, when its session is stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_file: Option<PathBuf>,
    /// The worktree it works in, while there is one: during a run of an
    /// agent with `isolation: "worktree"`, and after a run that left
    /// changes in it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<Worktree>,
    /// Who started it: the session, a workflow run or a job.
    pub origin: AgentOrigin,
}

/// How a started agent stands when [`AgentRegistry::start`] returns.
pub(crate) enum Started {
    /// It runs as the background task with the agent's id.
    Background(AgentInfo),
    /// It ran within the caller's turn, to this end.
    Finished(AgentInfo, ChildTurnOutcome),
}

/// What [`AgentRegistry::send`] did with a message.
pub(crate) enum Delivery {
    /// The agent runs: it reads the message at its next step.
    Queued(AgentInfo),
    /// The agent had ended: the message resumes it in the background.
    Resumed(AgentInfo),
}

/// One agent the registry knows.
pub(crate) struct AgentRecord {
    id: String,
    name: String,
    agent_type: String,
    origin: AgentOrigin,
    /// Built when the agent starts; for an agent rebuilt from its stored
    /// session, the first time it is needed.
    session: Mutex<Option<Arc<AgentSession>>>,
    /// Where its session is stored, when it is.
    file: Option<PathBuf>,
    /// What building the session of a stored agent needs.
    stored: Option<StoredAgent>,
    /// The worktree of an agent with `isolation: "worktree"`, as it was last
    /// made; its folder is gone after a run that changed nothing in it.
    worktree: Mutex<Option<Worktree>>,
    state: Mutex<RunState>,
}

/// An agent as its stored session tells of it.
#[derive(Debug, Clone)]
pub(crate) struct StoredAgent {
    pub file: PathBuf,
    pub id: String,
    pub name: String,
    pub agent_type: String,
    /// Provider and model id.
    pub model: Option<(String, String)>,
    pub thinking: Option<ThinkingLevel>,
    pub worktree: Option<Worktree>,
    pub origin: AgentOrigin,
    /// How its last run ended; `stopped` when the session ended during a
    /// run, so that its end was never recorded.
    pub status: TaskStatus,
    pub background: bool,
    /// When its session was made, for the order of the list.
    pub started: String,
}

struct RunState {
    status: TaskStatus,
    background: bool,
    /// Ends the run in flight: the task's stop signal for a background run,
    /// a token of the caller's turn for a foreground one.
    stop: Option<CancellationToken>,
}

impl AgentRecord {
    /// A record of an agent whose run `stop` ends, as
    /// [`AgentRegistry::insert`] takes it.
    pub(crate) fn new(
        id: String,
        name: String,
        agent_type: String,
        session: Arc<AgentSession>,
        background: bool,
        stop: CancellationToken,
    ) -> Self {
        let file = session
            .manager
            .lock()
            .unwrap()
            .session_file()
            .map(Path::to_path_buf);
        AgentRecord {
            id,
            name,
            agent_type,
            origin: AgentOrigin::Agent,
            session: Mutex::new(Some(session)),
            file,
            stored: None,
            worktree: Mutex::new(None),
            state: Mutex::new(RunState {
                status: TaskStatus::Running,
                background,
                stop: Some(stop),
            }),
        }
    }

    /// A record of an agent rebuilt from its stored session, which does not
    /// run; its session is built when it is needed.
    pub(crate) fn stored(stored: StoredAgent) -> Self {
        AgentRecord {
            id: stored.id.clone(),
            name: stored.name.clone(),
            agent_type: stored.agent_type.clone(),
            origin: stored.origin.clone(),
            session: Mutex::new(None),
            file: Some(stored.file.clone()),
            worktree: Mutex::new(stored.worktree.clone()),
            state: Mutex::new(RunState {
                status: stored.status,
                background: stored.background,
                stop: None,
            }),
            stored: Some(stored),
        }
    }

    /// The agent's session, when it was built.
    fn built(&self) -> Option<Arc<AgentSession>> {
        self.session.lock().unwrap().clone()
    }

    /// The agent's session, built from its stored session the first time
    /// for an agent the registry was rebuilt with; `owner` hears of it.
    fn session_in(&self, owner: &Arc<AgentSession>) -> anyhow::Result<Arc<AgentSession>> {
        let mut slot = self.session.lock().unwrap();
        if let Some(session) = slot.as_ref() {
            return Ok(session.clone());
        }
        let stored = self
            .stored
            .as_ref()
            .with_context(|| format!("Agent {} has no session to resume.", self.id))?;
        let session = owner.load_agent_session(stored).with_context(|| {
            format!(
                "Could not load agent {} from {}",
                self.id,
                stored.file.display()
            )
        })?;
        *slot = Some(session.clone());
        drop(slot);
        if let Some(observer) = owner.agent_observer() {
            observer.agent_session(&self.id, &session);
        }
        Ok(session)
    }

    /// Record in the agent's session how its run ended, so the registry of
    /// a session loaded again knows.
    fn save_run(&self) {
        let Some(session) = self.built() else {
            return;
        };
        let (status, background) = {
            let state = self.state.lock().unwrap();
            (state.status, state.background)
        };
        let worktree = self.worktree.lock().unwrap().clone();
        let _ = session.manager.lock().unwrap().append_custom(
            AGENT_RUN_ENTRY,
            Some(json!({
                "status": status,
                "background": background,
                "worktree": worktree,
            })),
        );
    }

    fn begin_run(&self, background: bool, stop: CancellationToken) {
        let mut state = self.state.lock().unwrap();
        state.status = TaskStatus::Running;
        state.background = background;
        state.stop = Some(stop);
    }

    fn end_run(&self, outcome: &ChildTurnOutcome) {
        let mut state = self.state.lock().unwrap();
        state.status = match outcome.status {
            AgentStatus::Completed => TaskStatus::Completed,
            AgentStatus::Failed => TaskStatus::Failed,
            AgentStatus::Interrupted => TaskStatus::Stopped,
        };
        state.stop = None;
    }

    fn info(&self, owner: Option<&AgentSession>) -> AgentInfo {
        let (mut status, background) = {
            let state = self.state.lock().unwrap();
            (state.status, state.background)
        };
        // A background run stopped through the task registry has ended
        // there before its child has finished stopping.
        if status.is_running()
            && background
            && let Some(task) = owner.and_then(|owner| owner.background_tasks().find(&self.id))
            && !task.status.is_running()
        {
            status = task.status;
        }
        AgentInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            agent_type: self.agent_type.clone(),
            status,
            background,
            session_file: self.file.clone(),
            worktree: self
                .worktree
                .lock()
                .unwrap()
                .clone()
                .filter(|worktree| worktree.path.exists()),
            origin: self.origin.clone(),
        }
    }

    fn is_running(&self) -> bool {
        self.state.lock().unwrap().status.is_running()
    }
}

/// The agents one session started, oldest first. Every session that may
/// start agents has one: the main session while the Subagents switch is on,
/// and an agent of a type that may start agents while it is not as deep as
/// agents nest.
pub struct AgentRegistry {
    /// The session that started the agents.
    owner: Weak<AgentSession>,
    /// How deep that session is: 0 for the main session.
    depth: usize,
    records: Mutex<Vec<Arc<AgentRecord>>>,
    /// Held while an agent is checked against the limit and started, while
    /// a message is delivered, and while a run's end is decided.
    starting: Mutex<()>,
    /// Called once, in tests, right before the end of a run is decided.
    #[cfg(test)]
    pub(crate) before_close: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl AgentRegistry {
    pub(crate) fn new(owner: Weak<AgentSession>, depth: usize) -> Arc<Self> {
        Arc::new(AgentRegistry {
            owner,
            depth,
            records: Mutex::new(Vec::new()),
            starting: Mutex::new(()),
            #[cfg(test)]
            before_close: Mutex::new(None),
        })
    }

    /// How deep the session that starts these agents is.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Every agent, oldest first.
    pub fn list(&self) -> Vec<AgentInfo> {
        let owner = self.owner.upgrade();
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|record| record.info(owner.as_deref()))
            .collect()
    }

    /// The agents the session started itself (with `Agent`, or a skill
    /// that runs in an agent), oldest first: what `ListAgents` lists.
    pub fn list_started(&self) -> Vec<AgentInfo> {
        let mut agents = self.list();
        agents.retain(|agent| agent.origin.is_agent_call());
        agents
    }

    /// The agent with this id, else the one agent with this name.
    pub fn find(&self, id_or_name: &str) -> anyhow::Result<AgentInfo> {
        let record = self.resolve(id_or_name, |_| true)?;
        Ok(record.info(self.owner.upgrade().as_deref()))
    }

    /// The session of the agent with this id or name, built from its stored
    /// session if it was not yet.
    pub fn session_of(&self, id_or_name: &str) -> anyhow::Result<Arc<AgentSession>> {
        let owner = self.owner()?;
        self.resolve(id_or_name, |_| true)?.session_in(&owner)
    }

    /// The agents whose sessions are built, with their ids, oldest first.
    pub fn built_sessions(&self) -> Vec<(String, Arc<AgentSession>)> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .filter_map(|record| Some((record.id.clone(), record.built()?)))
            .collect()
    }

    /// Add an agent. A record built from a stored session joins the same
    /// way as one [`AgentRegistry::start`] made.
    pub(crate) fn insert(&self, record: Arc<AgentRecord>) {
        self.records.lock().unwrap().push(record);
    }

    /// Add every agent stored in the owner's folder of agents that the
    /// registry does not know yet, oldest first: what a session loaded again
    /// does, so that `ListAgents` lists them and `SendMessage` resumes them.
    /// Returns how many were added.
    pub fn restore(&self) -> usize {
        let Some(owner) = self.owner.upgrade() else {
            return 0;
        };
        let files = owner.manager.lock().unwrap().stored_children();
        let mut stored: Vec<StoredAgent> = files
            .iter()
            .filter_map(|file| read_stored_agent(file))
            .collect();
        stored.sort_by(|a, b| a.started.cmp(&b.started).then(a.file.cmp(&b.file)));
        let mut records = self.records.lock().unwrap();
        let mut added = 0;
        for agent in stored {
            if records.iter().any(|record| record.id == agent.id) {
                continue;
            }
            records.push(Arc::new(AgentRecord::stored(agent)));
            added += 1;
        }
        added
    }

    fn owner(&self) -> anyhow::Result<Arc<AgentSession>> {
        self.owner
            .upgrade()
            .context("The session has ended, so it has no agents.")
    }

    /// Start an agent for the tool call `tool_use_id`. In the background it
    /// returns at once; in the foreground it returns when the agent is done,
    /// or stopped because `cancel` fired with the caller's turn.
    pub(crate) async fn start(
        self: &Arc<Self>,
        request: AgentRequest,
        tool_use_id: &str,
        cancel: CancellationToken,
    ) -> anyhow::Result<Started> {
        let owner = self
            .owner
            .upgrade()
            .context("The session has ended, so it cannot start an agent.")?;
        let description = request.description.trim();
        if description.is_empty() {
            anyhow::bail!("description is empty; say in a few words what the agent does.");
        }
        if request.prompt.trim().is_empty() {
            anyhow::bail!("prompt is empty; give the agent its task.");
        }
        if request.isolation == Some(Isolation::Remote) {
            anyhow::bail!(
                "isolation 'remote' is not available: agents run on this machine only. Leave isolation out."
            );
        }
        if self.depth >= MAX_DEPTH {
            anyhow::bail!(
                "Agents nest at most {MAX_DEPTH} levels deep, and this agent is at the deepest level; do the task here."
            );
        }
        let types = owner.agent_types();
        let agent_type = find_type(&types, request.agent_type.as_deref())?;
        let (model, thinking) = choose_model(&owner, agent_type, &request)?;

        let id = owner.background_tasks().new_id(TaskKind::Agent);
        // No worktree for an agent that could not start anyway.
        let worktree = if request.isolation == Some(Isolation::Worktree) {
            self.check_room()?;
            Some(agent_worktree(&owner, &id).await?)
        } else {
            None
        };
        let started = (|| {
            let _starting = self.starting.lock().unwrap();
            self.check_room()?;
            let child = owner.create_agent_session(crate::session_runner::AgentChild {
                id: &id,
                agent_type,
                description,
                model,
                thinking,
                depth: self.depth + 1,
                tool_use_id,
                worktree: worktree.as_ref(),
            })?;
            // A background run is stopped through its task, a waited one
            // with the caller's turn.
            let (handle, stop) = if request.background {
                let handle = register_run(&owner, &id, description, &child, tool_use_id)?;
                let stop = handle.stop_signal();
                (Some(handle), stop)
            } else {
                (None, cancel.child_token())
            };
            let record = Arc::new(AgentRecord::new(
                id.clone(),
                description.to_string(),
                agent_type.name.clone(),
                child.clone(),
                request.background,
                stop.clone(),
            ));
            *record.worktree.lock().unwrap() = worktree.clone();
            self.insert(record.clone());
            anyhow::Ok((record, child, handle, stop))
        })();
        let (record, child, handle, stop) = match started {
            Ok(started) => started,
            Err(error) => {
                if let Some(worktree) = worktree {
                    let _ = tokio::task::spawn_blocking(move || worktree::remove(&worktree)).await;
                }
                return Err(error);
            }
        };
        if let Some(observer) = owner.agent_observer() {
            observer.agent_session(&id, &child);
        }
        let prompt = first_message(agent_type, &request.prompt);

        if let Some(handle) = handle {
            self.spawn_run(record.clone(), child, handle, prompt);
            return Ok(Started::Background(record.info(Some(&owner))));
        }
        let outcome = self.run_to_end(&record, &child, prompt, stop, None).await;
        Ok(Started::Finished(
            record.info(self.owner.upgrade().as_deref()),
            outcome,
        ))
    }

    /// Deliver `message` to an agent: at its next step while it runs, else
    /// by resuming it in the background as a run that the tool call
    /// `tool_use_id` started.
    pub(crate) fn send(
        self: &Arc<Self>,
        to: &str,
        summary: Option<&str>,
        message: &str,
        tool_use_id: &str,
    ) -> anyhow::Result<Delivery> {
        let owner = self
            .owner
            .upgrade()
            .context("The session has ended, so it cannot send a message.")?;
        // A workflow's or a job's child runs as its workflow or job says.
        let record = self.resolve(to, |record| record.origin.is_agent_call())?;
        let _starting = self.starting.lock().unwrap();
        let note = agent_message("main", &record.id, summary, message);
        if record.is_running() {
            if !record.info(Some(&owner)).status.is_running() {
                anyhow::bail!(
                    "Agent {} is stopping; send the message again once it has stopped.",
                    record.id
                );
            }
            if let Some(session) = record.built() {
                session.queue_entry(note, Priority::Next);
            }
            return Ok(Delivery::Queued(record.info(Some(&owner))));
        }
        self.check_room()?;
        let session = record.session_in(&owner)?;
        let handle = register_run(&owner, &record.id, &record.name, &session, tool_use_id)?;
        record.begin_run(true, handle.stop_signal());
        self.spawn_run(record.clone(), session, handle, note);
        Ok(Delivery::Resumed(record.info(Some(&owner))))
    }

    /// Add the record of `session`, a child that a workflow run or a job
    /// starts (`origin`), and tell the owner's observer of it, so that it is
    /// listed, read and mirrored like an agent. It counts as running until
    /// its first run ends.
    pub(crate) fn adopt(
        &self,
        id: &str,
        name: &str,
        origin: AgentOrigin,
        session: &Arc<AgentSession>,
    ) -> Arc<AgentRecord> {
        let mut record = AgentRecord::new(
            id.to_string(),
            name.to_string(),
            origin.child_type().to_string(),
            session.clone(),
            false,
            CancellationToken::new(),
        );
        record.origin = origin;
        let record = Arc::new(record);
        self.insert(record.clone());
        if let Some(observer) = self
            .owner
            .upgrade()
            .and_then(|owner| owner.agent_observer())
        {
            observer.agent_session(id, session);
        }
        record
    }

    /// One run of the adopted child of `record` with `prompt`, as its
    /// workflow or job runs it: until it has nothing left to do, `cancel`
    /// fires or `timeout_ms` passes. The record shows the run, and its end
    /// is recorded in the child's session.
    pub(crate) async fn run_child(
        &self,
        record: &AgentRecord,
        session: &Arc<AgentSession>,
        prompt: AgentMessage,
        cancel: Option<CancellationToken>,
        timeout_ms: Option<u64>,
    ) -> ChildTurnOutcome {
        // Its own token, so that stopping the agent stops this run only.
        let stop = cancel.map_or_else(CancellationToken::new, |cancel| cancel.child_token());
        record.begin_run(false, stop.clone());
        let run = ChildRun::begin(session, Some(stop), timeout_ms);
        run.prompt(session, prompt).await;
        let _starting = self.starting.lock().unwrap();
        let outcome = run.end(&self.owner, session);
        self.close_run(record, &outcome, None, None);
        outcome
    }

    /// The workflow or job of the adopted child of `record` is over: a
    /// record that still counts as running, because the job ended before
    /// its first run, is stopped and its end recorded.
    pub(crate) fn release(&self, record: &AgentRecord) {
        let _starting = self.starting.lock().unwrap();
        if !record.is_running() {
            return;
        }
        {
            let mut state = record.state.lock().unwrap();
            state.status = TaskStatus::Stopped;
            state.stop = None;
        }
        record.save_run();
        if let Some(observer) = self
            .owner
            .upgrade()
            .and_then(|owner| owner.agent_observer())
        {
            observer.agent_settled(&record.id);
        }
    }

    /// Stop every agent that runs: a background run through the task
    /// registry, so it ends without a notice, a foreground one by ending
    /// its run.
    pub(crate) fn stop_all(&self) {
        // No run starts or resumes while the others are stopped.
        let _starting = self.starting.lock().unwrap();
        let owner = self.owner.upgrade();
        let records = self.records.lock().unwrap().clone();
        for record in records {
            let (running, background, stop) = {
                let state = record.state.lock().unwrap();
                (
                    state.status.is_running(),
                    state.background,
                    state.stop.clone(),
                )
            };
            if !running {
                continue;
            }
            if background && let Some(owner) = &owner {
                let _ = owner.background_tasks().stop(&record.id);
            }
            if let Some(stop) = stop {
                stop.cancel();
            }
        }
    }

    /// Stop every agent and forget them all, for a session that switches to
    /// another conversation.
    pub(crate) fn reset(&self) {
        self.stop_all();
        self.records.lock().unwrap().clear();
        if let Some(observer) = self
            .owner
            .upgrade()
            .and_then(|owner| owner.agent_observer())
        {
            observer.agents_reset();
        }
    }

    /// The agent with this id, else the one agent with this name, among
    /// those `known` admits.
    fn resolve(
        &self,
        id_or_name: &str,
        known: impl Fn(&AgentRecord) -> bool,
    ) -> anyhow::Result<Arc<AgentRecord>> {
        let records = self.records.lock().unwrap();
        let records: Vec<&Arc<AgentRecord>> =
            records.iter().filter(|record| known(record)).collect();
        if let Some(record) = records.iter().find(|record| record.id == id_or_name) {
            return Ok((*record).clone());
        }
        let named: Vec<&Arc<AgentRecord>> = records
            .iter()
            .filter(|record| record.name == id_or_name)
            .copied()
            .collect();
        match named.as_slice() {
            [record] => Ok((*record).clone()),
            [] => anyhow::bail!(
                "No agent of this session has the id or name '{id_or_name}'; ListAgents lists them."
            ),
            _ => anyhow::bail!(
                "Several agents of this session are named '{id_or_name}'; give the id of one of them."
            ),
        }
    }

    /// Refuse a run beyond [`MAX_RUNNING`]. A run stopped through the task
    /// registry no longer counts, though its child may still be finishing.
    fn check_room(&self) -> anyhow::Result<()> {
        let owner = self.owner.upgrade();
        let running = self
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| {
                record.origin.is_agent_call() && record.info(owner.as_deref()).status.is_running()
            })
            .count();
        if running >= MAX_RUNNING {
            anyhow::bail!(
                "{MAX_RUNNING} agents of this session run already, the most at once; wait for one to end or stop one with TaskStop."
            );
        }
        Ok(())
    }

    fn spawn_run(
        self: &Arc<Self>,
        record: Arc<AgentRecord>,
        session: Arc<AgentSession>,
        handle: TaskHandle,
        prompt: AgentMessage,
    ) {
        let registry = self.clone();
        tokio::spawn(async move {
            let stop = handle.stop_signal();
            registry
                .run_to_end(&record, &session, prompt, stop, Some(handle))
                .await;
        });
    }

    /// Run the agent of `record` with `prompt` until it is done, and end the
    /// run: its background task when it has one (`handle`), its worktree
    /// when nothing changed in it, and the end recorded in its session.
    ///
    /// The run is over only when the agent has nothing left to do at the
    /// moment its end is recorded: its own background tasks have ended,
    /// their notices are answered, and no message waits for it. A message
    /// [`send`](Self::send) delivers after the agent's last turn, before the
    /// end is recorded, is part of the run; one after it resumes the agent.
    async fn run_to_end(
        &self,
        record: &AgentRecord,
        session: &Arc<AgentSession>,
        prompt: AgentMessage,
        stop: CancellationToken,
        mut handle: Option<TaskHandle>,
    ) -> ChildTurnOutcome {
        if let Err(error) = prepare_worktree(record).await {
            let outcome = ChildTurnOutcome {
                status: AgentStatus::Failed,
                result: None,
                error: Some(format!("{error:#}")),
                usage: Default::default(),
                tool_uses: 0,
                duration: Default::default(),
            };
            let _starting = self.starting.lock().unwrap();
            self.close_run(record, &outcome, None, handle.take());
            return outcome;
        }
        let run = ChildRun::begin(session, Some(stop), None);
        run.prompt(session, prompt).await;
        loop {
            let kept = settle_worktree(record).await;
            #[cfg(test)]
            if let Some(hook) = self.before_close.lock().unwrap().take() {
                hook();
            }
            {
                // `send` decides under this lock whether the agent runs.
                let _starting = self.starting.lock().unwrap();
                if run.is_stopped() || session.has_nothing_to_do() {
                    let outcome = run.end(&self.owner, session);
                    self.close_run(record, &outcome, kept, handle.take());
                    return outcome;
                }
            }
            // A message reached the agent after its last turn: the run goes
            // on until it is answered, in its worktree again if the check
            // above removed it (if it cannot be made, the agent's tools say
            // what is wrong).
            let _ = prepare_worktree(record).await;
            run.settle(session).await;
        }
    }

    /// Record the end of a run of `record` in its state and its session,
    /// tell the owner's observer, and end its background task with a notice
    /// that names the worktree `kept`, when one is. Called with the
    /// `starting` lock held.
    fn close_run(
        &self,
        record: &AgentRecord,
        outcome: &ChildTurnOutcome,
        kept: Option<Worktree>,
        handle: Option<TaskHandle>,
    ) {
        record.end_run(outcome);
        record.save_run();
        if let Some(observer) = self
            .owner
            .upgrade()
            .and_then(|owner| owner.agent_observer())
        {
            observer.agent_settled(&record.id);
        }
        if let Some(handle) = handle {
            let mut ended = task_outcome(&record.name, outcome);
            if let Some(worktree) = kept {
                ended = ended.with_worktree(worktree);
            }
            handle.finish(ended);
        }
    }
}

/// Make the worktree of an agent with `isolation: "worktree"` again before
/// a run, when the run before removed it.
async fn prepare_worktree(record: &AgentRecord) -> anyhow::Result<()> {
    let Some(known) = record.worktree.lock().unwrap().clone() else {
        return Ok(());
    };
    if known.path.exists() {
        return Ok(());
    }
    let made = tokio::task::spawn_blocking(move || worktree::recreate(&known))
        .await
        .context("the worktree could not be made")?
        .context("Could not make the agent's worktree again")?;
    *record.worktree.lock().unwrap() = Some(made);
    Ok(())
}

/// After a run: remove the agent's worktree when nothing changed in it, and
/// return it when it stays. A worktree that cannot be checked or removed
/// stays.
async fn settle_worktree(record: &AgentRecord) -> Option<Worktree> {
    let known = record.worktree.lock().unwrap().clone()?;
    if !known.path.exists() {
        return None;
    }
    let checked = known.clone();
    let kept = tokio::task::spawn_blocking(move || match worktree::has_changes(&checked) {
        Ok(false) => worktree::remove(&checked).is_err(),
        Ok(true) | Err(_) => true,
    })
    .await
    .unwrap_or(true);
    kept.then_some(known)
}

/// The data of a stored session's `subagent` entry that the registry reads.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentEntry {
    agent_id: Option<String>,
    #[serde(default)]
    task_name: String,
    #[serde(default)]
    agent_type: Option<String>,
    #[serde(default)]
    model: Option<StoredModel>,
    #[serde(default)]
    thinking_level: Option<ThinkingLevel>,
    #[serde(default)]
    worktree: Option<Worktree>,
    #[serde(default)]
    origin: AgentOrigin,
}

#[derive(Deserialize)]
struct StoredModel {
    provider: String,
    id: String,
}

/// The data of an [`AGENT_RUN_ENTRY`] entry.
#[derive(Deserialize)]
struct RunEntry {
    status: TaskStatus,
    #[serde(default)]
    background: bool,
    #[serde(default)]
    worktree: Option<Worktree>,
}

/// One line of a stored session, as far as the registry reads it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntryLine {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    custom_type: Option<String>,
    #[serde(default)]
    data: Option<Value>,
}

/// What the stored session `file` of an agent tells of it: `None` for a
/// file without an agent id (a workflow's or a loop's child stored by an
/// earlier version) or one that cannot be read.
pub(crate) fn read_stored_agent(file: &Path) -> Option<StoredAgent> {
    let reader = std::io::BufReader::new(std::fs::File::open(file).ok()?);
    let mut lines = reader.lines().map_while(Result::ok);
    let header: crate::SessionHeader = loop {
        let line = lines.next()?;
        if !line.trim().is_empty() {
            break serde_json::from_str(&line).ok()?;
        }
    };
    let mut subagent: Option<SubagentEntry> = None;
    let mut last_run: Option<RunEntry> = None;
    let mut since_run = false;
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<EntryLine>(&line) else {
            continue;
        };
        match (entry.kind.as_str(), entry.custom_type.as_deref()) {
            ("custom", Some(SUBAGENT_ENTRY)) if subagent.is_none() => {
                subagent = serde_json::from_value(entry.data?).ok();
            }
            ("custom", Some(AGENT_RUN_ENTRY)) => {
                last_run = entry
                    .data
                    .and_then(|data| serde_json::from_value(data).ok());
                since_run = false;
            }
            ("message" | "custom_message", _) => since_run = true,
            _ => {}
        }
    }
    let subagent = subagent?;
    let id = subagent.agent_id?;
    // A run that left no end behind was cut off with its process.
    let (status, background) = match &last_run {
        Some(run) if !since_run && !run.status.is_running() => (run.status, run.background),
        Some(run) => (TaskStatus::Stopped, run.background),
        None => (TaskStatus::Stopped, false),
    };
    let worktree = last_run.and_then(|run| run.worktree).or(subagent.worktree);
    Some(StoredAgent {
        file: file.to_path_buf(),
        name: if subagent.task_name.is_empty() {
            id.clone()
        } else {
            subagent.task_name
        },
        id,
        agent_type: subagent
            .agent_type
            .unwrap_or_else(|| GENERAL_PURPOSE.to_string()),
        model: subagent.model.map(|model| (model.provider, model.id)),
        thinking: subagent.thinking_level,
        worktree,
        origin: subagent.origin,
        status,
        background,
        started: header.timestamp,
    })
}

/// Register a background run of the agent `id` as a task of `owner` with
/// that id, the agent's transcript as its output file, and the agent's name
/// while no other running task has it, so that `TaskStop` and `stop_task`
/// find the agent by its name too.
fn register_run(
    owner: &AgentSession,
    id: &str,
    name: &str,
    session: &AgentSession,
    tool_use_id: &str,
) -> anyhow::Result<TaskHandle> {
    let mut spec = TaskSpec::new(TaskKind::Agent, name).with_id(id);
    if !tool_use_id.is_empty() {
        spec = spec.with_tool_use_id(tool_use_id);
    }
    if let Some(file) = session.manager.lock().unwrap().session_file() {
        spec = spec.with_output_file(file);
    }
    let tasks = owner.background_tasks();
    if !name.trim().is_empty()
        && let Ok(handle) = tasks.register(spec.clone().with_name(name))
    {
        return Ok(handle);
    }
    tasks
        .register(spec)
        .map_err(|error| anyhow::anyhow!("{error}"))
}

/// The type named `name`, or `general-purpose`; a name in another case is
/// taken when it is the only such type.
fn find_type<'a>(types: &'a [AgentType], name: Option<&str>) -> anyhow::Result<&'a AgentType> {
    let name = name.map(str::trim).filter(|name| !name.is_empty());
    let wanted = name.unwrap_or(GENERAL_PURPOSE);
    if let Some(found) = types.iter().find(|known| known.name == wanted) {
        return Ok(found);
    }
    let folded: Vec<&AgentType> = types
        .iter()
        .filter(|known| known.name.eq_ignore_ascii_case(wanted))
        .collect();
    if let [found] = folded.as_slice() {
        return Ok(found);
    }
    let names: Vec<&str> = types.iter().map(|known| known.name.as_str()).collect();
    anyhow::bail!(
        "No agent type is named '{wanted}'; the types are {}.",
        names.join(", ")
    )
}

/// The model and thinking level the agent runs on: the call's, else its
/// type's, else the parent's. A fork always runs on the parent's.
fn choose_model(
    owner: &AgentSession,
    agent_type: &AgentType,
    request: &AgentRequest,
) -> anyhow::Result<(Model, ThinkingLevel)> {
    if agent_type.is_fork() {
        if request.model.is_some() || request.effort.is_some() {
            anyhow::bail!(
                "A fork runs on the parent's model and effort; leave out model and effort."
            );
        }
        return Ok((owner.model(), owner.thinking_level()));
    }
    let pattern = request
        .model
        .as_deref()
        .or(agent_type.model.as_deref())
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty() && *pattern != "inherit");
    let (model, suggested) = match pattern {
        Some(pattern) => owner.registry().resolve(pattern, None).with_context(|| {
            let current = owner.model();
            format!(
                "No model of the registry matches '{pattern}'; give one it has, such as {}/{}.",
                current.provider, current.id
            )
        })?,
        None => (owner.model(), None),
    };
    let effort = request
        .effort
        .as_deref()
        .or(agent_type.effort.as_deref())
        .map(str::trim)
        .filter(|effort| !effort.is_empty());
    let thinking = match effort {
        Some(effort) => {
            let level = ThinkingLevel::parse(&effort.to_ascii_lowercase()).with_context(|| {
                format!(
                    "effort '{effort}' is not a thinking level; give one of off, minimal, low, medium, high, xhigh or max."
                )
            })?;
            let accepted = model.supported_thinking_levels();
            if !accepted.contains(&level) {
                let accepted: Vec<&str> = accepted.iter().map(ThinkingLevel::as_str).collect();
                anyhow::bail!(
                    "{}/{} does not accept the effort '{}'; it accepts {}.",
                    model.provider,
                    model.id,
                    level.as_str(),
                    accepted.join(", ")
                );
            }
            level
        }
        None => suggested.unwrap_or_else(|| owner.thinking_level()),
    };
    Ok((model, thinking))
}

/// The message an agent starts with: the prompt, and for a fork the fork's
/// instructions before it, as a block of their own.
fn first_message(agent_type: &AgentType, prompt: &str) -> AgentMessage {
    if !agent_type.is_fork() {
        return AgentMessage::user(prompt);
    }
    AgentMessage::User(UserMessage {
        content: UserContent::Blocks(vec![
            ContentBlock::text(agent_type.instructions.clone()),
            ContentBlock::text(prompt),
        ]),
        timestamp: super_ai::now_ms(),
    })
}

/// The notice of a background run: the agent's report as its result, and
/// what the run used.
fn task_outcome(name: &str, outcome: &ChildTurnOutcome) -> TaskOutcome {
    let usage = TaskUsage {
        subagent_tokens: outcome.usage.total_tokens,
        tool_uses: outcome.tool_uses,
        duration_ms: u64::try_from(outcome.duration.as_millis()).unwrap_or(u64::MAX),
    };
    match outcome.status {
        AgentStatus::Completed => TaskOutcome::completed(format!("{name} (completed)"))
            .with_result(outcome.result.clone().unwrap_or_default()),
        AgentStatus::Failed => TaskOutcome::failed(format!(
            "{name} (failed: {})",
            outcome.error.as_deref().unwrap_or("the agent failed")
        )),
        AgentStatus::Interrupted => TaskOutcome::stopped(format!("{name} (stopped)")),
    }
    .with_usage(usage)
}

/// The git worktree an agent with `isolation: "worktree"` works in:
/// `.super/worktrees/agent-<id>` in the project of `owner`'s folder, on the
/// new branch `worktree-agent-<id>` from where the `worktree.baseRef`
/// setting says ([`worktree::start_point`]).
pub(crate) async fn agent_worktree(
    owner: &AgentSession,
    agent_id: &str,
) -> anyhow::Result<Worktree> {
    let project = owner.working_folder();
    let name = format!("agent-{agent_id}");
    let start = worktree::start_point(&project, owner.settings().worktree.base_ref).await;
    tokio::task::spawn_blocking(move || worktree::create(&project, &name, start.as_deref()))
        .await
        .context("the worktree could not be made")?
        .map_err(|error| {
            anyhow::anyhow!(
                "Could not make a worktree for the agent: {error}. Leave isolation out to run the agent in this folder."
            )
        })
}
