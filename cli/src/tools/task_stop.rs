use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};
use crate::state::store::TaskStatus;

pub struct TaskStopTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TaskStopTool {
    fn name(&self) -> &str { "TaskStop" }
    fn description(&self) -> &str { "Stops a running task and marks it as failed." }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {"type": "string"}
            },
            "required": ["task_id"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let task_id = input["task_id"].as_str().unwrap_or("");
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
}