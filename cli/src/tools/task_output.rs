use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use crate::tools::contract::{DescriptionCtx, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent, ProgressSink};

pub struct TaskOutputTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TaskOutputTool {
    fn name(&self) -> &str { "TaskOutput" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Returns output/info for a task by its ID.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/task_output.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {"type": "string", "description": "The task ID to get output from"},
                "block": {"type": "boolean", "default": true, "description": "Whether to wait for completion"},
                "timeout": {"type": "integer", "minimum": 0, "maximum": 600000, "default": 30000, "description": "Max wait time in ms"}
            },
            "required": ["task_id", "block", "timeout"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "task_id": {"type": "string"},
                "status": {"type": "string"},
                "output": {"type": "string"}
            }
        }))
    }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let task_id = input["task_id"].as_str().unwrap_or("");
        let block = input["block"].as_bool().unwrap_or(true);
        let timeout = input["timeout"].as_i64().unwrap_or(30000);
        let tasks = self.store.get_state().tasks;
        match tasks.get(task_id) {
            Some(task) => {
                let info = format!(
                    "Task: {}\n  ID:     {}\n  Status: {:?}\n  Desc:   {}",
                    task.subject, task.id, task.status, task.description
                );
                let metadata_map: std::collections::HashMap<String, String> = [
                    ("task_id".into(), task.id.clone()),
                    ("status".into(), format!("{:?}", task.status)),
                    ("block".into(), block.to_string()),
                    ("timeout".into(), timeout.to_string()),
                ].into();
                ToolResult {
                    content: info,
                    is_error: false,
                    metadata: Some(metadata_map),
                    inject_messages: Vec::new(),
                    mcp_meta: None,
                    new_messages: Vec::new(),
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