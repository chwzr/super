use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct AgentTool {
    pub store: Arc<crate::state::store::Store>,
    pub config: shared::CliConfig,
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &str { "Agent" }
    fn description(&self) -> &str {
        "Launches a sub-agent to handle complex multi-step tasks. \
         subagent_type: explore (read-only), plan (design), general-purpose (default). \
         Set run_in_background for async execution."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "description": {"type": "string"},
                "prompt": {"type": "string"},
                "subagent_type": {"type": "string", "enum": ["explore", "plan", "general-purpose"]},
                "run_in_background": {"type": "boolean"}
            },
            "required": ["description", "prompt"]
        })
    }

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        ToolResult {
            content: "Agent (subagent) execution is not implemented in this build. \
                      parent_tool_use_id support and nested engine spawning arrive in \
                      a later milestone."
                .to_string(),
            is_error: true,
            metadata: None,
        }
    }
}
