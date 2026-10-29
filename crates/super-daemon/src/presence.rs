//! The daemon's presence on the SUPER platform: the registry, one mirror per
//! hosted session, and the host-level command and event streams.

use crate::host::Hosted;
use crate::log;
use crate::server::{ServerContext, run_host_command};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_remote::commands::{CommandCursor, is_expired, start_position};
use super_remote::s2::{
    AppendOptions, AppendRecord, ReadEvent, S2Client, S2Error, SequencedRecord,
};
use super_remote::transport::{NoticeSink, SessionStreams, TransportOptions, event_record};
use super_remote::{HostIdentity, RegistryWriter, RemoteStatus, SessionRecord, SessionTransport};
use super_sdk::daemon::{
    ClientLine, DaemonEvent, DaemonRequest, SessionState, SessionSummary, decode_client_line,
};
use super_sdk::protocol::Response;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The two host-level streams: `(commands, events)`.
pub fn host_streams(host_id: &str) -> (String, String) {
    (
        format!("hosts/{host_id}/commands"),
        format!("hosts/{host_id}/events"),
    )
}

#[derive(Debug, Clone)]
pub struct LinkConfig {
    /// Accept the RPC `bash` command from controllers.
    pub allow_shell: bool,
    /// Where command cursors live; `None` means `~/.super/agent/remote/cursors`.
    pub cursor_dir: Option<PathBuf>,
    /// Every record is sent again this often, for readers whose copy expired.
    pub republish_interval: Duration,
    pub command_lookback: u64,
    pub default_command_expiry: Duration,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            allow_shell: true,
            cursor_dir: None,
            republish_interval: Duration::from_secs(6 * 3600),
            command_lookback: 50,
            default_command_expiry: Duration::from_secs(5 * 60),
        }
    }
}

/// The registry record for a session as the daemon sees it.
pub fn record_from_summary(summary: &SessionSummary) -> SessionRecord {
    SessionRecord {
        session_id: summary.session_id.clone(),
        workspace: summary.workspace.clone(),
        cwd: summary.cwd.clone(),
        state: summary.state,
        blocked_reason: summary.blocked_reason.clone(),
        model: summary.model.clone().unwrap_or_default(),
        thinking_level: summary.thinking_level.clone().unwrap_or_default(),
        name: summary.name.clone(),
        first_message: summary.first_message.clone(),
        resume_argv: vec![
            "super".into(),
            "--session".into(),
            summary.session_file.clone(),
        ],
        headless: false,
        daemon: true,
    }
}

/// What every task of a link shares.
struct Shared {
    client: Arc<S2Client>,
    ctx: Arc<ServerContext>,
    host_id: String,
    config: LinkConfig,
    registry: RegistryWriter,
    transports: Mutex<BTreeMap<String, SessionTransport>>,
    /// The newest summary per session, so an ended session can be announced
    /// without listing the directory again.
    summaries: Mutex<BTreeMap<String, SessionSummary>>,
    notices: NoticeSink,
    out: mpsc::UnboundedSender<AppendRecord>,
}

impl Shared {
    fn publish(&self, summary: &SessionSummary) {
        self.summaries
            .lock()
            .unwrap()
            .insert(summary.session_id.clone(), summary.clone());
        self.registry.publish(record_from_summary(summary));
    }

    fn mirror(&self, hosted: &Arc<Hosted>) {
        let mut transports = self.transports.lock().unwrap();
        if transports.contains_key(&hosted.session_id) {
            return;
        }
        let streams = SessionStreams::new(&self.host_id, &hosted.session_id);
        let options = TransportOptions {
            allow_shell: self.config.allow_shell,
            cursor_dir: self.config.cursor_dir.clone(),
            ..TransportOptions::default()
        };
        let transport = SessionTransport::start(
            self.client.clone(),
            hosted.session.clone(),
            streams,
            options,
            self.notices.clone(),
        );
        transports.insert(hosted.session_id.clone(), transport);
        log::info(
            "presence",
            format!("mirroring session {}", hosted.session_id),
        );
    }

    fn unmirror(&self, session_id: &str) {
        let transport = self.transports.lock().unwrap().remove(session_id);
        if let Some(transport) = transport {
            log::info(
                "presence",
                format!("mirror of session {session_id} stopped"),
            );
            tokio::spawn(transport.stop());
        }
    }

    fn unauthorized_transport(&self) -> bool {
        self.transports
            .lock()
            .unwrap()
            .values()
            .any(|transport| transport.status() == RemoteStatus::Unauthorized)
    }

    /// Announce every session the daemon knows and mirror the hosted ones.
    async fn sync_all(&self) {
        for summary in self.ctx.host.list(None).await {
            if summary.hosted
                && let Some(hosted) = self.ctx.host.get(&summary.session_id)
            {
                self.mirror(&hosted);
            }
            self.publish(&summary);
        }
    }
}

/// One connection of the daemon to the platform.
pub struct Link {
    cancel: CancellationToken,
    shared: Arc<Shared>,
    unauthorized: Arc<AtomicBool>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Link {
    pub fn start(
        client: Arc<S2Client>,
        ctx: Arc<ServerContext>,
        identity: HostIdentity,
        device_id: String,
        config: LinkConfig,
    ) -> Self {
        let cancel = CancellationToken::new();
        let (commands_stream, events_stream) = host_streams(&identity.host_id);
        let (out, out_rx) = mpsc::unbounded_channel::<AppendRecord>();
        let unauthorized = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            client: client.clone(),
            ctx,
            host_id: identity.host_id.clone(),
            config,
            registry: RegistryWriter::start(client.clone(), identity, device_id),
            transports: Mutex::new(BTreeMap::new()),
            summaries: Mutex::new(BTreeMap::new()),
            notices: Arc::new(|notice| log::debug("presence", format!("{notice:?}"))),
            out,
        });
        let tasks = vec![
            tokio::spawn(append_host_events(
                client,
                events_stream,
                out_rx,
                cancel.clone(),
                unauthorized.clone(),
            )),
            tokio::spawn(follow_host(shared.clone(), cancel.clone())),
            tokio::spawn(tail_host_commands(
                shared.clone(),
                commands_stream,
                cancel.clone(),
                unauthorized.clone(),
            )),
        ];
        Self {
            cancel,
            shared,
            unauthorized,
            tasks: Mutex::new(tasks),
        }
    }

    /// The platform rejected the token; a new sign-in is needed.
    pub fn unauthorized(&self) -> bool {
        self.unauthorized.load(Ordering::Relaxed) || self.shared.unauthorized_transport()
    }

    /// Stop tailing, flush every mirror and the registry, announce the end.
    pub async fn stop(self) {
        self.cancel.cancel();
        let tasks: Vec<tokio::task::JoinHandle<()>> =
            std::mem::take(&mut *self.tasks.lock().unwrap());
        for task in tasks {
            task.abort();
            // An aborted task keeps its `Arc<Shared>` until the runtime drops
            // it; wait for that, or the registry below cannot be unwrapped
            // and its goodbye would race the process exit.
            let _ = task.await;
        }
        let transports: Vec<SessionTransport> = {
            let mut guard = self.shared.transports.lock().unwrap();
            std::mem::take(&mut *guard).into_values().collect()
        };
        for transport in transports {
            transport.stop().await;
        }
        match Arc::try_unwrap(self.shared) {
            Ok(shared) => shared.registry.stop().await,
            Err(_) => log::warn(
                "presence",
                "registry writer still in use at stop; the goodbye is not awaited",
            ),
        }
    }
}

/// Mirror the host's daemon events to `hosts/{id}/events`, keep the registry
/// current, and start or stop per-session mirrors as sessions load and unload.
async fn follow_host(shared: Arc<Shared>, cancel: CancellationToken) {
    let mut events = shared.ctx.host.subscribe();
    shared.sync_all().await;
    let mut republish = tokio::time::interval(shared.config.republish_interval);
    republish.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    republish.tick().await; // the first tick fires at once
    loop {
        let event = tokio::select! {
            _ = cancel.cancelled() => break,
            _ = republish.tick() => {
                shared.sync_all().await;
                shared.registry.refresh();
                continue;
            }
            event = events.recv() => event,
        };
        let event = match event {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                shared.sync_all().await;
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };
        if let Ok(value) = serde_json::to_value(&event) {
            let _ = shared.out.send(event_record(value));
        }
        match &event {
            DaemonEvent::SessionCreated { session } | DaemonEvent::SessionUpdated { session } => {
                if session.hosted
                    && let Some(hosted) = shared.ctx.host.get(&session.session_id)
                {
                    shared.mirror(&hosted);
                }
                shared.publish(session);
            }
            DaemonEvent::SessionEnded { session_id, .. } => {
                shared.unmirror(session_id);
                let ended = shared.summaries.lock().unwrap().get(session_id).cloned();
                if let Some(mut summary) = ended {
                    // Exactly what a listing of the stored session says, so
                    // the next resync has nothing new to send for it.
                    summary.state = SessionState::Ended;
                    summary.blocked_reason = None;
                    summary.hosted = false;
                    summary.attached_clients = 0;
                    summary.model = None;
                    summary.thinking_level = None;
                    shared.publish(&summary);
                }
            }
            DaemonEvent::DaemonShutdown { .. } => {}
        }
    }
}

/// Append host events in order; retry with backoff while the platform is
/// unreachable; note a rejected token.
async fn append_host_events(
    client: Arc<S2Client>,
    stream: String,
    mut queue: mpsc::UnboundedReceiver<AppendRecord>,
    cancel: CancellationToken,
    unauthorized: Arc<AtomicBool>,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let record = tokio::select! {
            _ = cancel.cancelled() => break,
            record = queue.recv() => match record {
                Some(record) => record,
                None => break,
            },
        };
        loop {
            match client
                .append(
                    &stream,
                    std::slice::from_ref(&record),
                    &AppendOptions::default(),
                )
                .await
            {
                Ok(_) => {
                    backoff = Duration::from_secs(1);
                    break;
                }
                Err(error) => {
                    if matches!(error, S2Error::Unauthorized) {
                        unauthorized.store(true, Ordering::Relaxed);
                    }
                    log::warn("presence", format!("host event append failed: {error}"));
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }
}

/// Run the commands controllers append to `hosts/{id}/commands`.
async fn tail_host_commands(
    shared: Arc<Shared>,
    stream: String,
    cancel: CancellationToken,
    unauthorized: Arc<AtomicBool>,
) {
    let cursor = CommandCursor::new(shared.config.cursor_dir.clone(), &stream);
    let start = start_position(
        &shared.client,
        &stream,
        &cursor,
        shared.config.command_lookback,
    )
    .await;
    let mut reader = shared
        .client
        .tail_stream(stream.clone(), start, cancel.clone());
    while let Some(event) = reader.recv().await {
        match event {
            ReadEvent::Connected => log::info("presence", format!("tailing {stream}")),
            ReadEvent::Ping { .. } | ReadEvent::Disconnected(_) => {}
            ReadEvent::Failed(reason) => {
                log::warn("presence", format!("host commands tail failed: {reason}"));
                if reason.contains("401") {
                    unauthorized.store(true, Ordering::Relaxed);
                }
                break;
            }
            ReadEvent::Records(records) => {
                for record in records {
                    let seq_num = record.seq_num;
                    if !is_expired(&record, shared.config.default_command_expiry) {
                        run_host_record(&shared, &record).await;
                    }
                    cursor.store(seq_num);
                }
            }
        }
    }
}

async fn run_host_record(shared: &Arc<Shared>, record: &SequencedRecord) {
    let origin = record
        .header("name")
        .or_else(|| record.header("from"))
        .unwrap_or("remote")
        .to_string();
    let response = match decode_client_line(&record.body) {
        // A controller correlates by id even when the type is unknown, so
        // the id is read from the raw line before giving up on it.
        Err(error) => Response::err("parse", error.to_string()).with_id(
            serde_json::from_str::<Value>(&record.body)
                .ok()
                .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_string)),
        ),
        Ok(ClientLine::Session(request)) => {
            Response::err(request.command.name(), "not available over the platform")
                .with_id(request.id)
        }
        Ok(ClientLine::Daemon(DaemonRequest { id, command })) => {
            let name = command.name();
            log::info("presence", format!("{origin}: {name}"));
            match run_host_command(&shared.ctx, command).await {
                Ok(data) => Response::ok_data(name, data).with_id(id),
                Err(error) => Response::err(name, error).with_id(id),
            }
        }
    };
    let value = serde_json::to_value(response).unwrap_or(Value::Null);
    let _ = shared.out.send(event_record(value));
}

#[derive(Debug, Clone)]
pub struct PresenceConfig {
    pub platform_url: String,
    pub device_name: String,
    /// How often the stored sign-in is checked.
    pub check_interval: Duration,
    pub link: LinkConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Keep,
}

/// What the supervisor does on one check.
pub fn next_action(signed_in: bool, running: bool, unauthorized: bool) -> Action {
    match (signed_in, running) {
        (true, false) => Action::Start,
        (true, true) if unauthorized => Action::Stop,
        (true, true) => Action::Keep,
        (false, true) => Action::Stop,
        (false, false) => Action::Keep,
    }
}

/// Keeps one link up while the user is signed in.
pub struct Presence {
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Presence {
    pub fn supervise(
        ctx: Arc<ServerContext>,
        identity: HostIdentity,
        config: PresenceConfig,
    ) -> Self {
        let cancel = CancellationToken::new();
        let task = tokio::spawn(supervise(ctx, identity, config, cancel.clone()));
        Self { cancel, task }
    }

    pub async fn stop(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(10), self.task).await;
    }
}

async fn supervise(
    ctx: Arc<ServerContext>,
    identity: HostIdentity,
    config: PresenceConfig,
    cancel: CancellationToken,
) {
    let mut link: Option<Link> = None;
    loop {
        let signed_in = super_remote::credentials::is_signed_in();
        let unauthorized = link.as_ref().is_some_and(Link::unauthorized);
        match next_action(signed_in, link.is_some(), unauthorized) {
            Action::Start => {
                let credentials: Arc<dyn super_remote::RemoteCredentials> =
                    Arc::new(super_remote::PlatformCredentials::new(
                        config.platform_url.clone(),
                        config.device_name.clone(),
                    ));
                match super_remote::Remote::start(credentials, identity.clone()).await {
                    Ok(remote) => {
                        log::info(
                            "presence",
                            format!(
                                "connected to {} as device {}",
                                config.platform_url, remote.config.device_id
                            ),
                        );
                        link = Some(Link::start(
                            remote.client.clone(),
                            ctx.clone(),
                            identity.clone(),
                            remote.config.device_id.clone(),
                            config.link.clone(),
                        ));
                    }
                    Err(error) => log::warn("presence", format!("platform unavailable: {error:#}")),
                }
            }
            Action::Stop => {
                log::info(
                    "presence",
                    if unauthorized {
                        "sign-in rejected, presence stopped"
                    } else {
                        "signed out, presence stopped"
                    },
                );
                if let Some(link) = link.take() {
                    link.stop().await;
                }
            }
            Action::Keep => {}
        }
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(config.check_interval) => {}
        }
    }
    if let Some(link) = link.take() {
        link.stop().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_supervisor_starts_when_signed_in_and_stops_when_not() {
        assert_eq!(next_action(true, false, false), Action::Start);
        assert_eq!(next_action(true, true, false), Action::Keep);
        assert_eq!(next_action(true, true, true), Action::Stop);
        assert_eq!(next_action(false, true, false), Action::Stop);
        assert_eq!(next_action(false, false, false), Action::Keep);
    }

    #[test]
    fn a_record_names_the_session_file_the_apps_expect() {
        let summary = SessionSummary {
            session_id: "a1".into(),
            cwd: "/w".into(),
            workspace: "/w".into(),
            session_file: "/s/a1.jsonl".into(),
            name: None,
            first_message: Some("hi".into()),
            state: SessionState::Idle,
            blocked_reason: None,
            hosted: true,
            attached_clients: 1,
            model: Some("mock/mock-1".into()),
            thinking_level: Some("off".into()),
            modified_at: "2026-10-29T00:00:00Z".into(),
        };
        let record = record_from_summary(&summary);
        assert_eq!(record.resume_argv, ["super", "--session", "/s/a1.jsonl"]);
        assert!(record.daemon);
        assert!(!record.headless);
        assert_eq!(record.model, "mock/mock-1");
    }
}
