use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct SendMessageTool;

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str { "SendMessage" }
    fn description(&self) -> &str { "Sends a message to be routed to the appropriate handler." }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "message": {"type": "string"},
                "channel": {"type": "string"}
            },
            "required": ["message"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let message = input["message"].as_str().unwrap_or("");
        let channel = input["channel"].as_str().unwrap_or("default");
        ToolResult {
            content: format!("Message routed to '{channel}': {message}"),
            is_error: false,
            metadata: Some([
                ("channel".into(), channel.into()),
                ("message_length".into(), message.len().to_string()),
            ].into()),
        }
    }
}