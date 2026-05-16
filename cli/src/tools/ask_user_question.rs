use async_trait::async_trait;
use serde_json::json;
use super::contract::{DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent};

pub struct AskUserQuestionTool;

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str { "AskUserQuestion" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Presents a question to the user. Interactive rendering is handled by the TUI layer.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/ask_user_question.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "question": {"type": "string"}
            },
            "required": ["question"]
        })
    }

    fn requires_user_interaction(&self) -> bool { true }

    async fn call(&self, input: serde_json::Value, context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        if context.auto_deny_prompts {
            return ToolResult {
                content: "Permission denied: async subagents cannot prompt the user.".into(),
                is_error: true,
                inject_messages: Vec::new(),
                metadata: None,
                mcp_meta: None,
                new_messages: Vec::new(),
            };
        }
        let question = input["question"].as_str().unwrap_or("");
        ToolResult {
            content: format!("Question displayed: {question}"),
            is_error: false,
            metadata: Some([
                ("question".into(), question.into()),
                ("needs_response".into(), "true".into()),
            ].into()),
            inject_messages: Vec::new(),
            mcp_meta: None,
            new_messages: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::PermissionMode;

    #[tokio::test]
    async fn auto_denies_when_flag_set() {
        let tool = AskUserQuestionTool;
        let ctx = ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: true,
            tool_use_id: String::new(),
            progress_sink: None,
        };
        let result = tool.call(serde_json::json!({"question": "ok?"}), &ctx, None).await;
        assert!(result.is_error);
        assert!(result.content.contains("async") || result.content.contains("Permission denied"));
    }

    #[tokio::test]
    async fn allows_when_flag_unset() {
        let tool = AskUserQuestionTool;
        let ctx = ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: false,
            tool_use_id: String::new(),
            progress_sink: None,
        };
        let result = tool.call(serde_json::json!({"question": "ok?"}), &ctx, None).await;
        assert!(!result.is_error);
    }
}
