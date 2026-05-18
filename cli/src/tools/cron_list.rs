use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use super::contract::{DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent};
use super::cron_create::CronJob;

pub struct CronListTool {
    pub jobs: Arc<Mutex<HashMap<String, CronJob>>>,
}

#[async_trait]
impl Tool for CronListTool {
    fn name(&self) -> &str { "CronList" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "List all registered cron jobs.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/cron_list.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }
    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "jobs": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string"},
                            "cron": {"type": "string"},
                            "prompt": {"type": "string"},
                            "recurring": {"type": "boolean"},
                            "durable": {"type": "boolean"}
                        }
                    }
                }
            }
        }))
    }
    fn should_defer(&self) -> bool { true }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let jobs = self.jobs.lock().unwrap();
        if jobs.is_empty() {
            return ToolResult { content: "No cron jobs registered.".into(), is_error: false, inject_messages: Vec::new(), metadata: None, mcp_meta: None, new_messages: Vec::new() };
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
        ToolResult { content, is_error: false, inject_messages: Vec::new(), metadata: None, mcp_meta: None, new_messages: Vec::new() }
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
