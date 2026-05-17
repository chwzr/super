use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use crate::tools::contract::{DescriptionCtx, PromptCtx, RenderOpts, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent, ProgressSink};
use crate::state::store::TaskStatus;

pub struct TaskStopTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TaskStopTool {
    fn name(&self) -> &str { "TaskStop" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Stops a running task and marks it as failed.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/task_stop.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {"type": "string", "description": "The ID of the background task to stop"},
                "shell_id": {"type": "string", "description": "Deprecated: use task_id instead"}
            }
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "message": {"type": "string"},
                "task_id": {"type": "string"},
                "task_type": {"type": "string"}
            }
        }))
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let task_id = input.get("task_id")
            .or_else(|| input.get("shell_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let tid = task_id.to_string();

        let found = self.store.get_state().tasks.contains_key(&tid);
        if !found {
            return ToolResult {
                content: format!("Task not found: {task_id}"),
                is_error: true,
                ..Default::default()
            };
        }

        self.store.set_state(move |s| {
            if let Some(t) = s.tasks.get_mut(&tid) {
                t.status = TaskStatus::Failed;
            }
        });

        ToolResult {
            content: format!("Task {task_id} stopped."),
            is_error: false,
            ..Default::default()
        }
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(
                output.as_str().map(String::from).unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}