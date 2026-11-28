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

use super::types::{AgentType, GENERAL_PURPOSE};
use crate::child_turn::{self, AgentStatus, ChildTurnOutcome};
use crate::notifications::{Priority, agent_message};
use crate::session_runner::AgentSession;
use crate::tasks::{TaskHandle, TaskKind, TaskOutcome, TaskSpec, TaskStatus, TaskUsage};
use anyhow::Context as _;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use super_agent::AgentMessage;
use super_ai::{ContentBlock, Model, ThinkingLevel, UserContent, UserMessage};
use tokio_util::sync::CancellationToken;

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
    session: Arc<AgentSession>,
    state: Mutex<RunState>,
}

struct RunState {
    status: TaskStatus,
    background: bool,
    /// Ends the run in flight: the task's stop signal for a background run,
    /// a token of the caller's turn for a foreground one.
    stop: Option<CancellationToken>,
}

impl AgentRecord {
    /// A record of a running agent, as [`AgentRegistry::insert`] takes it.
    pub(crate) fn new(
        id: String,
        name: String,
        agent_type: String,
        session: Arc<AgentSession>,
        background: bool,
    ) -> Self {
        AgentRecord {
            id,
            name,
            agent_type,
            session,
            state: Mutex::new(RunState {
                status: TaskStatus::Running,
                background,
                stop: None,
            }),
        }
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
            session_file: self
                .session
                .manager
                .lock()
                .unwrap()
                .session_file()
                .map(Path::to_path_buf),
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
    /// Held while an agent is checked against the limit and started.
    starting: Mutex<()>,
}

impl AgentRegistry {
    pub(crate) fn new(owner: Weak<AgentSession>, depth: usize) -> Arc<Self> {
        Arc::new(AgentRegistry {
            owner,
            depth,
            records: Mutex::new(Vec::new()),
            starting: Mutex::new(()),
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

    /// The agent with this id, else the one agent with this name.
    pub fn find(&self, id_or_name: &str) -> anyhow::Result<AgentInfo> {
        let record = self.resolve(id_or_name)?;
        Ok(record.info(self.owner.upgrade().as_deref()))
    }

    /// The session of the agent with this id or name.
    pub fn session_of(&self, id_or_name: &str) -> anyhow::Result<Arc<AgentSession>> {
        Ok(self.resolve(id_or_name)?.session.clone())
    }

    /// Add an agent. A record built from a stored session joins the same
    /// way as one [`AgentRegistry::start`] made.
    pub(crate) fn insert(&self, record: Arc<AgentRecord>) {
        self.records.lock().unwrap().push(record);
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

        let record = {
            let _starting = self.starting.lock().unwrap();
            self.check_room()?;
            let id = owner.background_tasks().new_id(TaskKind::Agent);
            if request.isolation == Some(Isolation::Worktree) {
                let _folder = agent_worktree(&owner, &id)?;
            }
            let child = owner.create_agent_session(crate::session_runner::AgentChild {
                id: &id,
                agent_type,
                description,
                model,
                thinking,
                depth: self.depth + 1,
                tool_use_id,
            })?;
            let record = Arc::new(AgentRecord::new(
                id,
                description.to_string(),
                agent_type.name.clone(),
                child,
                request.background,
            ));
            self.insert(record.clone());
            record
        };
        let prompt = first_message(agent_type, &request.prompt);

        if request.background {
            let handle = self.register_run(&owner, &record, tool_use_id)?;
            self.spawn_run(record.clone(), handle, prompt);
            return Ok(Started::Background(record.info(Some(&owner))));
        }
        let stop = cancel.child_token();
        record.begin_run(false, stop.clone());
        let outcome =
            child_turn::run_child_turn(&self.owner, &record.session, prompt, Some(stop), None)
                .await;
        record.end_run(&outcome);
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
        let record = self.resolve(to)?;
        let _starting = self.starting.lock().unwrap();
        let note = agent_message("main", &record.id, summary, message);
        if record.is_running() {
            if !record.info(Some(&owner)).status.is_running() {
                anyhow::bail!(
                    "Agent {} is stopping; send the message again once it has stopped.",
                    record.id
                );
            }
            record.session.queue_entry(note, Priority::Next);
            return Ok(Delivery::Queued(record.info(Some(&owner))));
        }
        self.check_room()?;
        let handle = self.register_run(&owner, &record, tool_use_id)?;
        self.spawn_run(record.clone(), handle, note);
        Ok(Delivery::Resumed(record.info(Some(&owner))))
    }

    /// Stop every agent that runs: a background run through the task
    /// registry, so it ends without a notice, a foreground one by ending
    /// its run.
    pub(crate) fn stop_all(&self) {
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
    }

    fn resolve(&self, id_or_name: &str) -> anyhow::Result<Arc<AgentRecord>> {
        let records = self.records.lock().unwrap();
        if let Some(record) = records.iter().find(|record| record.id == id_or_name) {
            return Ok(record.clone());
        }
        let named: Vec<&Arc<AgentRecord>> = records
            .iter()
            .filter(|record| record.name == id_or_name)
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
            .filter(|record| record.info(owner.as_deref()).status.is_running())
            .count();
        if running >= MAX_RUNNING {
            anyhow::bail!(
                "{MAX_RUNNING} agents of this session run already, the most at once; wait for one to end or stop one with TaskStop."
            );
        }
        Ok(())
    }

    /// Register the background run of `record` as a task with the agent's
    /// id, its transcript as the output file.
    fn register_run(
        &self,
        owner: &AgentSession,
        record: &Arc<AgentRecord>,
        tool_use_id: &str,
    ) -> anyhow::Result<TaskHandle> {
        let mut spec = TaskSpec::new(TaskKind::Agent, record.name.clone()).with_id(&record.id);
        if !tool_use_id.is_empty() {
            spec = spec.with_tool_use_id(tool_use_id);
        }
        if let Some(file) = record.session.manager.lock().unwrap().session_file() {
            spec = spec.with_output_file(file);
        }
        let handle = owner
            .background_tasks()
            .register(spec)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        record.begin_run(true, handle.stop_signal());
        Ok(handle)
    }

    fn spawn_run(
        self: &Arc<Self>,
        record: Arc<AgentRecord>,
        handle: TaskHandle,
        prompt: AgentMessage,
    ) {
        let owner = self.owner.clone();
        tokio::spawn(async move {
            let outcome = child_turn::run_child_turn(
                &owner,
                &record.session,
                prompt,
                Some(handle.stop_signal()),
                None,
            )
            .await;
            record.end_run(&outcome);
            handle.finish(task_outcome(&record.name, &outcome));
        });
    }
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

/// The folder of the git worktree an agent with `isolation: "worktree"`
/// works in. Not built yet: the call fails, and no agent starts.
pub(crate) fn agent_worktree(_owner: &AgentSession, _agent_id: &str) -> anyhow::Result<PathBuf> {
    anyhow::bail!(
        "isolation 'worktree' is not available yet; leave isolation out to run the agent in this folder."
    )
}
