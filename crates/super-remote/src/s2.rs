//! A small S2 client for the subset this crate needs: append a batch, read
//! the tail position, and tail a stream over server-sent events with resume.
//!
//! The client talks to the platform's `/s2/v1` proxy, which adds the user's
//! basin, so no basin header is sent. Every request carries the platform
//! bearer token.

use crate::RemoteCredentials;
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// One record to append. Headers are `(name, value)` pairs; a command record
/// has an empty name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppendRecord {
    pub body: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, String)>,
}

impl AppendRecord {
    pub fn new(body: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            headers: Vec::new(),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The S2 `fence` command record that sets a stream's fencing token.
    pub fn fence(token: &str) -> Self {
        Self::new(token).header("", "fence")
    }

    /// Bytes as S2 meters them: body plus headers plus a fixed overhead.
    pub fn metered_bytes(&self) -> usize {
        8 + self.body.len()
            + self
                .headers
                .iter()
                .map(|(name, value)| 2 + name.len() + value.len())
                .sum::<usize>()
    }
}

#[derive(Debug, Clone, Default)]
pub struct AppendOptions {
    pub fencing_token: Option<String>,
    pub match_seq_num: Option<u64>,
    /// Applied when the stream is created on first append.
    pub stream_config: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct StreamPosition {
    pub seq_num: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct AppendAck {
    pub start: StreamPosition,
    pub end: StreamPosition,
    pub tail: StreamPosition,
}

#[derive(Debug)]
pub enum S2Error {
    /// The platform rejected the token.
    Unauthorized,
    /// The platform's rules forbid this operation for this device.
    Forbidden,
    /// Fencing token or `match_seq_num` mismatch.
    PreconditionFailed(String),
    /// Any other HTTP status.
    Status(u16, String),
    /// Network or protocol failure; worth retrying.
    Transport(String),
}

impl std::fmt::Display for S2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => write!(f, "unauthorized"),
            Self::Forbidden => write!(f, "forbidden"),
            Self::PreconditionFailed(body) => write!(f, "precondition failed: {body}"),
            Self::Status(status, body) => write!(f, "http {status}: {body}"),
            Self::Transport(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for S2Error {}

impl S2Error {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport(_) | Self::Status(500..=599, _) | Self::Status(429, _)
        )
    }
}

/// A record as read back from a stream.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SequencedRecord {
    pub seq_num: u64,
    pub timestamp: u64,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: String,
}

impl SequencedRecord {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Where a read starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStart {
    SeqNum(u64),
    /// This many records before the tail, saturating at the start.
    TailOffset(u64),
}

/// What a tailing reader delivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadEvent {
    Connected,
    Records(Vec<SequencedRecord>),
    /// A heartbeat; `tail` is the next sequence number.
    Ping {
        tail: Option<u64>,
    },
    /// The connection dropped; the reader reconnects by itself.
    Disconnected(String),
    /// The server refused the read for good; the reader stops.
    Failed(String),
}

pub struct S2Client {
    http: reqwest::Client,
    base_url: String,
    credentials: Arc<dyn RemoteCredentials>,
}

impl S2Client {
    pub fn new(
        http: reqwest::Client,
        base_url: String,
        credentials: Arc<dyn RemoteCredentials>,
    ) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            credentials,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn records_url(&self, stream: &str) -> String {
        format!(
            "{}/streams/{}/records",
            self.base_url,
            encode_stream(stream)
        )
    }

    async fn token(&self) -> Result<String, S2Error> {
        self.credentials
            .access_token()
            .await
            .map_err(|error| S2Error::Transport(format!("access token: {error:#}")))
    }

    /// Append one batch atomically.
    pub async fn append(
        &self,
        stream: &str,
        records: &[AppendRecord],
        options: &AppendOptions,
    ) -> Result<AppendAck, S2Error> {
        let token = self.token().await?;
        let mut body = json!({ "records": records });
        if let Some(fence) = &options.fencing_token {
            body["fencing_token"] = Value::String(fence.clone());
        }
        if let Some(seq) = options.match_seq_num {
            body["match_seq_num"] = json!(seq);
        }
        let mut request = self
            .http
            .post(self.records_url(stream))
            .bearer_auth(token)
            .json(&body);
        if let Some(config) = &options.stream_config {
            request = request.header("s2-stream-config", config.to_string());
        }
        let response = request
            .send()
            .await
            .map_err(|error| S2Error::Transport(error.to_string()))?;
        let status = response.status();
        if status.is_success() {
            return response
                .json::<AppendAck>()
                .await
                .map_err(|error| S2Error::Transport(format!("decode ack: {error}")));
        }
        let text = response.text().await.unwrap_or_default();
        Err(match status.as_u16() {
            401 => S2Error::Unauthorized,
            403 => S2Error::Forbidden,
            412 => S2Error::PreconditionFailed(text),
            code => S2Error::Status(code, text),
        })
    }

    /// The next sequence number the stream will assign.
    pub async fn tail(&self, stream: &str) -> Result<u64, S2Error> {
        let token = self.token().await?;
        let response = self
            .http
            .get(format!("{}/tail", self.records_url(stream)))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| S2Error::Transport(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(match status.as_u16() {
                401 => S2Error::Unauthorized,
                403 => S2Error::Forbidden,
                404 => S2Error::Status(404, text),
                code => S2Error::Status(code, text),
            });
        }
        let value: Value = response
            .json()
            .await
            .map_err(|error| S2Error::Transport(format!("decode tail: {error}")))?;
        value["tail"]["seq_num"]
            .as_u64()
            .ok_or_else(|| S2Error::Transport("tail without seq_num".into()))
    }

    /// The newest record of a stream, or `None` when it is empty or missing.
    pub async fn read_last(&self, stream: &str) -> Result<Option<SequencedRecord>, S2Error> {
        let token = self.token().await?;
        let response = self
            .http
            .get(self.records_url(stream))
            .bearer_auth(token)
            .header("accept", "application/json")
            .query(&[("tail_offset", "1"), ("count", "1"), ("clamp", "true")])
            .send()
            .await
            .map_err(|error| S2Error::Transport(error.to_string()))?;
        let status = response.status();
        if status.as_u16() == 404 || status.as_u16() == 416 {
            return Ok(None);
        }
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(match status.as_u16() {
                401 => S2Error::Unauthorized,
                403 => S2Error::Forbidden,
                code => S2Error::Status(code, text),
            });
        }
        let value: Value = response
            .json()
            .await
            .map_err(|error| S2Error::Transport(format!("decode read: {error}")))?;
        let records: Vec<SequencedRecord> =
            serde_json::from_value(value["records"].clone()).unwrap_or_default();
        Ok(records.into_iter().last())
    }

    /// Tail a stream forever. Reconnects with backoff and resumes after the
    /// last delivered record. Stops when `cancel` fires or the server refuses
    /// the read with a non-retryable status.
    pub fn tail_stream(
        self: &Arc<Self>,
        stream: String,
        start: ReadStart,
        cancel: CancellationToken,
    ) -> mpsc::UnboundedReceiver<ReadEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let client = self.clone();
        tokio::spawn(async move {
            let mut position = start;
            let mut backoff = Duration::from_secs(1);
            loop {
                if cancel.is_cancelled() {
                    break;
                }
                let outcome = client.read_once(&stream, position, &tx, &cancel).await;
                match outcome {
                    ReadOutcome::Cancelled => break,
                    ReadOutcome::Resume(next, reason) => {
                        backoff = Duration::from_secs(1);
                        if let Some(next) = next {
                            position = ReadStart::SeqNum(next);
                        }
                        if tx.send(ReadEvent::Disconnected(reason)).is_err() {
                            break;
                        }
                    }
                    ReadOutcome::Retry(next, reason) => {
                        if let Some(next) = next {
                            position = ReadStart::SeqNum(next);
                        }
                        if tx.send(ReadEvent::Disconnected(reason)).is_err() {
                            break;
                        }
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = tokio::time::sleep(backoff) => {}
                        }
                        backoff = (backoff * 2).min(Duration::from_secs(30));
                    }
                    ReadOutcome::Fatal(reason) => {
                        let _ = tx.send(ReadEvent::Failed(reason));
                        break;
                    }
                }
            }
        });
        rx
    }

    async fn read_once(
        &self,
        stream: &str,
        start: ReadStart,
        tx: &mpsc::UnboundedSender<ReadEvent>,
        cancel: &CancellationToken,
    ) -> ReadOutcome {
        let token = match self.token().await {
            Ok(token) => token,
            Err(error) => return ReadOutcome::Retry(None, error.to_string()),
        };
        let mut request = self
            .http
            .get(self.records_url(stream))
            .bearer_auth(token)
            .header("accept", "text/event-stream")
            .query(&[("clamp", "true")]);
        request = match start {
            ReadStart::SeqNum(seq) => request.query(&[("seq_num", seq.to_string())]),
            ReadStart::TailOffset(offset) => request.query(&[("tail_offset", offset.to_string())]),
        };
        let response = tokio::select! {
            _ = cancel.cancelled() => return ReadOutcome::Cancelled,
            response = request.send() => response,
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => return ReadOutcome::Retry(None, error.to_string()),
        };
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return match status.as_u16() {
                401 | 403 | 400 => ReadOutcome::Fatal(format!("http {status}: {text}")),
                _ => ReadOutcome::Retry(None, format!("http {status}: {text}")),
            };
        }
        if tx.send(ReadEvent::Connected).is_err() {
            return ReadOutcome::Cancelled;
        }
        let mut parser = SseParser::default();
        let mut next_seq: Option<u64> = None;
        let mut body = response.bytes_stream();
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => return ReadOutcome::Cancelled,
                chunk = body.next() => chunk,
                // The server heartbeats at least every 15 s; a silent minute
                // means the connection is dead.
                _ = tokio::time::sleep(Duration::from_secs(60)) => {
                    return ReadOutcome::Retry(next_seq, "read timed out".into());
                }
            };
            let chunk = match chunk {
                Some(Ok(chunk)) => chunk,
                Some(Err(error)) => return ReadOutcome::Retry(next_seq, error.to_string()),
                None => return ReadOutcome::Resume(next_seq, "stream ended".into()),
            };
            for event in parser.feed(&chunk) {
                match event.name.as_deref() {
                    Some("batch") => {
                        let batch: Value = match serde_json::from_str(&event.data) {
                            Ok(value) => value,
                            Err(error) => {
                                return ReadOutcome::Retry(next_seq, format!("bad batch: {error}"));
                            }
                        };
                        let records: Vec<SequencedRecord> =
                            serde_json::from_value(batch["records"].clone()).unwrap_or_default();
                        if let Some(last) = records.last() {
                            next_seq = Some(last.seq_num + 1);
                        }
                        if !records.is_empty() && tx.send(ReadEvent::Records(records)).is_err() {
                            return ReadOutcome::Cancelled;
                        }
                    }
                    Some("ping") => {
                        let tail = serde_json::from_str::<Value>(&event.data)
                            .ok()
                            .and_then(|value| value["tail"]["seq_num"].as_u64());
                        if tx.send(ReadEvent::Ping { tail }).is_err() {
                            return ReadOutcome::Cancelled;
                        }
                    }
                    Some("error") => {
                        return ReadOutcome::Retry(
                            next_seq,
                            format!("server error: {}", event.data),
                        );
                    }
                    _ => {}
                }
            }
        }
    }
}

enum ReadOutcome {
    Cancelled,
    /// Reconnect immediately from the given position.
    Resume(Option<u64>, String),
    /// Reconnect after a backoff.
    Retry(Option<u64>, String),
    Fatal(String),
}

fn encode_stream(stream: &str) -> String {
    // A stream name is one path segment to S2: `sessions/h/s/events` must
    // travel as `sessions%2Fh%2Fs%2Fevents`, or s2 lite answers 404. Only the
    // unreserved characters stay as they are.
    let mut out = String::with_capacity(stream.len());
    for byte in stream.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// A server-sent-events parser that keeps the `id` field, which S2 uses as
/// the resume token.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    name: Option<String>,
    id: Option<String>,
    data: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub name: Option<String>,
    pub id: Option<String>,
    pub data: String,
}

impl SseParser {
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() || self.name.is_some() {
                    out.push(SseEvent {
                        name: self.name.take(),
                        id: self.id.clone(),
                        data: self.data.join("\n"),
                    });
                    self.data.clear();
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line, ""),
            };
            match field {
                "event" => self.name = Some(value.to_string()),
                "data" => self.data.push(value.to_string()),
                "id" => self.id = Some(value.to_string()),
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_events_keep_their_ids_and_join_multi_line_data() {
        let mut parser = SseParser::default();
        let mut events = parser.feed(b"event: batch\nid: 7,1,2\ndata: {\"a\":\n");
        assert!(events.is_empty());
        events.extend(parser.feed(b"data: 1}\n\nevent: ping\ndata: {}\n\n"));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].name.as_deref(), Some("batch"));
        assert_eq!(events[0].id.as_deref(), Some("7,1,2"));
        assert_eq!(events[0].data, "{\"a\":\n1}");
        assert_eq!(events[1].name.as_deref(), Some("ping"));
    }

    #[test]
    fn stream_names_are_one_percent_encoded_segment() {
        assert_eq!(
            encode_stream("sessions/h/s/events"),
            "sessions%2Fh%2Fs%2Fevents"
        );
        assert_eq!(encode_stream("a b"), "a%20b");
        assert_eq!(encode_stream("registry"), "registry");
    }

    #[test]
    fn a_fence_record_is_a_command_record() {
        let record = AppendRecord::fence("tok");
        assert_eq!(record.headers, vec![(String::new(), "fence".to_string())]);
        assert_eq!(record.body, "tok");
        let json = serde_json::to_value(&record).expect("json");
        assert_eq!(json["headers"][0][1], "fence");
    }

    #[test]
    fn retryable_errors_are_the_transient_ones() {
        assert!(S2Error::Transport("x".into()).is_retryable());
        assert!(S2Error::Status(503, String::new()).is_retryable());
        assert!(!S2Error::Unauthorized.is_retryable());
        assert!(!S2Error::PreconditionFailed(String::new()).is_retryable());
    }
}
