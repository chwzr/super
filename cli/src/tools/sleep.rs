use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct SleepTool;

#[async_trait]
impl Tool for SleepTool {
    fn name(&self) -> &str { "Sleep" }
    fn description(&self) -> &str { "Sleep for a specified duration in milliseconds. Max 300000ms (5 minutes)." }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "duration_ms": {"type": "integer", "description": "Duration to sleep in milliseconds", "maximum": 300000}
            },
            "required": ["duration_ms"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let ms = input["duration_ms"].as_u64().unwrap_or(1000).min(300000);
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        ToolResult { content: format!("Slept for {ms}ms"), is_error: false, metadata: None }
    }
}