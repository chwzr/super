use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use super::contract::{DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent};
use super::cron_create::CronJob;

pub struct CronDeleteTool {
    pub jobs: Arc<Mutex<HashMap<String, CronJob>>>,
}

#[async_trait]
impl Tool for CronDeleteTool {
    fn name(&self) -> &str { "CronDelete" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Cancel a cron job previously created with CronCreate.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/cron_delete.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "The ID of the cron job to delete"}
            },
            "required": ["id"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let id = input["id"].as_str().unwrap_or("").to_string();
        let mut jobs = self.jobs.lock().unwrap();

        if jobs.remove(&id).is_some() {
            ToolResult { content: format!("Cron job {id} deleted"), is_error: false, inject_messages: Vec::new(), metadata: None, mcp_meta: None, new_messages: Vec::new() }
        } else {
            ToolResult { content: format!("Cron job {id} not found"), is_error: true, inject_messages: Vec::new(), metadata: None, mcp_meta: None, new_messages: Vec::new() }
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
