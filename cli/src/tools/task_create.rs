use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use uuid::Uuid;
use super::contract::{Tool, ToolCallContext, ToolResult};
use crate::state::store::{TaskRecord, TaskStatus};

pub struct TaskCreateTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TaskCreateTool {
    fn name(&self) -> &str { "TaskCreate" }
    fn description(&self) -> &str { "Creates a new task for tracking complex work." }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "subject": {"type": "string"},
                "description": {"type": "string"},
                "activeForm": {"type": "string"}
            },
            "required": ["subject", "description"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let subject = input["subject"].as_str().unwrap_or("");
        let description = input["description"].as_str().unwrap_or("");
        let id = Uuid::new_v4().to_string();
        let task = TaskRecord {
            id: id.clone(),
            subject: subject.to_string(),
            description: description.to_string(),
            status: TaskStatus::Pending,
            blocks: vec![],
            blocked_by: vec![],
        };
        self.store.set_state(move |s| {
            s.tasks.insert(id.clone(), task);
        });
        ToolResult {
            content: format!("Task created: {subject}"),
            is_error: false,
            ..Default::default()
        }
    }
}