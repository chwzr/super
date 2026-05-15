use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};
use crate::state::store::PermissionMode;

pub struct ExitPlanModeTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &str { "ExitPlanMode" }
    fn description(&self) -> &str {
        "Exits plan mode, restoring full tool access."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        self.store.set_state(|s| {
            s.permission_mode = PermissionMode::Default;
        });
        ToolResult {
            content: "Exited plan mode. Full tool access restored.".into(),
            is_error: false,
            ..Default::default()
        }
    }
}