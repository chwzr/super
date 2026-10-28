//! Remote control glue: what the terminal and `--mode rpc` share to mirror a
//! session to the SUPER platform while the user is signed in.
//!
//! Nothing here runs unless [`wanted`] says so, which requires a stored
//! `super` credential. Signed out, `super` behaves as if this module did not
//! exist.

use crate::args::Args;
use fs2::FileExt as _;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use super_coding::Settings;
use super_coding::session_runner::AgentSession;
use super_remote::transport::{NoticeSink, TransportOptions};
use super_remote::{
    HostIdentity, RegistryWriter, Remote, RemoteCredentials, RemoteNotice, RemoteStatus,
    SessionRecord, SessionState, SessionTransport,
};
use tokio::sync::{mpsc, watch};

pub const PROVIDER_ID: &str = super_ai::auth::super_platform::PROVIDER;

/// Is there a stored sign-in for the SUPER platform?
pub fn signed_in() -> bool {
    super_ai::auth::super_platform::is_signed_in()
}

/// Where the platform lives: `SUPER_PLATFORM_URL`, then the `platform.url`
/// setting, then the default.
pub fn platform_url(settings: &Settings) -> String {
    super_ai::auth::super_platform::resolve_base_url(
        settings.platform.url.as_deref(),
        std::env::var("SUPER_PLATFORM_URL").ok().as_deref(),
    )
}

/// Should this process mirror its session? Flags and settings can say no;
/// being signed out always says no.
pub fn wanted(args: &Args, settings: &Settings) -> bool {
    !args.no_remote && !args.no_session && settings.remote.is_enabled() && signed_in()
}

/// Credentials backed by the stored `super` OAuth credential. Token refresh
/// goes through the same path every provider uses.
struct PlatformCredentials {
    url: String,
    device_name: String,
}

impl RemoteCredentials for PlatformCredentials {
    fn platform_url(&self) -> String {
        self.url.clone()
    }

    fn device_name(&self) -> String {
        self.device_name.clone()
    }

    fn access_token(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + '_>> {
        Box::pin(async {
            super_ai::auth::resolve_api_key_async(PROVIDER_ID, &BTreeMap::new())
                .await?
                .ok_or_else(|| anyhow::anyhow!("not signed in to SUPER"))
        })
    }
}

/// What the owner of a [`RemoteControl`] hears about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteEvent {
    Started {
        host: HostIdentity,
        basin: String,
    },
    Notice(RemoteNotice),
    /// Could not start. The session keeps running locally.
    Failed(String),
    /// Another `super` process already mirrors this session file.
    Held,
}

struct Live {
    remote: Remote,
    transport: SessionTransport,
    registry: RegistryWriter,
}

/// One process's remote-control runtime for one session.
pub struct RemoteControl {
    live: Arc<Mutex<Option<Live>>>,
    status: Arc<Mutex<Option<watch::Receiver<RemoteStatus>>>>,
    _lock: Option<std::fs::File>,
    held: bool,
    starting: bool,
    /// The newest record handed to [`publish`](Self::publish). Shared with the
    /// start task so a record published before the platform answered is sent
    /// as soon as the registry writer exists.
    last_record: Arc<Mutex<Option<SessionRecord>>>,
}

impl RemoteControl {
    /// Start mirroring. Returns immediately; the network work happens on a
    /// task and reports through `events`.
    pub fn start(
        sdk: Arc<super_sdk::Session>,
        session_file: Option<PathBuf>,
        settings: &Settings,
        events: mpsc::UnboundedSender<RemoteEvent>,
    ) -> Self {
        let lock = match session_file.as_deref().map(acquire_session_lock) {
            Some(Ok(file)) => Some(file),
            Some(Err(LockError::Held)) => {
                let _ = events.send(RemoteEvent::Held);
                return Self::inert(true);
            }
            Some(Err(LockError::Io(error))) => {
                let _ = events.send(RemoteEvent::Failed(format!("session lock: {error}")));
                return Self::inert(false);
            }
            None => None,
        };
        let host_path = HostIdentity::default_path();
        let host = match host_path
            .as_deref()
            .map(HostIdentity::load_or_create)
            .transpose()
        {
            Ok(Some(host)) => host,
            Ok(None) => HostIdentity {
                host_id: "local".into(),
                name: "local".into(),
                platform: super_remote::host::platform_name().into(),
            },
            Err(error) => {
                let _ = events.send(RemoteEvent::Failed(format!("host identity: {error:#}")));
                return Self::inert(false);
            }
        };
        let credentials: Arc<dyn RemoteCredentials> = Arc::new(PlatformCredentials {
            url: platform_url(settings),
            device_name: host.name.clone(),
        });
        let options = TransportOptions {
            allow_shell: settings.remote.shell_allowed(),
            ..TransportOptions::default()
        };
        let live: Arc<Mutex<Option<Live>>> = Arc::new(Mutex::new(None));
        let status: Arc<Mutex<Option<watch::Receiver<RemoteStatus>>>> = Arc::new(Mutex::new(None));
        let last_record: Arc<Mutex<Option<SessionRecord>>> = Arc::new(Mutex::new(None));
        let control = Self {
            live: live.clone(),
            status: status.clone(),
            _lock: lock,
            held: false,
            starting: true,
            last_record: last_record.clone(),
        };
        let session_id = sdk.state().session_id;
        let notices: NoticeSink = {
            let events = events.clone();
            Arc::new(move |notice| {
                let _ = events.send(RemoteEvent::Notice(notice));
            })
        };
        tokio::spawn(async move {
            let remote = match Remote::start(credentials, host.clone()).await {
                Ok(remote) => remote,
                Err(error) => {
                    let _ = events.send(RemoteEvent::Failed(format!("{error:#}")));
                    return;
                }
            };
            let streams = remote.session_streams(&session_id);
            let transport =
                SessionTransport::start(remote.client.clone(), sdk, streams, options, notices);
            let registry = RegistryWriter::start(
                remote.client.clone(),
                host.clone(),
                remote.config.device_id.clone(),
            );
            *status.lock().unwrap() = Some(transport.status_watch());
            let basin = remote.config.s2.basin.clone();
            // Anything published while the platform was still answering.
            if let Some(record) = last_record.lock().unwrap().clone() {
                registry.publish(record);
            }
            *live.lock().unwrap() = Some(Live {
                remote,
                transport,
                registry,
            });
            let _ = events.send(RemoteEvent::Started { host, basin });
        });
        control
    }

    fn inert(held: bool) -> Self {
        Self {
            live: Arc::new(Mutex::new(None)),
            status: Arc::new(Mutex::new(None)),
            _lock: None,
            held,
            starting: false,
            last_record: Arc::new(Mutex::new(None)),
        }
    }

    /// Another process holds this session's mirror.
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// The transport state, or `None` while starting or after a failure.
    pub fn status(&self) -> Option<RemoteStatus> {
        let guard = self.status.lock().unwrap();
        guard.as_ref().map(|watch| *watch.borrow())
    }

    pub fn is_starting(&self) -> bool {
        self.starting && self.live.lock().unwrap().is_none()
    }

    pub fn host(&self) -> Option<HostIdentity> {
        self.live
            .lock()
            .unwrap()
            .as_ref()
            .map(|live| live.remote.host.clone())
    }

    pub fn basin(&self) -> Option<String> {
        self.live
            .lock()
            .unwrap()
            .as_ref()
            .map(|live| live.remote.config.s2.basin.clone())
    }

    /// Announce the session's current shape. Cheap to call every frame: the
    /// registry only sends changes.
    pub fn publish(&self, record: SessionRecord) {
        if self.last_record.lock().unwrap().as_ref() == Some(&record) {
            return;
        }
        if let Some(live) = self.live.lock().unwrap().as_ref() {
            live.registry.publish(record.clone());
        }
        *self.last_record.lock().unwrap() = Some(record);
    }

    /// A handle that publishes registry records without borrowing the control.
    pub fn publisher(&self) -> impl Fn(SessionRecord) + Send + Sync + 'static {
        let live = self.live.clone();
        move |record: SessionRecord| {
            if let Some(live) = live.lock().unwrap().as_ref() {
                live.registry.publish(record);
            }
        }
    }

    /// Flush and announce the end. Bounded; never hangs an exit.
    pub async fn stop(self) {
        let live = self.live.lock().unwrap().take();
        if let Some(live) = live {
            live.registry.stop().await;
            live.transport.stop().await;
        }
    }
}

/// Build the registry record for a session from the terminal's view of it.
pub fn session_record(
    state: SessionState,
    blocked_reason: Option<&str>,
    session: &AgentSession,
    args: &Args,
    headless: bool,
) -> SessionRecord {
    let model = session.model();
    let thinking = session.thinking_level();
    let manager = session.manager.lock().unwrap();
    let cwd = manager.cwd().display().to_string();
    let resume_argv = manager
        .session_file()
        .filter(|_| !manager.entries().is_empty())
        .and_then(|file| std::path::absolute(file).ok())
        .map(|file| {
            crate::terminal_hosts::resume_argv(&file.display().to_string(), &model, thinking, args)
        })
        .unwrap_or_default();
    let first_message = manager.entries().iter().find_map(|entry| match entry {
        super_coding::SessionEntry::Message {
            message: super_agent::AgentMessage::User(user),
            ..
        } => {
            let text = user.content.as_text();
            let text = text.trim();
            (!text.is_empty()).then(|| text.chars().take(200).collect::<String>())
        }
        _ => None,
    });
    SessionRecord {
        session_id: manager.session_id().to_string(),
        workspace: cwd.clone(),
        cwd,
        state,
        blocked_reason: blocked_reason.map(str::to_string),
        model: format!("{}/{}", model.provider, model.id),
        thinking_level: thinking.as_str().to_string(),
        name: manager.session_name(),
        first_message,
        resume_argv,
        headless,
    }
}

enum LockError {
    Held,
    Io(std::io::Error),
}

/// One mirror per session file. The lock lives next to the session so a
/// second `super` resuming the same file notices.
fn acquire_session_lock(session_file: &Path) -> Result<std::fs::File, LockError> {
    let path = session_file.with_extension("jsonl.remote.lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(LockError::Io)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(LockError::Io)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Err(LockError::Held),
        Err(error) => {
            // fs2 reports contention through its own error, not WouldBlock.
            if fs2::lock_contended_error().kind() == error.kind()
                && fs2::lock_contended_error().raw_os_error() == error.raw_os_error()
            {
                Err(LockError::Held)
            } else {
                Err(LockError::Io(error))
            }
        }
    }
}

/// The one-time hint after the first signed-in session. Returns whether the
/// hint should be shown now and records that it was.
pub fn first_run_notice_due() -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let marker = home.join(".super/agent/remote-notice-shown");
    if marker.exists() {
        return false;
    }
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&marker, b"1\n").is_ok()
}

/// One line for the footer.
pub fn footer_label(status: Option<RemoteStatus>, held: bool, starting: bool) -> &'static str {
    if held {
        return "♦ local";
    }
    match status {
        Some(RemoteStatus::Connected) => "♦ remote",
        Some(RemoteStatus::Reconnecting) | Some(RemoteStatus::Starting) => "♦ remote…",
        Some(RemoteStatus::Unauthorized) | Some(RemoteStatus::Stopped) => "♦ local",
        None if starting => "♦ remote…",
        None => "♦ local",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn signed_out_or_opted_out_means_no_remote() {
        let settings = Settings::default();
        // The plain invocation follows the stored sign-in of this machine.
        let args = Args::parse_from(["super"]);
        assert_eq!(wanted(&args, &settings), signed_in());
        let args = Args::parse_from(["super", "--no-remote"]);
        assert!(!wanted(&args, &settings));
        let mut off = Settings::default();
        off.remote.enabled = Some(false);
        let args = Args::parse_from(["super"]);
        assert!(!wanted(&args, &off));
    }

    #[test]
    fn the_platform_url_comes_from_settings_before_the_default() {
        let mut custom = Settings::default();
        custom.platform.url = Some("http://127.0.0.1:8080/".into());
        assert_eq!(platform_url(&custom), "http://127.0.0.1:8080");
    }

    #[test]
    fn a_second_process_cannot_take_the_session_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session = dir.path().join("2026_abc.jsonl");
        let first = acquire_session_lock(&session);
        assert!(first.is_ok());
        assert!(matches!(
            acquire_session_lock(&session),
            Err(LockError::Held)
        ));
        drop(first);
        assert!(acquire_session_lock(&session).is_ok());
    }

    #[test]
    fn footer_labels_follow_the_status() {
        assert_eq!(
            footer_label(Some(RemoteStatus::Connected), false, false),
            "♦ remote"
        );
        assert_eq!(
            footer_label(Some(RemoteStatus::Reconnecting), false, false),
            "♦ remote…"
        );
        assert_eq!(footer_label(None, true, false), "♦ local");
        assert_eq!(footer_label(None, false, true), "♦ remote…");
        assert_eq!(
            footer_label(Some(RemoteStatus::Unauthorized), false, false),
            "♦ local"
        );
    }
}
