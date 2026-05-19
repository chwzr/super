use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent, ValidationResult,
};
use async_trait::async_trait;
use serde_json::json;

pub struct EnterWorktreeTool;

#[async_trait]
impl Tool for EnterWorktreeTool {
    fn name(&self) -> &str {
        "EnterWorktree"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Create an isolated git worktree and switch the session into it. \
         Used for working on features in isolation."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/enter_worktree.txt").into()
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn is_destructive(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Optional name for a new worktree. Each \"/\"-separated segment may contain only letters, digits, dots, underscores, and dashes; max 64 chars total. A random name is generated if not provided. Mutually exclusive with 'path'."
                },
                "path": {
                    "type": "string",
                    "description": "Path to an existing worktree of the current repository to switch into instead of creating a new one. Must appear in 'git worktree list' for the current repo. Mutually exclusive with 'name'."
                }
            }
        })
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> ValidationResult {
        let name = input.get("name").and_then(|v| v.as_str());
        let path = input.get("path").and_then(|v| v.as_str());

        if name.is_some() && path.is_some() {
            return ValidationResult::Err {
                message:
                    "'name' and 'path' are mutually exclusive — provide one or the other, not both"
                        .into(),
                error_code: 1,
            };
        }

        ValidationResult::Ok
    }

    async fn call(
        &self,
        input: serde_json::Value,
        context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        // Guard: already in a worktree session?
        if crate::utils::worktree::get_session().is_some() {
            return ToolResult {
                content: "Already in a worktree session. Exit the current worktree with ExitWorktree before entering a new one.".into(),
                is_error: true,
                ..Default::default()
            };
        }

        // Resolve main repo root (if already in a worktree, walk back to main)
        let repo_root = match crate::utils::worktree::find_canonical_git_root(&context.cwd).await {
            Ok(root) => root,
            Err(e) => {
                return ToolResult {
                    content: e,
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let session = if let Some(path_str) = input.get("path").and_then(|v| v.as_str()) {
            // Enter existing worktree
            let path = std::path::PathBuf::from(path_str);
            match crate::utils::worktree::enter_existing_worktree(&repo_root, &path).await {
                Ok(s) => s,
                Err(e) => {
                    return ToolResult {
                        content: e,
                        is_error: true,
                        ..Default::default()
                    };
                }
            }
        } else {
            // Create new worktree
            let random_name = uuid::Uuid::new_v4().to_string();
            let raw_name = input
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(&random_name[..8]);

            let flat_slug = match crate::utils::worktree::validate_worktree_slug(raw_name) {
                Ok(slug) => slug,
                Err(e) => {
                    return ToolResult {
                        content: e,
                        is_error: true,
                        ..Default::default()
                    };
                }
            };

            match crate::utils::worktree::get_or_create_worktree(&repo_root, &flat_slug).await {
                Ok(s) => s,
                Err(e) => {
                    return ToolResult {
                        content: e,
                        is_error: true,
                        ..Default::default()
                    };
                }
            }
        };

        // Switch CWD into the worktree
        let worktree_path = session.worktree_path.clone();
        let branch_name = session.worktree_branch.clone();
        if let Err(e) = crate::utils::cwd::set_cwd(worktree_path.clone()) {
            return ToolResult {
                content: format!("Failed to change into worktree directory: {e}"),
                is_error: true,
                ..Default::default()
            };
        }

        crate::utils::worktree::set_session(Some(session));

        ToolResult {
            content: format!(
                "Entered worktree at {} on branch '{}'",
                worktree_path.display(),
                branch_name
            ),
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
        }
    }

    #[tokio::test]
    async fn validate_rejects_both_name_and_path() {
        let tool = EnterWorktreeTool;
        let input = json!({"name": "foo", "path": "/some/path"});
        let result = tool.validate_input(&input, &ctx()).await;
        match result {
            ValidationResult::Err { message, .. } => {
                assert!(message.contains("mutually exclusive"));
            }
            _ => panic!("expected Err"),
        }
    }

    #[tokio::test]
    async fn validate_allows_name_only() {
        let tool = EnterWorktreeTool;
        let input = json!({"name": "foo"});
        let result = tool.validate_input(&input, &ctx()).await;
        assert!(matches!(result, ValidationResult::Ok));
    }

    #[tokio::test]
    async fn validate_allows_path_only() {
        let tool = EnterWorktreeTool;
        let input = json!({"path": "/some/path"});
        let result = tool.validate_input(&input, &ctx()).await;
        assert!(matches!(result, ValidationResult::Ok));
    }

    #[tokio::test]
    async fn validate_allows_empty_input() {
        let tool = EnterWorktreeTool;
        let result = tool.validate_input(&json!({}), &ctx()).await;
        assert!(matches!(result, ValidationResult::Ok));
    }

    #[tokio::test]
    async fn call_returns_error_not_in_git_repo() {
        let mut test_ctx = ctx();
        test_ctx.cwd = std::env::temp_dir();
        let tool = EnterWorktreeTool;
        let result = tool.call(json!({"name": "test-wt"}), &test_ctx, None).await;
        if result.is_error {
            assert!(
                result.content.contains("not in a git repository")
                    || result.content.contains("git")
            );
        }
    }
}
