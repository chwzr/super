//! The registry: which machines exist, which sessions they have and what
//! state those are in. It lives in the platform's database. A signed-in
//! `super` writes its own machine and sessions over HTTP ([`RegistryWriter`])
//! and reads everyone's with one request ([`RegistryClient::read`]).

use crate::RemoteCredentials;
use crate::machine::MachineIdentity;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// How often a writer tells the platform that its machine is there. The
/// platform takes a machine offline 90 seconds after its last heartbeat.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// How long a writer waits before it sends again what the platform did not
/// take.
pub const RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// The most sessions the platform takes in one complete list.
pub const MAX_LISTED_SESSIONS: usize = 10_000;
/// No registry call may take longer, so a hanging one cannot hold up the
/// heartbeat for long.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub use super_sdk::daemon::SessionState;

/// One session as the registry holds it, without the machine it is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    #[serde(rename = "id")]
    pub session_id: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub workspace: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub cwd: String,
    pub state: SessionState,
    #[serde(default)]
    pub blocked_reason: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub model: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub thinking_level: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub first_message: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub resume_argv: Vec<String>,
    /// Set when the session is driven by a headless process a desktop app
    /// controls over local RPC.
    #[serde(default, deserialize_with = "null_as_default")]
    pub headless: bool,
    /// Set when the session is hosted by the SUPER daemon.
    #[serde(default, deserialize_with = "null_as_default")]
    pub daemon: bool,
    /// When something was last said in the session, RFC 3339 in UTC: the
    /// time its file was last written. A writer that leaves it out gets the
    /// time of the request for a new session and keeps the stored time for
    /// one the platform knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
}

impl SessionRecord {
    /// Whether `other` says the same about the session, whatever time of
    /// last activity each of them names.
    fn same_but_activity(&self, other: &Self) -> bool {
        self.session_id == other.session_id
            && self.workspace == other.workspace
            && self.cwd == other.cwd
            && self.state == other.state
            && self.blocked_reason == other.blocked_reason
            && self.model == other.model
            && self.thinking_level == other.thinking_level
            && self.name == other.name
            && self.first_message == other.first_message
            && self.resume_argv == other.resume_argv
            && self.headless == other.headless
            && self.daemon == other.daemon
    }

    /// The body of `PUT …/sessions/{sessionId}`: every field but the id,
    /// which the path names.
    fn body(&self) -> Value {
        let mut body = serde_json::to_value(self).unwrap_or_default();
        if let Some(fields) = body.as_object_mut() {
            fields.remove("id");
        }
        body
    }
}

fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// A point in time as the registry writes it: RFC 3339, seconds, UTC.
pub fn timestamp(time: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// When `file` was last written, as [`timestamp`] gives it; `None` when
/// there is no such file.
pub fn modified_at(file: &Path) -> Option<String> {
    std::fs::metadata(file)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(timestamp)
}

#[derive(Debug)]
pub enum RegistryError {
    /// The platform rejected the token.
    Unauthorized,
    /// The platform does not know the machine: it was never announced, or
    /// it was removed.
    NotFound,
    /// Any other HTTP status.
    Status(u16, String),
    /// Network or protocol failure.
    Transport(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => write!(f, "unauthorized"),
            Self::NotFound => write!(f, "not found"),
            Self::Status(status, body) => write!(f, "http {status}: {body}"),
            Self::Transport(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl RegistryError {
    /// The platform refused the request for what it says and would refuse
    /// it again: sending it once more is no use.
    fn refused(&self) -> bool {
        matches!(self, Self::Status(code, _)
            if (400..500).contains(code) && *code != 408 && *code != 429)
    }
}

/// The platform's registry endpoints, with the bearer token every other
/// platform call of this process uses.
pub struct RegistryClient {
    http: reqwest::Client,
    base_url: String,
    credentials: Arc<dyn RemoteCredentials>,
}

impl RegistryClient {
    /// `platform_url` is the platform's base URL, for example
    /// `https://super.flxkpe.io`.
    pub fn new(
        http: reqwest::Client,
        platform_url: String,
        credentials: Arc<dyn RemoteCredentials>,
    ) -> Self {
        Self {
            http,
            base_url: platform_url.trim_end_matches('/').to_string(),
            credentials,
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<reqwest::Response, RegistryError> {
        let token = self
            .credentials
            .access_token()
            .await
            .map_err(|error| RegistryError::Transport(format!("access token: {error:#}")))?;
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(token)
            .timeout(REQUEST_TIMEOUT);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| RegistryError::Transport(error.to_string()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let text = response.text().await.unwrap_or_default();
        Err(match status.as_u16() {
            401 => RegistryError::Unauthorized,
            404 => RegistryError::NotFound,
            code => RegistryError::Status(code, text),
        })
    }

    /// `PUT /api/machines/{machineId}`: create the machine or update its
    /// name and platform. Counts as a heartbeat.
    pub async fn announce(
        &self,
        machine: &MachineIdentity,
        daemon: bool,
    ) -> Result<(), RegistryError> {
        let mut body = json!({"name": machine.name, "platform": machine.platform});
        if daemon {
            body["daemon"] = Value::Bool(true);
        }
        self.send(
            reqwest::Method::PUT,
            &machine_path(&machine.machine_id),
            Some(&body),
        )
        .await
        .map(drop)
    }

    /// `POST /api/machines/{machineId}/heartbeat`.
    pub async fn heartbeat(&self, machine_id: &str, daemon: bool) -> Result<(), RegistryError> {
        let body = if daemon {
            json!({"daemon": true})
        } else {
            json!({})
        };
        let path = format!("{}/heartbeat", machine_path(machine_id));
        self.send(reqwest::Method::POST, &path, Some(&body))
            .await
            .map(drop)
    }

    /// `PUT /api/machines/{machineId}/sessions/{sessionId}`: create the
    /// session or replace what the platform has of it.
    pub async fn put_session(
        &self,
        machine_id: &str,
        session: &SessionRecord,
    ) -> Result<(), RegistryError> {
        let path = format!(
            "{}/sessions/{}",
            machine_path(machine_id),
            crate::s2::encode_segment(&session.session_id)
        );
        self.send(reqwest::Method::PUT, &path, Some(&session.body()))
            .await
            .map(drop)
    }

    /// `PUT /api/machines/{machineId}/sessions`: a daemon's complete list.
    /// The platform creates or replaces every session in it and removes the
    /// machine's daemon sessions that are not in it.
    pub async fn put_sessions(
        &self,
        machine_id: &str,
        sessions: &[SessionRecord],
    ) -> Result<(), RegistryError> {
        let path = format!("{}/sessions", machine_path(machine_id));
        self.send(
            reqwest::Method::PUT,
            &path,
            Some(&json!({"sessions": sessions})),
        )
        .await
        .map(drop)
    }

    /// `DELETE /api/machines/{machineId}/sessions/{sessionId}`: the
    /// session leaves the registry. Fine when it was not there.
    pub async fn delete_session(
        &self,
        machine_id: &str,
        session_id: &str,
    ) -> Result<(), RegistryError> {
        let path = format!(
            "{}/sessions/{}",
            machine_path(machine_id),
            crate::s2::encode_segment(session_id)
        );
        self.send(reqwest::Method::DELETE, &path, None)
            .await
            .map(drop)
    }

    /// `GET /api/registry`: every machine and session of the account as
    /// the platform knows them now.
    pub async fn read(&self) -> Result<Registry, RegistryError> {
        let response = self
            .send(reqwest::Method::GET, "/api/registry", None)
            .await?;
        let value: Value = response
            .json()
            .await
            .map_err(|error| RegistryError::Transport(format!("decode registry: {error}")))?;
        Ok(Registry::from_value(&value))
    }
}

fn machine_path(machine_id: &str) -> String {
    format!("/api/machines/{}", crate::s2::encode_segment(machine_id))
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// The pace of a [`RegistryWriter`].
#[derive(Debug, Clone, Copy)]
pub struct WriterOptions {
    pub heartbeat: Duration,
    pub retry: Duration,
}

impl Default for WriterOptions {
    fn default() -> Self {
        Self {
            heartbeat: HEARTBEAT_INTERVAL,
            retry: RETRY_INTERVAL,
        }
    }
}

enum Update {
    Publish(Box<SessionRecord>),
    PublishAll(Vec<SessionRecord>),
    Remove(String),
}

/// Keeps this machine and its sessions current in the registry. Drop or call
/// [`RegistryWriter::stop`] to mark the sessions as ended.
pub struct RegistryWriter {
    updates: mpsc::UnboundedSender<Update>,
    cancel: CancellationToken,
    unauthorized: Arc<AtomicBool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl RegistryWriter {
    /// Start writing. The machine is announced first, then every change to
    /// a session is sent, and a heartbeat every 30 seconds.
    pub fn start(client: Arc<RegistryClient>, machine: MachineIdentity) -> Self {
        Self::start_with(client, machine, false, WriterOptions::default())
    }

    /// [`start`](Self::start) for the SUPER daemon: the announcement and
    /// the heartbeats carry `"daemon": true`, also when no session exists
    /// yet.
    pub fn start_daemon(client: Arc<RegistryClient>, machine: MachineIdentity) -> Self {
        Self::start_with(client, machine, true, WriterOptions::default())
    }

    pub fn start_with(
        client: Arc<RegistryClient>,
        machine: MachineIdentity,
        daemon: bool,
        options: WriterOptions,
    ) -> Self {
        let (updates, updates_rx) = mpsc::unbounded_channel::<Update>();
        let cancel = CancellationToken::new();
        let unauthorized = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            client,
            machine,
            daemon,
            announced: false,
            sent: BTreeMap::new(),
            pending: BTreeMap::new(),
            refused: BTreeMap::new(),
            removals: BTreeSet::new(),
            list: None,
            unauthorized: unauthorized.clone(),
        };
        let task = tokio::spawn(worker.run(updates_rx, cancel.clone(), options));
        Self {
            updates,
            cancel,
            unauthorized,
            task: Some(task),
        }
    }

    /// Publish a session's state. Returns at once: the writer keeps the
    /// newest state per session and sends it, again every few seconds until
    /// the platform took it. A state that differs from the one the platform
    /// has only in the time of the last activity is not sent.
    pub fn publish(&self, record: SessionRecord) {
        let _ = self.updates.send(Update::Publish(Box::new(record)));
    }

    /// Publish a daemon's complete list in one request: the platform takes
    /// every session in it and drops this machine's daemon sessions that
    /// are not in it. Only the daemon may call this.
    pub fn publish_all(&self, records: Vec<SessionRecord>) {
        let _ = self.updates.send(Update::PublishAll(records));
    }

    /// Take a session out of the registry: one that ended with nothing said
    /// in it, which nobody can open again.
    pub fn remove(&self, session_id: &str) {
        let _ = self.updates.send(Update::Remove(session_id.to_string()));
    }

    /// The platform rejected the token; a new sign-in is needed.
    pub fn unauthorized(&self) -> bool {
        self.unauthorized.load(Ordering::Relaxed)
    }

    /// Mark the sessions as ended (and remove those nothing was said in) and
    /// wait for that, three seconds at most.
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

struct Worker {
    client: Arc<RegistryClient>,
    machine: MachineIdentity,
    daemon: bool,
    /// The platform knows the machine, as far as this writer can tell.
    announced: bool,
    /// What the platform has of each session.
    sent: BTreeMap<String, SessionRecord>,
    /// The newest state of each session that is still to send.
    pending: BTreeMap<String, SessionRecord>,
    /// What the platform refused of a session. It is not sent again as it
    /// is, and it holds up nobody else.
    refused: BTreeMap<String, SessionRecord>,
    /// Sessions that are still to take out of the registry.
    removals: BTreeSet<String>,
    /// A complete list that is still to send. It goes out before `pending`,
    /// which holds what changed after the list was made.
    list: Option<Vec<SessionRecord>>,
    unauthorized: Arc<AtomicBool>,
}

impl Worker {
    async fn run(
        mut self,
        mut updates: mpsc::UnboundedReceiver<Update>,
        cancel: CancellationToken,
        options: WriterOptions,
    ) {
        let mut heartbeat =
            tokio::time::interval_at(Instant::now() + options.heartbeat, options.heartbeat);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The announcement comes before anything else.
        let mut retry_at = self.flush_or_retry(options.retry).await;
        loop {
            let retry = async {
                match retry_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                // In this order, so that a stream of updates never keeps a
                // heartbeat from going out.
                biased;
                _ = cancel.cancelled() => break,
                _ = heartbeat.tick() => {
                    if self.beat().await && retry_at.is_none() {
                        retry_at = self.flush_or_retry(options.retry).await;
                    }
                }
                _ = retry => retry_at = self.flush_or_retry(options.retry).await,
                update = updates.recv() => {
                    let Some(update) = update else { break };
                    self.take(update);
                    // A burst is sent once, with the newest state of each.
                    while let Ok(update) = updates.try_recv() {
                        self.take(update);
                    }
                    // While something waits for its retry, so does this.
                    if retry_at.is_none() {
                        retry_at = self.flush_or_retry(options.retry).await;
                    }
                }
            }
        }
        self.goodbye().await;
    }

    fn take(&mut self, update: Update) {
        match update {
            Update::Publish(record) => {
                self.removals.remove(&record.session_id);
                self.pending.insert(record.session_id.clone(), *record);
            }
            Update::Remove(session_id) => {
                self.pending.remove(&session_id);
                self.refused.remove(&session_id);
                if let Some(list) = &mut self.list {
                    list.retain(|record| record.session_id != session_id);
                }
                self.removals.insert(session_id);
            }
            Update::PublishAll(mut records) => {
                if records.len() > MAX_LISTED_SESSIONS {
                    crate::log::line(format!(
                        "registry: {} sessions, the first {MAX_LISTED_SESSIONS} are listed",
                        records.len()
                    ));
                    records.truncate(MAX_LISTED_SESSIONS);
                }
                // The list is newer than anything that waited.
                self.pending.clear();
                self.removals.clear();
                self.refused.clear();
                self.list = Some(records);
            }
        }
    }

    /// Send what is to send. `None` when the platform has it all, else when
    /// to try again.
    async fn flush_or_retry(&mut self, retry: Duration) -> Option<Instant> {
        // A 404 means the platform forgot the machine: announce it again at
        // once, and wait for the retry only when that did not help.
        for _ in 0..2 {
            match self.flush().await {
                Ok(()) => return None,
                Err(RegistryError::NotFound) => self.forgotten(),
                Err(_) => break,
            }
        }
        Some(Instant::now() + retry)
    }

    async fn flush(&mut self) -> Result<(), RegistryError> {
        if !self.announced {
            let announced = self.client.announce(&self.machine, self.daemon).await;
            self.note("announce", announced)?;
            self.announced = true;
        }
        if let Some(list) = &self.list {
            let listed = self
                .client
                .put_sessions(&self.machine.machine_id, list)
                .await;
            match self.note("sessions", listed) {
                Ok(()) => {
                    self.sent = self
                        .list
                        .take()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|record| (record.session_id.clone(), record))
                        .collect();
                }
                // One session in it is to blame. They go one by one instead,
                // so the others are listed; what changed since stays newer.
                Err(error) if error.refused() => {
                    for record in self.list.take().unwrap_or_default() {
                        self.pending
                            .entry(record.session_id.clone())
                            .or_insert(record);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        while let Some((id, record)) = self.pending.pop_first() {
            if self
                .sent
                .get(&id)
                .is_some_and(|sent| sent.same_but_activity(&record))
                || self.refused.get(&id) == Some(&record)
            {
                continue;
            }
            let put = self
                .client
                .put_session(&self.machine.machine_id, &record)
                .await;
            match self.note("session", put) {
                Ok(()) => {
                    self.refused.remove(&id);
                    self.sent.insert(id, record);
                }
                // Refused for what it says: it would be refused again, and
                // the sessions after it must not wait for it.
                Err(error) if error.refused() => {
                    self.refused.insert(id, record);
                }
                Err(error) => {
                    self.pending.insert(id, record);
                    return Err(error);
                }
            }
        }
        while let Some(id) = self.removals.pop_first() {
            let removed = self
                .client
                .delete_session(&self.machine.machine_id, &id)
                .await;
            match self.note("removal", removed) {
                Ok(()) => {}
                Err(error) if error.refused() => {}
                Err(error) => {
                    self.removals.insert(id);
                    return Err(error);
                }
            }
            self.sent.remove(&id);
        }
        Ok(())
    }

    /// One heartbeat. Returns whether there is something to send after it:
    /// the machine was not announced yet, or the platform forgot it.
    async fn beat(&mut self) -> bool {
        if !self.announced {
            // The announcement that is due counts as the heartbeat.
            return true;
        }
        let beat = self
            .client
            .heartbeat(&self.machine.machine_id, self.daemon)
            .await;
        match self.note("heartbeat", beat) {
            Err(RegistryError::NotFound) => {
                self.forgotten();
                true
            }
            _ => false,
        }
    }

    /// The platform does not know the machine, so it has none of its
    /// sessions either: announce it again and send them all again.
    fn forgotten(&mut self) {
        self.announced = false;
        let known = std::mem::take(&mut self.sent);
        if self.daemon {
            if self.list.is_none() {
                self.list = Some(known.into_values().collect());
            }
        } else {
            for (id, record) in known {
                self.pending.entry(id).or_insert(record);
            }
        }
    }

    fn note(&self, what: &str, result: Result<(), RegistryError>) -> Result<(), RegistryError> {
        match &result {
            Ok(()) => crate::log::line(format!("registry {what} sent")),
            Err(error) => {
                if matches!(error, RegistryError::Unauthorized) {
                    self.unauthorized.store(true, Ordering::Relaxed);
                }
                crate::log::line(format!("registry {what} failed: {error}"));
            }
        }
        result
    }

    /// Mark every session the platform has as live as ended, so no client
    /// shows a stale "working". A session nothing was said in has nothing to
    /// open again (no file, so no `resume_argv`): it is taken out instead of
    /// staying listed for good.
    async fn goodbye(self) {
        for mut record in self.sent.into_values() {
            if record.state == SessionState::Ended {
                continue;
            }
            let machine_id = &self.machine.machine_id;
            let said = if record.resume_argv.is_empty() {
                self.client
                    .delete_session(machine_id, &record.session_id)
                    .await
            } else {
                record.state = SessionState::Ended;
                record.blocked_reason = None;
                self.client.put_session(machine_id, &record).await
            };
            match said {
                Ok(()) => {}
                Err(error) if error.refused() => {
                    crate::log::line(format!("registry goodbye refused: {error}"));
                }
                // The platform is not there; the others would fail too.
                Err(error) => {
                    crate::log::line(format!("registry goodbye failed: {error}"));
                    break;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// A machine as the registry lists it. The platform decides `online` and
/// `daemon` from the machine's heartbeats.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryMachine {
    pub id: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub platform: String,
    /// A SUPER daemon runs there, so sessions can be started and resumed.
    #[serde(default, deserialize_with = "null_as_default")]
    pub daemon: bool,
    #[serde(default, deserialize_with = "null_as_default")]
    pub online: bool,
    #[serde(default)]
    pub last_seen_at: Option<String>,
}

/// A session and the machine it is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrySession {
    pub machine_id: String,
    pub record: SessionRecord,
}

/// What `GET /api/registry` answers, as far as a terminal uses it: the
/// machines and their sessions. Chats and projects are left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    pub machines: Vec<RegistryMachine>,
    pub sessions: Vec<RegistrySession>,
}

impl Registry {
    /// Read the answer. A machine or session this build cannot read (a
    /// state it does not know, say) is skipped.
    pub fn from_value(value: &Value) -> Self {
        let list = |name: &str| value[name].as_array().cloned().unwrap_or_default();
        Self {
            machines: list("machines")
                .into_iter()
                .filter_map(|machine| serde_json::from_value::<RegistryMachine>(machine).ok())
                .filter(|machine| !machine.id.is_empty())
                .collect(),
            sessions: list("sessions")
                .into_iter()
                .filter_map(|session| {
                    let machine_id = session["machineId"].as_str()?.to_string();
                    let record = serde_json::from_value::<SessionRecord>(session).ok()?;
                    Some(RegistrySession { machine_id, record })
                })
                .collect(),
        }
    }

    pub fn machine(&self, machine_id: &str) -> Option<&RegistryMachine> {
        self.machines
            .iter()
            .find(|machine| machine.id == machine_id)
    }

    pub fn sessions_on<'a>(
        &'a self,
        machine_id: &'a str,
    ) -> impl Iterator<Item = &'a RegistrySession> {
        self.sessions
            .iter()
            .filter(move |session| session.machine_id == machine_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> SessionRecord {
        SessionRecord {
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
            last_activity_at: None,
        }
    }

    #[test]
    fn a_session_is_sent_in_the_documented_shape() {
        let body = record().body();
        assert_eq!(
            body,
            json!({
                "workspace": "/w", "cwd": "/w", "state": "blocked",
                "blockedReason": "Workflow approval required",
                "model": "super/gpt-5", "thinkingLevel": "medium", "name": null,
                "firstMessage": "Fix it", "resumeArgv": ["super"],
                "headless": false, "daemon": false
            }),
            "the path names the id; no time of last activity means the platform's"
        );

        // In a complete list every session names its id.
        let listed = SessionRecord {
            last_activity_at: Some("2026-11-12T16:00:12Z".into()),
            daemon: true,
            ..record()
        };
        let json = serde_json::to_value(&listed).expect("json");
        assert_eq!(json["id"], "a1b2c3d4");
        assert_eq!(json["lastActivityAt"], "2026-11-12T16:00:12Z");
        assert_eq!(json["daemon"], true);
        assert!(json.get("sessionId").is_none());
    }

    #[test]
    fn only_the_time_of_the_last_activity_may_differ_unsent() {
        let sent = record();
        let later = SessionRecord {
            last_activity_at: Some("2026-11-12T16:00:12Z".into()),
            ..record()
        };
        assert!(sent.same_but_activity(&later));
        let idle = SessionRecord {
            state: SessionState::Idle,
            blocked_reason: None,
            ..later.clone()
        };
        assert!(!sent.same_but_activity(&idle));
        let named = SessionRecord {
            name: Some("login".into()),
            ..later
        };
        assert!(!sent.same_but_activity(&named));
    }

    #[test]
    fn times_are_rfc_3339_seconds_in_utc() {
        let time = std::time::UNIX_EPOCH + Duration::from_millis(1_793_232_000_750);
        assert_eq!(timestamp(time), "2026-10-29T00:00:00Z");
        assert_eq!(modified_at(Path::new("/nonexistent/session.jsonl")), None);
    }

    #[test]
    fn the_registry_is_read_as_the_platform_answers_it() {
        let registry = Registry::from_value(&json!({
            "version": 1234,
            "reset": true,
            "machines": [
                {"id": "01J", "name": "mbp", "platform": "macos", "daemon": true, "online": true,
                 "lastSeenAt": "2026-11-13T08:12:00Z", "createdAt": "2026-10-01T00:00:00Z"},
                {"id": "02L", "name": "linuxbox", "platform": "linux", "daemon": false,
                 "online": false, "lastSeenAt": "2026-11-12T08:12:00Z"},
                {"name": "no id"}
            ],
            "sessions": [
                {"machineId": "01J", "id": "a1b2", "workspace": "/Users/felix/dev/super",
                 "cwd": "/Users/felix/dev/super", "state": "idle", "blockedReason": null,
                 "model": "super/gpt-5", "thinkingLevel": "medium", "name": null,
                 "firstMessage": "Fix the login bug",
                 "resumeArgv": ["super", "--session", "/Users/felix/a1b2.jsonl"],
                 "daemon": true, "headless": false,
                 "lastActivityAt": "2026-11-12T16:00:12Z",
                 "createdAt": "2026-11-12T15:00:00Z", "updatedAt": "2026-11-13T08:00:00Z"},
                // A stored session: no model, no thinking level.
                {"machineId": "02L", "id": "c3d4", "workspace": "/srv", "cwd": "/srv",
                 "state": "ended", "model": null, "thinkingLevel": null, "resumeArgv": null,
                 "lastActivityAt": "2026-11-01T00:00:00Z"},
                // A state from the future, and a session of no machine.
                {"machineId": "02L", "id": "e5f6", "state": "strange"},
                {"id": "g7h8", "state": "idle"}
            ],
            "projects": [{"id": "019a", "name": "Default"}],
            "chats": [{"id": "019b", "machineId": "cloud", "state": "idle"}],
            "deleted": []
        }));

        assert_eq!(registry.machines.len(), 2);
        let mbp = registry.machine("01J").expect("mbp");
        assert_eq!(mbp.name, "mbp");
        assert!(mbp.online && mbp.daemon);
        let linuxbox = registry.machine("02L").expect("linuxbox");
        assert!(!linuxbox.online && !linuxbox.daemon);
        assert_eq!(
            linuxbox.last_seen_at.as_deref(),
            Some("2026-11-12T08:12:00Z")
        );

        assert_eq!(registry.sessions.len(), 2, "{:?}", registry.sessions);
        let first = registry.sessions_on("01J").next().expect("a1b2");
        assert_eq!(first.record.session_id, "a1b2");
        assert_eq!(first.record.state, SessionState::Idle);
        assert_eq!(first.record.blocked_reason, None);
        assert!(first.record.daemon);
        assert_eq!(
            first.record.last_activity_at.as_deref(),
            Some("2026-11-12T16:00:12Z")
        );
        let stored = registry.sessions_on("02L").next().expect("c3d4");
        assert_eq!(stored.record.state, SessionState::Ended);
        assert_eq!(stored.record.model, "");
        assert!(stored.record.resume_argv.is_empty());
        assert!(!stored.record.daemon);

        assert_eq!(Registry::from_value(&json!({})), Registry::default());
    }
}
