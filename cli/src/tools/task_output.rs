use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct TaskOutputTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TaskOutputTool {
    fn name(&self) -> &str { "TaskOutput" }
    fn description(&self) -> &str { "Returns output/info for a task by its ID." }
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
                    "Task: {}\n  ID:     {}\n  Status: {:?}\n  Desc:   {}",
                    task.subject, task.id, task.status, task.description
                );
                ToolResult {
                    content: info,
                    is_error: false,
                    metadata: Some([
                        ("task_id".into(), task.id.clone()),
                        ("status".into(), format!("{:?}", task.status)),
                    ].into()),
                    inject_messages: Vec::new(),
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