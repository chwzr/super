//! The tool contract used by the agent runtime.

use crate::background::{ShellDirectory, TaskHost};
use crate::file_state::FileStates;
use serde_json::Value;
use std::sync::Arc;
use super_ai::{ContentBlock, ToolDef, Usage};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionMode {
    Sequential,
    #[default]
    Parallel,
}

#[derive(Debug, Clone, Default)]
pub struct ToolResult {
    /// Text or image content returned to the model.
    pub content: Vec<ContentBlock>,
    /// Structured details for logs / UI rendering (never sent to the model).
    pub details: Value,
    /// A tool can report a failure while retaining structured details and images.
    pub is_error: bool,
    /// Usage from nested LLM work performed by the tool, if any.
    pub usage: Option<Usage>,
    /// Hint that the agent should stop after the current tool batch.
    pub terminate: bool,
}

impl ToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        ToolResult {
            content: vec![ContentBlock::text(text)],
            ..Default::default()
        }
    }

    pub fn output_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Sink for streaming partial tool results (e.g. live bash output).
pub type ToolUpdateSink = Arc<dyn Fn(ToolResult) + Send + Sync>;

/// The free room of a session's context, in tokens, as the session estimates
/// it at the moment of the call. `None` when it cannot tell.
pub type ContextRoomFn = Arc<dyn Fn() -> Option<u64> + Send + Sync>;

/// What a tool may use of the session it runs for.
///
/// A session binds its tools to its own `ToolSession` with
/// [`AgentTool::for_session`], so that two sessions sharing one tool list (a
/// parent and its child agent) keep apart what is per session. A tool built
/// outside a session gets a fresh one of its own.
#[derive(Clone, Default)]
pub struct ToolSession {
    /// What the session has read of each file. `Read` writes it; `Edit` and
    /// `Write` check it.
    pub files: Arc<FileStates>,
    /// The free room of the session's context, for a read that may fill
    /// more of it than the default limit (`Read` with `allow_large`).
    pub context_room: Option<ContextRoomFn>,
    /// Where a `cd` left the shell, in a session whose `cd` carries over to
    /// the next command: the main session. `None` everywhere else, where
    /// every command starts in the session folder.
    pub shell: Option<Arc<ShellDirectory>>,
    /// The session's background tasks, for `Bash` with `run_in_background`.
    /// `None` outside a session.
    pub tasks: Option<Arc<dyn TaskHost>>,
}

impl ToolSession {
    /// A session state of its own, with an empty file record.
    pub fn new() -> Self {
        Self::default()
    }

    /// The free room of the context in tokens, when the session can tell.
    pub fn context_room(&self) -> Option<u64> {
        self.context_room.as_ref().and_then(|room| room())
    }
}

#[async_trait::async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn label(&self) -> &str {
        self.name()
    }
    fn description(&self) -> String;
    /// JSON schema object describing the arguments.
    fn parameters(&self) -> Value;
    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Parallel
    }
    /// Normalize raw arguments before schema validation (compat shims).
    fn prepare_arguments(&self, args: Value) -> Value {
        args
    }
    /// Whether this tool exists in the advanced mode only. A session in the
    /// basic mode (the `mode` setting) leaves such a tool out of every request,
    /// wherever it was added. A tool that is new with the advanced mode returns
    /// `true`; the core tools keep the default and are there in both modes.
    fn advanced_only(&self) -> bool {
        false
    }
    /// A copy of this tool that works with `session`, for a tool that keeps
    /// state per session (the file tools keep what was read). A session
    /// calls this for every tool it offers; `None`, the default, means the
    /// tool has no such state and is used as it is.
    fn for_session(&self, _session: &ToolSession) -> Option<DynTool> {
        None
    }
    /// Execute. Return Err on failure. The loop converts it into an error
    /// tool result visible to the model.
    async fn execute(
        &self,
        tool_call_id: &str,
        args: Value,
        cancel: CancellationToken,
        on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult>;

    fn to_def(&self) -> ToolDef {
        ToolDef {
            name: self.name().to_string(),
            description: self.description(),
            parameters: self.parameters(),
        }
    }
}

pub type DynTool = Arc<dyn AgentTool>;
