//! What a tool may keep per session for the shell: the folder a `cd` left
//! the shell in, and the session's background tasks.
//!
//! A session keeps its background tasks in a registry of its own (in
//! `super-coding`), which also announces a task's end to the agent. A tool in
//! this crate reaches that registry through [`TaskHost`], which the session
//! puts into the [`crate::ToolSession`] it binds its tools to. Outside a
//! session there is no host, and nothing can run in the background.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// The folder the shell of a session is in after a `cd`, for a session whose
/// `cd` carries over from one command to the next (the main session). `None`
/// is the session folder itself.
#[derive(Debug, Default)]
pub struct ShellDirectory {
    current: Mutex<Option<PathBuf>>,
}

impl ShellDirectory {
    pub fn new() -> Self {
        Self::default()
    }

    /// The folder the next command starts in, when it is not the session
    /// folder.
    pub fn get(&self) -> Option<PathBuf> {
        self.current.lock().unwrap().clone()
    }

    /// Set the folder the next command starts in; `None` is the session
    /// folder.
    pub fn set(&self, directory: Option<PathBuf>) {
        *self.current.lock().unwrap() = directory;
    }

    /// Put the shell back in the session folder.
    pub fn reset(&self) {
        self.set(None);
    }
}

/// How a background task ended by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskEndStatus {
    Completed,
    Failed,
    /// The task stopped itself, for example at its time limit.
    Stopped,
}

/// How a background task ended, as its notice reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEnd {
    pub status: TaskEndStatus,
    pub summary: String,
}

/// The session's registry of background tasks, as a tool sees it.
pub trait TaskHost: Send + Sync {
    /// Register a shell command that the tool call `tool_use_id` leaves
    /// running. The error is a sentence the tool can return to the model.
    fn start_shell(
        &self,
        description: &str,
        tool_use_id: &str,
    ) -> Result<Box<dyn HostedTask>, String>;
}

/// One registered background task, kept by the tool that runs it.
///
/// Dropping it without [`HostedTask::finish`] ends the task as failed
/// without a notice, for a task that could not start.
pub trait HostedTask: Send {
    /// The id the task is known by, which `TaskStop` takes.
    fn id(&self) -> &str;
    /// Where the task's output goes.
    fn output_file(&self) -> &Path;
    /// Cancelled when the task is stopped from outside (by `TaskStop`, a
    /// client, or because the session ends): the tool then ends its work.
    /// The task has ended already and gets no notice.
    fn stop_signal(&self) -> CancellationToken;
    /// End the task; the agent gets a notice.
    fn finish(self: Box<Self>, end: TaskEnd);
}
