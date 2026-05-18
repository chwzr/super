use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
use async_trait::async_trait;
use serde_json::json;

pub struct StructuredOutputTool;

#[async_trait]
impl Tool for StructuredOutputTool {
    fn name(&self) -> &str {
        "StructuredOutput"
    }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Validates and returns structured output. Ensures the response matches a specified schema."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/structured_output.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "value": {"description": "The value to validate and return as structured output"},
                "schema": {"description": "Optional JSON schema to validate against"}
            },
            "required": ["value"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "string",
            "description": "Structured output tool result"
        }))
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let value = match input.get("value") {
            Some(v) => v.clone(),
            None => {
                return ToolResult {
                    content: "StructuredOutput: no value provided".into(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        // If schema is provided, validate the value against it
        if let Some(schema) = input.get("schema") {
            match validate_against_schema(&value, schema) {
                Ok(()) => {}
                Err(e) => {
                    return ToolResult {
                        content: format!("Output does not match required schema: {e}"),
                        is_error: true,
                        ..Default::default()
                    };
                }
            }
        }

        ToolResult {
            content: value.to_string(),
            is_error: false,
            ..Default::default()
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
                output
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}

/// Basic JSON Schema validation using the `jsonschema` crate.
/// Returns Ok(()) if the value matches the schema, or Err with a description.
fn validate_against_schema(
    value: &serde_json::Value,
    schema: &serde_json::Value,
) -> Result<(), String> {
    match jsonschema::validator_for(schema) {
        Ok(validator) => {
            let mut errors = validator.iter_errors(value);
            if let Some(first) = errors.next() {
                let mut msg = format!("{}: {}", first.instance_path(), first);
                for e in errors.take(9) {
                    msg.push_str(&format!("; {}: {}", e.instance_path(), e));
                }
                if validator.iter_errors(value).count() > 10 {
                    msg.push_str("; ...");
                }
                Err(msg)
            } else {
                Ok(())
            }
        }
        Err(e) => Err(format!("Invalid JSON schema: {e}")),
    }
}
