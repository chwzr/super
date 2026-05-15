use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct TaskListTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TaskListTool {
    fn name(&self) -> &str { "TaskList" }
    fn description(&self) -> &str { "Lists all tracked tasks." }
    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object", "properties": {}})
    }
    fn is_read_only(&self) -> bool { true }

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let tasks = self.store.get_state().tasks;
        if tasks.is_empty() {
            return ToolResult {
                content: "No tasks.".into(),
                is_error: false,
                ..Default::default()
            };
        }
        let mut lines = vec!["Tasks:".to_string()];
        for t in tasks.values() {
            let short_id: String = t.id.chars().take(8).collect();
            lines.push(format!("  [{}] {} — {:?}", short_id, t.subject, t.status));
        }
        ToolResult {
            content: lines.join("\n"),
            is_error: false,
            ..Default::default()
        }
    }
}