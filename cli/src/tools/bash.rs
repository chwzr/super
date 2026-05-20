use std::sync::Arc;

use crate::conversation::message_queue::MessageQueue;
use crate::state::store::Store;
use crate::tools::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
use async_trait::async_trait;
use serde_json::json;
use tokio::process::Command;

pub struct BashTool {
    pub queue: Arc<MessageQueue>,
    pub store: Arc<Store>,
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "Bash"
    }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Executes a bash command. Commands run in the current working directory. \
         Set 'run_in_background' for long-running commands. \
         Set 'timeout' in milliseconds (default 120000ms, max 600000ms)."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/bash.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute"
                },
                "description": {
                    "type": "string",
                    "description": "Clear, concise description of what this command does in active voice. For simple commands (git, npm, standard CLI tools), keep it brief (5-10 words). For commands that are harder to parse at a glance (piped commands, obscure flags, etc.), add enough context to clarify what it does."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional timeout in milliseconds (max 600000)",
                    "minimum": 0,
                    "maximum": 600000
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Set to true to run this command in the background. Only use this if you don't need the result immediately and are OK being notified when the command completes later. You do not need to check the output right away - you'll be notified when it finishes. You do not need to use '&' at the end of the command when using this parameter."
                },
                "dangerouslyDisableSandbox": {
                    "type": "boolean",
                    "description": "Set this to true to dangerously override sandbox mode and run commands without sandboxing."
                }
            },
            "required": ["command"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "stdout": { "type": "string" },
                "stderr": { "type": "string" },
                "exit_code": { "type": "integer" },
                "timed_out": { "type": "boolean" },
                "background": { "type": "boolean" }
            }
        }))
    }

    fn get_activity_description(&self, input: &serde_json::Value) -> Option<String> {
        input
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                input.get("command").and_then(|v| v.as_str()).map(|c| {
                    if c.len() > 80 {
                        format!("{}...", &c[..77])
                    } else {
                        c.to_string()
                    }
                })
            })
    }
    async fn prepare_permission_matcher(
        &self,
        input: &serde_json::Value,
    ) -> Option<Box<dyn Fn(&str) -> bool + Send + Sync>> {
        let command_stem = input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|c| c.split_whitespace().next().unwrap_or("").to_string())?;

        if command_stem.is_empty() {
            return None;
        }

        Some(Box::new(move |rule_content: &str| -> bool {
            let rule_stem = rule_content.split_whitespace().next().unwrap_or("");
            rule_stem == command_stem || rule_content == "*"
        }))
    }

    fn is_destructive(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let command_str = input["command"].as_str().unwrap_or("");
        let timeout_ms = input["timeout"].as_u64().unwrap_or(120_000);
        let run_in_bg = input["run_in_background"].as_bool().unwrap_or(false);
        let dangerously_disable_sandbox = input["dangerouslyDisableSandbox"]
            .as_bool()
            .unwrap_or(false);
        let _ = dangerously_disable_sandbox; // Read but not yet enforced (no sandbox implementation)

        // Block dangerous patterns
        if let Some(reason) = security_check(command_str) {
            return ToolResult {
                content: format!("Command blocked: {reason}"),
                is_error: true,
                ..Default::default()
            };
        }

        if run_in_bg {
            let desc = input["description"]
                .as_str()
                .unwrap_or("background bash")
                .to_string();
            let task_id = uuid::Uuid::new_v4().to_string();
            let output_path = crate::state::store::Store::task_output_path(&task_id);
            let agent_id = context.parent_tool_use_id.clone();

            // Ensure parent directories exist
            if let Some(parent) = output_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            let queue = context
                .queue
                .clone()
                .expect("MessageQueue must be available in ToolCallContext");

            match Command::new("bash")
                .arg("-c")
                .arg(command_str)
                .current_dir(&context.cwd)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(mut child) => {
                    let stdout = child.stdout.take();
                    let stderr = child.stderr.take();
                    let output_path_for_writer = output_path.clone();
                    let task_id_for_closure = task_id.clone();

                    tokio::spawn(async move {
                        use tokio::io::AsyncReadExt;

                        let stdout_bytes = if let Some(stdout) = stdout {
                            let mut reader = tokio::io::BufReader::new(stdout);
                            let mut buf = Vec::new();
                            let _ = reader.read_to_end(&mut buf).await;
                            buf
                        } else {
                            Vec::new()
                        };

                        let stderr_bytes = if let Some(stderr) = stderr {
                            let mut reader = tokio::io::BufReader::new(stderr);
                            let mut buf = Vec::new();
                            let _ = reader.read_to_end(&mut buf).await;
                            buf
                        } else {
                            Vec::new()
                        };

                        // Write output to file
                        let mut output = String::from_utf8_lossy(&stdout_bytes).to_string();
                        let stderr_str = String::from_utf8_lossy(&stderr_bytes);
                        if !stderr_str.is_empty() {
                            output.push_str("\nstderr:\n");
                            output.push_str(&stderr_str);
                        }
                        let _ = std::fs::write(&output_path_for_writer, &output);

                        // Wait for process to finish
                        let exit_status = child.wait().await;
                        let exit_code = exit_status.as_ref().ok().and_then(|s| s.code()).unwrap_or(-1);
                        let status = if exit_status.map(|s| s.success()).unwrap_or(false) {
                            "completed"
                        } else {
                            "failed"
                        };

                        let summary = format!(
                            "Background bash \"{desc}\" {} (exit code {exit_code})",
                            if status == "completed" { "completed" } else { "failed" }
                        );
                        let notification = format!(
                            "<task-notification>\n  <task-id>{task_id}</task-id>\n  <output-file>{output_path}</output-file>\n  <status>{status}</status>\n  <summary>{summary}</summary>\n</task-notification>",
                            task_id = task_id_for_closure,
                            output_path = output_path_for_writer.display(),
                            status = status,
                            summary = summary,
                        );

                        use crate::conversation::message_queue::{PromptInputMode, QueuePriority};
                        queue.enqueue_pending_notification(
                            notification,
                            PromptInputMode::TaskNotification,
                            QueuePriority::Next,
                            agent_id,
                        );
                    });

                    ToolResult {
                        content: json!({
                            "background": true,
                            "task_id": task_id,
                            "message": format!("Command launched in background. Output: {}", output_path.display()),
                        })
                        .to_string(),
                        is_error: false,
                        ..Default::default()
                    }
                }
                Err(e) => ToolResult {
                    content: format!("Failed to spawn: {e}"),
                    is_error: true,
                    ..Default::default()
                },
            }
        } else {
            let result = tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                Command::new("bash")
                    .arg("-c")
                    .arg(command_str)
                    .current_dir(&context.cwd)
                    .output(),
            )
            .await;
            match result {
                Ok(Ok(out)) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let success = out.status.success();
                    let content = if success {
                        if stderr.is_empty() {
                            stdout.to_string()
                        } else {
                            format!("stdout:\n{stdout}\nstderr:\n{stderr}")
                        }
                    } else {
                        let code = out.status.code().unwrap_or(-1);
                        // Match Claude's format: header line followed by
                        // captured output (stderr first, then stdout if any).
                        let mut body = String::new();
                        if !stderr.is_empty() {
                            body.push_str(stderr.trim_end_matches('\n'));
                        }
                        if !stdout.is_empty() {
                            if !body.is_empty() {
                                body.push('\n');
                            }
                            body.push_str(stdout.trim_end_matches('\n'));
                        }
                        if body.is_empty() {
                            format!("Error: Exit code {code}")
                        } else {
                            format!("Error: Exit code {code}\n{body}")
                        }
                    };
                    let truncated = if content.len() > 50000 {
                        format!("{}...\n[output truncated]", &content[..50000])
                    } else {
                        content
                    };
                    ToolResult {
                        content: truncated,
                        is_error: !success,
                        ..Default::default()
                    }
                }
                Ok(Err(e)) => ToolResult {
                    content: format!("Command failed: {e}"),
                    is_error: true,
                    ..Default::default()
                },
                Err(_) => ToolResult {
                    content: "Command timed out".into(),
                    is_error: true,
                    ..Default::default()
                },
            }
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

fn security_check(cmd: &str) -> Option<&'static str> {
    let dangerous: &[(&str, &str)] = &[
        ("rm -rf /", "destroys root filesystem"),
        ("mkfs.", "filesystem formatting"),
        ("dd if=", "raw device write"),
        ("sudo ", "privilege escalation"),
        ("curl | sh", "pipe to shell"),
        ("wget | bash", "pipe to shell"),
        ("passwd", "password change"),
        ("chsh", "shell change"),
    ];
    for (pattern, reason) in dangerous {
        if cmd.contains(pattern) {
            return Some(reason);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::PermissionMode;
    use crate::tools::contract::{Tool, ToolCallContext};

    fn make_tool() -> BashTool {
        BashTool {
            queue: Arc::new(MessageQueue::new()),
            store: Arc::new(Store::new()),
        }
    }

    fn ctx() -> ToolCallContext {
        ToolCallContext {
            cwd: std::env::temp_dir(),
            permission_mode: PermissionMode::default(),
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: true,
            tool_use_id: String::new(),
            progress_sink: None,
            queue: None,
        }
    }

    #[tokio::test]
    async fn bash_error_content_starts_with_exit_code_header() {
        let t = make_tool();
        let out = t
            .call(
                serde_json::json!({"command": "bash -c 'echo ohno >&2; exit 2'"}),
                &ctx(),
                None,
            )
            .await;
        assert!(out.is_error, "expected error");
        assert!(
            out.content.starts_with("Error: Exit code 2"),
            "got: {:?}",
            out.content
        );
        assert!(
            out.content.contains("ohno"),
            "stderr preserved: {:?}",
            out.content
        );
    }

    #[tokio::test]
    async fn bash_success_content_does_not_prepend_exit_header() {
        let t = make_tool();
        let out = t
            .call(serde_json::json!({"command": "echo hello"}), &ctx(), None)
            .await;
        assert!(!out.is_error);
        assert!(!out.content.starts_with("Error:"), "got: {:?}", out.content);
        assert!(out.content.trim() == "hello");
    }

    #[tokio::test]
    async fn permission_matcher_matches_command_stem() {
        let t = make_tool();
        let input = serde_json::json!({"command": "git status"});
        let matcher = t
            .prepare_permission_matcher(&input)
            .await
            .expect("should return matcher");
        assert!(matcher("git *"), "git * should match git status");
        assert!(
            matcher("git diff"),
            "git diff should match git status (stem check only)"
        );
        assert!(!matcher("ls *"), "ls * should not match git status");
        assert!(matcher("*"), "wildcard should match anything");
    }

    #[tokio::test]
    async fn permission_matcher_handles_no_command() {
        let t = make_tool();
        let input = serde_json::json!({});
        let matcher = t.prepare_permission_matcher(&input).await;
        assert!(matcher.is_none(), "no command = no matcher");
    }

    #[tokio::test]
    async fn permission_matcher_handles_whitespace_command() {
        let t = make_tool();
        let input = serde_json::json!({"command": "   echo hello"});
        let matcher = t
            .prepare_permission_matcher(&input)
            .await
            .expect("should return matcher");
        assert!(matcher("echo *"), "should trim command");
    }
}
