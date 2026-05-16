use async_trait::async_trait;
use serde_json::json;
use tokio::process::Command;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str { "Bash" }
    fn description(&self) -> &str {
        "Executes a bash command. Commands run in the current working directory. \
         Set 'run_in_background' for long-running commands. \
         Set 'timeout' in milliseconds (default 120000ms, max 600000ms)."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "description": {"type": "string"},
                "timeout": {"type": "integer"},
                "run_in_background": {"type": "boolean"}
            },
            "required": ["command"]
        })
    }
    fn is_destructive(&self) -> bool { true }

    async fn call(&self, input: serde_json::Value, context: &ToolCallContext) -> ToolResult {
        let command_str = input["command"].as_str().unwrap_or("");
        let timeout_ms = input["timeout"].as_u64().unwrap_or(120_000);
        let run_in_bg = input["run_in_background"].as_bool().unwrap_or(false);

        // Block dangerous patterns
        if let Some(reason) = security_check(command_str) {
            return ToolResult {
                content: format!("Command blocked: {reason}"),
                is_error: true,
                ..Default::default()
            };
        }

        if run_in_bg {
            match Command::new("bash").arg("-c").arg(command_str).current_dir(&context.cwd).spawn() {
                Ok(mut c) => {
                    tokio::spawn(async move { c.wait().await.ok(); });
                    ToolResult { content: "Command launched in background".into(), is_error: false, ..Default::default() }
                }
                Err(e) => ToolResult { content: format!("Failed to spawn: {e}"), is_error: true, ..Default::default() },
            }
        } else {
            let result = tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                Command::new("bash").arg("-c").arg(command_str).current_dir(&context.cwd).output(),
            ).await;
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
                            if !body.is_empty() { body.push('\n'); }
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
                    ToolResult { content: truncated, is_error: !success, ..Default::default() }
                }
                Ok(Err(e)) => ToolResult { content: format!("Command failed: {e}"), is_error: true, ..Default::default() },
                Err(_) => ToolResult { content: "Command timed out".into(), is_error: true, ..Default::default() },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::PermissionMode;
    use crate::tools::contract::{Tool, ToolCallContext};

    fn ctx() -> ToolCallContext {
        ToolCallContext {
            cwd: std::env::temp_dir(),
            permission_mode: PermissionMode::default(),
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: true,
            tool_use_id: String::new(),
        }
    }

    #[tokio::test]
    async fn bash_error_content_starts_with_exit_code_header() {
        let t = BashTool;
        let out = t.call(serde_json::json!({"command": "bash -c 'echo ohno >&2; exit 2'"}), &ctx()).await;
        assert!(out.is_error, "expected error");
        assert!(out.content.starts_with("Error: Exit code 2"), "got: {:?}", out.content);
        assert!(out.content.contains("ohno"), "stderr preserved: {:?}", out.content);
    }

    #[tokio::test]
    async fn bash_success_content_does_not_prepend_exit_header() {
        let t = BashTool;
        let out = t.call(serde_json::json!({"command": "echo hello"}), &ctx()).await;
        assert!(!out.is_error);
        assert!(!out.content.starts_with("Error:"), "got: {:?}", out.content);
        assert!(out.content.trim() == "hello");
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
