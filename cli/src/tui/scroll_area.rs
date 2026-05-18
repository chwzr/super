//! Accumulator for legacy `Message`s and live bus events.
//!
//! Originally a fully-featured scrolling widget; now a thin store. The TUI
//! drains items from here straight into the terminal's native scroll buffer
//! via `Terminal::insert_before`, so there is no in-app scrolling, scrollback,
//! or paging — terminal-native scroll is the only navigation surface.
//!
//! Two queues are tracked separately:
//!
//! * `messages` — synchronous outputs from slash commands, login flow, etc.
//!   Kept as a `Vec<Message>` because the legacy `Message` type is also used
//!   by `state::store::AppState.messages` (for `/context` token tallying,
//!   `/export`, and `compact_messages`). The legacy callers keep working
//!   without changes; the TUI just flushes whatever is appended.
//!
//! * `events` — `BusMessage` envelopes from the conversation engine. The TUI
//!   runs `transcript::fold` over this and flushes completed items per-tick.

use crate::sdk::protocol::BusMessage;

/// Legacy in-TUI message type. Still used by:
/// * `commands/dispatch.rs` — `/context`, `/export`
/// * `conversation/compaction.rs` — token-budget estimation
/// * `state/store.rs` — `AppState.messages`
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum Message {
    User(String),
    Assistant(String),
    ToolCall {
        name: String,
        input: String,
        result: Option<String>,
    },
    System(String),
    Trail(String),
    Thinking,
}

pub struct ScrollArea {
    pub messages: Vec<Message>,
    pub events: Vec<BusMessage>,
    pub show_detailed_transcript: bool,
}

impl ScrollArea {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            events: Vec::new(),
            show_detailed_transcript: false,
        }
    }

    pub fn toggle_detailed_transcript(&mut self) {
        self.show_detailed_transcript = !self.show_detailed_transcript;
    }

    pub fn is_detailed_transcript(&self) -> bool {
        self.show_detailed_transcript
    }

    /// Append a synchronous command output. The TUI will flush it to terminal
    /// scrollback on the next tick.
    pub fn push(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    /// Append a bus event. The TUI runs `transcript::fold` over the events
    /// vec and flushes completed items.
    pub fn push_event(&mut self, ev: BusMessage) {
        self.events.push(ev);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.events.clear();
    }
}

impl Default for ScrollArea {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::protocol::{AnthropicUsage, ContentBlockFinal, UserPayload};
    use uuid::Uuid;

    #[test]
    fn legacy_push_appends_to_messages() {
        let mut sa = ScrollArea::new();
        sa.push(Message::System("hi".into()));
        assert_eq!(sa.messages.len(), 1);
        assert_eq!(sa.events.len(), 0);
    }

    #[test]
    fn push_event_appends_to_events() {
        let mut sa = ScrollArea::new();
        sa.push_event(BusMessage::User {
            message: UserPayload {
                role: "user".into(),
                content: vec![ContentBlockFinal::Text { text: "hi".into() }],
            },
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        });
        assert_eq!(sa.events.len(), 1);
        assert_eq!(sa.messages.len(), 0);
    }

    #[test]
    fn clear_wipes_both_paths() {
        let mut sa = ScrollArea::new();
        sa.push(Message::System("a".into()));
        sa.push_event(BusMessage::Result {
            stop_reason: None,
            usage: AnthropicUsage::default(),
            total_cost_usd: 0.0,
            duration_ms: 0,
            num_turns: 0,
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        });
        sa.clear();
        assert_eq!(sa.messages.len(), 0);
        assert_eq!(sa.events.len(), 0);
    }
}
