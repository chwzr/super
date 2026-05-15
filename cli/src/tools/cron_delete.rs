use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use super::contract::{Tool, ToolCallContext, ToolResult};
use super::cron_create::CronJob;

pub struct CronDeleteTool {
    pub jobs: Arc<Mutex<HashMap<String, CronJob>>>,
}

#[async_trait]
impl Tool for CronDeleteTool {
    fn name(&self) -> &str { "CronDelete" }
    fn description(&self) -> &str { "Cancel a cron job previously created with CronCreate." }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "The ID of the cron job to delete"}
            },
            "required": ["id"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let id = input["id"].as_str().unwrap_or("").to_string();
        let mut jobs = self.jobs.lock().unwrap();

        if jobs.remove(&id).is_some() {
            ToolResult { content: format!("Cron job {id} deleted"), is_error: false, ..Default::default() }
        } else {
            ToolResult { content: format!("Cron job {id} not found"), is_error: true, ..Default::default() }
        }
    }
}