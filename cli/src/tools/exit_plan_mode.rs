use super::contract::{
    DescriptionCtx, InterruptBehavior, ProgressSink, PromptCtx, RenderOpts, Tool, ToolCallContext,
    ToolResult, ToolResultBlock, ToolResultContent, ValidationResult,
};
use crate::state::store::PermissionMode;
use crate::tools::permission::PermissionResult;
use async_trait::async_trait;
use serde_json::json;
use shared::{InteractiveWidget, RenderSpec, StatusState};
use std::sync::Arc;

pub struct ExitPlanModeTool {
    pub store: Arc<crate::state::store::Store>,
}

impl ExitPlanModeTool {
    fn build_output(
        &self,
        plan: &str,
        is_agent: bool,
        file_path: Option<&str>,
        plan_was_edited: bool,
    ) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("plan".into(), plan.into());
        map.insert("isAgent".into(), is_agent.into());
        map.insert("planWasEdited".into(), plan_was_edited.into());
        if let Some(fp) = file_path {
            map.insert("filePath".into(), fp.into());
        }
        map.insert("awaitingLeaderApproval".into(), false.into());
        map.insert("requestId".into(), serde_json::Value::Null);
        serde_json::Value::Object(map)
    }
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &str {
        "ExitPlanMode"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Exits plan mode and presents the plan to the user for approval. \
         The plan content must have been written to the plan file before calling this tool."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/exit_plan_mode.txt").into()
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "allowedPrompts": {
                    "type": "array",
                    "description": "Prompts the user is allowed to send while awaiting approval",
                    "items": {"type": "string"}
                },
                "plan": {
                    "type": "string",
                    "description": "The full plan markdown content"
                },
                "planFilePath": {
                    "type": "string",
                    "description": "The file path where the plan was written"
                }
            },
            "required": ["plan"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "plan": {"type": "string"},
                "isAgent": {"type": "boolean"},
                "filePath": {"type": "string"},
                "planWasEdited": {"type": "boolean"},
                "awaitingLeaderApproval": {"type": "boolean"},
                "requestId": {"type": "string"}
            },
            "required": ["plan", "isAgent", "planWasEdited", "awaitingLeaderApproval"]
        }))
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&'static str> {
        Some("Exit plan mode")
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn requires_user_interaction(&self) -> bool {
        true
    }

    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn max_result_size_chars(&self) -> usize {
        100_000
    }

    async fn validate_input(
        &self,
        _input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> ValidationResult {
        let state = self.store.get_state();
        if !matches!(state.permission_mode, PermissionMode::Plan) {
            ValidationResult::Err {
                message: "Not in plan mode. Use EnterPlanMode first to enter plan mode.".into(),
                error_code: 1,
            }
        } else {
            ValidationResult::Ok
        }
    }

    async fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> PermissionResult {
        PermissionResult::Ask {
            updated_input: None,
            rule_suggestions: vec![],
        }
    }

    fn render_tool_use_message(&self, input: &serde_json::Value, _opts: &RenderOpts) -> RenderSpec {
        let plan_markdown = input["plan"]
            .as_str()
            .unwrap_or("(no plan provided)")
            .to_string();

        RenderSpec::Interactive {
            widget: InteractiveWidget::PlanApproval { plan_markdown },
            response_schema: self.output_schema().unwrap_or(serde_json::Value::Null),
        }
    }

    fn render_tool_result_message(
        &self,
        output: &serde_json::Value,
        _progress: &[super::contract::ProgressEvent],
        _opts: &RenderOpts,
    ) -> Option<RenderSpec> {
        let parsed: serde_json::Value =
            serde_json::from_str(output.as_str().unwrap_or("{}")).unwrap_or_default();

        let plan = parsed["plan"].as_str().unwrap_or("").to_string();

        Some(RenderSpec::Group {
            children: vec![
                RenderSpec::Status {
                    state: StatusState::Success,
                    message: Some("Plan approved".into()),
                },
                RenderSpec::Text {
                    body: plan,
                    dim: true,
                },
            ],
        })
    }

    fn render_tool_use_rejected_message(
        &self,
        _input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> Option<RenderSpec> {
        Some(RenderSpec::Group {
            children: vec![
                RenderSpec::Status {
                    state: StatusState::Rejected,
                    message: Some("Plan rejected".into()),
                },
                RenderSpec::Text {
                    body: "User rejected the plan. Return to plan mode to revise.".into(),
                    dim: false,
                },
            ],
        })
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let plan = input["plan"].as_str().unwrap_or("").to_string();
        let file_path = input["planFilePath"].as_str().map(|s| s.to_string());
        let is_agent = false; // ExitPlanMode is not run by an agent in v1

        // Exit plan mode — restore full tool access
        self.store.set_state(|s| {
            s.permission_mode = PermissionMode::Default;
        });

        let output = self.build_output(&plan, is_agent, file_path.as_deref(), false);

        ToolResult {
            content: output.to_string(),
            is_error: false,
            metadata: Some([("plan_length".into(), plan.len().to_string())].into()),
            ..Default::default()
        }
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        let parsed: serde_json::Value =
            serde_json::from_str(output.as_str().unwrap_or("{}")).unwrap_or_default();

        let plan = parsed["plan"].as_str().unwrap_or("");

        let text = format!(
            "User has approved your plan. You may now proceed with implementation using all available tools.\n\n{}",
            plan
        );

        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(text),
            is_error: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::{PermissionMode, Store};

    #[tokio::test]
    async fn validates_not_in_plan_mode() {
        let store = Arc::new(Store::new());
        // Default mode is not Plan
        let tool = ExitPlanModeTool {
            store: store.clone(),
        };
        let ctx = ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: false,
            tool_use_id: "tu_test".into(),
            progress_sink: None,
            queue: None,
        };
        let result = tool
            .validate_input(&json!({"plan": "some plan"}), &ctx)
            .await;
        match result {
            ValidationResult::Err { message, .. } => {
                assert!(message.contains("Not in plan mode"));
            }
            ValidationResult::Ok => panic!("expected Err"),
        }
    }

    #[tokio::test]
    async fn validates_ok_when_in_plan_mode() {
        let store = Arc::new(Store::new());
        store.set_state(|s| {
            s.permission_mode = PermissionMode::Plan;
        });
        let tool = ExitPlanModeTool {
            store: store.clone(),
        };
        let ctx = ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Plan,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: false,
            tool_use_id: "tu_test".into(),
            progress_sink: None,
            queue: None,
        };
        let result = tool
            .validate_input(&json!({"plan": "some plan"}), &ctx)
            .await;
        assert!(matches!(result, ValidationResult::Ok));
    }

    #[test]
    fn render_tool_use_emits_plan_approval() {
        let store = Arc::new(Store::new());
        let tool = ExitPlanModeTool { store };
        let input = json!({"plan": "# My Plan\n\nDo the thing."});
        let spec = tool.render_tool_use_message(&input, &RenderOpts::default());
        match spec {
            RenderSpec::Interactive { widget, .. } => match widget {
                InteractiveWidget::PlanApproval { plan_markdown } => {
                    assert!(plan_markdown.contains("My Plan"));
                }
                _ => panic!("expected PlanApproval widget"),
            },
            _ => panic!("expected Interactive spec"),
        }
    }
}
