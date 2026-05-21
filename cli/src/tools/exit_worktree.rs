use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent, ValidationResult,
};
use async_trait::async_trait;
use serde_json::json;

pub struct ExitWorktreeTool;

#[async_trait]
impl Tool for ExitWorktreeTool {
    fn name(&self) -> &str {
        "ExitWorktree"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Exit a worktree session and return to the original working directory. \
         Optionally keep or remove the worktree on disk."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/exit_worktree.txt").into()
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn is_destructive(&self, input: &serde_json::Value) -> bool {
        input
            .get("action")
            .and_then(|v| v.as_str())
            .map(|a| a == "remove")
            .unwrap_or(false)
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["keep", "remove"],
                    "description": "\"keep\" leaves the worktree and branch on disk; \"remove\" deletes both."
                },
                "discard_changes": {
                    "type": "boolean",
                    "description": "Required true when action is \"remove\" and the worktree has uncommitted files or unmerged commits. The tool will refuse and list them otherwise.",
                    "default": false
                }
            },
            "required": ["action"]
        })
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> ValidationResult {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");

        if action != "keep" && action != "remove" {
            return ValidationResult::Err {
                message: "action must be 'keep' or 'remove'".into(),
                error_code: 1,
            };
        }

        // If removing without discard_changes, check for modifications
        if action == "remove" {
            let discard = input
                .get("discard_changes")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if !discard {
                if let Some(session) = crate::utils::worktree::get_session() {
                    let rt = tokio::runtime::Handle::try_current();
                    if let Ok(handle) = rt {
                        match handle.block_on(crate::utils::worktree::count_worktree_changes(
                            &session.worktree_path,
                            &session.original_head_commit,
                        )) {
                            Ok(changes) => {
                                if !changes.uncommitted_files.is_empty()
                                    || changes.commits_ahead > 0
                                {
                                    let mut msg =
                                        String::from("Worktree has uncommitted changes:\n");
                                    for f in &changes.uncommitted_files {
                                        msg.push_str(&format!("  {f}\n"));
                                    }
                                    if changes.commits_ahead > 0 {
                                        msg.push_str(&format!(
                                            "  {} commit(s) ahead of original HEAD\n",
                                            changes.commits_ahead
                                        ));
                                    }
                                    msg.push_str(
                                        "Re-invoke with discard_changes: true to force removal.",
                                    );
                                    return ValidationResult::Err {
                                        message: msg,
                                        error_code: 2,
                                    };
                                }
                            }
                            Err(e) => {
                                return ValidationResult::Err {
                                    message: format!("Failed to check worktree status: {e}"),
                                    error_code: 3,
                                };
                            }
                        }
                    }
                }
            }
        }

        ValidationResult::Ok
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let session = match crate::utils::worktree::get_session() {
            Some(s) => s,
            None => {
                return ToolResult {
                    content: "No active worktree session — nothing to exit.".into(),
                    is_error: false,
                    ..Default::default()
                };
            }
        };

        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("keep");

        match action {
            "keep" => {
                let path = session.worktree_path.clone();
                crate::utils::worktree::keep_worktree(&session);
                ToolResult {
                    content: format!("Left worktree session. Worktree kept at {}", path.display()),
                    is_error: false,
                    ..Default::default()
                }
            }
            "remove" => {
                let path = session.worktree_path.clone();
                match crate::utils::worktree::cleanup_worktree(&session).await {
                    Ok(()) => ToolResult {
                        content: format!(
                            "Left worktree session. Worktree at {} removed, branch deleted.",
                            path.display()
                        ),
                        is_error: false,
                        ..Default::default()
                    },
                    Err(e) => ToolResult {
                        content: format!("Failed to clean up worktree: {e}"),
                        is_error: true,
                        ..Default::default()
                    },
                }
            }
            _ => ToolResult {
                content: format!("Unknown action: {action}"),
                is_error: true,
                ..Default::default()
            },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::PermissionMode;
    use crate::tools::contract::ToolCallContext;

    fn ctx() -> ToolCallContext {
        ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: false,
            tool_use_id: "tu_test".into(),
            progress_sink: None,
            queue: None,
        }
    }

    #[tokio::test]
    async fn validate_accepts_keep_action() {
        let tool = ExitWorktreeTool;
        let input = json!({"action": "keep"});
        let result = tool.validate_input(&input, &ctx()).await;
        assert!(matches!(result, ValidationResult::Ok));
    }

    #[tokio::test]
    async fn validate_rejects_invalid_action() {
        let tool = ExitWorktreeTool;
        let input = json!({"action": "invalid"});
        let result = tool.validate_input(&input, &ctx()).await;
        match result {
            ValidationResult::Err { message, .. } => {
                assert!(message.contains("must be 'keep' or 'remove'"));
            }
            _ => panic!("expected Err"),
        }
    }

    #[tokio::test]
    async fn call_no_session_returns_graceful_noop() {
        let tool = ExitWorktreeTool;
        crate::utils::worktree::set_session(None);
        let result = tool.call(json!({"action": "keep"}), &ctx(), None).await;
        assert!(!result.is_error);
        assert!(result.content.contains("No active worktree session"));
    }

    #[test]
    fn is_destructive_true_for_remove() {
        let tool = ExitWorktreeTool;
        assert!(tool.is_destructive(&json!({"action": "remove"})));
        assert!(!tool.is_destructive(&json!({"action": "keep"})));
    }
}
