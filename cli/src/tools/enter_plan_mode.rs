use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};
use crate::state::store::PermissionMode;

pub struct EnterPlanModeTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn name(&self) -> &str { "EnterPlanMode" }
    fn description(&self) -> &str {
        "Enters plan mode, restricting tools to read-only operations."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        self.store.set_state(|s| {
            s.permission_mode = PermissionMode::Plan;
        });
        ToolResult {
            content: "Entered plan mode. Only read-only tools are available.".into(),
            is_error: false,
            ..Default::default()
        }
    }
}