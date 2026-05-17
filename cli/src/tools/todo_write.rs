use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use shared::RenderSpec;

use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, RenderOpts, Tool, ToolCallContext, ToolResult,
    ToolResultBlock, ToolResultContent,
};
use crate::state::store::{Store, TaskRecord, TaskStatus};
use crate::tools::permission::{DecisionReason, PermissionResult};

pub struct TodoWriteTool {
    pub store: Arc<Store>,
}

fn task_status_to_str(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "pending",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Deleted => "deleted",
    }
}

fn status_str_to_task_status(s: &str) -> TaskStatus {
    match s {
        "in_progress" => TaskStatus::InProgress,
        "completed" => TaskStatus::Completed,
        _ => TaskStatus::Pending,
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "TodoWrite"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Creates and updates a structured task list for your current coding session."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/todo_write.txt").into()
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The complete list of todos. Replaces the existing list.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "The content/description of the todo item."
                            },
                            "status": {
                                "type": "string",
                                "description": "The status of the todo item.",
                                "enum": ["pending", "in_progress", "completed"]
                            },
                            "activeForm": {
                                "type": "string",
                                "description": "Shown to the user while the agent is actively working on this todo."
                            }
                        },
                        "required": ["content", "status", "activeForm"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        let item_schema = json!({
            "type": "object",
            "properties": {
                "content": { "type": "string" },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed"]
                },
                "activeForm": { "type": "string" }
            },
            "required": ["content", "status", "activeForm"]
        });

        Some(json!({
            "type": "object",
            "properties": {
                "oldTodos": {
                    "type": "array",
                    "items": item_schema.clone()
                },
                "newTodos": {
                    "type": "array",
                    "items": item_schema
                },
                "verificationNudgeNeeded": {
                    "type": "boolean"
                }
            },
            "required": ["oldTodos", "newTodos"]
        }))
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn strict(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn to_auto_classifier_input(&self, input: &serde_json::Value) -> serde_json::Value {
        let count = input["todos"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        serde_json::Value::String(format!("{count} items"))
    }

    fn render_tool_use_message(
        &self,
        input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> RenderSpec {
        let count = input["todos"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        RenderSpec::Header {
            verb: format!("Updating todo list ({count} items)"),
            target: None,
            tag: None,
        }
    }

    fn render_tool_result_message(
        &self,
        output: &serde_json::Value,
        _progress: &[super::contract::ProgressEvent],
        _opts: &RenderOpts,
    ) -> Option<RenderSpec> {
        let todos = output["newTodos"].as_array();
        let lines: Vec<String> = match todos {
            Some(items) => items
                .iter()
                .map(|item| {
                    let status = item["status"].as_str().unwrap_or("pending");
                    let content = item["content"].as_str().unwrap_or("");
                    let icon = match status {
                        "completed" => "[x]",
                        "in_progress" => "[>]",
                        _ => "[ ]",
                    };
                    format!("{icon} {content}")
                })
                .collect(),
            None => vec!["(empty)".to_string()],
        };
        let body = format!("Todo list updated:\n{}", lines.join("\n"));
        Some(RenderSpec::Text { body, dim: false })
    }

    async fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> PermissionResult {
        PermissionResult::Allow {
            updated_input: None,
            decision_reason: Some(DecisionReason::ToolDefault),
        }
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        // 1. Get old todos from store and convert to JSON array
        let old_state = self.store.get_state();
        let old_todos: Vec<serde_json::Value> = old_state
            .tasks
            .values()
            .map(|task| {
                json!({
                    "content": task.description,
                    "status": task_status_to_str(&task.status),
                    "activeForm": task.subject
                })
            })
            .collect();

        // 2. Collect new todos from input
        let new_todos_arr = input["todos"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let new_todos: Vec<serde_json::Value> = new_todos_arr.clone();

        let new_count = new_todos_arr.len();

        // 3. Clear and repopulate store tasks
        self.store.set_state(move |state| {
            state.tasks.clear();
            for todo_item in &new_todos_arr {
                let content = todo_item["content"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let id = content.clone(); // Use content as the task id
                let status_str = todo_item["status"].as_str().unwrap_or("pending");
                let active_form = todo_item["activeForm"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();

                state.tasks.insert(
                    id.clone(),
                    TaskRecord {
                        id,
                        subject: active_form,
                        description: content,
                        active_form: None,
                        status: status_str_to_task_status(status_str),
                        owner: None,
                        blocks: vec![],
                        blocked_by: vec![],
                        metadata: None,
                    },
                );
            }
        });

        // 4. Build output
        let all_completed = new_todos.iter().all(|item| {
            item["status"].as_str() == Some("completed")
        });

        let mut output = json!({
            "oldTodos": old_todos,
            "newTodos": new_todos
        });

        if all_completed && new_count >= 3 {
            output["verificationNudgeNeeded"] = json!(true);
        }

        // 5. If all tasks are complete AND there are 3+ of them, push a verification nudge
        let mut new_messages = Vec::new();
        if all_completed && new_count >= 3 {
            let nudge = json!({
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "All tasks are marked complete. Please run verification to confirm everything works before wrapping up."
                }]
            });
            new_messages.push(nudge);
        }

        ToolResult {
            content: output.to_string(),
            is_error: false,
            new_messages,
            ..Default::default()
        }
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        let count = output["newTodos"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        let text = format!(
            "Todos have been modified successfully. The updated list contains {count} items. Ensure that you continue to use the todo list to track your progress."
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

    #[test]
    fn schema_requires_active_form() {
        let tool = TodoWriteTool {
            store: Arc::new(Store::new()),
        };
        let schema = tool.input_schema();
        let items = &schema["properties"]["todos"]["items"];
        let required: Vec<&str> = items["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(
            required.contains(&"activeForm"),
            "activeForm must be in item required fields, got: {:?}",
            required
        );
    }

    #[test]
    fn output_schema_has_old_and_new_todos() {
        let tool = TodoWriteTool {
            store: Arc::new(Store::new()),
        };
        let schema = tool.output_schema().unwrap();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(
            required.contains(&"oldTodos"),
            "output required must contain oldTodos, got: {:?}",
            required
        );
        assert!(
            required.contains(&"newTodos"),
            "output required must contain newTodos, got: {:?}",
            required
        );
    }
}
