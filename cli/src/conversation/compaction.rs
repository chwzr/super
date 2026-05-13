use crate::tui::scroll_area::Message;

/// Truncate conversation history to fit within estimated token budget.
/// Keeps most recent messages, estimates 1 token ~ 4 chars.
pub fn compact_messages(messages: &[Message], max_tokens: usize) -> Vec<Message> {
    let max_chars = max_tokens * 4;
    let mut kept: Vec<Message> = Vec::new();
    let mut total_chars = 0usize;

    for msg in messages.iter().rev() {
        let chars = estimate_chars(msg);
        if total_chars + chars > max_chars && !kept.is_empty() {
            break;
        }
        total_chars += chars;
        kept.push(msg.clone());
    }
    kept.reverse();
    kept
}

fn estimate_chars(msg: &Message) -> usize {
    match msg {
        Message::User(s) => s.len() + 12,
        Message::Assistant(s) => s.len() + 15,
        Message::ToolCall { input, result, .. } => {
            input.len() + result.as_ref().map_or(0, |r| r.len()) + 20
        }
        Message::System(s) => s.len(),
        Message::Thinking => 10,
    }
}