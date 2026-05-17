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

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Unique job ID"},
                "cron": {"type": "string", "description": "The cron expression"},
                "humanSchedule": {"type": "string", "description": "Human-readable schedule description"},
                "recurring": {"type": "boolean"},
                "durable": {"type": "boolean"}
            }
        }))
    }

    fn should_defer(&self) -> bool { true }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let cron = input["cron"].as_str().unwrap_or("").to_string();
        let prompt = input["prompt"].as_str().unwrap_or("").to_string();
        let recurring = input.get("recurring").and_then(|v| v.as_bool()).unwrap_or(true);
        let durable = input.get("durable").and_then(|v| v.as_bool()).unwrap_or(false);

        // Validate 5-field cron expression with per-field constraints
        let fields: Vec<&str> = cron.split_whitespace().collect();
        if fields.len() != 5 {
            return ToolResult {
                content: "Invalid cron expression: must have exactly 5 space-separated fields (minute hour day-of-month month day-of-week)".into(),
                is_error: true,
                ..Default::default()
            };
        }

        // Validate each field's domain
        fn valid_field(field: &str, min: i32, max: i32) -> bool {
            if field == "*" { return true; }
            for part in field.split(',') {
                let (part, _step) = match part.split_once('/') {
                    Some((p, s)) => (p, Some(s)),
                    None => (part, None),
                };
                let (lo, hi) = match part.split_once('-') {
                    Some((l, h)) => (l, h),
                    None => (part, part),
                };
                for val in [lo, hi] {
                    if let Ok(n) = val.parse::<i32>() {
                        if n < min || n > max { return false; }
                    }
                }
            }
            true
        }

        let field_constraints = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
        let field_names = ["minute", "hour", "day-of-month", "month", "day-of-week"];
        for (i, field) in fields.iter().enumerate() {
            let (min, max) = field_constraints[i];
            if !valid_field(field, min, max) {
                return ToolResult {
                    content: format!("Invalid cron field '{}': {} must be in range {}-{}", field_names[i], field, min, max),
                    is_error: true,
                    ..Default::default()
                };
            }
        }

        let id = uuid::Uuid::new_v4().to_string();
        let job = CronJob { cron: cron.clone(), prompt, recurring, durable };

        let mut jobs = self.jobs.lock().unwrap();
        jobs.insert(id.clone(), job);

        let human = format!("cron: {cron} recurring: {recurring} durable: {durable}");

        ToolResult {
            content: json!({
                "id": id,
                "cron": cron,
                "humanSchedule": human,
                "recurring": recurring,
                "durable": durable
            }).to_string(),
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
