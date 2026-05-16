use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use super::contract::{DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent};

pub struct CronJob {
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    pub durable: bool,
}

pub struct CronCreateTool {
    pub jobs: Arc<Mutex<HashMap<String, CronJob>>>,
}

#[async_trait]
impl Tool for CronCreateTool {
    fn name(&self) -> &str { "CronCreate" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Schedule a prompt to be enqueued at a future time via a cron expression. \
         Supports recurring and one-shot schedules.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/cron_create.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "cron": {"type": "string", "description": "Standard 5-field cron expression: M H DoM Mon DoW"},
                "prompt": {"type": "string", "description": "The prompt to enqueue at each fire time"},
                "recurring": {"type": "boolean", "description": "If true, fires on every cron match; if false, fires once then auto-deletes", "default": true},
                "durable": {"type": "boolean", "description": "If true, persists across restarts", "default": false}
            },
            "required": ["cron", "prompt"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let cron = input["cron"].as_str().unwrap_or("").to_string();
        let prompt = input["prompt"].as_str().unwrap_or("").to_string();
        let recurring = input.get("recurring").and_then(|v| v.as_bool()).unwrap_or(true);
        let durable = input.get("durable").and_then(|v| v.as_bool()).unwrap_or(false);

        // Validate 5-field cron expression
        let fields: Vec<&str> = cron.split_whitespace().collect();
        if fields.len() != 5 {
            return ToolResult {
                content: "Invalid cron expression: must have exactly 5 space-separated fields (minute hour day-of-month month day-of-week)".into(),
                is_error: true,
                inject_messages: Vec::new(),
                metadata: None,
                mcp_meta: None,
                new_messages: Vec::new(),
            };
        }

        let id = uuid::Uuid::new_v4().to_string();
        let job = CronJob { cron: cron.clone(), prompt, recurring, durable };

        let mut jobs = self.jobs.lock().unwrap();
        jobs.insert(id.clone(), job);

        ToolResult {
            content: format!("Cron job created with ID: {id} — \"{cron}\" (recurring: {recurring}, durable: {durable})"),
            is_error: false,
            inject_messages: Vec::new(),
            metadata: None,
            mcp_meta: None,
            new_messages: Vec::new(),
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
