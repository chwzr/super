//! Remote control glue: what the terminal and `--mode rpc` share to mirror a
//! session to the SUPER platform while the user is signed in.
//!
//! Nothing here runs unless [`wanted`] says so, which requires a stored
//! `super` credential. Signed out, `super` behaves as if this module did not
//! exist.

use crate::args::Args;
use fs2::FileExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use super_coding::Settings;
use super_coding::session_runner::AgentSession;
use super_remote::transport::{NoticeSink, TransportOptions};
use super_remote::{
    MachineIdentity, RegistryWriter, Remote, RemoteCredentials, RemoteNotice, RemoteStatus,
    SessionRecord, SessionState, SessionTransport,
};
use tokio::sync::{mpsc, watch};

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

/// What the owner of a [`RemoteControl`] hears about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteEvent {
    Started {
        machine: MachineIdentity,
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
    /// The newest state handed to [`publish`](Self::publish). Shared with the
    /// start task so a state published before the platform answered is sent
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
        let machine_path = MachineIdentity::default_path();
        let machine = match machine_path
            .as_deref()
            .map(MachineIdentity::load_or_create)
            .transpose()
        {
            Ok(Some(machine)) => machine,
            Ok(None) => MachineIdentity {
                machine_id: "local".into(),
                name: "local".into(),
                platform: super_remote::machine::platform_name().into(),
            },
            Err(error) => {
                let _ = events.send(RemoteEvent::Failed(format!("machine identity: {error:#}")));
                return Self::inert(false);
            }
        };
        let credentials: Arc<dyn RemoteCredentials> = Arc::new(
            super_remote::PlatformCredentials::new(platform_url(settings), machine.name.clone()),
        );
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
            let remote = match Remote::start(credentials, machine.clone()).await {
                Ok(remote) => remote,
                Err(error) => {
                    let _ = events.send(RemoteEvent::Failed(format!("{error:#}")));
                    return;
                }
            };
            let streams = remote.session_streams(&session_id);
            let transport =
                SessionTransport::start(remote.client.clone(), sdk, streams, options, notices);
            let registry = RegistryWriter::start(remote.registry.clone(), machine.clone());
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
            let _ = events.send(RemoteEvent::Started { machine, basin });
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

    pub fn machine(&self) -> Option<MachineIdentity> {
        self.live
            .lock()
            .unwrap()
            .as_ref()
            .map(|live| live.remote.machine.clone())
    }

    pub fn basin(&self) -> Option<String> {
        self.live
            .lock()
            .unwrap()
            .as_ref()
            .map(|live| live.remote.config.s2.basin.clone())
    }

    /// Announce the session's current shape. Cheap to call every frame:
    /// only changes are sent.
    pub fn publish(&self, record: SessionRecord) {
        if self.last_record.lock().unwrap().as_ref() == Some(&record) {
            return;
        }
        if let Some(live) = self.live.lock().unwrap().as_ref() {
            live.registry.publish(record.clone());
        }
        *self.last_record.lock().unwrap() = Some(record);
    }

    /// A handle that publishes session states without borrowing the control.
    pub fn publisher(&self) -> impl Fn(SessionRecord) + Send + Sync + 'static {
        let live = self.live.clone();
        move |record: SessionRecord| {
            if let Some(live) = live.lock().unwrap().as_ref() {
                live.registry.publish(record);
            }
        }
    }

    /// Flush and mark the session as ended. Bounded; never hangs an exit.
    pub async fn stop(self) {
        let live = self.live.lock().unwrap().take();
        if let Some(live) = live {
            live.registry.stop().await;
            live.transport.stop().await;
        }
    }
}

/// A session as the registry gets it, from the terminal's view of it.
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
    // The session's file changes with every entry, so its age is the
    // session's. A session with nothing said yet has no file, and the
    // platform takes the time it first hears of it.
    let last_activity_at = manager
        .session_file()
        .and_then(super_remote::registry::modified_at);
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
        daemon: false,
        last_activity_at,
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

    fn session(directory: &Path, saved: bool) -> Arc<AgentSession> {
        let registry = super_ai::Registry::from_builtin();
        let model = registry.all()[0].clone();
        let mut manager =
            super_coding::SessionManager::create(directory, Some(directory.join("sessions")))
                .unwrap();
        if saved {
            manager
                .append_message(super_agent::AgentMessage::user("Fix the login bug"))
                .unwrap();
        }
        AgentSession::new(
            manager,
            Vec::new(),
            registry,
            Settings::default(),
            "test".into(),
            model,
            super_ai::ThinkingLevel::Off,
            None,
            Arc::new(|_| {}),
        )
    }

    #[tokio::test]
    async fn a_terminal_session_was_last_active_when_its_file_was_last_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = Args::parse_from(["super"]);

        // Nothing said yet, so no file: the platform takes its own time.
        let fresh = session(&dir.path().join("fresh"), false);
        let record = session_record(SessionState::Idle, None, &fresh, &args, false);
        assert_eq!(record.last_activity_at, None);
        assert!(!record.daemon);

        // A session last used two days ago, opened again today.
        let saved = session(&dir.path().join("saved"), true);
        let file = saved
            .manager
            .lock()
            .unwrap()
            .session_file()
            .expect("a session file")
            .to_path_buf();
        let two_days_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .expect("open")
            .set_modified(two_days_ago)
            .expect("set the modification time");
        let record = session_record(SessionState::Idle, None, &saved, &args, false);
        assert_eq!(
            record.last_activity_at,
            Some(super_remote::registry::timestamp(two_days_ago))
        );
        assert_eq!(record.first_message.as_deref(), Some("Fix the login bug"));
        assert!(!record.daemon);
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
