use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use crate::tools::contract::{DescriptionCtx, PromptCtx, RenderOpts, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent, ProgressSink};
use crate::state::store::TaskStatus;

pub struct TaskUpdateTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TaskUpdateTool {
    fn name(&self) -> &str { "TaskUpdate" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Updates a task's status. Status: pending, in_progress, completed, failed, deleted."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/task_update.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "taskId": {"type": "string"},
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "failed", "deleted"]
                }
            },
            "required": ["taskId", "status"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let task_id = input["taskId"].as_str().unwrap_or("");
        let status_str = input["status"].as_str().unwrap_or("");
        let status = match status_str {
            "pending" => TaskStatus::Pending,
            "in_progress" => TaskStatus::InProgress,
            "completed" => TaskStatus::Completed,
            "failed" => TaskStatus::Failed,
            "deleted" => TaskStatus::Deleted,
            _ => {
                return ToolResult {
                    content: format!("Unknown status: {status_str}"),
                    is_error: true,
                    ..Default::default()
                };
            }
        };
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
                t.status = status;
            }
        });
        ToolResult {
            content: format!("Task {task_id} updated to {status_str}"),
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