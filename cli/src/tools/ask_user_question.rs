use super::contract::{
    ColorHint, DescriptionCtx, InterruptBehavior, ProgressSink, PromptCtx, RenderOpts, Tool,
    ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent,
};
use crate::tools::permission::PermissionResult;
use async_trait::async_trait;
use serde_json::json;
use shared::{InteractiveWidget, Question, QuestionOption, RenderSpec, TextStyle};

pub struct AskUserQuestionTool;

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str {
        "AskUserQuestion"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Asks the user a set of questions and returns their answers. \
         Use this tool when you need to clarify requirements, gather preferences, \
         or make a decision between multiple approaches."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/ask_user_question.txt").into()
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The question to ask the user"
                            },
                            "header": {
                                "type": "string",
                                "description": "A short header label displayed above the question"
                            },
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {"type": "string"},
                                        "description": {"type": "string"}
                                    },
                                    "required": ["label", "description"]
                                }
                            },
                            "multiSelect": {
                                "type": "boolean",
                                "description": "Whether the user can select multiple options"
                            }
                        },
                        "required": ["question", "header", "options"]
                    }
                },
                "answers": {
                    "type": "object",
                    "description": "Map of question headers to selected answers"
                },
                "annotations": {
                    "type": "object",
                    "description": "Additional metadata for the questions"
                },
                "metadata": {
                    "type": "object",
                    "description": "Arbitrary metadata to include with the result"
                }
            },
            "required": ["questions"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "The questions that were asked"
                },
                "answers": {
                    "type": "object",
                    "description": "Map of question headers to user-provided answers"
                },
                "annotations": {
                    "type": "object",
                    "description": "Additional annotations"
                }
            },
            "required": ["questions", "answers", "annotations"]
        }))
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&'static str> {
        Some("Ask user a question")
    }

    fn max_result_size_chars(&self) -> usize {
        100_000
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn requires_user_interaction(&self) -> bool {
        true
    }

    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    fn user_facing_name_bg_color(&self, _input: Option<&serde_json::Value>) -> Option<ColorHint> {
        Some(ColorHint::Permission)
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

    fn to_auto_classifier_input(&self, input: &serde_json::Value) -> serde_json::Value {
        let question_texts: Vec<&str> = input["questions"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|q| q["question"].as_str()).collect())
            .unwrap_or_default();
        serde_json::Value::String(question_texts.join(" | "))
    }

    async fn call(
        &self,
        input: serde_json::Value,
        context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        if context.auto_deny_prompts {
            return ToolResult {
                content: "Permission denied: async subagents cannot prompt the user.".into(),
                is_error: true,
                ..Default::default()
            };
        }

        let questions = &input["questions"];
        let answers = input
            .get("answers")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let annotations = input
            .get("annotations")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        ToolResult {
            content: json!({
                "questions": questions,
                "answers": answers,
                "annotations": annotations
            })
            .to_string(),
            is_error: false,
            metadata: Some([("needs_response".into(), "true".into())].into()),
            ..Default::default()
        }
    }

    fn render_tool_use_message(&self, input: &serde_json::Value, _opts: &RenderOpts) -> RenderSpec {
        let questions: Vec<Question> = input["questions"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|q| {
                        let options: Vec<QuestionOption> = q["options"]
                            .as_array()
                            .map(|opts| {
                                opts.iter()
                                    .map(|o| QuestionOption {
                                        label: o["label"].as_str().unwrap_or("").to_string(),
                                        description: o["description"]
                                            .as_str()
                                            .unwrap_or("")
                                            .to_string(),
                                        preview: None,
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();

                        Question {
                            question: q["question"].as_str().unwrap_or("").to_string(),
                            header: q["header"].as_str().unwrap_or("").to_string(),
                            multi_select: q["multiSelect"].as_bool().unwrap_or(false),
                            options,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        RenderSpec::Interactive {
            widget: InteractiveWidget::MultiQuestion { questions },
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

        let answers = parsed.get("answers");
        let questions = parsed.get("questions");

        let mut parts: Vec<String> = Vec::new();

        if let Some(qs) = questions.and_then(|v| v.as_array()) {
            for q in qs {
                let header = q["header"].as_str().unwrap_or("");
                let answer = answers
                    .and_then(|a| a.get(header))
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "(no answer)".to_string());
                parts.push(format!("**{}**: {}", header, answer));
            }
        }

        let body = if parts.is_empty() {
            output.as_str().unwrap_or("Questions answered").to_string()
        } else {
            parts.join("\n")
        };

        Some(RenderSpec::Group {
            children: vec![RenderSpec::Text {
                body,
                style: TextStyle::Plain,
            }],
        })
    }

    fn render_tool_use_rejected_message(
        &self,
        _input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> Option<RenderSpec> {
        Some(RenderSpec::Text {
            body: "User declined to answer questions".to_string(),
            style: TextStyle::Plain,
        })
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        let parsed: serde_json::Value =
            serde_json::from_str(output.as_str().unwrap_or("{}")).unwrap_or_default();

        let answers = parsed.get("answers");
        let questions = parsed.get("questions");

        let mut lines: Vec<String> = Vec::new();

        if let Some(qs) = questions.and_then(|v| v.as_array()) {
            for q in qs {
                let header = q["header"].as_str().unwrap_or("");
                let answer_str = answers
                    .and_then(|a| a.get(header))
                    .map(|v| {
                        if v.is_string() {
                            v.as_str().unwrap_or("").to_string()
                        } else if v.is_array() {
                            v.as_array()
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|x| x.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                })
                                .unwrap_or_default()
                        } else {
                            v.to_string()
                        }
                    })
                    .unwrap_or_else(|| "(no answer)".to_string());

                lines.push(format!("\"{}\"=\"{}\"", header, answer_str));
            }
        }

        let text = if lines.is_empty() {
            output
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| output.to_string())
        } else {
            lines.join("\n")
        };

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
    use crate::state::store::PermissionMode;

    #[test]
    fn schema_has_questions_array() {
        let tool = AskUserQuestionTool;
        let schema = tool.input_schema();
        let props = schema.get("properties").unwrap();
        let questions = props.get("questions").unwrap();
        assert_eq!(questions["type"], "array");
        assert_eq!(questions["minItems"], 1);
        assert_eq!(questions["maxItems"], 4);
    }

    #[test]
    fn schema_has_output_schema() {
        let tool = AskUserQuestionTool;
        let out = tool.output_schema();
        assert!(out.is_some());
        let schema = out.unwrap();
        let required = schema.get("required").unwrap();
        assert!(required.as_array().unwrap().iter().any(|v| v == "answers"));
    }

    #[test]
    fn render_tool_use_emits_interactive() {
        let tool = AskUserQuestionTool;
        let input = json!({
            "questions": [
                {
                    "question": "What is your preference?",
                    "header": "Preference",
                    "options": [
                        {"label": "Option A", "description": "First option"},
                        {"label": "Option B", "description": "Second option"}
                    ],
                    "multiSelect": false
                }
            ]
        });
        let spec = tool.render_tool_use_message(&input, &RenderOpts::default());
        match spec {
            RenderSpec::Interactive { widget, .. } => match widget {
                InteractiveWidget::MultiQuestion { questions } => {
                    assert_eq!(questions.len(), 1);
                    assert_eq!(questions[0].header, "Preference");
                    assert_eq!(questions[0].options.len(), 2);
                }
                _ => panic!("expected MultiQuestion widget"),
            },
            _ => panic!("expected Interactive spec"),
        }
    }

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
            queue: None,
        };
        let result = tool
            .call(
                json!({"questions": [{"question": "ok?", "header": "Q", "options": []}]}),
                &ctx,
                None,
            )
            .await;
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
            queue: None,
        };
        let result = tool
            .call(
                json!({"questions": [{"question": "ok?", "header": "Q", "options": []}]}),
                &ctx,
                None,
            )
            .await;
        assert!(!result.is_error);
    }
}
