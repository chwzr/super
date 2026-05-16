use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use uuid::Uuid;
use crate::tools::contract::{DescriptionCtx, PromptCtx, RenderOpts, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent, ProgressSink};
use crate::state::store::{TaskRecord, TaskStatus};

pub struct TaskCreateTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TaskCreateTool {
    fn name(&self) -> &str { "TaskCreate" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Creates a new task for tracking complex work.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/task_create.txt").into()
    }
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

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
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