//! The `registry` stream: which hosts exist, which sessions run on them, and
//! what state they are in. Readers fold the stream to the latest record per
//! host and per session.

use crate::host::HostIdentity;
use crate::s2::{AppendOptions, AppendRecord, S2Client};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub const REGISTRY_STREAM: &str = "registry";
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Idle,
    Working,
    Blocked,
    Ended,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Ended => "ended",
        }
    }
}

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

/// Publishes one session's presence. Drop or call [`RegistryWriter::stop`]
/// to announce the end.
pub struct RegistryWriter {
    latest: watch::Sender<Option<SessionRecord>>,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl RegistryWriter {
    /// Start announcing. The host record goes out first, then every change
    /// to the session record, and a heartbeat every 30 seconds.
    pub fn start(client: Arc<S2Client>, host: HostIdentity, device_id: String) -> Self {
        let (latest, mut changes) = watch::channel::<Option<SessionRecord>>(None);
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let mut announced_host = false;
            let mut last_sent: Option<SessionRecord> = None;
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
                let current = changes.borrow_and_update().clone();
                if let Some(record) = &current
                    && last_sent.as_ref() != Some(record)
                {
                    let body = SessionBody {
                        kind: "session",
                        host_id: &host.host_id,
                        session: record,
                        at: crate::now_ms(),
                    };
                    if append(&client, "session", &body).await {
                        last_sent = Some(record.clone());
                    }
                }
                tokio::select! {
                    _ = worker_cancel.cancelled() => break,
                    changed = changes.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                    _ = tokio::time::sleep(HEARTBEAT_INTERVAL) => {
                        let body = HeartbeatBody { kind: "heartbeat", host_id: &host.host_id, at: crate::now_ms() };
                        append(&client, "heartbeat", &body).await;
                    }
                }
            }
            // Say goodbye so controllers do not show a stale "working".
            if let Some(mut record) = last_sent {
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
            latest,
            cancel,
            task: Some(task),
        }
    }

    /// Publish a new snapshot. Unchanged snapshots are not re-sent.
    pub fn publish(&self, record: SessionRecord) {
        self.latest.send_replace(Some(record));
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
