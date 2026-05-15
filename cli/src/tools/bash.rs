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
                    let content = if stderr.is_empty() { stdout.to_string() } else { format!("stdout:\n{stdout}\nstderr:\n{stderr}") };
                    let truncated = if content.len() > 50000 { format!("{}...\n[output truncated]", &content[..50000]) } else { content };
                    ToolResult { content: truncated, is_error: !out.status.success(), ..Default::default() }
                }
                Ok(Err(e)) => ToolResult { content: format!("Command failed: {e}"), is_error: true, ..Default::default() },
                Err(_) => ToolResult { content: "Command timed out".into(), is_error: true, ..Default::default() },
            }
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
