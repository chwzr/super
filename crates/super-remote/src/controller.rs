//! The controlling side of the platform: what the apps, and a terminal that
//! switches to a session on another device, do to drive it.
//!
//! A command is one record on a `commands` stream with the headers `t` (its
//! type), `from` (this device's id), `name` (shown to others as its origin)
//! and `exp` (Unix milliseconds after which the host drops it). The host
//! answers with a `response` line carrying the command's `id` on the matching
//! events stream, which this side reads from the tail it had before the
//! append, so the answer cannot slip past.

use crate::s2::{AppendOptions, AppendRecord, ReadEvent, ReadStart, S2Client, S2Error};
use crate::transport::SessionStreams;
use anyhow::{Context as _, Result, anyhow};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_sdk::daemon::DaemonCommand;
use super_sdk::protocol::{Command, Request, Response};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// How long a command may wait for its answer before it counts as lost.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a session command stays valid on the stream, as the apps send it.
pub const COMMAND_EXPIRY: Duration = Duration::from_secs(5 * 60);

/// Who sends commands: this device's id and the name other clients see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sender {
    pub device_id: String,
    pub name: String,
}

impl Sender {
    /// One command record, expiring `expiry` from now.
    pub fn record(&self, kind: &str, body: String, expiry: Duration) -> AppendRecord {
        let expires = crate::now_ms() + expiry.as_millis() as i64;
        AppendRecord::new(body)
            .header("t", kind)
            .header("from", self.device_id.clone())
            .header("name", self.name.clone())
            .header("exp", expires.to_string())
    }
}

/// The next sequence number of `stream`; a stream nobody wrote is empty.
async fn tail_or_start(client: &S2Client, stream: &str) -> Result<u64, S2Error> {
    match client.tail(stream).await {
        Ok(tail) => Ok(tail),
        Err(S2Error::Status(404, _)) => Ok(0),
        Err(error) => Err(error),
    }
}

/// A `response` line for `id`, if the body is one.
fn answer_for(body: &str, id: &str) -> Option<Response> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value["type"] != "response" || value["id"] != id {
        return None;
    }
    serde_json::from_value(value).ok()
}

/// Daemon-level commands for the SUPER daemon on another host:
/// `hosts/{hostId}/commands` in, `hosts/{hostId}/events` out.
pub struct HostCommands {
    client: Arc<S2Client>,
    host_id: String,
    sender: Sender,
    timeout: Duration,
}

impl HostCommands {
    pub fn new(client: Arc<S2Client>, host_id: impl Into<String>, sender: Sender) -> Self {
        Self {
            client,
            host_id: host_id.into(),
            sender,
            timeout: ANSWER_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    /// Send `command` and wait for its response. The command expires when the
    /// wait ends, so a daemon that comes back later never runs one this side
    /// already reported as lost.
    pub async fn call(&self, command: &DaemonCommand) -> Result<Response> {
        let events = format!("hosts/{}/events", self.host_id);
        let commands = format!("hosts/{}/commands", self.host_id);
        let start = tail_or_start(&self.client, &events)
            .await
            .map_err(|error| anyhow!("{events}: {error}"))?;
        let id = crate::ulid::generate();
        let mut body = serde_json::to_value(command)?;
        body["id"] = Value::String(id.clone());
        let cancel = CancellationToken::new();
        let mut reader = self
            .client
            .tail_stream(events, ReadStart::SeqNum(start), cancel.clone());
        let record = self
            .sender
            .record(command.name(), body.to_string(), self.timeout);
        let appended = self
            .client
            .append(&commands, &[record], &AppendOptions::default())
            .await;
        if let Err(error) = appended {
            cancel.cancel();
            return Err(anyhow!("{commands}: {error}"));
        }
        let answer = tokio::time::timeout(self.timeout, async {
            while let Some(event) = reader.recv().await {
                match event {
                    ReadEvent::Records(records) => {
                        if let Some(response) = records
                            .iter()
                            .find_map(|record| answer_for(&record.body, &id))
                        {
                            return Ok(response);
                        }
                    }
                    ReadEvent::Failed(reason) => return Err(anyhow!(reason)),
                    _ => {}
                }
            }
            Err(anyhow!("the host's event stream ended"))
        })
        .await;
        cancel.cancel();
        answer.unwrap_or_else(|_| {
            Err(anyhow!(
                "the SUPER daemon on that device did not answer `{}` within {}s",
                command.name(),
                self.timeout.as_secs()
            ))
        })
    }
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Response>>>>;

/// Drive one session on another host: its `events` stream in, its
/// `commands` stream out, the same RPC commands and events a local client
/// speaks.
pub struct SessionController {
    client: Arc<S2Client>,
    streams: SessionStreams,
    sender: Sender,
    pending: Pending,
    timeout: Duration,
    cancel: CancellationToken,
}

impl SessionController {
    /// Follow the session's events from now on. The receiver gets every
    /// event except answers to commands; it closes when the stream fails
    /// for good or the controller goes away.
    pub async fn open(
        client: Arc<S2Client>,
        host_id: &str,
        session_id: &str,
        sender: Sender,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Value>)> {
        let streams = SessionStreams::new(host_id, session_id);
        let start = tail_or_start(&client, &streams.events)
            .await
            .map_err(|error| anyhow!("{}: {error}", streams.events))?;
        let cancel = CancellationToken::new();
        let mut reader = client.tail_stream(
            streams.events.clone(),
            ReadStart::SeqNum(start),
            cancel.clone(),
        );
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        {
            let pending = pending.clone();
            tokio::spawn(async move {
                while let Some(event) = reader.recv().await {
                    match event {
                        ReadEvent::Records(records) => {
                            for record in records {
                                let Ok(value) = serde_json::from_str::<Value>(&record.body) else {
                                    continue;
                                };
                                if value["type"] == "response" {
                                    let waiting = value["id"]
                                        .as_str()
                                        .and_then(|id| pending.lock().unwrap().remove(id));
                                    if let (Some(waiting), Ok(response)) =
                                        (waiting, serde_json::from_value::<Response>(value))
                                    {
                                        let _ = waiting.send(response);
                                    }
                                    // Answers to other controllers are theirs.
                                    continue;
                                }
                                if events_tx.send(value).is_err() {
                                    return;
                                }
                            }
                        }
                        ReadEvent::Failed(_) => break,
                        _ => {}
                    }
                }
                // Every call still waiting fails with "closed".
                pending.lock().unwrap().clear();
            });
        }
        Ok((
            Self {
                client,
                streams,
                sender,
                pending,
                timeout: ANSWER_TIMEOUT,
                cancel,
            },
            events_rx,
        ))
    }

    /// Send a session command and wait for its response.
    pub async fn execute(&self, command: Command) -> Result<Response> {
        if self.cancel.is_cancelled() {
            return Err(anyhow!("the session's event stream is closed"));
        }
        let name = command.name();
        let id = crate::ulid::generate();
        let body = serde_json::to_string(&Request::with_id(id.clone(), command))?;
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), sender);
        let record = self.sender.record(name, body, COMMAND_EXPIRY);
        if let Err(error) = self
            .client
            .append(&self.streams.commands, &[record], &AppendOptions::default())
            .await
        {
            self.pending.lock().unwrap().remove(&id);
            return Err(anyhow!("{}: {error}", self.streams.commands));
        }
        match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(anyhow!("the session's event stream closed")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(anyhow!(
                    "the session did not answer `{name}` within {}s; is its device online?",
                    self.timeout.as_secs()
                ))
            }
        }
        .context(name)
    }

    pub fn streams(&self) -> &SessionStreams {
        &self.streams
    }

    /// Stop following the session.
    pub fn close(&self) {
        self.cancel.cancel();
    }
}

impl Drop for SessionController {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_records_carry_type_origin_and_expiry() {
        let sender = Sender {
            device_id: "device-1".into(),
            name: "Terminal on mbp".into(),
        };
        let before = crate::now_ms();
        let record = sender.record("prompt", "{}".into(), Duration::from_secs(60));
        let header = |name: &str| {
            record
                .headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap()
        };
        assert_eq!(header("t"), "prompt");
        assert_eq!(header("from"), "device-1");
        assert_eq!(header("name"), "Terminal on mbp");
        let expires: i64 = header("exp").parse().unwrap();
        assert!(expires >= before + 60_000 && expires <= crate::now_ms() + 60_000);
    }

    #[test]
    fn only_the_answer_with_our_id_counts() {
        let answer = r#"{"type":"response","id":"c1","command":"status","success":true}"#;
        assert!(answer_for(answer, "c1").is_some());
        assert!(answer_for(answer, "c2").is_none());
        assert!(answer_for(r#"{"type":"agent_start","id":"c1"}"#, "c1").is_none());
        assert!(answer_for("garbage", "c1").is_none());
    }
}
