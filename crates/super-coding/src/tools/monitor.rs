//! Monitor tool: watch a script's output or a WebSocket in the background
//! and hand each event to the agent through the session's queue.
//!
//! - `command` runs a script in the same shell environment as `Bash`: the
//!   same shell, profile and prefix, and the folder a `cd` left the main
//!   session in. Every line it writes to stdout is an event; what it writes
//!   to stderr goes to the task's output file only, where stdout goes too.
//! - `ws` opens a WebSocket. Every text frame is an event; a binary frame
//!   becomes a short marker line; a close from the server ends the watch.
//!
//! Events that arrive within [`GROUP_WINDOW`] of the first one are delivered
//! together, as one [`crate::notifications::monitor_event`] queued at
//! [`Priority::Next`]. A monitor that delivers more than [`MAX_EVENTS`]
//! events within [`EVENT_WINDOW`] is stopped. A monitor ends after
//! `timeout_ms`, [`DEFAULT_TIMEOUT_MS`] by default and at most
//! [`MAX_TIMEOUT_MS`]. However it ends by itself, the agent gets one notice;
//! a monitor stopped through the registry (by `TaskStop`, a client, or the
//! session ending) gets none.

use crate::notifications::{Priority, monitor_event};
use crate::session_runner::AgentSession;
use crate::tasks::{TaskHandle, TaskKind, TaskOutcome, TaskSpec};
use anyhow::Context as _;
use futures::StreamExt as _;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::Write as _;
use std::process::Stdio;
use std::sync::{Arc, Weak};
use std::time::Duration;
use super_agent::placeholder;
use super_agent::tools::command_rules;
use super_agent::tools::shell::{self, ShellProcess};
use super_agent::{AgentTool, ToolResult, ToolUpdateSink};
use tokio::io::AsyncReadExt as _;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::{HeaderValue, header};
use tokio_util::sync::CancellationToken;

/// How long a monitor watches when `timeout_ms` is not set.
pub const DEFAULT_TIMEOUT_MS: u64 = 300_000;
/// The longest a monitor watches.
pub const MAX_TIMEOUT_MS: u64 = 600_000;
/// Lines that arrive within this time of the first one form one event.
pub const GROUP_WINDOW: Duration = Duration::from_millis(200);
/// A monitor that would deliver more events than this within
/// [`EVENT_WINDOW`] is stopped.
pub const MAX_EVENTS: usize = 60;
/// The window [`MAX_EVENTS`] counts in.
pub const EVENT_WINDOW: Duration = Duration::from_secs(60);
/// The most characters of lines one event carries; the rest of its lines
/// are counted in `<omitted-lines>` and are in the output file.
pub const MAX_EVENT_CHARS: usize = 10_000;
/// A line longer than this is cut into lines of this many bytes.
const MAX_LINE_BYTES: usize = 16 * 1024;
/// How long opening a WebSocket may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct MonitorTool {
    session: Weak<AgentSession>,
    limits: Limits,
}

impl MonitorTool {
    pub(crate) fn new(session: Weak<AgentSession>) -> Self {
        MonitorTool {
            session,
            limits: Limits::default(),
        }
    }

    /// A monitor with other limits, so tests need not wait for the real ones.
    #[cfg(test)]
    pub(crate) fn with_limits(session: Weak<AgentSession>, limits: Limits) -> Self {
        MonitorTool { session, limits }
    }
}

/// How a monitor groups its events and how many it may deliver.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub group_window: Duration,
    pub max_events: usize,
    pub event_window: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            group_window: GROUP_WINDOW,
            max_events: MAX_EVENTS,
            event_window: EVENT_WINDOW,
        }
    }
}

/// What a monitor watches.
enum Source {
    Command(String),
    Socket { url: String, protocols: Vec<String> },
}

#[async_trait::async_trait]
impl AgentTool for MonitorTool {
    fn name(&self) -> &str {
        "Monitor"
    }

    fn advanced_only(&self) -> bool {
        true
    }

    fn description(&self) -> String {
        placeholder::tool_description("Monitor")
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": placeholder::tool_parameter("Monitor", "description"),
                },
                "timeout_ms": {
                    "default": DEFAULT_TIMEOUT_MS,
                    "type": "number",
                    "minimum": 1000,
                    "maximum": 3_600_000,
                    "description": placeholder::tool_parameter("Monitor", "timeout_ms"),
                },
                "command": {
                    "type": "string",
                    "description": placeholder::tool_parameter("Monitor", "command"),
                },
                "ws": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": placeholder::tool_parameter("Monitor", "ws.url"),
                        },
                        "protocols": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "pattern": "^[!#$%&'*+.^_`|~0-9A-Za-z-]+$",
                            },
                            "description": placeholder::tool_parameter("Monitor", "ws.protocols"),
                        },
                    },
                    "required": ["url"],
                    "additionalProperties": false,
                    "description": placeholder::tool_parameter("Monitor", "ws"),
                },
            },
            "required": ["description", "timeout_ms"],
            "additionalProperties": false,
        })
    }

    fn prepare_arguments(&self, mut args: Value) -> Value {
        if let Some(object) = args.as_object_mut() {
            super_agent::tools::arguments::drop_nulls(object, &["timeout_ms", "command", "ws"]);
            super_agent::tools::arguments::integer(object, "timeout_ms");
            if let Some(Value::Object(ws)) = object.get_mut("ws") {
                super_agent::tools::arguments::drop_nulls(ws, &["protocols"]);
            }
        }
        args
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        args: Value,
        cancel: CancellationToken,
        _on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        let session = self
            .session
            .upgrade()
            .context("The session has ended, so nothing can be monitored.")?;
        let description = args["description"].as_str().unwrap_or_default().trim();
        if description.is_empty() {
            anyhow::bail!("description is empty; say in a few words what the monitor watches.");
        }
        let timeout_ms = match args["timeout_ms"].as_f64() {
            None => DEFAULT_TIMEOUT_MS,
            Some(timeout) if timeout.is_finite() && timeout >= 1000.0 => {
                (timeout as u64).min(MAX_TIMEOUT_MS)
            }
            Some(_) => anyhow::bail!(
                "timeout_ms must be at least 1000; leave it out for {DEFAULT_TIMEOUT_MS}."
            ),
        };
        let command = args["command"]
            .as_str()
            .map(str::trim)
            .filter(|command| !command.is_empty());
        let source = match (command, args.get("ws").filter(|ws| !ws.is_null())) {
            (Some(_), Some(_)) => {
                anyhow::bail!("Give either command or ws, not both; a monitor watches one source.")
            }
            (None, None) => {
                anyhow::bail!("Give a command to run or a ws source to watch.")
            }
            (Some(command), None) => Source::Command(command.to_string()),
            (None, Some(ws)) => Source::Socket {
                url: ws["url"].as_str().unwrap_or_default().trim().to_string(),
                protocols: ws["protocols"]
                    .as_array()
                    .map(|protocols| {
                        protocols
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
            },
        };

        let tasks = session.background_tasks();
        let id = tasks.new_id(TaskKind::Monitor);
        let output = tasks.output_file_for(&id);
        let open_output = || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&output)
                .map_err(|e| anyhow::anyhow!("Could not open {}: {e}.", output.display()))
        };
        let spec = TaskSpec::new(TaskKind::Monitor, description)
            .with_id(id.clone())
            .with_tool_use_id(tool_call_id)
            .with_output_file(&output);
        let timeout = Duration::from_millis(timeout_ms);
        match source {
            Source::Command(command) => {
                let file = open_output()?;
                let settings = session.settings();
                let shell_path = settings
                    .shell_path
                    .as_deref()
                    .filter(|path| !path.trim().is_empty());
                let directory = session
                    .tool_session()
                    .shell
                    .as_ref()
                    .and_then(|shell| shell.get())
                    .filter(|directory| directory.is_dir())
                    .unwrap_or_else(|| session.manager.lock().unwrap().cwd().to_path_buf());
                let mut process = shell::script(
                    shell_path,
                    &command,
                    settings.shell_command_prefix.as_deref(),
                    &directory,
                    None,
                );
                let errors = file
                    .try_clone()
                    .map_err(|e| anyhow::anyhow!("Could not open {}: {e}.", output.display()))?;
                let shell_name = process
                    .as_std()
                    .get_program()
                    .to_string_lossy()
                    .into_owned();
                process.stdout(Stdio::piped()).stderr(Stdio::from(errors));
                let mut process = ShellProcess::spawn(process).map_err(|e| {
                    anyhow::anyhow!("Could not start shell {shell_name}: {e}. Install Bash or set shellPath to an installed Bash, PowerShell, or cmd.exe executable.")
                })?;
                let stdout = process.child.stdout.take().expect("piped stdout");
                let handle = tasks.register(spec).map_err(|e| anyhow::anyhow!("{e}"))?;
                let events = Events::new(&session, &handle, file, description, self.limits);
                tokio::spawn(watch_command(
                    process, stdout, command, handle, events, timeout,
                ));
            }
            Source::Socket { url, protocols } => {
                let socket = tokio::select! {
                    socket = connect(&url, &protocols) => socket?,
                    _ = cancel.cancelled() => anyhow::bail!("Monitor aborted"),
                };
                let file = open_output()?;
                let handle = tasks.register(spec).map_err(|e| anyhow::anyhow!("{e}"))?;
                let events = Events::new(&session, &handle, file, description, self.limits);
                tokio::spawn(watch_socket(socket, handle, events, timeout));
            }
        }
        Ok(ToolResult {
            content: vec![super_ai::ContentBlock::text(format!(
                "The monitor runs as task {id} for up to {timeout_ms} ms. Its output is written to {}.",
                output.display()
            ))],
            details: json!({"taskId": id, "outputFile": output.display().to_string(), "timeoutMs": timeout_ms}),
            ..Default::default()
        })
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open the WebSocket at `url`, asking for `protocols`.
async fn connect(url: &str, protocols: &[String]) -> anyhow::Result<Socket> {
    let parsed = reqwest::Url::parse(url)
        .ok()
        .filter(|parsed| matches!(parsed.scheme(), "ws" | "wss"));
    if parsed.is_none() {
        anyhow::bail!("ws.url must be a ws:// or wss:// address, and '{url}' is not.");
    }
    super_ai::stream::ensure_tls_crypto_provider();
    let mut request = url
        .into_client_request()
        .map_err(|e| anyhow::anyhow!("Could not open the WebSocket {url}: {e}."))?;
    if !protocols.is_empty() {
        let value = HeaderValue::from_str(&protocols.join(", "))
            .map_err(|e| anyhow::anyhow!("ws.protocols cannot be sent: {e}."))?;
        request
            .headers_mut()
            .insert(header::SEC_WEBSOCKET_PROTOCOL, value);
    }
    match tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request)).await {
        Ok(Ok((socket, _))) => Ok(socket),
        Ok(Err(e)) => anyhow::bail!("Could not open the WebSocket {url}: {e}."),
        Err(_) => anyhow::bail!(
            "Could not open the WebSocket {url} within {} seconds; check that the server runs.",
            CONNECT_TIMEOUT.as_secs()
        ),
    }
}

/// Watch a script until it ends, reaches its time limit, produces too many
/// events or is stopped.
async fn watch_command(
    mut process: ShellProcess,
    mut stdout: tokio::process::ChildStdout,
    command: String,
    handle: TaskHandle,
    mut events: Events,
    timeout: Duration,
) {
    let stop = handle.stop_signal();
    let deadline = Instant::now() + timeout;
    let mut buffer = [0u8; 8192];
    let mut lines = Lines::default();
    let mut open = true;
    loop {
        let flush_at = events.flush_at;
        tokio::select! {
            biased;
            _ = stop.cancelled() => {
                process.kill();
                return;
            }
            _ = tokio::time::sleep_until(deadline) => {
                process.kill();
                events.push_all(lines.rest());
                events.end_at_time_limit(handle, timeout);
                return;
            }
            _ = sleep_until(flush_at), if flush_at.is_some() => {
                if events.flush(&handle).is_err() {
                    process.kill();
                    events.end_with_too_many(handle);
                    return;
                }
            }
            read = stdout.read(&mut buffer), if open => match read {
                Ok(0) | Err(_) => {
                    open = false;
                    events.push_all(lines.rest());
                }
                Ok(n) => {
                    events.record(&buffer[..n]);
                    events.push_all(lines.feed(&buffer[..n]));
                }
            },
            status = process.child.wait(), if !open => {
                if events.flush(&handle).is_err() {
                    events.end_with_too_many(handle);
                    return;
                }
                let code = status.ok().and_then(|status| status.code());
                let summary = match code {
                    Some(code) => format!("{} (exit code {code})", events.description),
                    None => format!("{} (no exit code)", events.description),
                };
                // Exit codes count as they count for Bash.
                let completed = code.is_some_and(|code| !command_rules::is_error_exit(&command, code));
                handle.finish(if completed {
                    TaskOutcome::completed(summary)
                } else {
                    TaskOutcome::failed(summary)
                });
                return;
            }
        }
    }
}

/// Watch a WebSocket until the server closes it, the connection fails, the
/// time limit is reached, it produces too many events or it is stopped.
async fn watch_socket(
    mut socket: Socket,
    handle: TaskHandle,
    mut events: Events,
    timeout: Duration,
) {
    let stop = handle.stop_signal();
    let deadline = Instant::now() + timeout;
    loop {
        let flush_at = events.flush_at;
        tokio::select! {
            biased;
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep_until(deadline) => {
                events.end_at_time_limit(handle, timeout);
                let _ = tokio::time::timeout(Duration::from_secs(1), socket.close(None)).await;
                return;
            }
            _ = sleep_until(flush_at), if flush_at.is_some() => {
                if events.flush(&handle).is_err() {
                    events.end_with_too_many(handle);
                    return;
                }
            }
            message = socket.next() => {
                let line = match message {
                    Some(Ok(Message::Text(text))) => text.to_string(),
                    Some(Ok(Message::Binary(bytes))) => {
                        format!("[binary frame of {} bytes]", bytes.len())
                    }
                    Some(Ok(Message::Close(frame))) => {
                        let summary = match frame {
                            Some(frame) if frame.reason.is_empty() => format!(
                                "{} (the server closed the connection with code {})",
                                events.description,
                                u16::from(frame.code)
                            ),
                            Some(frame) => format!(
                                "{} (the server closed the connection with code {}: {})",
                                events.description,
                                u16::from(frame.code),
                                frame.reason
                            ),
                            None => format!("{} (the server closed the connection)", events.description),
                        };
                        events.end(handle, TaskOutcome::completed(summary));
                        return;
                    }
                    None => {
                        let summary = format!("{} (the connection ended)", events.description);
                        events.end(handle, TaskOutcome::completed(summary));
                        return;
                    }
                    Some(Err(error)) => {
                        let summary = format!("{} (the connection failed: {error})", events.description);
                        events.end(handle, TaskOutcome::failed(summary));
                        return;
                    }
                    Some(Ok(_)) => continue,
                };
                events.record_line(&line);
                events.push(line);
            }
        }
    }
}

/// Sleep until `at`; never wakes for `None`.
async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Lines of a monitor waiting to be delivered, and what was delivered when.
struct Events {
    session: Weak<AgentSession>,
    task_id: String,
    description: String,
    /// Where everything the monitor watches is written.
    file: std::fs::File,
    pending: Vec<String>,
    /// When the pending lines are delivered: [`GROUP_WINDOW`] after the
    /// first of them arrived.
    flush_at: Option<Instant>,
    /// When the recent events were delivered, oldest first.
    delivered: VecDeque<Instant>,
    limits: Limits,
}

/// More than [`MAX_EVENTS`] events within [`EVENT_WINDOW`].
struct TooManyEvents;

impl Events {
    fn new(
        session: &Arc<AgentSession>,
        handle: &TaskHandle,
        file: std::fs::File,
        description: &str,
        limits: Limits,
    ) -> Self {
        Events {
            session: Arc::downgrade(session),
            task_id: handle.id().to_string(),
            description: description.to_string(),
            file,
            pending: Vec::new(),
            flush_at: None,
            delivered: VecDeque::new(),
            limits,
        }
    }

    /// Write output to the output file.
    fn record(&mut self, bytes: &[u8]) {
        let _ = self.file.write_all(bytes);
    }

    fn record_line(&mut self, line: &str) {
        let _ = self.file.write_all(line.as_bytes());
        let _ = self.file.write_all(b"\n");
    }

    /// Add a line to the next event. A blank line is no event.
    fn push(&mut self, line: String) {
        if line.trim().is_empty() {
            return;
        }
        if self.pending.is_empty() {
            self.flush_at = Some(Instant::now() + self.limits.group_window);
        }
        self.pending.push(line);
    }

    fn push_all(&mut self, lines: impl IntoIterator<Item = String>) {
        for line in lines {
            self.push(line);
        }
    }

    /// Deliver the pending lines as one event. Fails, delivering nothing,
    /// when that event would be one too many.
    fn flush(&mut self, handle: &TaskHandle) -> Result<(), TooManyEvents> {
        self.flush_at = None;
        if self.pending.is_empty() {
            return Ok(());
        }
        let lines = std::mem::take(&mut self.pending);
        let now = Instant::now();
        while self
            .delivered
            .front()
            .is_some_and(|at| now.duration_since(*at) >= self.limits.event_window)
        {
            self.delivered.pop_front();
        }
        if self.delivered.len() >= self.limits.max_events {
            return Err(TooManyEvents);
        }
        self.delivered.push_back(now);
        let (event, omitted) = event_text(&lines);
        let Some(session) = self.session.upgrade() else {
            return Ok(());
        };
        let Some(task) = session
            .background_tasks()
            .find(&self.task_id)
            .filter(|task| task.status.is_running())
        else {
            return Ok(());
        };
        if let Some(last) = lines.last() {
            handle.set_progress(Some(last.chars().take(120).collect()));
        }
        session.queue_entry(monitor_event(&task, &event, omitted), Priority::Next);
        Ok(())
    }

    /// Deliver what is pending and end the task with `outcome`.
    fn end(mut self, handle: TaskHandle, outcome: TaskOutcome) {
        if self.flush(&handle).is_err() {
            return self.end_with_too_many(handle);
        }
        handle.finish(outcome);
    }

    fn end_at_time_limit(self, handle: TaskHandle, timeout: Duration) {
        let summary = format!(
            "{} (stopped at its time limit of {} ms)",
            self.description,
            timeout.as_millis()
        );
        self.end(handle, TaskOutcome::stopped(summary));
    }

    fn end_with_too_many(self, handle: TaskHandle) {
        handle.finish(TaskOutcome::stopped(format!(
            "{} (stopped after more than {} events within {} seconds)",
            self.description,
            self.limits.max_events,
            self.limits.event_window.as_secs()
        )));
    }
}

/// The text of one event: its lines, as many as fit in
/// [`MAX_EVENT_CHARS`], and how many were left out.
fn event_text(lines: &[String]) -> (String, usize) {
    let mut text = String::new();
    let mut chars = 0;
    for (index, line) in lines.iter().enumerate() {
        let length = line.chars().count();
        if index > 0 && chars + 1 + length > MAX_EVENT_CHARS {
            return (text, lines.len() - index);
        }
        if index > 0 {
            text.push('\n');
            chars += 1;
        }
        if length > MAX_EVENT_CHARS {
            text.extend(line.chars().take(MAX_EVENT_CHARS));
            chars += MAX_EVENT_CHARS;
        } else {
            text.push_str(line);
            chars += length;
        }
    }
    (text, 0)
}

/// Bytes of output cut into lines.
#[derive(Default)]
struct Lines {
    partial: Vec<u8>,
}

impl Lines {
    /// The lines `bytes` completes. A line longer than [`MAX_LINE_BYTES`]
    /// is cut.
    fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                lines.push(decode(std::mem::take(&mut self.partial)));
            } else {
                self.partial.push(byte);
                if self.partial.len() >= MAX_LINE_BYTES {
                    lines.push(decode(std::mem::take(&mut self.partial)));
                }
            }
        }
        lines
    }

    /// The last line, when the output did not end with a line break.
    fn rest(&mut self) -> Option<String> {
        (!self.partial.is_empty()).then(|| decode(std::mem::take(&mut self.partial)))
    }
}

fn decode(mut bytes: Vec<u8>) -> String {
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_cut_into_lines() {
        let mut lines = Lines::default();
        assert_eq!(lines.feed(b"one\ntw"), ["one"]);
        assert_eq!(lines.feed(b"o\r\nthree"), ["two"]);
        assert_eq!(lines.rest(), Some("three".to_string()));
        assert_eq!(lines.rest(), None);
        let long = vec![b'x'; MAX_LINE_BYTES + 10];
        let cut = lines.feed(&long);
        assert_eq!(cut.len(), 1);
        assert_eq!(cut[0].len(), MAX_LINE_BYTES);
        assert_eq!(lines.rest().unwrap().len(), 10);
    }

    #[test]
    fn an_event_carries_what_fits_and_counts_the_rest() {
        let lines: Vec<String> = vec![
            "a".repeat(6_000),
            "b".repeat(3_000),
            "c".repeat(2_000),
            "d".into(),
        ];
        let (text, omitted) = event_text(&lines);
        assert_eq!(
            text,
            format!("{}\n{}", "a".repeat(6_000), "b".repeat(3_000))
        );
        assert_eq!(omitted, 2);
        let (text, omitted) = event_text(&["x".repeat(MAX_EVENT_CHARS + 5)]);
        assert_eq!(text.len(), MAX_EVENT_CHARS);
        assert_eq!(omitted, 0);
    }
}
