//! One session's mirror: RPC events out to the `events` stream, durable
//! entries out to the `entries` stream, RPC commands in from the `commands`
//! stream.
//!
//! The transport never blocks the session. Events are queued, batched with a
//! short linger, and appended in order. While the platform is unreachable the
//! queue is bounded: streaming deltas are dropped first, finished messages
//! last, and an `event_lag` record tells readers what they missed.

use crate::commands::{CommandCursor, is_expired, start_position};
use crate::s2::{AppendOptions, AppendRecord, ReadEvent, S2Client, S2Error};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use super_sdk::protocol::{Command, decode_request};
use super_sdk::{Response, Session};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

/// S2 caps a batch at 1 MiB of metered bytes; stay well below it.
pub const MAX_RECORD_BYTES: usize = 900 * 1024;
const MAX_BATCH_BYTES: usize = 960 * 1024;
const MAX_BATCH_RECORDS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStreams {
    pub events: String,
    pub entries: String,
    pub commands: String,
}

impl SessionStreams {
    pub fn new(host_id: &str, session_id: &str) -> Self {
        let base = format!("sessions/{host_id}/{session_id}");
        Self {
            events: format!("{base}/events"),
            entries: format!("{base}/entries"),
            commands: format!("{base}/commands"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteStatus {
    Starting,
    Connected,
    Reconnecting,
    /// The token was rejected; nothing will work until a new sign-in.
    Unauthorized,
    Stopped,
}

impl RemoteStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Connected => "connected",
            Self::Reconnecting => "reconnecting",
            Self::Unauthorized => "unauthorized",
            Self::Stopped => "stopped",
        }
    }
}

/// Things worth a line in the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteNotice {
    Status(RemoteStatus),
    /// Commands dropped because their `exp` header had passed.
    ExpiredCommands(usize),
    /// Events dropped from the offline buffer.
    DroppedEvents(usize),
    /// A remote command was refused because of a local setting.
    Refused {
        origin: String,
        command: String,
    },
}

pub type NoticeSink = Arc<dyn Fn(RemoteNotice) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct TransportOptions {
    /// Accept the RPC `bash` command from controllers.
    pub allow_shell: bool,
    pub max_buffer_records: usize,
    pub max_buffer_bytes: usize,
    pub linger: Duration,
    /// Applied when a command carries no `exp` header, counted from the
    /// record's timestamp.
    pub default_command_expiry: Duration,
    /// How many recent commands to consider when starting a session that has
    /// no command cursor yet.
    pub command_lookback: u64,
    /// Where the last applied command sequence number is kept per stream, so
    /// a resumed session does not execute commands again. `None` means
    /// `~/.super/agent/remote/cursors`.
    pub cursor_dir: Option<std::path::PathBuf>,
}

impl Default for TransportOptions {
    fn default() -> Self {
        Self {
            allow_shell: true,
            max_buffer_records: 10_000,
            max_buffer_bytes: 16 * 1024 * 1024,
            linger: Duration::from_millis(30),
            default_command_expiry: Duration::from_secs(5 * 60),
            command_lookback: 50,
            cursor_dir: None,
        }
    }
}

enum Outgoing {
    Event(AppendRecord),
    SyncEntries,
}

pub struct SessionTransport {
    status: watch::Receiver<RemoteStatus>,
    cancel: CancellationToken,
    flush_done: Option<tokio::task::JoinHandle<()>>,
}

impl SessionTransport {
    pub fn start(
        client: Arc<S2Client>,
        session: Arc<Session>,
        streams: SessionStreams,
        options: TransportOptions,
        notices: NoticeSink,
    ) -> Self {
        let cancel = CancellationToken::new();
        let (status_tx, status_rx) = watch::channel(RemoteStatus::Starting);
        let (queue_tx, queue_rx) = mpsc::unbounded_channel::<Outgoing>();

        // Events out.
        let pump = Pump {
            session: session.clone(),
            queue: queue_tx.clone(),
            cancel: cancel.clone(),
        };
        tokio::spawn(pump.run());

        // Commands in.
        let tailer = Tailer {
            client: client.clone(),
            session: session.clone(),
            stream: streams.commands.clone(),
            queue: queue_tx,
            options: options.clone(),
            notices: notices.clone(),
            cancel: cancel.clone(),
            link: Arc::new(LinkState::default()),
            status: status_tx.clone(),
        };
        let link = tailer.link.clone();
        tokio::spawn(tailer.run());

        // The appender owns the buffer and the status.
        let flusher = Flusher {
            client,
            session,
            streams,
            options,
            notices,
            queue: queue_rx,
            cancel: cancel.clone(),
            status: status_tx,
            link,
            buffer: VecDeque::new(),
            buffered_bytes: 0,
            dropped: 0,
            last_entry_id: None,
            entries_primed: false,
        };
        let flush_done = tokio::spawn(flusher.run());

        Self {
            status: status_rx,
            cancel,
            flush_done: Some(flush_done),
        }
    }

    pub fn status(&self) -> RemoteStatus {
        *self.status.borrow()
    }

    pub fn status_watch(&self) -> watch::Receiver<RemoteStatus> {
        self.status.clone()
    }

    /// Stop tailing, flush what is queued (for a bounded time), and end.
    pub async fn stop(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.flush_done.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
    }
}

impl Drop for SessionTransport {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Shared between the appender and the tailer so the status reflects both.
#[derive(Default)]
struct LinkState {
    append_ok: std::sync::atomic::AtomicBool,
    tail_ok: std::sync::atomic::AtomicBool,
    unauthorized: std::sync::atomic::AtomicBool,
}

impl LinkState {
    fn status(&self) -> RemoteStatus {
        use std::sync::atomic::Ordering::Relaxed;
        if self.unauthorized.load(Relaxed) {
            RemoteStatus::Unauthorized
        } else if self.append_ok.load(Relaxed) && self.tail_ok.load(Relaxed) {
            RemoteStatus::Connected
        } else {
            RemoteStatus::Reconnecting
        }
    }
}

fn publish_status(status: &watch::Sender<RemoteStatus>, link: &LinkState, notices: &NoticeSink) {
    let next = link.status();
    let changed = status.send_if_modified(|current| {
        if *current == next {
            false
        } else {
            *current = next;
            true
        }
    });
    if changed {
        crate::log::line(format!("status {}", next.label()));
        notices(RemoteNotice::Status(next));
    }
}

// ---------------------------------------------------------------------------
// Events out
// ---------------------------------------------------------------------------

struct Pump {
    session: Arc<Session>,
    queue: mpsc::UnboundedSender<Outgoing>,
    cancel: CancellationToken,
}

impl Pump {
    async fn run(self) {
        let mut events = self.session.events();
        loop {
            let event = tokio::select! {
                _ = self.cancel.cancelled() => break,
                event = events.recv() => event,
            };
            let Some(event) = event else { break };
            let value = event.0;
            let kind = value["type"].as_str().unwrap_or("").to_string();
            if self
                .queue
                .send(Outgoing::Event(event_record(value)))
                .is_err()
            {
                break;
            }
            // A turn starts right after the user's message was recorded, and
            // controllers only learn about that message from the entries
            // stream, so sync then as well as after every message. After a
            // lag the message_end that would have triggered a sync may be
            // gone, so a lag syncs too.
            if matches!(
                kind.as_str(),
                "agent_start"
                    | "turn_start"
                    | "message_end"
                    | "turn_end"
                    | "agent_settled"
                    | "compaction_end"
                    | "model_changed"
                    | "event_lag"
            ) && self.queue.send(Outgoing::SyncEntries).is_err()
            {
                break;
            }
        }
    }
}

/// Encode one RPC event or response as a record, truncated to fit.
pub fn event_record(mut value: Value) -> AppendRecord {
    let kind = value["type"].as_str().unwrap_or("").to_string();
    let (body, truncated) = fit_to_bytes(&mut value, MAX_RECORD_BYTES);
    let mut record = AppendRecord::new(body).header("t", kind);
    if truncated {
        record = record.header("trunc", "1");
    }
    record
}

/// Replace the largest strings in `value` until it serializes under `limit`.
/// Returns whether anything was cut. The note left behind names the size so a
/// reader can tell what happened.
pub fn shrink_to_fit(value: &mut Value, limit: usize) -> bool {
    fit_to_bytes(value, limit).1
}

/// Like [`shrink_to_fit`], returning the serialized body so callers do not
/// encode twice.
pub fn fit_to_bytes(value: &mut Value, limit: usize) -> (String, bool) {
    let mut truncated = false;
    let mut body = value.to_string();
    for _ in 0..64 {
        if body.len() <= limit {
            break;
        }
        let Some(path) = largest_string(value) else {
            break;
        };
        let mut slot: &mut Value = &mut *value;
        for step in &path {
            slot = match step {
                PathStep::Key(key) => &mut slot[key.as_str()],
                PathStep::Index(index) => &mut slot[*index],
            };
        }
        if let Value::String(text) = slot {
            let note = format!("[truncated: {} bytes]", text.len());
            *slot = Value::String(note);
            truncated = true;
        } else {
            break;
        }
        body = value.to_string();
    }
    if truncated {
        if let Value::Object(map) = value {
            map.insert("truncated".into(), Value::Bool(true));
        }
        body = value.to_string();
    }
    (body, truncated)
}

#[derive(Clone)]
enum PathStep {
    Key(String),
    Index(usize),
}

fn largest_string(value: &Value) -> Option<Vec<PathStep>> {
    fn walk(value: &Value, path: &mut Vec<PathStep>, best: &mut (usize, Vec<PathStep>)) {
        match value {
            Value::String(text) if text.len() > best.0 => {
                *best = (text.len(), path.clone());
            }
            Value::String(_) => {}
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    path.push(PathStep::Index(index));
                    walk(item, path, best);
                    path.pop();
                }
            }
            Value::Object(map) => {
                for (key, item) in map {
                    path.push(PathStep::Key(key.clone()));
                    walk(item, path, best);
                    path.pop();
                }
            }
            _ => {}
        }
    }
    let mut best = (0, Vec::new());
    walk(value, &mut Vec::new(), &mut best);
    // Strings under 64 bytes are never worth cutting.
    (best.0 >= 64).then_some(best.1)
}

// ---------------------------------------------------------------------------
// Appending
// ---------------------------------------------------------------------------

struct Flusher {
    client: Arc<S2Client>,
    session: Arc<Session>,
    streams: SessionStreams,
    options: TransportOptions,
    notices: NoticeSink,
    queue: mpsc::UnboundedReceiver<Outgoing>,
    cancel: CancellationToken,
    status: watch::Sender<RemoteStatus>,
    link: Arc<LinkState>,
    buffer: VecDeque<AppendRecord>,
    buffered_bytes: usize,
    dropped: usize,
    last_entry_id: Option<String>,
    entries_primed: bool,
}

impl Flusher {
    async fn run(mut self) {
        let mut backoff = Duration::from_secs(1);
        let mut sync_entries = true;
        loop {
            // Collect what is queued, waiting briefly so deltas batch up.
            let first = tokio::select! {
                _ = self.cancel.cancelled() => None,
                item = self.queue.recv() => item,
            };
            let mut cancelled = false;
            match first {
                Some(item) => {
                    if self.enqueue(item) {
                        sync_entries = true;
                    }
                    let deadline = tokio::time::Instant::now() + self.options.linger;
                    loop {
                        let item = tokio::select! {
                            _ = tokio::time::sleep_until(deadline) => break,
                            item = self.queue.recv() => item,
                        };
                        match item {
                            Some(item) => {
                                if self.enqueue(item) {
                                    sync_entries = true;
                                }
                            }
                            None => {
                                cancelled = true;
                                break;
                            }
                        }
                    }
                }
                None => cancelled = true,
            }
            if cancelled {
                // Drain whatever is left without waiting for more.
                while let Ok(item) = self.queue.try_recv() {
                    if self.enqueue(item) {
                        sync_entries = true;
                    }
                }
            }

            // Push as much as the connection takes.
            let mut failed = false;
            while !self.buffer.is_empty() {
                match self.flush_batch().await {
                    Ok(()) => backoff = Duration::from_secs(1),
                    Err(error) => {
                        failed = true;
                        self.note_failure(&error);
                        break;
                    }
                }
            }
            if !failed && sync_entries {
                match self.sync_entries().await {
                    Ok(()) => sync_entries = false,
                    Err(error) => {
                        failed = true;
                        self.note_failure(&error);
                    }
                }
            }
            if cancelled {
                break;
            }
            if failed {
                tokio::select! {
                    _ = self.cancel.cancelled() => {}
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
        self.status.send_replace(RemoteStatus::Stopped);
    }

    fn note_failure(&mut self, error: &S2Error) {
        use std::sync::atomic::Ordering::Relaxed;
        crate::log::line(format!("append failed: {error}"));
        self.link.append_ok.store(false, Relaxed);
        if matches!(error, S2Error::Unauthorized) {
            self.link.unauthorized.store(true, Relaxed);
        }
        publish_status(&self.status, &self.link, &self.notices);
    }

    fn note_success(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        self.link.append_ok.store(true, Relaxed);
        self.link.unauthorized.store(false, Relaxed);
        publish_status(&self.status, &self.link, &self.notices);
    }

    /// Returns whether entries should be synced.
    fn enqueue(&mut self, item: Outgoing) -> bool {
        match item {
            Outgoing::SyncEntries => true,
            Outgoing::Event(record) => {
                self.buffered_bytes += record.metered_bytes();
                self.buffer.push_back(record);
                self.trim_buffer();
                false
            }
        }
    }

    fn trim_buffer(&mut self) {
        let over = |me: &Self| {
            me.buffer.len() > me.options.max_buffer_records
                || me.buffered_bytes > me.options.max_buffer_bytes
        };
        if !over(self) {
            return;
        }
        // Deltas first: a reader rebuilds from message_end anyway.
        let mut dropped = 0;
        let mut kept = VecDeque::with_capacity(self.buffer.len());
        let mut bytes = self.buffered_bytes;
        while let Some(record) = self.buffer.pop_front() {
            let is_delta = matches!(
                record.headers.first().map(|(_, v)| v.as_str()),
                Some("message_update" | "tool_execution_update" | "bash_execution_update")
            );
            if is_delta
                && (kept.len() + self.buffer.len() > self.options.max_buffer_records
                    || bytes > self.options.max_buffer_bytes)
            {
                bytes -= record.metered_bytes();
                dropped += 1;
            } else {
                kept.push_back(record);
            }
        }
        self.buffer = kept;
        self.buffered_bytes = bytes;
        while over(self) {
            if let Some(record) = self.buffer.pop_front() {
                self.buffered_bytes -= record.metered_bytes();
                dropped += 1;
            } else {
                break;
            }
        }
        if dropped > 0 {
            self.dropped += dropped;
            (self.notices)(RemoteNotice::DroppedEvents(dropped));
        }
    }

    async fn flush_batch(&mut self) -> Result<(), S2Error> {
        let mut batch = Vec::new();
        let mut bytes = 0;
        if self.dropped > 0 {
            let lag = event_record(json!({"type": "event_lag", "skipped": self.dropped}));
            bytes += lag.metered_bytes();
            batch.push(lag);
        }
        while let Some(record) = self.buffer.front() {
            let size = record.metered_bytes();
            if !batch.is_empty()
                && (bytes + size > MAX_BATCH_BYTES || batch.len() >= MAX_BATCH_RECORDS)
            {
                break;
            }
            bytes += size;
            if let Some(record) = self.buffer.pop_front() {
                self.buffered_bytes -= record.metered_bytes();
                batch.push(record);
            }
        }
        let had_lag = self.dropped > 0;
        let count = batch.len();
        match self
            .client
            .append(&self.streams.events, &batch, &AppendOptions::default())
            .await
        {
            Ok(ack) => {
                crate::log::line(format!(
                    "events append {count} records, seq {}..{}",
                    ack.start.seq_num, ack.end.seq_num
                ));
                if had_lag {
                    self.dropped = 0;
                }
                self.note_success();
                Ok(())
            }
            Err(error) => {
                // Put the records back, in order, without the lag marker.
                let restore: Vec<AppendRecord> = batch
                    .into_iter()
                    .filter(|record| !had_lag || record.header_is("t", "event_lag").not())
                    .collect();
                for record in restore.into_iter().rev() {
                    self.buffered_bytes += record.metered_bytes();
                    self.buffer.push_front(record);
                }
                Err(error)
            }
        }
    }

    /// Append the session entries that are not yet in the entries stream.
    async fn sync_entries(&mut self) -> Result<(), S2Error> {
        if !self.entries_primed {
            match self.client.read_last(&self.streams.entries).await {
                Ok(last) => {
                    self.last_entry_id =
                        last.and_then(|record| record.header("eid").map(str::to_string));
                    self.entries_primed = true;
                }
                Err(error) if error.is_retryable() => return Err(error),
                Err(error) => {
                    crate::log::line(format!("entries tail unavailable: {error}"));
                    self.entries_primed = true;
                }
            }
        }
        let entries = self.new_entries().await;
        if entries.is_empty() {
            return Ok(());
        }
        let config = json!({"retention_policy": {"infinite": {}}});
        let mut batch: Vec<AppendRecord> = Vec::new();
        let mut bytes = 0;
        let mut ids: Vec<String> = Vec::new();
        for mut entry in entries {
            let id = entry["id"].as_str().unwrap_or("").to_string();
            let kind = entry["type"].as_str().unwrap_or("entry").to_string();
            let (body, truncated) = fit_to_bytes(&mut entry, MAX_RECORD_BYTES);
            let mut record = AppendRecord::new(body)
                .header("t", kind)
                .header("eid", id.clone());
            if truncated {
                record = record.header("trunc", "1");
            }
            let size = record.metered_bytes();
            if !batch.is_empty()
                && (bytes + size > MAX_BATCH_BYTES || batch.len() >= MAX_BATCH_RECORDS)
            {
                self.append_entries(&batch, &config).await?;
                self.last_entry_id = ids.last().cloned();
                batch.clear();
                bytes = 0;
            }
            bytes += size;
            batch.push(record);
            ids.push(id);
        }
        if !batch.is_empty() {
            self.append_entries(&batch, &config).await?;
            self.last_entry_id = ids.last().cloned();
        }
        Ok(())
    }

    async fn append_entries(&self, batch: &[AppendRecord], config: &Value) -> Result<(), S2Error> {
        let options = AppendOptions {
            stream_config: Some(config.clone()),
            ..AppendOptions::default()
        };
        self.client
            .append(&self.streams.entries, batch, &options)
            .await
            .map(|ack| {
                crate::log::line(format!(
                    "entries append {} records, seq {}..{}",
                    batch.len(),
                    ack.start.seq_num,
                    ack.end.seq_num
                ));
            })
    }

    async fn new_entries(&mut self) -> Vec<Value> {
        let since = self.last_entry_id.clone();
        let response: Response = self
            .session
            .execute(Command::GetEntries {
                since: since.clone(),
            })
            .await;
        if !response.success && since.is_some() {
            // The cursor is gone (fork, new session): start over.
            self.last_entry_id = None;
            let response = self
                .session
                .execute(Command::GetEntries { since: None })
                .await;
            return entries_of(response);
        }
        entries_of(response)
    }
}

fn entries_of(response: Response) -> Vec<Value> {
    response
        .data
        .and_then(|data| data.get("entries").cloned())
        .and_then(|entries| match entries {
            Value::Array(items) => Some(items),
            _ => None,
        })
        .unwrap_or_default()
}

trait HeaderIs {
    fn header_is(&self, name: &str, value: &str) -> bool;
}

impl HeaderIs for AppendRecord {
    fn header_is(&self, name: &str, value: &str) -> bool {
        self.headers.iter().any(|(k, v)| k == name && v == value)
    }
}

trait Not {
    fn not(self) -> bool;
}

impl Not for bool {
    fn not(self) -> bool {
        !self
    }
}

// ---------------------------------------------------------------------------
// Commands in
// ---------------------------------------------------------------------------

struct Tailer {
    client: Arc<S2Client>,
    session: Arc<Session>,
    stream: String,
    queue: mpsc::UnboundedSender<Outgoing>,
    options: TransportOptions,
    notices: NoticeSink,
    cancel: CancellationToken,
    link: Arc<LinkState>,
    status: watch::Sender<RemoteStatus>,
}

impl Tailer {
    async fn run(self) {
        use std::sync::atomic::Ordering::Relaxed;
        let cursor = CommandCursor::new(self.options.cursor_dir.clone(), &self.stream);
        let start = start_position(
            &self.client,
            &self.stream,
            &cursor,
            self.options.command_lookback,
        )
        .await;
        crate::log::line(format!("commands tail start: {start:?}"));
        let mut reader = self
            .client
            .tail_stream(self.stream.clone(), start, self.cancel.clone());
        while let Some(event) = reader.recv().await {
            match event {
                ReadEvent::Connected => {
                    crate::log::line(format!("commands tail open: {}", self.stream));
                    self.link.tail_ok.store(true, Relaxed);
                    publish_status(&self.status, &self.link, &self.notices);
                }
                ReadEvent::Ping { .. } => {}
                ReadEvent::Disconnected(reason) => {
                    crate::log::line(format!("commands tail dropped: {reason}"));
                    self.link.tail_ok.store(false, Relaxed);
                    publish_status(&self.status, &self.link, &self.notices);
                }
                ReadEvent::Failed(reason) => {
                    crate::log::line(format!("commands tail failed: {reason}"));
                    self.link.tail_ok.store(false, Relaxed);
                    if reason.contains("401") {
                        self.link.unauthorized.store(true, Relaxed);
                    }
                    publish_status(&self.status, &self.link, &self.notices);
                    break;
                }
                ReadEvent::Records(records) => {
                    let mut expired = 0;
                    for record in records {
                        let seq_num = record.seq_num;
                        if is_expired(&record, self.options.default_command_expiry) {
                            expired += 1;
                        } else {
                            self.apply(record);
                        }
                        cursor.store(seq_num);
                    }
                    if expired > 0 {
                        (self.notices)(RemoteNotice::ExpiredCommands(expired));
                    }
                }
            }
        }
    }

    fn apply(&self, record: crate::s2::SequencedRecord) {
        let origin = record
            .header("name")
            .or_else(|| record.header("from"))
            .unwrap_or("remote")
            .to_string();
        let request = match decode_request(&record.body) {
            Ok(request) => request,
            Err(error) => {
                let response = Response::err("parse", error.to_string());
                let _ = self.queue.send(Outgoing::Event(event_record(
                    serde_json::to_value(response).unwrap_or(Value::Null),
                )));
                return;
            }
        };
        if matches!(request.command, Command::Bash { .. }) && !self.options.allow_shell {
            (self.notices)(RemoteNotice::Refused {
                origin: origin.clone(),
                command: "bash".into(),
            });
            let response =
                Response::err("bash", "shell over remote control is disabled").with_id(request.id);
            let _ = self.queue.send(Outgoing::Event(event_record(
                serde_json::to_value(response).unwrap_or(Value::Null),
            )));
            return;
        }
        let session = self.session.clone();
        let queue = self.queue.clone();
        tokio::spawn(async move {
            let response = session
                .execute_from(request.command, request.id, Some(origin))
                .await;
            let _ = queue.send(Outgoing::Event(event_record(
                serde_json::to_value(response).unwrap_or(Value::Null),
            )));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_names_follow_the_layout() {
        let streams = SessionStreams::new("01H", "a1b2");
        assert_eq!(streams.events, "sessions/01H/a1b2/events");
        assert_eq!(streams.entries, "sessions/01H/a1b2/entries");
        assert_eq!(streams.commands, "sessions/01H/a1b2/commands");
    }

    #[test]
    fn oversized_records_lose_their_largest_strings_first() {
        let big = "x".repeat(3000);
        let mut value = json!({
            "type": "tool_execution_end",
            "toolCallId": "call_1",
            "result": {"content": big, "details": {"small": "ok"}},
        });
        assert!(shrink_to_fit(&mut value, 500));
        assert_eq!(value["truncated"], true);
        assert_eq!(value["toolCallId"], "call_1");
        assert_eq!(value["result"]["details"]["small"], "ok");
        assert!(
            value["result"]["content"]
                .as_str()
                .unwrap()
                .starts_with("[truncated: 3000 bytes]")
        );
        assert!(value.to_string().len() <= 500);
    }

    #[test]
    fn small_records_are_left_alone() {
        let mut value = json!({"type": "agent_start"});
        assert!(!shrink_to_fit(&mut value, MAX_RECORD_BYTES));
        assert!(value.get("truncated").is_none());
        let record = event_record(value);
        assert_eq!(
            record.headers,
            vec![("t".to_string(), "agent_start".to_string())]
        );
    }
}
