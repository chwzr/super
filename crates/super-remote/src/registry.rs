//! The `registry` stream: which hosts exist, which sessions run on them, and
//! what state they are in. Readers fold the stream to the latest record per
//! host and per session.

use crate::host::HostIdentity;
use crate::s2::{AppendOptions, AppendRecord, ReadEvent, ReadStart, S2Client, S2Error};
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
    /// Set when the SUPER daemon runs on the host, so apps can ask it to
    /// start and resume sessions there.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    daemon: bool,
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
        Self::spawn(client, host, device_id, false)
    }

    /// [`start`](Self::start) for the SUPER daemon: the host record carries
    /// `"daemon": true`, also when no session exists yet.
    pub fn start_daemon(client: Arc<S2Client>, host: HostIdentity, device_id: String) -> Self {
        Self::spawn(client, host, device_id, true)
    }

    fn spawn(client: Arc<S2Client>, host: HostIdentity, device_id: String, daemon: bool) -> Self {
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
                        daemon,
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
                            // Forget what was sent, the host record included, so
                            // the next pass sends it all again.
                            announced_host = false;
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

    /// Send the host record and every known session record again, for
    /// readers whose copy expired.
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

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// A host is online when anything about it (host record, heartbeat, session
/// record) is at most this old. Heartbeats come every 30 seconds; the apps
/// use the same window.
pub const ONLINE_WINDOW: Duration = Duration::from_secs(90);

/// How long a host record's `daemon` flag outlives a newer host record
/// without it, as in the apps. A daemon sends its host record again every six
/// hours; a standalone `super` on the same machine (`-p`, `--mode rpc`,
/// `daemon.attach: false`) writes host records without the flag, which must
/// not hide the daemon in between.
pub const DAEMON_FLAG_WINDOW: Duration = Duration::from_secs(6 * 3600 + 90);

/// A host as the registry knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryHost {
    pub host_id: String,
    /// The host id until a host record names it.
    pub name: String,
    pub platform: String,
    /// The newest host record says the SUPER daemon runs there. Whether it
    /// takes host commands is [`RegistryView::runs_daemon`].
    pub daemon: bool,
    /// Unix milliseconds of the newest host record that said `daemon: true`.
    pub daemon_flagged_at: Option<i64>,
    /// Unix milliseconds of the newest record about the host.
    pub last_seen: i64,
}

/// A session record and the host that announced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrySession {
    pub host_id: String,
    pub record: SessionRecord,
    /// Unix milliseconds of the record.
    pub at: i64,
}

/// The `registry` stream folded to the newest record per host and per
/// session, as the apps fold it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryView {
    hosts: BTreeMap<String, RegistryHost>,
    sessions: BTreeMap<(String, String), RegistrySession>,
}

impl RegistryView {
    /// Apply one record body, in stream order. Unknown kinds and malformed
    /// bodies are skipped.
    pub fn apply(&mut self, body: &str) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
            return;
        };
        let host_id = value["hostId"].as_str().unwrap_or_default().to_string();
        if host_id.is_empty() {
            return;
        }
        let at = value["at"].as_i64().unwrap_or(0);
        match value["kind"].as_str() {
            Some("host") => {
                let host = self.host(&host_id);
                if let Some(name) = value["name"].as_str() {
                    host.name = name.to_string();
                }
                if let Some(platform) = value["platform"].as_str() {
                    host.platform = platform.to_string();
                }
                host.daemon = value["daemon"].as_bool().unwrap_or(false);
                if host.daemon && host.daemon_flagged_at.is_none_or(|flagged| flagged < at) {
                    host.daemon_flagged_at = Some(at);
                }
                host.last_seen = host.last_seen.max(at);
            }
            Some("heartbeat") => {
                let host = self.host(&host_id);
                host.last_seen = host.last_seen.max(at);
            }
            Some("session") => {
                let Ok(record) = serde_json::from_value::<SessionRecord>(value) else {
                    return;
                };
                let host = self.host(&host_id);
                host.last_seen = host.last_seen.max(at);
                self.sessions.insert(
                    (host_id.clone(), record.session_id.clone()),
                    RegistrySession {
                        host_id,
                        record,
                        at,
                    },
                );
            }
            _ => {}
        }
    }

    fn host(&mut self, host_id: &str) -> &mut RegistryHost {
        self.hosts
            .entry(host_id.to_string())
            .or_insert_with(|| RegistryHost {
                host_id: host_id.to_string(),
                name: host_id.to_string(),
                platform: "unknown".into(),
                daemon: false,
                daemon_flagged_at: None,
                last_seen: 0,
            })
    }

    pub fn hosts(&self) -> impl Iterator<Item = &RegistryHost> {
        self.hosts.values()
    }

    pub fn host_by_id(&self, host_id: &str) -> Option<&RegistryHost> {
        self.hosts.get(host_id)
    }

    pub fn sessions(&self) -> impl Iterator<Item = &RegistrySession> {
        self.sessions.values()
    }

    /// Whether anything about the host is at most [`ONLINE_WINDOW`] old at
    /// `now` (Unix milliseconds).
    pub fn is_online(&self, host_id: &str, now: i64) -> bool {
        self.hosts.get(host_id).is_some_and(|host| {
            host.last_seen > 0 && now - host.last_seen <= ONLINE_WINDOW.as_millis() as i64
        })
    }

    /// Whether the SUPER daemon runs on the host at `now`, as the apps
    /// decide it: its newest host record says so, a host record said so
    /// within [`DAEMON_FLAG_WINDOW`], or one of its session records says so
    /// (a daemon marks every session it hosts or keeps).
    pub fn runs_daemon(&self, host_id: &str, now: i64) -> bool {
        let flagged = self.hosts.get(host_id).is_some_and(|host| {
            host.daemon
                || host
                    .daemon_flagged_at
                    .is_some_and(|flagged| now - flagged <= DAEMON_FLAG_WINDOW.as_millis() as i64)
        });
        flagged
            || self
                .sessions
                .values()
                .any(|session| session.host_id == host_id && session.record.daemon)
    }
}

/// Read the whole `registry` stream as it is now and fold it. Gives up after
/// `limit`.
pub async fn read_registry(
    client: &Arc<S2Client>,
    limit: Duration,
) -> Result<RegistryView, S2Error> {
    let mut view = RegistryView::default();
    let tail = match client.tail(REGISTRY_STREAM).await {
        Ok(tail) => tail,
        // Nobody has announced anything yet.
        Err(S2Error::Status(404, _)) => return Ok(view),
        Err(error) => return Err(error),
    };
    if tail == 0 {
        return Ok(view);
    }
    let cancel = CancellationToken::new();
    let mut reader = client.tail_stream(
        REGISTRY_STREAM.to_string(),
        ReadStart::SeqNum(0),
        cancel.clone(),
    );
    let read = tokio::time::timeout(limit, async {
        while let Some(event) = reader.recv().await {
            match event {
                ReadEvent::Records(records) => {
                    for record in &records {
                        view.apply(&record.body);
                    }
                    if records.last().is_some_and(|last| last.seq_num + 1 >= tail) {
                        return Ok(());
                    }
                }
                // Older records may be gone; a ping at the tail means done.
                ReadEvent::Ping { tail: Some(at) } if at >= tail => return Ok(()),
                ReadEvent::Failed(reason) => return Err(S2Error::Transport(reason)),
                _ => {}
            }
        }
        Err(S2Error::Transport("the registry read ended early".into()))
    })
    .await;
    cancel.cancel();
    match read {
        Ok(Ok(())) => Ok(view),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(S2Error::Transport(format!(
            "reading the registry took longer than {}s",
            limit.as_secs()
        ))),
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

    #[test]
    fn a_daemon_stays_known_beside_a_standalone_super_on_the_same_host() {
        let host = |daemon: bool, at: i64| {
            format!(
                r#"{{"kind":"host","hostId":"01H","name":"linuxbox","platform":"linux","daemon":{daemon},"at":{at}}}"#
            )
        };
        let window = DAEMON_FLAG_WINDOW.as_millis() as i64;
        let mut view = RegistryView::default();
        view.apply(&host(true, 1_000));
        // A `super -p` there announces the host without the flag.
        view.apply(&host(false, 2_000));
        assert!(!view.host_by_id("01H").unwrap().daemon, "the newest record");
        assert!(view.runs_daemon("01H", 2_000));
        assert!(view.runs_daemon("01H", 1_000 + window));
        assert!(
            !view.runs_daemon("01H", 1_000 + window + 1),
            "a daemon renews its host record well before the flag lapses"
        );

        // A session record of the daemon says it runs there too.
        let mut sessions = RegistryView::default();
        sessions.apply(&host(false, 1_000));
        assert!(!sessions.runs_daemon("01H", 1_000));
        sessions.apply(
            r#"{"kind":"session","hostId":"01H","sessionId":"a1","workspace":"/w","cwd":"/w","state":"ended","model":"m","thinkingLevel":"off","daemon":true,"at":900}"#,
        );
        assert!(sessions.runs_daemon("01H", 1_000 + window + 1));
        assert!(!sessions.runs_daemon("02X", 1_000));
    }

    #[test]
    fn the_view_keeps_the_newest_record_per_host_and_session() {
        let mut view = RegistryView::default();
        view.apply(
            r#"{"kind":"host","hostId":"01H","name":"mbp","platform":"macos","deviceId":"d","daemon":true,"at":1000}"#,
        );
        let session = |state: &str, at: i64| {
            format!(
                r#"{{"kind":"session","hostId":"01H","sessionId":"a1","workspace":"/w","cwd":"/w","state":"{state}","model":"mock/mock-1","thinkingLevel":"off","at":{at}}}"#
            )
        };
        view.apply(&session("working", 2000));
        view.apply(&session("idle", 3000));
        // A session on a host whose host record has expired, and junk.
        view.apply(
            r#"{"kind":"session","hostId":"02L","sessionId":"b2","workspace":"/srv","cwd":"/srv","state":"ended","model":"m","thinkingLevel":"off","at":500}"#,
        );
        view.apply("not json");
        view.apply(
            r#"{"kind":"session","hostId":"02L","sessionId":"c3","state":"strange","at":9}"#,
        );
        view.apply(r#"{"kind":"heartbeat","hostId":"01H","at":120000}"#);

        let hosts: Vec<&RegistryHost> = view.hosts().collect();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].name, "mbp");
        assert!(hosts[0].daemon);
        assert_eq!(hosts[0].last_seen, 120_000);
        assert_eq!(hosts[1].name, "02L", "no host record: the id stands in");
        assert!(!hosts[1].daemon);

        let sessions: Vec<&RegistrySession> = view.sessions().collect();
        assert_eq!(
            sessions.len(),
            2,
            "the record with an unknown state is skipped"
        );
        assert_eq!(sessions[0].record.state, SessionState::Idle);
        assert_eq!(sessions[0].at, 3000);

        assert!(view.is_online("01H", 120_000 + 90_000));
        assert!(!view.is_online("01H", 120_000 + 90_001));
        assert!(!view.is_online("02L", 500 + 90_001));
        assert!(!view.is_online("nobody", 0));
    }

    #[test]
    fn the_host_record_names_a_daemon_and_omits_the_flag_otherwise() {
        let body = |daemon| HostBody {
            kind: "host",
            host_id: "01H",
            name: "mbp",
            platform: "macos",
            device_id: "device-1",
            daemon,
            at: 1,
        };
        let json = serde_json::to_value(body(true)).expect("json");
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "host", "hostId": "01H", "name": "mbp", "platform": "macos",
                "deviceId": "device-1", "daemon": true, "at": 1
            })
        );
        let json = serde_json::to_value(body(false)).expect("json");
        assert!(json.get("daemon").is_none(), "{json}");
        assert_eq!(json["kind"], "host");
    }
}
