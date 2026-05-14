use crate::sdk::protocol::StreamEvent;

/// Incremental SSE parser. Accumulates bytes from a streaming HTTP response and
/// emits `StreamEvent`s for each complete `event/data` frame (frames are
/// separated by a blank line per the SSE spec).
///
/// Malformed JSON in a `data:` field is silently skipped — the upstream may
/// send proprietary frame types we don't model yet, and we don't want one
/// unknown frame to kill the stream.
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self { buffer: String::new() }
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<StreamEvent> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut out = Vec::new();

        // SSE frames end at "\n\n". Split, parse complete frames, keep the
        // trailing partial frame in the buffer.
        loop {
            let Some(end) = self.buffer.find("\n\n") else { break };
            let frame = self.buffer[..end].to_string();
            self.buffer.drain(..end + 2);

            if let Some(event) = parse_frame(&frame) {
                out.push(event);
            }
        }

        out
    }
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_frame(frame: &str) -> Option<StreamEvent> {
    let mut data_lines: Vec<&str> = Vec::new();
    for line in frame.lines() {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let Some((field, value)) = line.split_once(':') else { continue };
        // Per SSE, a single leading space after the colon is stripped.
        let value = value.strip_prefix(' ').unwrap_or(value);
        if field == "data" {
            data_lines.push(value);
        }
        // We deliberately ignore `event:` — the type discriminator lives
        // inside the JSON payload (`"type": "..."`), which is what serde uses.
    }
    if data_lines.is_empty() {
        return None;
    }
    let joined = data_lines.join("\n");
    serde_json::from_str::<StreamEvent>(&joined).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::protocol::BlockDelta;

    /// One SSE frame split across two byte chunks must still produce one event.
    #[test]
    fn parses_event_split_across_chunks() {
        let mut parser = SseParser::new();
        let chunk_a = b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\"";
        let chunk_b = b",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n";

        let events_a = parser.feed(chunk_a);
        assert!(events_a.is_empty(), "no complete frames in first chunk yet");

        let events_b = parser.feed(chunk_b);
        assert_eq!(events_b.len(), 1);
        match &events_b[0] {
            StreamEvent::ContentBlockDelta { index, delta: BlockDelta::TextDelta { text } } => {
                assert_eq!(*index, 0);
                assert_eq!(text, "hi");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parses_multiple_events_in_one_chunk() {
        let mut parser = SseParser::new();
        let chunk = b"event: ping\ndata: {\"type\":\"ping\"}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let events = parser.feed(chunk);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], StreamEvent::Ping));
        assert!(matches!(events[1], StreamEvent::MessageStop));
    }

    #[test]
    fn ignores_comments_and_unknown_fields() {
        let mut parser = SseParser::new();
        let chunk = b": this is a comment\nretry: 1000\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let events = parser.feed(chunk);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], StreamEvent::MessageStop));
    }

    #[test]
    fn data_only_frames_use_data_field_only() {
        // Anthropic typically sends an `event:` field, but the SSE spec allows
        // data-only frames. Our parser should still try to deserialize them.
        let mut parser = SseParser::new();
        let chunk = b"data: {\"type\":\"message_stop\"}\n\n";
        let events = parser.feed(chunk);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], StreamEvent::MessageStop));
    }

    #[test]
    fn malformed_json_is_skipped_not_panicked() {
        let mut parser = SseParser::new();
        let chunk = b"event: garbage\ndata: not-json\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let events = parser.feed(chunk);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], StreamEvent::MessageStop));
    }
}
