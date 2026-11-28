//! ScheduleWakeup tool: bring the agent back after a delay, continue a
//! self-paced loop with its next round, or end the loop. See
//! [`crate::wakeup`].

use crate::session_runner::AgentSession;
use crate::wakeup::{Scheduled, Stopped, WakeupRequest};
use anyhow::Context as _;
use serde_json::{Value, json};
use std::sync::Weak;
use super_agent::placeholder;
use super_agent::{AgentTool, ExecutionMode, ToolResult, ToolUpdateSink};
use tokio_util::sync::CancellationToken;

pub struct ScheduleWakeupTool {
    session: Weak<AgentSession>,
}

impl ScheduleWakeupTool {
    pub(crate) fn new(session: Weak<AgentSession>) -> Self {
        ScheduleWakeupTool { session }
    }
}

#[async_trait::async_trait]
impl AgentTool for ScheduleWakeupTool {
    fn name(&self) -> &str {
        "ScheduleWakeup"
    }

    fn advanced_only(&self) -> bool {
        true
    }

    /// Two calls of one answer take effect in its order: a stop after a
    /// wake-up cancels it, and the second wake-up replaces the first.
    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Sequential
    }

    fn description(&self) -> String {
        placeholder::tool_description("ScheduleWakeup")
    }

    fn parameters(&self) -> Value {
        let parameter = |name: &str| placeholder::tool_parameter("ScheduleWakeup", name);
        json!({
            "type": "object",
            "properties": {
                "delaySeconds": {"type": "number", "description": parameter("delaySeconds")},
                "reason": {"type": "string", "description": parameter("reason")},
                "prompt": {"type": "string", "description": parameter("prompt")},
                "stop": {"type": "boolean", "description": parameter("stop")},
                "noop": {"type": "boolean", "description": parameter("noop")},
            },
            "additionalProperties": false,
        })
    }

    fn prepare_arguments(&self, mut args: Value) -> Value {
        if let Some(object) = args.as_object_mut() {
            super_agent::tools::arguments::drop_nulls(
                object,
                &["delaySeconds", "reason", "prompt", "stop", "noop"],
            );
            if let Some(Value::String(text)) = object.get("delaySeconds")
                && let Ok(seconds) = text.trim().parse::<f64>()
                && let Some(number) = serde_json::Number::from_f64(seconds)
            {
                object.insert("delaySeconds".into(), Value::Number(number));
            }
            super_agent::tools::arguments::boolean(object, "stop");
            super_agent::tools::arguments::boolean(object, "noop");
        }
        args
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        args: Value,
        _cancel: CancellationToken,
        _on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        let session = self
            .session
            .upgrade()
            .context("The session has ended, so no wake-up can be scheduled.")?;
        let wakeups = session.wakeups();
        let text = |name: &str| args[name].as_str().map(str::to_string);
        let reason = text("reason");
        let noop = args["noop"].as_bool().unwrap_or(false);
        let shown = json!({"reason": reason, "noop": noop});
        if args["stop"].as_bool().unwrap_or(false) {
            let (message, stopped) = match wakeups.stop() {
                Stopped::Loop { round } => (
                    format!("The loop ends after round {round}."),
                    json!({"loop": {"round": round}}),
                ),
                Stopped::Wakeup(wakeup) => (
                    "The pending wake-up is cancelled.".to_string(),
                    json!({"wakeup": wakeup}),
                ),
                Stopped::Nothing => (
                    "No loop runs and no wake-up is pending, so there was nothing to stop."
                        .to_string(),
                    Value::Null,
                ),
            };
            return Ok(result(
                message,
                json!({"stopped": stopped, "reason": shown["reason"], "noop": noop}),
            ));
        }
        let delay = args["delaySeconds"].as_f64().with_context(|| {
            format!(
                "Give delaySeconds, the seconds until the wake-up ({} to {}), or set stop to end the loop.",
                crate::wakeup::MIN_DELAY_SECONDS,
                crate::wakeup::MAX_DELAY_SECONDS
            )
        })?;
        let scheduled = wakeups
            .schedule(WakeupRequest {
                delay_seconds: delay,
                reason,
                prompt: text("prompt"),
                noop,
            })
            .map_err(|error| anyhow::anyhow!(error))?;
        Ok(match scheduled {
            Scheduled::Pending(wakeup) => {
                let message = match wakeup.round {
                    Some(round) => format!(
                        "The loop continues with round {round} in {} seconds.",
                        wakeup.delay_seconds
                    ),
                    None => format!("The wake-up comes in {} seconds.", wakeup.delay_seconds),
                };
                result(
                    message,
                    json!({"wakeup": wakeup, "requestedDelaySeconds": delay}),
                )
            }
            Scheduled::LoopEnds { round, max_rounds } => result(
                format!(
                    "Round {round} of {max_rounds} was the loop's last, the most it may run, so the loop ends here."
                ),
                json!({"stopped": {"loop": {"round": round}}, "maxRounds": max_rounds, "reason": shown["reason"], "noop": noop}),
            ),
        })
    }
}

fn result(text: String, details: Value) -> ToolResult {
    ToolResult {
        content: vec![super_ai::ContentBlock::text(text)],
        details,
        ..Default::default()
    }
}
