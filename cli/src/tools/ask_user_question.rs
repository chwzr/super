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

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
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