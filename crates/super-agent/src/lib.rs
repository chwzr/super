//! super-agent runtime: turn loop, events, tool contract, and
//! the four harness-core tools (Read, Bash, Edit, Write).

pub mod agent_loop;
pub mod background;
pub mod config;
pub mod events;
pub mod file_state;
pub mod message;
pub mod placeholder;
pub mod tokens;
pub mod tool;
#[cfg(feature = "native-tools")]
pub mod tools;
pub mod validate;

pub use agent_loop::{EventSink, run_agent_loop, run_agent_loop_continue};
pub use background::{HostedTask, ShellDirectory, TaskEnd, TaskEndStatus, TaskHost};
pub use config::{
    AgentContext, AgentLoopConfig, BeforeToolCallResult, StreamFn, TurnInfo, TurnUpdate,
};
pub use events::AgentEvent;
pub use file_state::{FileRead, FileStates};
pub use message::{
    AgentMessage, BashExecutionMessage, BranchSummaryMessage, CompactionSummaryMessage,
    CustomMessage, convert_to_llm,
};
pub use tool::{
    AgentTool, ContextRoomFn, DynTool, ExecutionMode, ToolResult, ToolSession, ToolUpdateSink,
};
