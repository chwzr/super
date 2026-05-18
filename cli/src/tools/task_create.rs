use crate::state::store::{TaskRecord, TaskStatus};
use crate::tools::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

pub struct TaskCreateTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TaskCreateTool {
    fn name(&self) -> &str {
        "TaskCreate"
    }
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
                "subject": {
                    "type": "string",
                    "description": "A brief, actionable title in imperative form (e.g., \"Fix authentication bug in login flow\")"
                },
                "description": {
                    "type": "string",
                    "description": "What needs to be done"
                },
                "activeForm": {
                    "type": "string",
                    "description": "Present continuous form shown in the spinner when the task is in_progress (e.g., \"Fixing authentication bug\"). If omitted, the spinner shows the subject instead."
                },
                "metadata": {
                    "type": "object",
                    "description": "Arbitrary metadata to attach to the task"
                }
            },
            "required": ["subject", "description"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "subject": {"type": "string"}
                    }
                }
            },
            "required": ["task"]
        }))
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let subject = input["subject"].as_str().unwrap_or("").to_string();
        let description = input["description"].as_str().unwrap_or("").to_string();
        let active_form = input["activeForm"].as_str().map(|s| s.to_string());
        let metadata = input.get("metadata").cloned();
        let id = Uuid::new_v4().to_string();
        let output = json!({"task": {"id": id.clone(), "subject": subject.clone()}}).to_string();
        let task = TaskRecord {
            id: id.clone(),
            subject,
            description,
            active_form,
            status: TaskStatus::Pending,
            owner: None,
            blocks: vec![],
            blocked_by: vec![],
            metadata,
        };
        self.store.set_state(move |s| {
            s.tasks.insert(id, task);
        });
        ToolResult {
            content: output,
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
