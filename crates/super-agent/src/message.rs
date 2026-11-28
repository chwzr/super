//! Session-level message union: provider messages plus harness roles.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use super_ai::{
    AssistantMessage, ContentBlock, Message, TimestampMs, ToolResultMessage, UserContent,
    UserMessage,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashExecutionMessage {
    pub command: String,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
    /// True for `!!` commands whose output stays out of the LLM context.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exclude_from_context: bool,
    pub timestamp: TimestampMs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomMessage {
    pub custom_type: String,
    pub content: UserContent,
    pub display: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    pub timestamp: TimestampMs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryMessage {
    pub summary: String,
    /// Entry id we branched away from.
    pub from_id: String,
    pub timestamp: TimestampMs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSummaryMessage {
    pub summary: String,
    pub tokens_before: u64,
    pub timestamp: TimestampMs,
}

/// Every message the harness stores or renders, tagged by `role` on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum AgentMessage {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    BashExecution(BashExecutionMessage),
    Custom(CustomMessage),
    BranchSummary(BranchSummaryMessage),
    CompactionSummary(CompactionSummaryMessage),
}

impl AgentMessage {
    pub fn role(&self) -> &'static str {
        match self {
            AgentMessage::User(_) => "user",
            AgentMessage::Assistant(_) => "assistant",
            AgentMessage::ToolResult(_) => "toolResult",
            AgentMessage::BashExecution(_) => "bashExecution",
            AgentMessage::Custom(_) => "custom",
            AgentMessage::BranchSummary(_) => "branchSummary",
            AgentMessage::CompactionSummary(_) => "compactionSummary",
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        AgentMessage::User(UserMessage {
            content: UserContent::Text(text.into()),
            timestamp: super_ai::now_ms(),
        })
    }
}

impl From<Message> for AgentMessage {
    fn from(m: Message) -> Self {
        match m {
            Message::User(u) => AgentMessage::User(u),
            Message::Assistant(a) => AgentMessage::Assistant(a),
            Message::ToolResult(t) => AgentMessage::ToolResult(t),
        }
    }
}

/// Default conversion of harness messages to provider messages, matching
/// pi's `convertToLlm`: harness roles become user messages with labeled
/// text, UI-only content is dropped. A tool call without a result gets an
/// aborted one, so the messages make a request a provider takes.
pub fn convert_to_llm(messages: &[AgentMessage]) -> Vec<Message> {
    answer_cut_off_calls(convert_to_llm_as_is(messages))
}

/// [`convert_to_llm`] without the results it adds for tool calls that have
/// none, for a check of the conversation as it is stored.
pub fn convert_to_llm_as_is(messages: &[AgentMessage]) -> Vec<Message> {
    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        match m {
            AgentMessage::User(u) => out.push(Message::User(u.clone())),
            AgentMessage::Assistant(a) => out.push(Message::Assistant(a.clone())),
            AgentMessage::ToolResult(t) => out.push(Message::ToolResult(t.clone())),
            AgentMessage::BashExecution(b) => {
                if b.exclude_from_context {
                    continue;
                }
                let mut text = format!("Ran shell command: {}\nOutput:\n{}", b.command, b.output);
                if let Some(code) = b.exit_code
                    && code != 0
                {
                    text.push_str(&format!("\nExit code: {code}"));
                }
                out.push(Message::User(UserMessage {
                    content: UserContent::Text(text),
                    timestamp: b.timestamp,
                }));
            }
            AgentMessage::Custom(c) => out.push(Message::User(UserMessage {
                content: c.content.clone(),
                timestamp: c.timestamp,
            })),
            AgentMessage::BranchSummary(b) => out.push(Message::User(UserMessage {
                content: UserContent::Text(format!(
                    "Summary of an abandoned conversation branch:\n\n{}",
                    b.summary
                )),
                timestamp: b.timestamp,
            })),
            AgentMessage::CompactionSummary(c) => out.push(Message::User(UserMessage {
                content: UserContent::Text(format!(
                    "The conversation history before this point was compacted into this summary:\n\n{}",
                    c.summary
                )),
                timestamp: c.timestamp,
            })),
        }
    }
    // Providers reject empty-content user turns. Drop them defensively.
    out.retain(|m| match m {
        Message::User(u) => !u.content.as_text().trim().is_empty() || matches!(&u.content, UserContent::Blocks(b) if b.iter().any(|c| matches!(c, ContentBlock::Image { .. }))),
        _ => true,
    });
    out
}

/// The text of a tool call's result when the call did not run to its end:
/// the run was cancelled, or the conversation has no result for the call.
pub const ABORTED_RESULT: &str = "Operation aborted";

/// Give every tool call without a result an aborted one, right after the
/// results that follow its assistant message. A call has no result when the
/// process that ran it ended before the result was stored (a stored
/// transcript of a session or an agent that was cut off), and providers
/// refuse a request with such a call. A result anywhere before the next
/// assistant message counts, so nothing is answered twice.
fn answer_cut_off_calls(messages: Vec<Message>) -> Vec<Message> {
    let unanswered = |index: usize, assistant: &AssistantMessage| -> Calls {
        let answered: std::collections::HashSet<&str> = messages[index + 1..]
            .iter()
            .take_while(|message| !matches!(message, Message::Assistant(_)))
            .filter_map(|message| match message {
                Message::ToolResult(result) => Some(result.tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assistant
            .tool_calls()
            .filter(|call| !answered.contains(call.id.as_str()))
            .map(|call| (call.id.clone(), call.name.clone()))
            .collect()
    };
    let missing: Vec<(usize, Calls, TimestampMs)> = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| match message {
            Message::Assistant(assistant) => {
                let calls = unanswered(index, assistant);
                (!calls.is_empty()).then_some((index, calls, assistant.timestamp))
            }
            _ => None,
        })
        .collect();
    if missing.is_empty() {
        return messages;
    }
    let mut out = Vec::with_capacity(messages.len() + missing.len());
    let mut pending: Calls = Vec::new();
    let mut timestamp = 0;
    let mut missing = missing.into_iter().peekable();
    for (index, message) in messages.into_iter().enumerate() {
        if !pending.is_empty() && !matches!(message, Message::ToolResult(_)) {
            out.extend(cut_off_results(std::mem::take(&mut pending), timestamp));
        }
        out.push(message);
        if let Some((_, calls, at)) = missing.next_if(|(at, _, _)| *at == index) {
            pending = calls;
            timestamp = at;
        }
    }
    out.extend(cut_off_results(pending, timestamp));
    out
}

/// Tool calls by id and tool name.
type Calls = Vec<(String, String)>;

fn cut_off_results(calls: Calls, timestamp: TimestampMs) -> Vec<Message> {
    calls
        .into_iter()
        .map(|(tool_call_id, tool_name)| {
            Message::ToolResult(ToolResultMessage {
                tool_call_id,
                tool_name,
                content: vec![ContentBlock::text(ABORTED_RESULT)],
                details: None,
                usage: None,
                is_error: true,
                timestamp,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_tags_match_pi() {
        let m = AgentMessage::BashExecution(BashExecutionMessage {
            command: "ls".into(),
            output: "a".into(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            exclude_from_context: false,
            timestamp: 1,
        });
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["role"], "bashExecution");
        assert_eq!(v["exitCode"], 0);

        let m = AgentMessage::CompactionSummary(CompactionSummaryMessage {
            summary: "s".into(),
            tokens_before: 5,
            timestamp: 1,
        });
        assert_eq!(
            serde_json::to_value(&m).unwrap()["role"],
            "compactionSummary"
        );
    }

    #[test]
    fn excluded_bash_dropped_from_llm() {
        let messages = vec![
            AgentMessage::user("hi"),
            AgentMessage::BashExecution(BashExecutionMessage {
                command: "secret".into(),
                output: "x".into(),
                exit_code: Some(0),
                cancelled: false,
                truncated: false,
                full_output_path: None,
                exclude_from_context: true,
                timestamp: 1,
            }),
        ];
        assert_eq!(convert_to_llm(&messages).len(), 1);
    }

    fn calls(ids: &[&str]) -> AgentMessage {
        let mut message = AssistantMessage::empty("api", "provider", "model");
        message.content = ids
            .iter()
            .map(|id| {
                ContentBlock::ToolCall(super_ai::ToolCall {
                    id: id.to_string(),
                    name: "Bash".into(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                })
            })
            .collect();
        message.stop_reason = super_ai::StopReason::ToolUse;
        AgentMessage::Assistant(message)
    }

    fn result(id: &str) -> AgentMessage {
        AgentMessage::ToolResult(ToolResultMessage {
            tool_call_id: id.into(),
            tool_name: "Bash".into(),
            content: vec![ContentBlock::text("done")],
            details: None,
            usage: None,
            is_error: false,
            timestamp: 0,
        })
    }

    /// Each message as `role:id` (tool results) or its role.
    fn shape(messages: &[Message]) -> Vec<String> {
        messages
            .iter()
            .map(|message| match message {
                Message::User(_) => "user".to_string(),
                Message::Assistant(_) => "assistant".to_string(),
                Message::ToolResult(result) if result.is_error => {
                    format!("aborted:{}", result.tool_call_id)
                }
                Message::ToolResult(result) => format!("result:{}", result.tool_call_id),
            })
            .collect()
    }

    #[test]
    fn a_tool_call_cut_off_without_a_result_gets_an_aborted_one_in_the_request() {
        // An agent cut off while its call ran, then resumed with a message.
        let messages = vec![
            AgentMessage::user("go"),
            calls(&["a", "b"]),
            result("a"),
            AgentMessage::user("go on"),
            calls(&["c"]),
        ];
        let converted = convert_to_llm(&messages);
        assert_eq!(
            shape(&converted),
            [
                "user",
                "assistant",
                "result:a",
                "aborted:b",
                "user",
                "assistant",
                "aborted:c"
            ]
        );
        let Message::ToolResult(aborted) = &converted[3] else {
            unreachable!()
        };
        assert_eq!(aborted.tool_name, "Bash");
        assert_eq!(aborted.content, vec![ContentBlock::text(ABORTED_RESULT)]);

        // A conversation where every call has its result is sent as it is,
        // also when something else came between a call and its result.
        let whole = vec![
            AgentMessage::user("go"),
            calls(&["a"]),
            AgentMessage::user("meanwhile"),
            result("a"),
            calls(&["b"]),
            result("b"),
        ];
        assert_eq!(
            shape(&convert_to_llm(&whole)),
            [
                "user",
                "assistant",
                "user",
                "result:a",
                "assistant",
                "result:b"
            ]
        );
    }
}
