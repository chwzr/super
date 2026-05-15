use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct TaskGetTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TaskGetTool {
    fn name(&self) -> &str { "TaskGet" }
    fn description(&self) -> &str { "Retrieves a task by its ID." }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "taskId": {"type": "string"}
            },
            "required": ["taskId"]
        })
    }
    fn is_read_only(&self) -> bool { true }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
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
}