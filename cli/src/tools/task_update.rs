use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use crate::tools::contract::{DescriptionCtx, PromptCtx, RenderOpts, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent, ProgressSink};
use crate::state::store::TaskStatus;

pub struct TaskUpdateTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TaskUpdateTool {
    fn name(&self) -> &str { "TaskUpdate" }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Updates a task in the task list. Supports updating status, subject, description, activeForm, owner, metadata, addBlocks, and addBlockedBy."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/task_update.txt").into()
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "taskId": {"type": "string"},
                "subject": {"type": "string", "description": "New subject for the task"},
                "description": {"type": "string", "description": "New description for the task"},
                "activeForm": {"type": "string", "description": "Present continuous form shown in spinner when in_progress"},
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "deleted"]
                },
                "owner": {"type": "string", "description": "New owner for the task"},
                "addBlocks": {"type": "array", "items": {"type": "string"}, "description": "Task IDs that this task blocks"},
                "addBlockedBy": {"type": "array", "items": {"type": "string"}, "description": "Task IDs that block this task"},
                "metadata": {"type": "object", "description": "Metadata keys to merge into the task"}
            },
            "required": ["taskId"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "success": {"type": "boolean"},
                "taskId": {"type": "string"},
                "updatedFields": {"type": "array", "items": {"type": "string"}},
                "error": {"type": "string"},
                "statusChange": {"type": "string"}
            },
            "required": ["success", "taskId"]
        }))
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let task_id = input["taskId"].as_str().unwrap_or("").to_string();

        if task_id.is_empty() {
            return ToolResult {
                content: json!({"success": false, "taskId": "", "error": "taskId is required"}).to_string(),
                is_error: true,
                ..Default::default()
            };
        }

        // Check task existence
        let found = self.store.get_state().tasks.contains_key(&task_id);
        if !found {
            return ToolResult {
                content: json!({"success": false, "taskId": task_id, "error": "Task not found"}).to_string(),
                is_error: true,
                ..Default::default()
            };
        }

        // Collect updated fields outside the closure
        let mut updated_fields: Vec<String> = Vec::new();
        let mut status_change: Option<String> = None;

        // Parse optional fields from input
        let subject = input.get("subject").and_then(|v| v.as_str().map(String::from));
        let description = input.get("description").and_then(|v| v.as_str().map(String::from));
        let active_form = input.get("activeForm").and_then(|v| v.as_str().map(String::from));
        let status_str = input["status"].as_str();
        let owner = input.get("owner").and_then(|v| v.as_str().map(String::from));
        let add_blocks: Option<Vec<String>> = input.get("addBlocks").and_then(|v| {
            v.as_array().map(|a| a.iter().filter_map(|i| i.as_str().map(String::from)).collect())
        });
        let add_blocked_by: Option<Vec<String>> = input.get("addBlockedBy").and_then(|v| {
            v.as_array().map(|a| a.iter().filter_map(|i| i.as_str().map(String::from)).collect())
        });
        let metadata_merge: Option<serde_json::Value> = input.get("metadata").cloned();

        // Parse status
        let status = status_str.map(|s| match s {
            "pending" => Some(TaskStatus::Pending),
            "in_progress" => Some(TaskStatus::InProgress),
            "completed" => Some(TaskStatus::Completed),
            "deleted" => Some(TaskStatus::Deleted),
            _ => None,
        }).flatten();

        if let Some(s) = status_str {
            if status.is_none() {
                return ToolResult {
                    content: json!({"success": false, "taskId": task_id, "error": format!("Unknown status: {s}")}).to_string(),
                    is_error: true,
                    ..Default::default()
                };
            }
        }

        // Build the list of fields being updated
        if subject.is_some() { updated_fields.push("subject".into()); }
        if description.is_some() { updated_fields.push("description".into()); }
        if active_form.is_some() { updated_fields.push("activeForm".into()); }
        if status.is_some() {
            updated_fields.push("status".into());
            status_change = status_str.map(String::from);
        }
        if owner.is_some() { updated_fields.push("owner".into()); }
        if add_blocks.is_some() { updated_fields.push("addBlocks".into()); }
        if add_blocked_by.is_some() { updated_fields.push("addBlockedBy".into()); }
        if metadata_merge.is_some() { updated_fields.push("metadata".into()); }

        let tid = task_id.clone();
        let updated_fields_for_result = updated_fields.clone();
        let status_change_for_result = status_change.clone();

        self.store.set_state(move |s| {
            if let Some(t) = s.tasks.get_mut(&tid) {
                if let Some(ref v) = subject { t.subject = v.clone(); }
                if let Some(ref v) = description { t.description = v.clone(); }
                if let Some(ref v) = active_form { t.active_form = Some(v.clone()); }
                if let Some(v) = status { t.status = v; }
                if let Some(ref v) = owner { t.owner = Some(v.clone()); }
                if let Some(ref ids) = add_blocks {
                    for id in ids {
                        if !t.blocks.contains(id) {
                            t.blocks.push(id.clone());
                        }
                    }
                }
                if let Some(ref ids) = add_blocked_by {
                    for id in ids {
                        if !t.blocked_by.contains(id) {
                            t.blocked_by.push(id.clone());
                        }
                    }
                }
                if let Some(ref merge) = metadata_merge {
                    let existing = t.metadata.get_or_insert(json!({}));
                    if let (Some(obj), Some(merge_obj)) = (existing.as_object_mut(), merge.as_object()) {
                        for (key, val) in merge_obj {
                            if val.is_null() {
                                obj.remove(key);
                            } else {
                                obj.insert(key.clone(), val.clone());
                            }
                        }
                    }
                }
            }
        });

        let mut result = json!({
            "success": true,
            "taskId": task_id,
            "updatedFields": updated_fields_for_result
        });

        if let Some(sc) = status_change_for_result {
            result["statusChange"] = json!(sc);
        }

        ToolResult {
            content: result.to_string(),
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
                output.as_str().map(String::from).unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}
