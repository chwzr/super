use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, RenderOpts, Tool, ToolCallContext, ToolResult,
    ToolResultBlock, ToolResultContent,
};
use async_trait::async_trait;
use serde_json::json;
use shared::{RenderSpec, TextStyle};

pub struct SendMessageTool;

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        "SendMessage"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Sends a message to be routed to the appropriate handler (peer agent, sub-agent, user, or frontend)."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/send_message.txt").into()
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "The recipient of the message: 'user', a sub-agent name, or a channel identifier."
                },
                "summary": {
                    "type": "string",
                    "description": "Optional one-line summary shown in the UI while the message is being sent."
                },
                "message": {
                    "oneOf": [
                        {
                            "type": "string",
                            "description": "Plain-text message body."
                        },
                        {
                            "type": "object",
                            "description": "Structured message payload.",
                            "properties": {
                                "shutdown_request": {
                                    "type": "object",
                                    "properties": {
                                        "reason": { "type": "string" }
                                    },
                                    "required": ["reason"]
                                },
                                "shutdown_response": {
                                    "type": "object",
                                    "properties": {
                                        "acknowledged": { "type": "boolean" }
                                    },
                                    "required": ["acknowledged"]
                                },
                                "plan_approval_response": {
                                    "type": "object",
                                    "properties": {
                                        "approved": { "type": "boolean" },
                                        "feedback": { "type": "string" }
                                    },
                                    "required": ["approved"]
                                }
                            }
                        }
                    ]
                }
            },
            "required": ["to", "message"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "success": { "type": "boolean" },
                "message": { "type": "string" },
                "routing": {
                    "type": "object",
                    "properties": {
                        "sender": { "type": "string" },
                        "target": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["sender", "target", "content"]
                }
            },
            "required": ["success", "message", "routing"]
        }))
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&'static str> {
        Some("sends a message to a peer agent, sub-agent, user, or frontend")
    }

    fn render_tool_use_message(&self, input: &serde_json::Value, _opts: &RenderOpts) -> RenderSpec {
        let to = input["to"].as_str().unwrap_or("unknown");
        let summary = input["summary"]
            .as_str()
            .map(|s| format!(": {s}"))
            .unwrap_or_default();
        RenderSpec::Header {
            verb: format!("Sending message to {to}{summary}"),
            target: None,
            tag: None,
        }
    }

    fn render_tool_result_message(
        &self,
        output: &serde_json::Value,
        _progress: &[super::contract::ProgressEvent],
        _opts: &RenderOpts,
    ) -> Option<RenderSpec> {
        let msg = output["message"].as_str().unwrap_or("Message sent.");
        Some(RenderSpec::Text {
            body: msg.to_string(),
            style: TextStyle::Plain,
        })
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let to = input["to"].as_str().unwrap_or("unknown");
        let message_value = &input["message"];

        let content_str = if let Some(s) = message_value.as_str() {
            s.to_string()
        } else {
            message_value.to_string()
        };

        let routing = json!({
            "sender": "assistant",
            "target": to,
            "content": content_str
        });

        let output = json!({
            "success": true,
            "message": format!("Message routed to '{to}'"),
            "routing": routing
        });

        ToolResult {
            content: output.to_string(),
            is_error: false,
            ..Default::default()
        }
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        let text = output["message"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| output.to_string());
        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(text),
            is_error: false,
        }
    }
}
