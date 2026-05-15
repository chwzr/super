use crate::sdk::protocol::StreamEvent;

/// Incremental SSE parser. Accumulates bytes from a streaming HTTP response and
/// emits `StreamEvent`s for each complete `event/data` frame (frames are
/// separated by a blank line per the SSE spec).
///
/// The buffer is byte-oriented: TCP chunk boundaries can split a multi-byte
/// UTF-8 codepoint, so we cannot lossily decode each chunk into a `String`
/// without corrupting text. We split on the byte sequence `\n\n` (and
/// normalize CRLF beforehand), then `String::from_utf8_lossy` on each *whole
/// frame* — at which point any codepoint is fully present.
///
/// Malformed JSON in a `data:` field is silently skipped — the upstream may
/// send proprietary frame types we don't model yet, and we don't want one
/// unknown frame to kill the stream.
pub struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<StreamEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut out = Vec::new();

        // Find frame boundaries. Per SSE, a frame ends at "\n\n", "\r\n\r\n",
        // or "\r\r". We scan for any of those.
        loop {
            let Some((end, sep_len)) = find_frame_boundary(&self.buffer) else { break };
            let frame_bytes: Vec<u8> = self.buffer.drain(..end + sep_len).collect();
            let frame_bytes = &frame_bytes[..end]; // strip the trailing separator
            let frame = String::from_utf8_lossy(frame_bytes);
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

/// Find the first frame boundary in `buf`. Returns `(start_of_separator, separator_len)`.
fn find_frame_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    // We accept LF-LF, CRLF-CRLF, and CR-CR as frame terminators per the SSE spec.
    // Search for whichever appears first.
    let candidates: [(&[u8], usize); 3] = [
        (b"\r\n\r\n", 4),
        (b"\n\n", 2),
        (b"\r\r", 2),
    ];
    let mut best: Option<(usize, usize)> = None;
    for (pat, sep_len) in candidates {
        if let Some(idx) = find_subslice(buf, pat) {
            best = Some(match best {
                Some((b_idx, _)) if b_idx <= idx => best.unwrap(),
                _ => (idx, sep_len),
            });
        }
    }
    best
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

fn parse_frame(frame: &str) -> Option<StreamEvent> {
    let mut data_lines: Vec<&str> = Vec::new();
    // Iterate physical lines, normalizing on \n. We also accept lone \r as a line
    // separator inside a frame (unlikely but spec-compliant). Most upstreams emit
    // LF only; the boundary detector already handled CR/CRLF terminators.
    for line in frame.split('\n') {
        let line = line.trim_end_matches('\r');
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

    /// A chunk boundary that bisects a multi-byte UTF-8 codepoint must NOT
    /// corrupt the text. The waving-hand emoji 👋 (U+1F44B) is 4 bytes in UTF-8:
    /// F0 9F 91 8B. We feed the first 2 bytes in chunk A and the rest in chunk B.
    #[test]
    fn parses_event_split_inside_multibyte_codepoint() {
        let mut parser = SseParser::new();
        let full = b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"\xF0\x9F\x91\x8B!\"}}\n\n";
        // Find the position of the emoji's first byte (0xF0) in the buffer.
        let emoji_start = full.iter().position(|&b| b == 0xF0).unwrap();
        let split_at = emoji_start + 2; // splits the 4-byte sequence in half
        let chunk_a = &full[..split_at];
        let chunk_b = &full[split_at..];

        let events_a = parser.feed(chunk_a);
        assert!(events_a.is_empty());

        let events_b = parser.feed(chunk_b);
        assert_eq!(events_b.len(), 1);
        match &events_b[0] {
            StreamEvent::ContentBlockDelta { delta: BlockDelta::TextDelta { text }, .. } => {
                assert_eq!(text, "👋!", "multi-byte codepoint must survive the split");
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

    /// CRLF frame terminators per SSE spec.
    #[test]
    fn parses_event_with_crlf_terminators() {
        let mut parser = SseParser::new();
        let chunk = b"event: message_stop\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n";
        let events = parser.feed(chunk);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], StreamEvent::MessageStop));
    }
}
