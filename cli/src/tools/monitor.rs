use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

use crate::conversation::message_queue::{MessageQueue, PromptInputMode, QueuePriority};

pub struct MonitorTool {
    pub queue: Arc<MessageQueue>,
}

#[async_trait]
impl Tool for MonitorTool {
    fn name(&self) -> &str {
        "Monitor"
    }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Start a background monitor that streams events from a long-running script. \
         Each stdout line is an event. Supports tailing logs, polling for changes, \
         and watching processes."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/monitor.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command or script to monitor"},
                "description": {"type": "string", "description": "Human-readable description"},
                "timeout_ms": {"type": "integer", "description": "Maximum time to run in milliseconds"},
                "persistent": {"type": "boolean", "description": "Run for the lifetime of the session", "default": false}
            },
            "required": ["command", "description"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "acknowledged": {"type": "boolean"},
                "message": {"type": "string"},
                "task_id": {"type": "string"}
            }
        }))
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let command_str = input["command"].as_str().unwrap_or("");
        let description = input["description"]
            .as_str()
            .unwrap_or("monitor")
            .to_string();

        let task_id = Uuid::new_v4().to_string();
        let agent_id = context.parent_tool_use_id.clone();
        let queue = self.queue.clone();
        let desc_for_completion = description.clone();
        let task_id_for_completion = task_id.clone();

        // Spawn the child process, piping stdout
        let mut child = match Command::new("bash")
            .arg("-c")
            .arg(command_str.to_string())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to spawn monitor command: {e}"),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        // Take stdout for line streaming, then move both reader and child into the spawn
        let stdout = child.stdout.take().expect("stdout not piped");
        let reader = BufReader::new(stdout);

        // Spawn background task for line-by-line streaming
        tokio::spawn(async move {
            let mut lines = reader.lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        queue.enqueue_pending_notification(
                            line,
                            PromptInputMode::TaskNotification,
                            QueuePriority::Next,
                            agent_id.clone(),
                        );
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            let status = match child.wait().await {
                Ok(exit) if exit.success() => "completed",
                Ok(_) => "failed",
                Err(_) => "failed",
            };
            let summary = match status {
                "completed" => format!("Monitor \"{desc_for_completion}\" stream ended"),
                _ => format!("Monitor \"{desc_for_completion}\" script failed"),
            };
            let notification = format!(
                "<task-notification>\n  <task-id>{}</task-id>\n  <status>{}</status>\n  <summary>{}</summary>\n</task-notification>",
                task_id_for_completion, status, summary
            );
            queue.enqueue_pending_notification(
                notification,
                PromptInputMode::TaskNotification,
                QueuePriority::Next,
                agent_id,
            );
        });

        // Return immediately
        ToolResult {
            content: json!({
                "acknowledged": true,
                "message": format!("Monitor \"{description}\" started"),
                "task_id": task_id
            })
            .to_string(),
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
