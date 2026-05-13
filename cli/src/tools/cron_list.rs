use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use super::contract::{Tool, ToolCallContext, ToolResult};
use super::cron_create::CronJob;

pub struct CronListTool {
    pub jobs: Arc<Mutex<HashMap<String, CronJob>>>,
}

#[async_trait]
impl Tool for CronListTool {
    fn name(&self) -> &str { "CronList" }
    fn description(&self) -> &str { "List all registered cron jobs." }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }
    fn is_read_only(&self) -> bool { true }

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let jobs = self.jobs.lock().unwrap();
        if jobs.is_empty() {
            return ToolResult { content: "No cron jobs registered.".into(), is_error: false, metadata: None };
        }

        let mut content = String::from("Registered cron jobs:\n");
        for (id, job) in jobs.iter() {
            content.push_str(&format!(
                "- {id}: \"{cron}\" prompt=\"{prompt}\" recurring={recurring} durable={durable}\n",
                cron = job.cron,
                prompt = job.prompt,
                recurring = job.recurring,
                durable = job.durable,
            ));
        }
        ToolResult { content, is_error: false, metadata: None }
    }
}