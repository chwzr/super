use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, RenderOpts, Tool, ToolCallContext, ToolResult,
    ToolResultBlock, ToolResultContent,
};
use crate::state::store::PermissionMode;
use async_trait::async_trait;
use serde_json::json;
use shared::{RenderSpec, TextStyle};
use std::sync::Arc;

pub struct EnterPlanModeTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn name(&self) -> &str {
        "EnterPlanMode"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Enters plan mode, restricting tools to read-only operations. \
         Use this tool proactively before starting non-trivial implementation tasks \
         to get user sign-off on your approach before writing code."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/enter_plan_mode.txt").into()
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
                "message": {"type": "string"}
            },
            "required": ["message"]
        }))
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&'static str> {
        Some("Enter plan mode")
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn max_result_size_chars(&self) -> usize {
        100_000
    }

    fn get_activity_description(&self, _input: &serde_json::Value) -> Option<String> {
        Some("Entering plan mode".to_string())
    }

    fn render_tool_use_message(
        &self,
        _input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> RenderSpec {
        RenderSpec::Header {
            verb: "Entering".into(),
            target: Some("plan mode".into()),
            tag: None,
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

        let message = parsed["message"].as_str().unwrap_or("Entered plan mode");

        Some(RenderSpec::Group {
            children: vec![
                RenderSpec::Text {
                    body: message.to_string(),
                    style: TextStyle::Plain,
                },
                RenderSpec::Text {
                    body: "Plan mode is active. Only read-only tools are available. \
                          Use AskUserQuestion if you need to clarify the approach. \
                          Write your plan to the plan file and call ExitPlanMode when ready for approval."
                        .to_string(),
                    style: TextStyle::Dim,
                },
            ],
        })
    }

    async fn call(
        &self,
        _input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        self.store.set_state(|s| {
            s.permission_mode = PermissionMode::Plan;
        });

        let output = json!({
            "message": "Entered plan mode. Only read-only tools are available. \
                        Use Glob, Grep, and Read to explore the codebase. \
                        Use AskUserQuestion to clarify requirements. \
                        Write your plan to the plan file, then call ExitPlanMode to present it for approval."
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
        let parsed: serde_json::Value =
            serde_json::from_str(output.as_str().unwrap_or("{}")).unwrap_or_default();

        let message = parsed["message"].as_str().unwrap_or("Entered plan mode");

        let text = format!(
            "{}\n\n\
             Plan mode is active. Only read-only tools are available. \
             Use AskUserQuestion if you need to clarify the approach. \
             Write your plan to the plan file and call ExitPlanMode when ready for approval.",
            message
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
    use crate::state::store::Store;

    #[tokio::test]
    async fn call_enters_plan_mode() {
        let store = Arc::new(Store::new());
        let tool = EnterPlanModeTool {
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
            api_key: None,
            api_messages_base_url: String::new(),
            provider: String::new(),
        };
        let result = tool.call(json!({}), &ctx, None).await;
        assert!(!result.is_error);
        assert!(matches!(
            store.get_state().permission_mode,
            PermissionMode::Plan
        ));
    }

    #[test]
    fn output_schema_has_message() {
        let store = Arc::new(Store::new());
        let tool = EnterPlanModeTool { store };
        let schema = tool.output_schema().unwrap();
        let props = schema.get("properties").unwrap();
        assert!(props.get("message").is_some());
        let required = schema.get("required").unwrap();
        assert!(required.as_array().unwrap().contains(&json!("message")));
    }

    #[test]
    fn is_read_only_returns_true() {
        let store = Arc::new(Store::new());
        let tool = EnterPlanModeTool { store };
        assert!(tool.is_read_only(&json!({})));
    }
}
