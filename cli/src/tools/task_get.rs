use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use crate::tools::contract::{DescriptionCtx, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent, ProgressSink};

pub struct TaskGetTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TaskGetTool {
    fn name(&self) -> &str { "TaskGet" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Retrieves a task by its ID.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/task_get.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "taskId": {"type": "string"}
            },
            "required": ["taskId"]
        })
    }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }
    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": ["object", "null"],
            "properties": {
                "id": {"type": "string"},
                "subject": {"type": "string"},
                "description": {"type": "string"},
                "status": {"type": "string"},
                "blocks": {"type": "array", "items": {"type": "string"}},
                "blockedBy": {"type": "array", "items": {"type": "string"}}
            }
        }))
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let task_id = input["taskId"].as_str().unwrap_or("");
        let tasks = self.store.get_state().tasks;
        match tasks.get(task_id) {
            Some(task) => {
                let info = format!(
                    "ID: {}\nSubject: {}\nDescription: {}\nStatus: {:?}\nBlocks: {:?}\nBlocked By: {:?}",
                    task.id, task.subject, task.description, task.status, task.blocks, task.blocked_by
                );
                ToolResult {
                    content: info,
                    is_error: false,
                    ..Default::default()
                }
            }
            None => ToolResult {
                content: format!("Task not found: {task_id}"),
                is_error: true,
                ..Default::default()
            },
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