//! The `registry` stream: which hosts exist, which sessions run on them, and
//! what state they are in. Readers fold the stream to the latest record per
//! host and per session.

use crate::host::HostIdentity;
use crate::s2::{AppendOptions, AppendRecord, S2Client};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub const REGISTRY_STREAM: &str = "registry";
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

pub use super_sdk::daemon::SessionState;

/// One session as announced to controllers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub session_id: String,
    pub workspace: String,
    pub cwd: String,
    pub state: SessionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    pub model: String,
    pub thinking_level: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_message: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resume_argv: Vec<String>,
    /// Set when the session is driven by a headless process a desktop app
    /// controls over local RPC.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub headless: bool,
    /// Set when the session is hosted by the SUPER daemon.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub daemon: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HostBody<'a> {
    kind: &'static str,
    host_id: &'a str,
    name: &'a str,
    platform: &'a str,
    device_id: &'a str,
    at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionBody<'a> {
    kind: &'static str,
    host_id: &'a str,
    #[serde(flatten)]
    session: &'a SessionRecord,
    at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatBody<'a> {
    kind: &'static str,
    host_id: &'a str,
    at: i64,
}

enum Update {
    Publish(Box<SessionRecord>),
    Refresh,
}

/// Publishes the host's presence and a record per session. Drop or call
/// [`RegistryWriter::stop`] to announce the end.
pub struct RegistryWriter {
    updates: mpsc::UnboundedSender<Update>,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl RegistryWriter {
    /// Start announcing. The host record goes out first, then every change
    /// to any session record, and a heartbeat every 30 seconds.
    pub fn start(client: Arc<S2Client>, host: HostIdentity, device_id: String) -> Self {
        let (updates, mut updates_rx) = mpsc::unbounded_channel::<Update>();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let mut announced_host = false;
            let mut sent: BTreeMap<String, SessionRecord> = BTreeMap::new();
            let mut pending: BTreeMap<String, SessionRecord> = BTreeMap::new();
            loop {
                if !announced_host {
                    let body = HostBody {
                        kind: "host",
                        host_id: &host.host_id,
                        name: &host.name,
                        platform: &host.platform,
                        device_id: &device_id,
                        at: crate::now_ms(),
                    };
                    announced_host = append(&client, "host", &body).await;
                }
                let due: Vec<SessionRecord> = pending.values().cloned().collect();
                for record in due {
                    if sent.get(&record.session_id) == Some(&record) {
                        pending.remove(&record.session_id);
                        continue;
                    }
                    let body = SessionBody {
                        kind: "session",
                        host_id: &host.host_id,
                        session: &record,
                        at: crate::now_ms(),
                    };
                    if append(&client, "session", &body).await {
                        pending.remove(&record.session_id);
                        sent.insert(record.session_id.clone(), record);
                    } else {
                        break;
                    }
                }
                let retry = if pending.is_empty() {
                    HEARTBEAT_INTERVAL
                } else {
                    Duration::from_secs(5)
                };
                tokio::select! {
                    _ = worker_cancel.cancelled() => break,
                    update = updates_rx.recv() => match update {
                        Some(Update::Publish(record)) => {
                            pending.insert(record.session_id.clone(), *record);
                        }
                        Some(Update::Refresh) => {
                            // Forget what was sent so the next pass sends it all again.
                            let known = std::mem::take(&mut sent);
                            for (id, record) in known {
                                pending.entry(id).or_insert(record);
                            }
                        }
                        None => break,
                    },
                    _ = tokio::time::sleep(retry) => {
                        if pending.is_empty() {
                            let body = HeartbeatBody { kind: "heartbeat", host_id: &host.host_id, at: crate::now_ms() };
                            append(&client, "heartbeat", &body).await;
                        }
                    }
                }
            }
            // Say goodbye so controllers do not show a stale "working".
            for (_, mut record) in sent {
                if record.state == SessionState::Ended {
                    continue;
                }
                record.state = SessionState::Ended;
                record.blocked_reason = None;
                let body = SessionBody {
                    kind: "session",
                    host_id: &host.host_id,
                    session: &record,
                    at: crate::now_ms(),
                };
                append(&client, "session", &body).await;
            }
        });
        Self {
            updates,
            cancel,
            task: Some(task),
        }
    }

    /// Publish a session's snapshot. An unchanged snapshot is not re-sent.
    pub fn publish(&self, record: SessionRecord) {
        let _ = self.updates.send(Update::Publish(Box::new(record)));
    }

    /// Send every known record again, for readers whose copy expired.
    pub fn refresh(&self) {
        let _ = self.updates.send(Update::Refresh);
    }

    /// Announce the end and wait for the last append.
    pub async fn stop(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
        }
    }
}

impl Drop for RegistryWriter {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn append<T: Serialize>(client: &S2Client, kind: &str, body: &T) -> bool {
    let Ok(body) = serde_json::to_string(body) else {
        return false;
    };
    let record = AppendRecord::new(body).header("t", kind);
    match client
        .append(REGISTRY_STREAM, &[record], &AppendOptions::default())
        .await
    {
        Ok(ack) => {
            crate::log::line(format!("registry append {kind} seq {}", ack.start.seq_num));
            true
        }
        Err(error) => {
            crate::log::line(format!("registry append {kind} failed: {error}"));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_records_serialize_in_the_documented_shape() {
        let record = SessionRecord {
            session_id: "a1b2c3d4".into(),
            workspace: "/w".into(),
            cwd: "/w".into(),
            state: SessionState::Blocked,
            blocked_reason: Some("Workflow approval required".into()),
            model: "super/gpt-5".into(),
            thinking_level: "medium".into(),
            name: None,
            first_message: Some("Fix it".into()),
            resume_argv: vec!["super".into()],
            headless: false,
            daemon: false,
        };
        let body = SessionBody {
            kind: "session",
            host_id: "01H",
            session: &record,
            at: 1,
        };
        let json = serde_json::to_value(&body).expect("json");
        assert_eq!(json["kind"], "session");
        assert_eq!(json["hostId"], "01H");
        assert_eq!(json["sessionId"], "a1b2c3d4");
        assert_eq!(json["state"], "blocked");
        assert_eq!(json["blockedReason"], "Workflow approval required");
        assert!(json.get("headless").is_none());
        assert!(json.get("name").is_none());
    }
}
