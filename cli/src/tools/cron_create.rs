use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use super::contract::{Tool, ToolCallContext, ToolResult};

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
    fn description(&self) -> &str {
        "Schedule a prompt to be enqueued at a future time via a cron expression. \
         Supports recurring and one-shot schedules."
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

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
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
                ..Default::default()
            };
        }

        let id = uuid::Uuid::new_v4().to_string();
        let job = CronJob { cron: cron.clone(), prompt, recurring, durable };

        let mut jobs = self.jobs.lock().unwrap();
        jobs.insert(id.clone(), job);

        ToolResult {
            content: format!("Cron job created with ID: {id} — \"{cron}\" (recurring: {recurring}, durable: {durable})"),
            is_error: false,
            ..Default::default()
        }
    }
}