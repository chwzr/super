use crate::tools::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct TaskListTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TaskListTool {
    fn name(&self) -> &str {
        "TaskList"
    }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Lists all tracked tasks.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/task_list.txt").into()
    }
    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "subject": {"type": "string"},
                    "status": {"type": "string"},
                    "owner": {"type": "string"},
                    "blockedBy": {"type": "array", "items": {"type": "string"}}
                }
            }
        }))
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object", "properties": {}})
    }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(
        &self,
        _input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
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

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(
                output
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}
