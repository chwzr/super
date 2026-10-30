//! Turning internal harness events into the JSON every SDK surface publishes.
//!
//! There is exactly one encoder so the `--mode json` stream, the `--mode rpc`
//! stream, the Rust event channel, and the Python and TypeScript event
//! iterators all show a caller the same object for the same occurrence.
//!
//! Streaming `message_update` events carry only the delta, never a cumulative
//! snapshot of the partial message. A client that wants a live partial message
//! assembles it from `message_start` plus the deltas, keyed by `contentIndex`,
//! and treats `message_end.message` as authoritative. Sending the snapshot on
//! every token would multiply the bytes on the wire by the length of the reply.

use crate::protocol::Command;
use serde_json::{Value, json};
use super_agent::AgentEvent;
use super_ai::AssistantEvent;
use super_coding::session_runner::{SessionEvent, WorkflowTurnStatus};

/// Encode one streaming assistant delta.
pub fn assistant_event_json(event: &AssistantEvent) -> Value {
    match event {
        AssistantEvent::Start { .. } => json!({"type": "start"}),
        AssistantEvent::TextStart { content_index, .. } => {
            json!({"type": "text_start", "contentIndex": content_index})
        }
        AssistantEvent::TextDelta {
            content_index,
            delta,
            ..
        } => {
            json!({"type": "text_delta", "contentIndex": content_index, "delta": delta})
        }
        AssistantEvent::TextEnd {
            content_index,
            content,
            ..
        } => {
            json!({"type": "text_end", "contentIndex": content_index, "content": content})
        }
        AssistantEvent::ThinkingStart { content_index, .. } => {
            json!({"type": "thinking_start", "contentIndex": content_index})
        }
        AssistantEvent::ThinkingDelta {
            content_index,
            delta,
            ..
        } => {
            json!({"type": "thinking_delta", "contentIndex": content_index, "delta": delta})
        }
        AssistantEvent::ThinkingEnd {
            content_index,
            content,
            ..
        } => {
            json!({"type": "thinking_end", "contentIndex": content_index, "content": content})
        }
        AssistantEvent::ToolCallStart {
            content_index,
            tool_call,
        } => {
            json!({
                "type": "toolcall_start", "contentIndex": content_index,
                "id": tool_call.id, "toolName": tool_call.name,
            })
        }
        AssistantEvent::ToolCallDelta {
            content_index,
            delta,
            ..
        } => {
            json!({"type": "toolcall_delta", "contentIndex": content_index, "delta": delta})
        }
        AssistantEvent::ToolCallEnd {
            content_index,
            tool_call,
            ..
        } => {
            json!({"type": "toolcall_end", "contentIndex": content_index, "toolCall": tool_call})
        }
        AssistantEvent::Done { reason, message } => {
            json!({"type": "done", "reason": reason, "message": message})
        }
        AssistantEvent::Error { reason, message } => {
            json!({"type": "error", "reason": reason, "error": message})
        }
    }
}

/// Encode one harness event, or `None` when it has no public form.
pub fn session_event_json(event: &SessionEvent) -> Option<Value> {
    Some(match event {
        SessionEvent::Agent(agent_event) => match agent_event.as_ref() {
            AgentEvent::AgentStart => json!({"type": "agent_start"}),
            AgentEvent::AgentEnd { messages } => json!({"type": "agent_end", "messages": messages}),
            AgentEvent::TurnStart => json!({"type": "turn_start"}),
            AgentEvent::TurnEnd {
                message,
                tool_results,
            } => {
                json!({"type": "turn_end", "message": message, "toolResults": tool_results})
            }
            AgentEvent::MessageStart { message } => {
                json!({"type": "message_start", "message": message})
            }
            AgentEvent::MessageUpdate {
                assistant_event, ..
            } => {
                json!({
                    "type": "message_update",
                    "assistantMessageEvent": assistant_event_json(assistant_event.as_ref()),
                })
            }
            AgentEvent::MessageEnd { message } => {
                json!({"type": "message_end", "message": message})
            }
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                json!({
                    "type": "tool_execution_start", "toolCallId": tool_call_id,
                    "toolName": tool_name, "args": args,
                })
            }
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                tool_name,
                args,
                partial,
            } => json!({
                "type": "tool_execution_update", "toolCallId": tool_call_id, "toolName": tool_name,
                "args": args,
                "partialResult": {"content": partial.content, "details": partial.details},
            }),
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => json!({
                "type": "tool_execution_end", "toolCallId": tool_call_id, "toolName": tool_name,
                "result": {"content": result.content, "details": result.details},
                "isError": is_error,
            }),
        },
        SessionEvent::QueueUpdate {
            steering,
            follow_up,
        } => {
            json!({"type": "queue_update", "steering": steering, "followUp": follow_up})
        }
        SessionEvent::CompactionStart { auto } => json!({"type": "compaction_start", "auto": auto}),
        SessionEvent::CompactionEnd {
            summary,
            tokens_before,
            error,
        } => json!({
            "type": "compaction_end", "summary": summary,
            "tokensBefore": tokens_before, "error": error,
        }),
        SessionEvent::Retry {
            attempt,
            max,
            delay_ms,
            error,
        } => json!({
            "type": "retry", "attempt": attempt, "max": max,
            "delayMs": delay_ms, "error": error,
        }),
        SessionEvent::ModelChanged { provider, model_id } => {
            json!({"type": "model_changed", "provider": provider, "modelId": model_id})
        }
        SessionEvent::ReasoningEffortChanged { level, generations } => json!({
            "type": "reasoning_effort_changed",
            "level": level.as_str(),
            "generations": generations,
        }),
        SessionEvent::ReasoningEffortFallback { level, reason } => json!({
            "type": "reasoning_effort_fallback",
            "level": level.as_str(),
            "reason": reason,
        }),
        SessionEvent::Workflow { run, version } => {
            json!({"type": "workflow_progress", "run": run, "version": version})
        }
        SessionEvent::WorkflowOutcome { run, name, status } => {
            let status = match status {
                WorkflowTurnStatus::Cancelled => "cancelled",
                WorkflowTurnStatus::Completed => "completed",
                WorkflowTurnStatus::Failed => "failed",
                WorkflowTurnStatus::Stopped => "stopped",
            };
            json!({"type": "workflow_outcome", "run": run, "name": name, "status": status})
        }
        SessionEvent::Iterative { job, version } => {
            json!({"type": "iterative_progress", "job": job, "version": version})
        }
        SessionEvent::AccountSwitched(switch) => json!({
            "type": "account_switched", "provider": switch.provider, "from": switch.from,
            "to": switch.to, "index": switch.index, "total": switch.total,
            "retryAfterSecs": switch.retry_after_secs,
        }),
    })
}

/// Turn one encoded streaming delta back into the harness type. `start`
/// carries no partial message on the wire and comes back with an empty one.
pub fn decode_assistant_event(value: &Value) -> Option<AssistantEvent> {
    let index = || value["contentIndex"].as_u64().unwrap_or(0) as usize;
    let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();
    Some(match value["type"].as_str()? {
        "start" => AssistantEvent::Start {
            partial: super_ai::AssistantMessage::empty("", "", ""),
        },
        "text_start" => AssistantEvent::TextStart {
            content_index: index(),
        },
        "text_delta" => AssistantEvent::TextDelta {
            content_index: index(),
            delta: text("delta"),
        },
        "text_end" => AssistantEvent::TextEnd {
            content_index: index(),
            content: text("content"),
        },
        "thinking_start" => AssistantEvent::ThinkingStart {
            content_index: index(),
        },
        "thinking_delta" => AssistantEvent::ThinkingDelta {
            content_index: index(),
            delta: text("delta"),
        },
        "thinking_end" => AssistantEvent::ThinkingEnd {
            content_index: index(),
            content: text("content"),
        },
        "toolcall_start" => AssistantEvent::ToolCallStart {
            content_index: index(),
            tool_call: super_ai::ToolCall {
                id: text("id"),
                name: text("toolName"),
                arguments: json!({}),
                thought_signature: None,
            },
        },
        "toolcall_delta" => AssistantEvent::ToolCallDelta {
            content_index: index(),
            delta: text("delta"),
        },
        "toolcall_end" => AssistantEvent::ToolCallEnd {
            content_index: index(),
            tool_call: serde_json::from_value(value["toolCall"].clone()).ok()?,
        },
        "done" => AssistantEvent::Done {
            reason: serde_json::from_value(value["reason"].clone()).ok()?,
            message: serde_json::from_value(value["message"].clone()).ok()?,
        },
        "error" => AssistantEvent::Error {
            reason: serde_json::from_value(value["reason"].clone()).ok()?,
            message: serde_json::from_value(value["error"].clone()).ok()?,
        },
        _ => return None,
    })
}

/// Turn one event of the public stream back into the harness event it was
/// encoded from, the inverse of [`session_event_json`]. A front end that
/// shows a session another process runs (the terminal attached to the
/// SUPER daemon) feeds the result to the same code that renders its own
/// events. `None` for events with no harness form (`agent_settled`,
/// approvals, `command_applied`, ...) and for malformed ones.
pub fn decode_session_event(value: &Value) -> Option<SessionEvent> {
    fn field<T: serde::de::DeserializeOwned>(value: &Value, key: &str) -> Option<T> {
        serde_json::from_value(value.get(key)?.clone()).ok()
    }
    fn tool_result(value: &Value) -> super_agent::ToolResult {
        super_agent::ToolResult {
            content: field(value, "content").unwrap_or_default(),
            details: value.get("details").cloned().unwrap_or(Value::Null),
            ..Default::default()
        }
    }
    let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();
    let agent = |event: AgentEvent| Some(SessionEvent::Agent(Box::new(event)));
    match value["type"].as_str()? {
        "agent_start" => agent(AgentEvent::AgentStart),
        "agent_end" => agent(AgentEvent::AgentEnd {
            messages: field(value, "messages").unwrap_or_default(),
        }),
        "turn_start" => agent(AgentEvent::TurnStart),
        "turn_end" => agent(AgentEvent::TurnEnd {
            message: field(value, "message")?,
            tool_results: field(value, "toolResults").unwrap_or_default(),
        }),
        "message_start" => agent(AgentEvent::MessageStart {
            message: field(value, "message")?,
        }),
        "message_update" => agent(AgentEvent::MessageUpdate {
            assistant_event: Box::new(decode_assistant_event(&value["assistantMessageEvent"])?),
        }),
        "message_end" => agent(AgentEvent::MessageEnd {
            message: field(value, "message")?,
        }),
        "tool_execution_start" => agent(AgentEvent::ToolExecutionStart {
            tool_call_id: text("toolCallId"),
            tool_name: text("toolName"),
            args: value["args"].clone(),
        }),
        "tool_execution_update" => agent(AgentEvent::ToolExecutionUpdate {
            tool_call_id: text("toolCallId"),
            tool_name: text("toolName"),
            args: value["args"].clone(),
            partial: tool_result(&value["partialResult"]),
        }),
        "tool_execution_end" => {
            let is_error = value["isError"].as_bool().unwrap_or(false);
            agent(AgentEvent::ToolExecutionEnd {
                tool_call_id: text("toolCallId"),
                tool_name: text("toolName"),
                result: super_agent::ToolResult {
                    is_error,
                    ..tool_result(&value["result"])
                },
                is_error,
            })
        }
        "queue_update" => Some(SessionEvent::QueueUpdate {
            steering: field(value, "steering").unwrap_or_default(),
            follow_up: field(value, "followUp").unwrap_or_default(),
        }),
        "compaction_start" => Some(SessionEvent::CompactionStart {
            auto: value["auto"].as_bool().unwrap_or(false),
        }),
        "compaction_end" => Some(SessionEvent::CompactionEnd {
            summary: text("summary"),
            tokens_before: value["tokensBefore"].as_u64().unwrap_or(0),
            error: field(value, "error"),
        }),
        "retry" => Some(SessionEvent::Retry {
            attempt: value["attempt"].as_u64().unwrap_or(0) as u32,
            max: value["max"].as_u64().unwrap_or(0) as u32,
            delay_ms: value["delayMs"].as_u64().unwrap_or(0),
            error: text("error"),
        }),
        "model_changed" => Some(SessionEvent::ModelChanged {
            provider: text("provider"),
            model_id: text("modelId"),
        }),
        "reasoning_effort_changed" => Some(SessionEvent::ReasoningEffortChanged {
            level: super_ai::ThinkingLevel::parse(value["level"].as_str()?)?,
            generations: value["generations"].as_u64().unwrap_or(0) as u8,
        }),
        "reasoning_effort_fallback" => Some(SessionEvent::ReasoningEffortFallback {
            level: super_ai::ThinkingLevel::parse(value["level"].as_str()?)?,
            reason: text("reason"),
        }),
        "workflow_progress" => Some(SessionEvent::Workflow {
            run: value["run"].as_u64()?,
            version: value["version"].as_u64().unwrap_or(0),
        }),
        "workflow_outcome" => Some(SessionEvent::WorkflowOutcome {
            run: value["run"].as_u64(),
            name: text("name"),
            status: match value["status"].as_str()? {
                "cancelled" => WorkflowTurnStatus::Cancelled,
                "completed" => WorkflowTurnStatus::Completed,
                "failed" => WorkflowTurnStatus::Failed,
                "stopped" => WorkflowTurnStatus::Stopped,
                _ => return None,
            },
        }),
        "iterative_progress" => Some(SessionEvent::Iterative {
            job: value["job"].as_u64()?,
            version: value["version"].as_u64().unwrap_or(0),
        }),
        "account_switched" => Some(SessionEvent::AccountSwitched(
            super_ai::auth::accounts::AccountSwitch {
                provider: text("provider"),
                from: text("from"),
                to: text("to"),
                index: value["index"].as_u64().unwrap_or(0) as usize,
                total: value["total"].as_u64().unwrap_or(0) as usize,
                retry_after_secs: value["retryAfterSecs"].as_u64().unwrap_or(0),
            },
        )),
        _ => None,
    }
}

/// Emitted by the SDK once a prompt run has fully settled: no retry, no
/// automatic compaction, and no queued message remains. A client uses it to
/// know the agent is idle again.
pub fn agent_settled() -> Value {
    json!({"type": "agent_settled"})
}

/// Emitted while a direct `bash` *command* streams output. `id` is the
/// correlation id of the originating request, so a client can tell two
/// concurrent commands apart. This is distinct from `tool_execution_update`,
/// which reports a shell command the *model* asked for.
pub fn bash_execution_update(id: Option<&str>, delta: &str) -> Value {
    match id {
        Some(id) => json!({"type": "bash_execution_update", "id": id, "delta": delta}),
        None => json!({"type": "bash_execution_update", "delta": delta}),
    }
}

/// The command as `command_applied` carries it, or `None` for a command
/// that is not announced: reads change nothing, and an answer is announced
/// as `approval_resolved` already. Images are counted, not repeated, since
/// every client receives them with the message.
pub fn announced_command(command: &Command) -> Option<Value> {
    let name = command.name();
    if name.starts_with("get_") || matches!(name, "ping" | "answer_approval") {
        return None;
    }
    let mut value = serde_json::to_value(command).ok()?;
    if let Some(object) = value.as_object_mut()
        && let Some(Value::Array(images)) = object.remove("images")
    {
        object.insert("imageCount".into(), json!(images.len()));
    }
    Some(value)
}

/// Emitted after a command from another surface ran: a daemon client, the
/// app through the platform. `origin` is the name that surface gave itself
/// (`hello.name` on the daemon socket), `command` the command as sent (with
/// `imageCount` in place of images) and `data` its response's data. Every
/// client of the session receives it, the sender included, so all of them
/// can show what changed and who changed it.
pub fn command_applied(
    origin: &str,
    command: Value,
    success: bool,
    error: Option<&str>,
    data: Option<Value>,
) -> Value {
    let mut event = json!({
        "type": "command_applied",
        "origin": origin,
        "command": command,
        "success": success,
    });
    if let Some(error) = error {
        event["error"] = json!(error);
    }
    if let Some(data) = data {
        event["data"] = data;
    }
    event
}

/// Emitted when a subscriber consumed events too slowly and the buffer
/// overwrote some. The client should re-read state rather than assume it saw
/// everything.
pub fn event_lag(skipped: u64) -> Value {
    json!({"type": "event_lag", "skipped": skipped})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_outcomes_have_a_controller_owned_json_event() {
        let event = session_event_json(&SessionEvent::WorkflowOutcome {
            run: None,
            name: "audit".into(),
            status: WorkflowTurnStatus::Cancelled,
        })
        .expect("workflow outcome event");
        assert_eq!(
            event,
            json!({
                "type": "workflow_outcome",
                "run": null,
                "name": "audit",
                "status": "cancelled"
            })
        );
    }

    #[test]
    fn reasoning_effort_changes_have_a_public_json_event() {
        let event = session_event_json(&SessionEvent::ReasoningEffortChanged {
            level: super_ai::ThinkingLevel::Xhigh,
            generations: 2,
        })
        .unwrap();
        assert_eq!(
            event,
            json!({
                "type": "reasoning_effort_changed",
                "level": "xhigh",
                "generations": 2
            })
        );
    }

    #[test]
    fn reasoning_fallback_reports_the_saved_effort_and_cause() {
        let event = session_event_json(&SessionEvent::ReasoningEffortFallback {
            level: super_ai::ThinkingLevel::Medium,
            reason: "request timed out".into(),
        })
        .unwrap();
        assert_eq!(
            event,
            json!({
                "type": "reasoning_effort_fallback",
                "level": "medium",
                "reason": "request timed out"
            })
        );
    }

    #[test]
    fn a_tool_call_start_names_the_tool_so_clients_can_show_it_immediately() {
        let event = assistant_event_json(&AssistantEvent::ToolCallStart {
            content_index: 1,
            tool_call: super_ai::ToolCall {
                id: "call_1".into(),
                name: "write".into(),
                arguments: json!({}),
                thought_signature: None,
            },
        });
        assert_eq!(event["type"], "toolcall_start");
        assert_eq!(event["id"], "call_1");
        assert_eq!(event["toolName"], "write");
    }

    /// Encode then decode: the attached terminal must see the event the
    /// daemon's engine emitted, field for field.
    fn round_trip(event: SessionEvent) {
        let encoded = session_event_json(&event).expect("a public form");
        let decoded = decode_session_event(&encoded).expect("decodes");
        assert_eq!(
            session_event_json(&decoded),
            Some(encoded.clone()),
            "{encoded}"
        );
    }

    #[test]
    fn every_session_event_survives_the_wire() {
        use super_agent::AgentMessage;
        let assistant = super_ai::AssistantMessage::empty("openai-completions", "mock", "mock-1");
        let tool_call = super_ai::ToolCall {
            id: "call_1".into(),
            name: "read".into(),
            arguments: json!({"path": "a.rs"}),
            thought_signature: None,
        };
        let result = super_agent::ToolResult {
            content: vec![super_ai::ContentBlock::text("fn main() {}")],
            details: json!({"lines": 1}),
            ..Default::default()
        };
        let agent = |event| SessionEvent::Agent(Box::new(event));
        for event in [
            agent(AgentEvent::AgentStart),
            agent(AgentEvent::AgentEnd {
                messages: vec![AgentMessage::user("hi")],
            }),
            agent(AgentEvent::TurnStart),
            agent(AgentEvent::TurnEnd {
                message: AgentMessage::Assistant(assistant.clone()),
                tool_results: Vec::new(),
            }),
            agent(AgentEvent::MessageStart {
                message: AgentMessage::user("hi"),
            }),
            agent(AgentEvent::MessageEnd {
                message: AgentMessage::Assistant(assistant.clone()),
            }),
            agent(AgentEvent::ToolExecutionStart {
                tool_call_id: "call_1".into(),
                tool_name: "read".into(),
                args: json!({"path": "a.rs"}),
            }),
            agent(AgentEvent::ToolExecutionUpdate {
                tool_call_id: "call_1".into(),
                tool_name: "read".into(),
                args: json!({"path": "a.rs"}),
                partial: result.clone(),
            }),
            agent(AgentEvent::ToolExecutionEnd {
                tool_call_id: "call_1".into(),
                tool_name: "read".into(),
                result: result.clone(),
                is_error: true,
            }),
            SessionEvent::QueueUpdate {
                steering: vec!["a".into()],
                follow_up: vec!["b".into()],
            },
            SessionEvent::CompactionStart { auto: true },
            SessionEvent::CompactionEnd {
                summary: "s".into(),
                tokens_before: 42,
                error: Some("nope".into()),
            },
            SessionEvent::Retry {
                attempt: 1,
                max: 3,
                delay_ms: 2000,
                error: "429".into(),
            },
            SessionEvent::ModelChanged {
                provider: "mock".into(),
                model_id: "mock-1".into(),
            },
            SessionEvent::ReasoningEffortChanged {
                level: super_ai::ThinkingLevel::High,
                generations: 2,
            },
            SessionEvent::ReasoningEffortFallback {
                level: super_ai::ThinkingLevel::Low,
                reason: "timeout".into(),
            },
            SessionEvent::Workflow { run: 7, version: 3 },
            SessionEvent::WorkflowOutcome {
                run: Some(7),
                name: "audit".into(),
                status: WorkflowTurnStatus::Completed,
            },
            SessionEvent::Iterative { job: 2, version: 9 },
            SessionEvent::AccountSwitched(super_ai::auth::accounts::AccountSwitch {
                provider: "anthropic".into(),
                from: "work".into(),
                to: "personal".into(),
                index: 1,
                total: 2,
                retry_after_secs: 60,
            }),
        ] {
            round_trip(event);
        }
        for delta in [
            AssistantEvent::TextStart { content_index: 0 },
            AssistantEvent::TextDelta {
                content_index: 0,
                delta: "Hel".into(),
            },
            AssistantEvent::TextEnd {
                content_index: 0,
                content: "Hello".into(),
            },
            AssistantEvent::ThinkingStart { content_index: 1 },
            AssistantEvent::ThinkingDelta {
                content_index: 1,
                delta: "hm".into(),
            },
            AssistantEvent::ThinkingEnd {
                content_index: 1,
                content: "hmm".into(),
            },
            AssistantEvent::ToolCallStart {
                content_index: 2,
                tool_call: super_ai::ToolCall {
                    arguments: json!({}),
                    ..tool_call.clone()
                },
            },
            AssistantEvent::ToolCallDelta {
                content_index: 2,
                delta: "{".into(),
            },
            AssistantEvent::ToolCallEnd {
                content_index: 2,
                tool_call: tool_call.clone(),
            },
            AssistantEvent::Done {
                reason: super_ai::StopReason::Stop,
                message: assistant.clone(),
            },
        ] {
            round_trip(agent(AgentEvent::MessageUpdate {
                assistant_event: Box::new(delta),
            }));
        }
    }

    #[test]
    fn events_without_a_harness_form_decode_to_none() {
        for value in [
            agent_settled(),
            event_lag(3),
            json!({"type": "approval_request", "approvalId": "a1"}),
            command_applied("Terminal", json!({"type": "abort"}), true, None, None),
            json!({"type": "message_end"}),
            json!({"no": "type"}),
        ] {
            assert!(decode_session_event(&value).is_none(), "{value}");
        }
    }

    #[test]
    fn a_command_applied_event_names_origin_command_and_outcome() {
        assert_eq!(
            command_applied(
                "Terminal",
                json!({"type": "set_thinking_level", "level": "low"}),
                false,
                Some("model mock/mock-1 does not support thinking level low"),
                None,
            ),
            json!({
                "type": "command_applied",
                "origin": "Terminal",
                "command": {"type": "set_thinking_level", "level": "low"},
                "success": false,
                "error": "model mock/mock-1 does not support thinking level low",
            })
        );
    }

    #[test]
    fn bash_updates_carry_their_request_id() {
        assert_eq!(
            bash_execution_update(Some("req-1"), "hello"),
            json!({"type": "bash_execution_update", "id": "req-1", "delta": "hello"})
        );
    }
}
