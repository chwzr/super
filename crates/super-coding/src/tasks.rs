//! The background tasks of a session: shell commands, monitors, agents and
//! workflows that keep running after the tool call that started them.
//!
//! A tool registers its task with [`BackgroundTasks::register`] and keeps
//! the [`TaskHandle`]. The handle tells the task when it is asked to stop
//! ([`TaskHandle::stop_signal`]) and ends it ([`TaskHandle::finish`]): the
//! registry then queues the task's notice
//! ([`crate::notifications::task_notification`]) at
//! [`Priority::Next`](crate::notifications::Priority::Next), so an idle agent
//! starts a new turn and a busy one reads the notice at its next step.
//!
//! A task stopped through the registry ([`BackgroundTasks::stop`],
//! [`BackgroundTasks::stop_all`]) ends at once with the status `stopped`
//! and queues no notice: whoever stopped it, the model through a tool or the
//! user, knows already. What the task does when it is told to stop (kill a
//! process, abort a child session) is up to its owner.
//!
//! Every change is reported to the session's clients as
//! [`SessionEvent::TaskStarted`], [`SessionEvent::TaskUpdated`] and
//! [`SessionEvent::TaskEnded`].

use crate::notifications::{Priority, task_notification};
use crate::session_runner::{AgentSession, SessionEvent};
use rand::Rng as _;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use super_agent::{HostedTask, TaskEnd, TaskEndStatus, TaskHost};
use tokio_util::sync::CancellationToken;

/// At most this many tasks are remembered; the oldest ended ones go first.
const MAX_RECORDS: usize = 256;

/// What kind of work a task is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Shell,
    Monitor,
    Agent,
    Workflow,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskKind::Shell => "shell",
            TaskKind::Monitor => "monitor",
            TaskKind::Agent => "agent",
            TaskKind::Workflow => "workflow",
        }
    }

    /// The first character of the ids the registry gives this kind.
    fn id_prefix(self) -> char {
        match self {
            TaskKind::Shell => 'b',
            TaskKind::Monitor => 'm',
            TaskKind::Agent => 'a',
            TaskKind::Workflow => 'w',
        }
    }
}

/// Where a task stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
    Stopped,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Stopped => "stopped",
        }
    }

    pub fn is_running(self) -> bool {
        self == TaskStatus::Running
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a tool says about the task it registers.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskSpec {
    pub kind: TaskKind,
    /// The id to use; `None` lets the registry make one (`b…` for a shell
    /// command, `m…` for a monitor, `a…` for an agent, `w…` for a workflow).
    pub id: Option<String>,
    /// A name the task can also be found and stopped by. Unique among the
    /// running tasks of the session.
    pub name: Option<String>,
    /// A short description for the clients' task lists.
    pub description: String,
    /// The id of the tool call that started the task.
    pub tool_use_id: Option<String>,
    /// Where the task's full output or transcript is written.
    pub output_file: Option<PathBuf>,
}

impl TaskSpec {
    pub fn new(kind: TaskKind, description: impl Into<String>) -> Self {
        TaskSpec {
            kind,
            id: None,
            name: None,
            description: description.into(),
            tool_use_id: None,
            output_file: None,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_tool_use_id(mut self, tool_use_id: impl Into<String>) -> Self {
        self.tool_use_id = Some(tool_use_id.into());
        self
    }

    pub fn with_output_file(mut self, output_file: impl Into<PathBuf>) -> Self {
        self.output_file = Some(output_file.into());
        self
    }
}

/// A task as the registry and its clients see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub kind: TaskKind,
    pub description: String,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file: Option<PathBuf>,
    /// A short line on what the task is doing now, set by its owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    /// The summary it ended with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
}

/// What an agent task used, for the `<usage>` element of its notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUsage {
    pub subagent_tokens: u64,
    pub tool_uses: u64,
    pub duration_ms: u64,
}

/// How a task ended, as its notice reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOutcome {
    /// `completed`, `failed` or `stopped`; `running` counts as `failed`.
    pub status: TaskStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Present for agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TaskUsage>,
}

impl TaskOutcome {
    fn with_status(status: TaskStatus, summary: impl Into<String>) -> Self {
        TaskOutcome {
            status,
            summary: summary.into(),
            result: None,
            usage: None,
        }
    }

    pub fn completed(summary: impl Into<String>) -> Self {
        Self::with_status(TaskStatus::Completed, summary)
    }

    pub fn failed(summary: impl Into<String>) -> Self {
        Self::with_status(TaskStatus::Failed, summary)
    }

    /// Stopped by the task itself, for example a monitor at its time limit.
    pub fn stopped(summary: impl Into<String>) -> Self {
        Self::with_status(TaskStatus::Stopped, summary)
    }

    pub fn with_result(mut self, result: impl Into<String>) -> Self {
        self.result = Some(result.into());
        self
    }

    pub fn with_usage(mut self, usage: TaskUsage) -> Self {
        self.usage = Some(usage);
        self
    }
}

/// Why a task could not be registered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskRegisterError {
    #[error("A background task with the id '{0}' exists already. Use another id.")]
    DuplicateId(String),
    #[error(
        "A running background task is named '{0}' already. Choose another name or stop that task first."
    )]
    DuplicateName(String),
}

/// Why a task could not be stopped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskStopError {
    #[error("No background task has the id or name '{0}'. Check the id of a running task.")]
    Unknown(String),
    #[error("Background task {id} is not running: it {status} already.")]
    NotRunning { id: String, status: TaskStatus },
}

struct TaskRecord {
    info: Mutex<TaskInfo>,
    stop: CancellationToken,
}

/// The background tasks of one session. Every [`AgentSession`] has one:
/// [`AgentSession::background_tasks`].
pub struct BackgroundTasks {
    session: Weak<AgentSession>,
    records: Mutex<Vec<Arc<TaskRecord>>>,
    holds: Arc<AtomicUsize>,
}

impl BackgroundTasks {
    pub(crate) fn new(session: Weak<AgentSession>) -> Arc<Self> {
        Arc::new(BackgroundTasks {
            session,
            records: Mutex::new(Vec::new()),
            holds: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Register a running task. The caller keeps the handle and ends the
    /// task with [`TaskHandle::finish`].
    pub fn register(self: &Arc<Self>, spec: TaskSpec) -> Result<TaskHandle, TaskRegisterError> {
        let info = {
            let mut records = self.records.lock().unwrap();
            let id = match spec.id {
                Some(id) => {
                    if records
                        .iter()
                        .any(|record| record.info.lock().unwrap().id == id)
                    {
                        return Err(TaskRegisterError::DuplicateId(id));
                    }
                    id
                }
                None => loop {
                    let id = new_task_id(spec.kind);
                    if !records
                        .iter()
                        .any(|record| record.info.lock().unwrap().id == id)
                    {
                        break id;
                    }
                },
            };
            if let Some(name) = &spec.name
                && records.iter().any(|record| {
                    let info = record.info.lock().unwrap();
                    info.status.is_running() && info.name.as_ref() == Some(name)
                })
            {
                return Err(TaskRegisterError::DuplicateName(name.clone()));
            }
            let info = TaskInfo {
                id: id.clone(),
                name: spec.name,
                kind: spec.kind,
                description: spec.description,
                status: TaskStatus::Running,
                tool_use_id: spec.tool_use_id,
                output_file: spec.output_file,
                progress: None,
                summary: None,
                started_at: super_ai::now_ms(),
                ended_at: None,
            };
            records.push(Arc::new(TaskRecord {
                info: Mutex::new(info.clone()),
                stop: CancellationToken::new(),
            }));
            prune(&mut records);
            info
        };
        let stop = self
            .record(&info.id)
            .map(|record| record.stop.clone())
            .unwrap_or_default();
        self.emit(SessionEvent::TaskStarted(info.clone()));
        Ok(TaskHandle {
            id: info.id,
            stop,
            tasks: Arc::downgrade(self),
            finished: false,
        })
    }

    /// Every task the session remembers, oldest first.
    pub fn list(&self) -> Vec<TaskInfo> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|record| record.info.lock().unwrap().clone())
            .collect()
    }

    /// The tasks that are still running, oldest first.
    pub fn running(&self) -> Vec<TaskInfo> {
        self.list()
            .into_iter()
            .filter(|task| task.status.is_running())
            .collect()
    }

    pub fn has_running(&self) -> bool {
        self.records
            .lock()
            .unwrap()
            .iter()
            .any(|record| record.info.lock().unwrap().status.is_running())
    }

    /// The task with this id, else the newest task with this name.
    pub fn find(&self, id_or_name: &str) -> Option<TaskInfo> {
        self.find_record(id_or_name)
            .map(|record| record.info.lock().unwrap().clone())
    }

    /// Stop a running task by id or by name. It ends at once with the status
    /// `stopped`, its owner is told through [`TaskHandle::stop_signal`], and
    /// no notice is queued.
    pub fn stop(&self, id_or_name: &str) -> Result<TaskInfo, TaskStopError> {
        let record = self
            .find_record(id_or_name)
            .ok_or_else(|| TaskStopError::Unknown(id_or_name.to_string()))?;
        let ended = {
            let mut info = record.info.lock().unwrap();
            if !info.status.is_running() {
                return Err(TaskStopError::NotRunning {
                    id: info.id.clone(),
                    status: info.status,
                });
            }
            info.status = TaskStatus::Stopped;
            info.ended_at = Some(super_ai::now_ms());
            info.clone()
        };
        record.stop.cancel();
        self.ended(&ended);
        Ok(ended)
    }

    /// Stop every running task; returns the ones that were stopped.
    pub fn stop_all(&self) -> Vec<TaskInfo> {
        let ids: Vec<String> = self.running().into_iter().map(|task| task.id).collect();
        ids.iter().filter_map(|id| self.stop(id).ok()).collect()
    }

    /// Keep the session loaded while the returned guard lives, for work
    /// that is not a task but will reach the agent later, such as a pending
    /// wake-up.
    pub fn hold(&self) -> KeepAlive {
        self.holds.fetch_add(1, Ordering::SeqCst);
        KeepAlive {
            holds: self.holds.clone(),
        }
    }

    /// Whether a task runs or a [`KeepAlive`] is held.
    pub fn is_busy(&self) -> bool {
        self.holds.load(Ordering::SeqCst) > 0 || self.has_running()
    }

    /// A new id for a task of `kind` that no remembered task has, for a tool
    /// that needs the id before it registers the task (to name its output
    /// file, say).
    pub fn new_id(&self, kind: TaskKind) -> String {
        loop {
            let id = new_task_id(kind);
            if self.record(&id).is_none() {
                return id;
            }
        }
    }

    /// This registry as the tools of its session reach it, for `Bash` with
    /// `run_in_background`; it goes into the session's
    /// [`super_agent::ToolSession`].
    pub(crate) fn host(self: &Arc<Self>) -> Arc<dyn TaskHost> {
        Arc::new(ToolTaskHost(Arc::downgrade(self)))
    }

    /// Where a task with this id writes its output when its tool has no
    /// better place: `<temp>/super-tasks/<session id>/<id>.output`. The
    /// folder is created.
    pub fn output_file_for(&self, id: &str) -> PathBuf {
        let session = self
            .session
            .upgrade()
            .map(|session| session.manager.lock().unwrap().session_id().to_string())
            .unwrap_or_else(|| "detached".into());
        let directory = std::env::temp_dir().join("super-tasks").join(session);
        let _ = std::fs::create_dir_all(&directory);
        directory.join(format!("{id}.output"))
    }

    /// Stop every task and forget them all, for a session that switches to
    /// another conversation.
    pub(crate) fn reset(&self) {
        self.stop_all();
        self.records.lock().unwrap().clear();
    }

    fn record(&self, id: &str) -> Option<Arc<TaskRecord>> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .find(|record| record.info.lock().unwrap().id == id)
            .cloned()
    }

    fn find_record(&self, id_or_name: &str) -> Option<Arc<TaskRecord>> {
        let records = self.records.lock().unwrap();
        records
            .iter()
            .find(|record| record.info.lock().unwrap().id == id_or_name)
            .or_else(|| {
                records
                    .iter()
                    .rev()
                    .find(|record| record.info.lock().unwrap().name.as_deref() == Some(id_or_name))
            })
            .cloned()
    }

    fn emit(&self, event: SessionEvent) {
        if let Some(session) = self.session.upgrade() {
            session.emit_event(event);
        }
    }

    fn ended(&self, task: &TaskInfo) {
        self.emit(SessionEvent::TaskEnded(task.clone()));
        if let Some(session) = self.session.upgrade() {
            session.note_activity();
        }
    }

    fn set_progress(&self, id: &str, progress: Option<String>) {
        let Some(record) = self.record(id) else {
            return;
        };
        let updated = {
            let mut info = record.info.lock().unwrap();
            if !info.status.is_running() || info.progress == progress {
                return;
            }
            info.progress = progress;
            info.clone()
        };
        self.emit(SessionEvent::TaskUpdated(updated));
    }

    /// End a task its owner finished. Returns whether a notice was queued.
    fn finish(&self, id: &str, outcome: Option<TaskOutcome>) -> bool {
        let Some(record) = self.record(id) else {
            return false;
        };
        let ended = {
            let mut info = record.info.lock().unwrap();
            if !info.status.is_running() {
                // Stopped through the registry: that ended it already.
                return false;
            }
            info.status = match outcome.as_ref().map(|outcome| outcome.status) {
                Some(TaskStatus::Completed) => TaskStatus::Completed,
                Some(TaskStatus::Stopped) => TaskStatus::Stopped,
                Some(TaskStatus::Failed | TaskStatus::Running) | None => TaskStatus::Failed,
            };
            info.summary = outcome.as_ref().map(|outcome| outcome.summary.clone());
            info.ended_at = Some(super_ai::now_ms());
            info.clone()
        };
        let Some(mut outcome) = outcome else {
            self.ended(&ended);
            return false;
        };
        outcome.status = ended.status;
        self.emit(SessionEvent::TaskEnded(ended.clone()));
        match self.session.upgrade() {
            Some(session) => {
                session.queue_entry(task_notification(&ended, &outcome), Priority::Next);
                true
            }
            None => false,
        }
    }
}

impl Drop for BackgroundTasks {
    /// The session is gone: nothing would hear from its tasks any more.
    fn drop(&mut self) {
        for record in self.records.get_mut().unwrap().iter() {
            record.stop.cancel();
        }
    }
}

/// Forget the oldest ended tasks beyond [`MAX_RECORDS`].
fn prune(records: &mut Vec<Arc<TaskRecord>>) {
    while records.len() > MAX_RECORDS {
        let Some(position) = records
            .iter()
            .position(|record| !record.info.lock().unwrap().status.is_running())
        else {
            return;
        };
        records.remove(position);
    }
}

/// A kind's prefix and eight random lower-case letters and digits.
fn new_task_id(kind: TaskKind) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    let mut id = String::with_capacity(9);
    id.push(kind.id_prefix());
    for _ in 0..8 {
        id.push(ALPHABET[rng.random_range(0..ALPHABET.len())] as char);
    }
    id
}

/// What the owner of a registered task keeps.
///
/// Dropping a handle without [`TaskHandle::finish`] ends the task as
/// `failed` without a notice, so a task whose owner went away does not keep
/// the session busy.
pub struct TaskHandle {
    id: String,
    stop: CancellationToken,
    tasks: Weak<BackgroundTasks>,
    finished: bool,
}

impl TaskHandle {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Cancelled when the task is stopped through the registry: by the
    /// model, by a client, or because the session ends.
    pub fn stop_signal(&self) -> CancellationToken {
        self.stop.clone()
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.is_cancelled()
    }

    /// Show a short line on what the task is doing now in the clients'
    /// task lists; `None` clears it.
    pub fn set_progress(&self, progress: Option<String>) {
        if let Some(tasks) = self.tasks.upgrade() {
            tasks.set_progress(&self.id, progress);
        }
    }

    /// End the task and queue its notice. A task that was stopped through
    /// the registry ended already: nothing more happens and `false` comes
    /// back.
    pub fn finish(mut self, outcome: TaskOutcome) -> bool {
        self.finished = true;
        match self.tasks.upgrade() {
            Some(tasks) => tasks.finish(&self.id, Some(outcome)),
            None => false,
        }
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        if !self.finished
            && let Some(tasks) = self.tasks.upgrade()
        {
            tasks.finish(&self.id, None);
        }
    }
}

/// The registry as a tool in `super-agent` sees it: `Bash` registers a
/// command it leaves running here.
struct ToolTaskHost(Weak<BackgroundTasks>);

impl TaskHost for ToolTaskHost {
    fn start_shell(
        &self,
        description: &str,
        tool_use_id: &str,
    ) -> Result<Box<dyn HostedTask>, String> {
        let tasks = self.0.upgrade().ok_or_else(|| {
            "The session has ended, so the command cannot run in the background.".to_string()
        })?;
        let id = tasks.new_id(TaskKind::Shell);
        let output = tasks.output_file_for(&id);
        let handle = tasks
            .register(
                TaskSpec::new(TaskKind::Shell, description)
                    .with_id(id)
                    .with_tool_use_id(tool_use_id)
                    .with_output_file(&output),
            )
            .map_err(|error| error.to_string())?;
        Ok(Box::new(HostedShell { handle, output }))
    }
}

/// A shell command `Bash` left running, with the handle of its task.
struct HostedShell {
    handle: TaskHandle,
    output: PathBuf,
}

impl HostedTask for HostedShell {
    fn id(&self) -> &str {
        self.handle.id()
    }

    fn output_file(&self) -> &Path {
        &self.output
    }

    fn stop_signal(&self) -> CancellationToken {
        self.handle.stop_signal()
    }

    fn finish(self: Box<Self>, end: TaskEnd) {
        let outcome = match end.status {
            TaskEndStatus::Completed => TaskOutcome::completed(end.summary),
            TaskEndStatus::Failed => TaskOutcome::failed(end.summary),
            TaskEndStatus::Stopped => TaskOutcome::stopped(end.summary),
        };
        self.handle.finish(outcome);
    }
}

/// Keeps the session loaded while it lives; see [`BackgroundTasks::hold`].
pub struct KeepAlive {
    holds: Arc<AtomicUsize>,
}

impl Drop for KeepAlive {
    fn drop(&mut self) {
        self.holds.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_name_their_kind() {
        let id = new_task_id(TaskKind::Shell);
        assert_eq!(id.len(), 9);
        assert!(id.starts_with('b'));
        assert!(
            id[1..]
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        );
        assert!(new_task_id(TaskKind::Monitor).starts_with('m'));
        assert!(new_task_id(TaskKind::Agent).starts_with('a'));
        assert!(new_task_id(TaskKind::Workflow).starts_with('w'));
    }

    #[test]
    fn a_task_on_the_wire_uses_camel_case_and_lower_case_values() {
        let task = TaskInfo {
            id: "b1".into(),
            name: Some("tests".into()),
            kind: TaskKind::Shell,
            description: "cargo test".into(),
            status: TaskStatus::Running,
            tool_use_id: Some("call_1".into()),
            output_file: None,
            progress: None,
            summary: None,
            started_at: 5,
            ended_at: None,
        };
        let value = serde_json::to_value(&task).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "id": "b1", "name": "tests", "kind": "shell", "description": "cargo test",
                "status": "running", "toolUseId": "call_1", "startedAt": 5,
            })
        );
        assert_eq!(serde_json::from_value::<TaskInfo>(value).unwrap(), task);
    }
}
