use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct AskUserQuestionTool;

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str { "AskUserQuestion" }
    fn description(&self) -> &str {
        "Presents a question to the user. Interactive rendering is handled by the TUI layer."
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

    async fn call(&self, input: serde_json::Value, context: &ToolCallContext) -> ToolResult {
        if context.auto_deny_prompts {
            return ToolResult {
                content: "Permission denied: async subagents cannot prompt the user.".into(),
                is_error: true,
                metadata: None,
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
        };
        let result = tool.call(serde_json::json!({"question": "ok?"}), &ctx).await;
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
        };
        let result = tool.call(serde_json::json!({"question": "ok?"}), &ctx).await;
        assert!(!result.is_error);
    }
}