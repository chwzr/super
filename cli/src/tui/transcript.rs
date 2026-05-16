use crate::sdk::protocol::{
    BlockDelta, BusMessage, ContentBlockFinal, ContentBlockStream, StreamEvent,
    SystemSubtype,
};

/// One renderable item in the transcript.
#[derive(Debug, Clone)]
pub enum TranscriptItem {
    User { text: String },
    AssistantText { text: String, complete: bool },
    Thinking { text: String, collapsed: bool, elapsed_ms: u64, complete: bool },
    ToolCall {
        tool_use_id: String,
        name: String,
        input: serde_json::Value,
        result: Option<ToolResultRender>,
        elapsed_ms: u64,
    },
    System { subtype: SystemSubtype, message: String },
    /// One or more consecutive read/search tool calls within an assistant turn.
    /// Renders as a single dim-gray summary line by default (e.g. "Read 3 files
    /// (ctrl+o to expand)"); when the global `show_detailed_transcript` flag is
    /// on, each call renders as an individual block. See
    /// cli/docs/tool-call-render-spec.md "Render mode 1".
    ToolBatch { calls: Vec<BatchCall> },
}

#[derive(Debug, Clone)]
pub struct ToolResultRender {
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug, Clone)]
pub struct BatchCall {
    pub tool_use_id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub result: Option<ToolResultRender>,
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
                // A user message that contains any ToolResult is the synthetic
                // tool-results turn emitted after a model turn. Any sibling Text
                // blocks in that message are tool inject_messages (e.g. the
                // SkillTool's full skill body, sent to the model only) — never
                // user input. Matches Claude Code's `isMeta` skip in
                // VirtualMessageList.tsx.
                let is_tool_results_turn = message.content.iter().any(|b| {
                    matches!(b, ContentBlockFinal::ToolResult { .. })
                });
                for block in &message.content {
                    match block {
                        ContentBlockFinal::Text { text } => {
                            if is_tool_results_turn {
                                // Inject_messages — sent to the model, hidden from the screen.
                                continue;
                            }
                            let stripped = strip_system_reminders(text);
                            if !stripped.is_empty() {
                                out.push(TranscriptItem::User { text: stripped });
                            }
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
                                complete: false,
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
                        TranscriptItem::Thinking { complete, .. } => {
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
            BusMessage::RenderEvent { .. } => {
                // RenderSpec events are consumed by the TUI render dispatcher;
                // no transcript item needed.
            }
        }
    }

    out
}

/// Strip `<system-reminder>…</system-reminder>` blocks from user-facing text.
///
/// Mirrors Claude Code's `stripSystemReminders` (components/messageActions.tsx):
/// reminders are sent to the model verbatim, but the TUI hides them so the
/// transcript only shows what the user actually typed.
pub(crate) fn strip_system_reminders(text: &str) -> String {
    const OPEN: &str = "<system-reminder>";
    const CLOSE: &str = "</system-reminder>";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        match rest.find(OPEN) {
            None => {
                out.push_str(rest);
                break;
            }
            Some(start) => {
                out.push_str(&rest[..start]);
                let after_open = &rest[start + OPEN.len()..];
                match after_open.find(CLOSE) {
                    None => {
                        // Unterminated reminder — drop the rest (matches CC behavior of trimming).
                        break;
                    }
                    Some(end) => {
                        rest = &after_open[end + CLOSE.len()..];
                    }
                }
            }
        }
    }
    out.trim().to_string()
}

/// Post-fold transformation: collapse consecutive read/search tool calls into
/// a single `ToolBatch`. Mirrors Claude Code's `collapseReadSearch` pass.
pub fn group_tool_batches(items: Vec<TranscriptItem>) -> Vec<TranscriptItem> {
    use crate::tui::render::tool_family::{classify, ToolFamily};

    let mut out: Vec<TranscriptItem> = Vec::with_capacity(items.len());
    let mut pending: Vec<BatchCall> = Vec::new();

    fn flush(out: &mut Vec<TranscriptItem>, pending: &mut Vec<BatchCall>) {
        if !pending.is_empty() {
            let calls = std::mem::take(pending);
            out.push(TranscriptItem::ToolBatch { calls });
        }
    }

    for item in items {
        match item {
            TranscriptItem::ToolCall { tool_use_id, name, input, result, .. }
                if classify(&name) == ToolFamily::ReadSearch =>
            {
                pending.push(BatchCall { tool_use_id, name, input, result });
            }
            other => {
                flush(&mut out, &mut pending);
                out.push(other);
            }
        }
    }
    flush(&mut out, &mut pending);
    out
}

fn matches_filter(ev: &BusMessage, filter: Option<&str>) -> bool {
    let parent = match ev {
        BusMessage::User { parent_tool_use_id, .. }
        | BusMessage::Assistant { parent_tool_use_id, .. }
        | BusMessage::StreamEvent { parent_tool_use_id, .. }
        | BusMessage::ToolProgress { parent_tool_use_id, .. }
        | BusMessage::SystemEvent { parent_tool_use_id, .. }
        | BusMessage::Result { parent_tool_use_id, .. }
        | BusMessage::RenderEvent { parent_tool_use_id, .. } => parent_tool_use_id.as_deref(),
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

    fn tc(name: &str, id: &str) -> TranscriptItem {
        TranscriptItem::ToolCall {
            tool_use_id: id.into(),
            name: name.into(),
            input: serde_json::json!({}),
            result: Some(ToolResultRender { content: "ok".into(), is_error: false }),
            elapsed_ms: 0,
        }
    }

    #[test]
    fn group_collapses_consecutive_reads_into_one_batch() {
        let items = vec![tc("Read", "1"), tc("Read", "2"), tc("Read", "3")];
        let g = group_tool_batches(items);
        assert_eq!(g.len(), 1);
        match &g[0] {
            TranscriptItem::ToolBatch { calls } => assert_eq!(calls.len(), 3),
            other => panic!("expected ToolBatch, got {other:?}"),
        }
    }

    #[test]
    fn group_does_not_collapse_a_single_mutating_tool() {
        let items = vec![tc("Bash", "1")];
        let g = group_tool_batches(items);
        assert_eq!(g.len(), 1);
        assert!(matches!(g[0], TranscriptItem::ToolCall { .. }));
    }

    #[test]
    fn group_collapses_single_read_into_a_one_call_batch() {
        // Per spec: even N=1 read collapses into "Read 1 file (ctrl+o to expand)".
        let items = vec![tc("Read", "1")];
        let g = group_tool_batches(items);
        assert_eq!(g.len(), 1);
        match &g[0] {
            TranscriptItem::ToolBatch { calls } => assert_eq!(calls.len(), 1),
            other => panic!("expected ToolBatch, got {other:?}"),
        }
    }

    #[test]
    fn group_breaks_at_mutating_tool() {
        let items = vec![
            tc("Read", "1"),
            tc("Read", "2"),
            tc("Bash", "3"),
            tc("Read", "4"),
        ];
        let g = group_tool_batches(items);
        assert_eq!(g.len(), 3);
        assert!(matches!(&g[0], TranscriptItem::ToolBatch { calls } if calls.len() == 2));
        assert!(matches!(&g[1], TranscriptItem::ToolCall { name, .. } if name == "Bash"));
        assert!(matches!(&g[2], TranscriptItem::ToolBatch { calls } if calls.len() == 1));
    }

    #[test]
    fn group_breaks_at_user_or_assistant_text() {
        let items = vec![
            tc("Read", "1"),
            TranscriptItem::AssistantText { text: "thinking".into(), complete: true },
            tc("Read", "2"),
        ];
        let g = group_tool_batches(items);
        assert_eq!(g.len(), 3);
        assert!(matches!(&g[0], TranscriptItem::ToolBatch { .. }));
        assert!(matches!(&g[1], TranscriptItem::AssistantText { .. }));
        assert!(matches!(&g[2], TranscriptItem::ToolBatch { .. }));
    }

    #[test]
    fn group_mixes_grep_and_glob_into_one_batch() {
        let items = vec![tc("Grep", "1"), tc("Glob", "2")];
        let g = group_tool_batches(items);
        assert_eq!(g.len(), 1);
        match &g[0] {
            TranscriptItem::ToolBatch { calls } => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].name, "Grep");
                assert_eq!(calls[1].name, "Glob");
            }
            other => panic!("expected ToolBatch, got {other:?}"),
        }
    }

    fn env(event: StreamEvent) -> BusMessage {
        BusMessage::StreamEvent {
            event,
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        }
    }

    #[test]
    fn strip_system_reminders_removes_wrapper() {
        let s = "<system-reminder>\nSkills available:\n- foo\n</system-reminder>";
        assert_eq!(strip_system_reminders(s), "");
    }

    #[test]
    fn strip_system_reminders_keeps_user_text_intact() {
        let s = "<system-reminder>x</system-reminder>hello world";
        assert_eq!(strip_system_reminders(s), "hello world");
    }

    #[test]
    fn strip_system_reminders_handles_multiple_blocks() {
        let s = "<system-reminder>a</system-reminder>middle<system-reminder>b</system-reminder>tail";
        assert_eq!(strip_system_reminders(s), "middletail");
    }

    #[test]
    fn inject_messages_in_tool_results_turn_are_hidden() {
        // Simulate the engine's tool-results emission: a User message with
        // a ToolResult plus a sibling Text block (the SkillTool's body).
        let events = vec![
            // First: the tool_use (so tool_use_idx is populated).
            env(StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockStream::ToolUse {
                    id: "tu_skill".into(),
                    name: "Skill".into(),
                    input: serde_json::json!({"skill": "brainstorming"}),
                },
            }),
            env(StreamEvent::ContentBlockStop { index: 0 }),
            // Then: the synthetic user turn with the result + injected body.
            BusMessage::User {
                message: UserPayload {
                    role: "user".into(),
                    content: vec![
                        ContentBlockFinal::ToolResult {
                            tool_use_id: "tu_skill".into(),
                            content: "Launching skill: brainstorming".into(),
                            is_error: false,
                        },
                        ContentBlockFinal::Text {
                            text: "# brainstorming\n\nFull skill body here".into(),
                        },
                    ],
                },
                parent_tool_use_id: None,
                uuid: Uuid::new_v4(),
                session_id: "s1".into(),
            },
        ];
        let t = fold(&events, None);
        // Exactly the ToolCall — no leaked User entry for the skill body.
        assert_eq!(t.len(), 1);
        match &t[0] {
            TranscriptItem::ToolCall { name, result, .. } => {
                assert_eq!(name, "Skill");
                let r = result.as_ref().expect("tool result attached");
                assert_eq!(r.content, "Launching skill: brainstorming");
            }
            other => panic!("wrong: {other:?}"),
        }
    }

    #[test]
    fn user_text_block_that_is_pure_reminder_is_not_rendered() {
        let events = vec![BusMessage::User {
            message: UserPayload {
                role: "user".into(),
                content: vec![
                    ContentBlockFinal::Text {
                        text: "<system-reminder>\nSkills...\n</system-reminder>".into(),
                    },
                    ContentBlockFinal::Text { text: "hello".into() },
                ],
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
