use crate::sdk::protocol::{
    BlockDelta, BusMessage, ContentBlockFinal, ContentBlockStream, StreamEvent,
    SystemSubtype,
};

/// One renderable item in the transcript.
#[derive(Debug, Clone)]
pub enum TranscriptItem {
    User { text: String },
    AssistantText { text: String, complete: bool },
    Thinking { text: String, collapsed: bool, elapsed_ms: u64 },
    ToolCall {
        tool_use_id: String,
        name: String,
        input: serde_json::Value,
        result: Option<ToolResultRender>,
        elapsed_ms: u64,
    },
    System { subtype: SystemSubtype, message: String },
}

#[derive(Debug, Clone)]
pub struct ToolResultRender {
    pub content: String,
    pub is_error: bool,
}

/// Pure fold from a slice of bus events to a transcript.
///
/// `filter` — if `Some(tool_use_id)`, only events whose `parent_tool_use_id`
/// equals that id are folded (used for drilling into a subagent's transcript).
/// `None` returns the root view (events without a parent_tool_use_id).
pub fn fold(events: &[BusMessage], filter: Option<&str>) -> Vec<TranscriptItem> {
    let mut out: Vec<TranscriptItem> = Vec::new();
    // content-block index for the current assistant turn -> position in `out`.
    // Cleared on each new MessageStart.
    let mut block_to_idx: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    // tool_use_id -> position in `out` (so tool_result can attach across turns)
    let mut tool_use_idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for ev in events {
        if !matches_filter(ev, filter) { continue; }
        match ev {
            BusMessage::User { message, .. } => {
                for block in &message.content {
                    match block {
                        ContentBlockFinal::Text { text } => {
                            out.push(TranscriptItem::User { text: text.clone() });
                        }
                        ContentBlockFinal::ToolResult { tool_use_id, content, is_error } => {
                            if let Some(idx) = tool_use_idx.get(tool_use_id) {
                                if let TranscriptItem::ToolCall { result, .. } = &mut out[*idx] {
                                    *result = Some(ToolResultRender {
                                        content: content.clone(),
                                        is_error: *is_error,
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            BusMessage::Assistant { .. } => {
                // Already folded from stream events; no-op.
            }
            BusMessage::StreamEvent { event, .. } => match event {
                StreamEvent::MessageStart { .. } => {
                    // New assistant turn — content-block indices restart.
                    block_to_idx.clear();
                }
                StreamEvent::ContentBlockStart { index, content_block } => {
                    match content_block {
                        ContentBlockStream::Text { text } => {
                            let pos = out.len();
                            out.push(TranscriptItem::AssistantText {
                                text: text.clone(), complete: false,
                            });
                            block_to_idx.insert(*index, pos);
                        }
                        ContentBlockStream::Thinking { thinking, .. } => {
                            let pos = out.len();
                            out.push(TranscriptItem::Thinking {
                                text: thinking.clone(), collapsed: true, elapsed_ms: 0,
                            });
                            block_to_idx.insert(*index, pos);
                        }
                        ContentBlockStream::ToolUse { id, name, input } => {
                            let pos = out.len();
                            out.push(TranscriptItem::ToolCall {
                                tool_use_id: id.clone(),
                                name: name.clone(),
                                input: input.clone(),
                                result: None,
                                elapsed_ms: 0,
                            });
                            block_to_idx.insert(*index, pos);
                            tool_use_idx.insert(id.clone(), pos);
                        }
                    }
                }
                StreamEvent::ContentBlockDelta { index, delta } => {
                    let Some(&pos) = block_to_idx.get(index) else { continue };
                    match (&mut out[pos], delta) {
                        (TranscriptItem::AssistantText { text, .. }, BlockDelta::TextDelta { text: d }) => {
                            text.push_str(d);
                        }
                        (TranscriptItem::Thinking { text, .. }, BlockDelta::ThinkingDelta { thinking: d }) => {
                            text.push_str(d);
                        }
                        (TranscriptItem::ToolCall { input, .. }, BlockDelta::InputJsonDelta { partial_json }) => {
                            // Accumulate partial JSON in a side string under
                            // "__partial__". We finalize on ContentBlockStop.
                            // If the ContentBlockStart already delivered a
                            // non-empty input (rare but allowed by the spec),
                            // seed `__partial__` from the existing object so
                            // the prefix isn't lost.
                            let cur = if let Some(partial) = input.get("__partial__").and_then(|v| v.as_str()) {
                                partial.to_string()
                            } else if input.is_object() && !input.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                                // Serialize the prior input so concatenation
                                // remains valid (or at least recoverable) JSON.
                                serde_json::to_string(input).unwrap_or_default()
                            } else {
                                String::new()
                            };
                            let next = format!("{cur}{partial_json}");
                            *input = serde_json::json!({"__partial__": next});
                        }
                        _ => {}
                    }
                }
                StreamEvent::ContentBlockStop { index } => {
                    let Some(&pos) = block_to_idx.get(index) else { continue };
                    match &mut out[pos] {
                        TranscriptItem::AssistantText { complete, .. } => {
                            *complete = true;
                        }
                        TranscriptItem::ToolCall { input, .. } => {
                            if let Some(partial) = input.get("__partial__").and_then(|v| v.as_str()) {
                                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(partial) {
                                    *input = parsed;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            },
            BusMessage::ToolProgress { tool_use_id, elapsed_seconds, .. } => {
                if let Some(&pos) = tool_use_idx.get(tool_use_id) {
                    if let TranscriptItem::ToolCall { elapsed_ms, .. } = &mut out[pos] {
                        *elapsed_ms = (*elapsed_seconds * 1000.0) as u64;
                    }
                }
            }
            BusMessage::SystemEvent { subtype, message, .. } => {
                out.push(TranscriptItem::System {
                    subtype: subtype.clone(),
                    message: message.clone(),
                });
            }
            BusMessage::Result { .. } => {
                // Renderers may show usage in the header; no transcript item.
            }
        }
    }

    out
}

fn matches_filter(ev: &BusMessage, filter: Option<&str>) -> bool {
    let parent = match ev {
        BusMessage::User { parent_tool_use_id, .. }
        | BusMessage::Assistant { parent_tool_use_id, .. }
        | BusMessage::StreamEvent { parent_tool_use_id, .. }
        | BusMessage::ToolProgress { parent_tool_use_id, .. }
        | BusMessage::SystemEvent { parent_tool_use_id, .. }
        | BusMessage::Result { parent_tool_use_id, .. } => parent_tool_use_id.as_deref(),
    };
    match filter {
        None => parent.is_none(),
        Some(want) => parent == Some(want),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::protocol::{
        AnthropicUsage, AssistantPayload, MessageMeta, UserPayload,
    };
    use uuid::Uuid;

    fn env(event: StreamEvent) -> BusMessage {
        BusMessage::StreamEvent {
            event,
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        }
    }

    #[test]
    fn user_message_appended() {
        let events = vec![BusMessage::User {
            message: UserPayload {
                role: "user".into(),
                content: vec![ContentBlockFinal::Text { text: "hello".into() }],
            },
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        }];
        let t = fold(&events, None);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], TranscriptItem::User { text } if text == "hello"));
    }

    #[test]
    fn streaming_text_deltas_accumulate_into_one_assistant_text_item() {
        let events = vec![
            env(StreamEvent::ContentBlockStart { index: 0, content_block: ContentBlockStream::Text { text: String::new() } }),
            env(StreamEvent::ContentBlockDelta { index: 0, delta: BlockDelta::TextDelta { text: "hel".into() } }),
            env(StreamEvent::ContentBlockDelta { index: 0, delta: BlockDelta::TextDelta { text: "lo".into() } }),
            env(StreamEvent::ContentBlockStop { index: 0 }),
        ];
        let t = fold(&events, None);
        assert_eq!(t.len(), 1);
        match &t[0] {
            TranscriptItem::AssistantText { text, complete } => {
                assert_eq!(text, "hello");
                assert!(*complete);
            }
            other => panic!("wrong: {other:?}"),
        }
    }

    #[test]
    fn tool_use_followed_by_tool_result_attaches() {
        let events = vec![
            env(StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockStream::ToolUse {
                    id: "tu_1".into(), name: "Read".into(), input: serde_json::json!({}),
                },
            }),
            env(StreamEvent::ContentBlockDelta {
                index: 0,
                delta: BlockDelta::InputJsonDelta { partial_json: "{\"file_path\":\"/x\"}".into() },
            }),
            env(StreamEvent::ContentBlockStop { index: 0 }),
            BusMessage::User {
                message: UserPayload {
                    role: "user".into(),
                    content: vec![ContentBlockFinal::ToolResult {
                        tool_use_id: "tu_1".into(),
                        content: "file contents".into(),
                        is_error: false,
                    }],
                },
                parent_tool_use_id: None,
                uuid: Uuid::new_v4(),
                session_id: "s1".into(),
            },
        ];
        let t = fold(&events, None);
        assert_eq!(t.len(), 1);
        match &t[0] {
            TranscriptItem::ToolCall { tool_use_id, name, input, result, .. } => {
                assert_eq!(tool_use_id, "tu_1");
                assert_eq!(name, "Read");
                assert_eq!(input["file_path"], "/x");
                let r = result.as_ref().expect("result attached");
                assert_eq!(r.content, "file contents");
                assert!(!r.is_error);
            }
            other => panic!("wrong: {other:?}"),
        }
    }

    #[test]
    fn result_message_does_not_add_transcript_item() {
        let events = vec![BusMessage::Result {
            stop_reason: Some("end_turn".into()),
            usage: AnthropicUsage::default(),
            total_cost_usd: 0.0, duration_ms: 0, num_turns: 1,
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(), session_id: "s1".into(),
        }];
        let t = fold(&events, None);
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn assistant_final_message_is_skipped_when_already_folded_from_deltas() {
        // The engine emits BusMessage::Assistant after all deltas. If we've
        // already folded the deltas, the Assistant message must not double-add.
        let final_id = "msg_1".to_string();
        let events = vec![
            env(StreamEvent::MessageStart { message: MessageMeta {
                id: final_id.clone(), model: "anthropic/claude-sonnet-4-5".into(), role: "assistant".into(),
                content: vec![], stop_reason: None, stop_sequence: None,
                usage: AnthropicUsage::default(),
            }}),
            env(StreamEvent::ContentBlockStart { index: 0, content_block: ContentBlockStream::Text { text: String::new() } }),
            env(StreamEvent::ContentBlockDelta { index: 0, delta: BlockDelta::TextDelta { text: "hi".into() } }),
            env(StreamEvent::ContentBlockStop { index: 0 }),
            BusMessage::Assistant {
                message: AssistantPayload {
                    id: final_id, model: "anthropic/claude-sonnet-4-5".into(), role: "assistant".into(),
                    content: vec![ContentBlockFinal::Text { text: "hi".into() }],
                    stop_reason: Some("end_turn".into()), usage: AnthropicUsage::default(),
                },
                parent_tool_use_id: None, uuid: Uuid::new_v4(), session_id: "s1".into(),
            },
        ];
        let t = fold(&events, None);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], TranscriptItem::AssistantText { text, .. } if text == "hi"));
    }

    #[test]
    fn tool_progress_preserves_sub_second_precision() {
        let events = vec![
            env(StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockStream::ToolUse {
                    id: "tu_p".into(), name: "Bash".into(), input: serde_json::json!({}),
                },
            }),
            env(StreamEvent::ContentBlockStop { index: 0 }),
            BusMessage::ToolProgress {
                tool_use_id: "tu_p".into(),
                tool_name: "Bash".into(),
                elapsed_seconds: 1.5,
                parent_tool_use_id: None,
                uuid: Uuid::new_v4(),
                session_id: "s1".into(),
            },
        ];
        let t = fold(&events, None);
        assert_eq!(t.len(), 1);
        match &t[0] {
            TranscriptItem::ToolCall { elapsed_ms, .. } => {
                assert_eq!(*elapsed_ms, 1500, "1.5s should be 1500ms not 1000ms");
            }
            other => panic!("wrong: {other:?}"),
        }
    }

    #[test]
    fn fold_demuxes_subagent_events_under_parent_tool_use_id() {
        let tu = "tu_task_1".to_string();
        let events = vec![
            // Root user prompt
            BusMessage::User {
                message: UserPayload {
                    role: "user".into(),
                    content: vec![ContentBlockFinal::Text { text: "find auth".into() }],
                },
                parent_tool_use_id: None,
                uuid: Uuid::new_v4(),
                session_id: "s-root".into(),
            },
            // Root assistant invokes Task
            env(StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockStream::ToolUse {
                    id: tu.clone(), name: "Task".into(),
                    input: serde_json::json!({"description": "find auth", "subagent_type": "Explore"}),
                },
            }),
            env(StreamEvent::ContentBlockStop { index: 0 }),
            // Subagent emits its own user prompt + assistant text — these should
            // NOT show up at the root level.
            BusMessage::User {
                message: UserPayload {
                    role: "user".into(),
                    content: vec![ContentBlockFinal::Text { text: "<subagent prompt>".into() }],
                },
                parent_tool_use_id: Some(tu.clone()),
                uuid: Uuid::new_v4(),
                session_id: "agent-xyz".into(),
            },
            BusMessage::StreamEvent {
                event: StreamEvent::ContentBlockStart { index: 0, content_block: ContentBlockStream::Text { text: "".into() } },
                parent_tool_use_id: Some(tu.clone()),
                uuid: Uuid::new_v4(),
                session_id: "agent-xyz".into(),
            },
            BusMessage::StreamEvent {
                event: StreamEvent::ContentBlockDelta { index: 0, delta: BlockDelta::TextDelta { text: "found auth in src/auth.rs".into() } },
                parent_tool_use_id: Some(tu.clone()),
                uuid: Uuid::new_v4(),
                session_id: "agent-xyz".into(),
            },
            BusMessage::StreamEvent {
                event: StreamEvent::ContentBlockStop { index: 0 },
                parent_tool_use_id: Some(tu.clone()),
                uuid: Uuid::new_v4(),
                session_id: "agent-xyz".into(),
            },
            // The tool_result that closes the Task block
            BusMessage::User {
                message: UserPayload {
                    role: "user".into(),
                    content: vec![ContentBlockFinal::ToolResult {
                        tool_use_id: tu.clone(),
                        content: "found auth in src/auth.rs".into(),
                        is_error: false,
                    }],
                },
                parent_tool_use_id: None,
                uuid: Uuid::new_v4(),
                session_id: "s-root".into(),
            },
        ];

        let root = fold(&events, None);
        // Root should have: User(prompt) + ToolCall(Task, with result attached).
        assert_eq!(root.len(), 2, "root view: {root:?}");
        assert!(matches!(&root[0], TranscriptItem::User { text } if text == "find auth"));
        match &root[1] {
            TranscriptItem::ToolCall { name, result, .. } => {
                assert_eq!(name, "Task");
                let r = result.as_ref().expect("tool result attached");
                assert!(r.content.contains("found auth"));
            }
            other => panic!("wrong root[1]: {other:?}"),
        }

        let sub = fold(&events, Some(&tu));
        // Subagent view should have: User(<subagent prompt>) + AssistantText("found auth in src/auth.rs")
        assert_eq!(sub.len(), 2, "sub view: {sub:?}");
        assert!(matches!(&sub[0], TranscriptItem::User { text } if text.contains("subagent prompt")));
        match &sub[1] {
            TranscriptItem::AssistantText { text, complete } => {
                assert_eq!(text, "found auth in src/auth.rs");
                assert!(*complete);
            }
            other => panic!("wrong sub[1]: {other:?}"),
        }
    }
}
