//! `/remote` (alias `/rc`): hand the session a standalone terminal runs
//! in-process to the SUPER daemon, so the apps see it and it outlives the
//! terminal.
//!
//! The steps, in order: sign in to the platform when signed out; make a
//! daemon available on this device; wait until the running turn is over;
//! then the daemon resumes the session from its file and the terminal
//! attaches to it. A failure at any step leaves the in-process session as it
//! was. The decisions are pure functions with tests; the terminal glue is
//! `modes/interactive/remote_handover.rs`, the attached front end
//! [`crate::attach`].

use std::path::{Path, PathBuf};

/// What the terminal knows when `/remote` runs or a step finishes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    /// This terminal is a client of the daemon already.
    pub attached: bool,
    /// `--no-session`: the session has no file the daemon could load.
    pub ephemeral: bool,
    /// `remote.enabled` is not false.
    pub remote_enabled: bool,
    pub signed_in: bool,
    /// A daemon answered and the connection waits.
    pub daemon_ready: bool,
    /// A turn, or anything else that writes the session, is running, or a
    /// question is open on screen.
    pub busy: bool,
}

/// What `/remote` does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// Attached already: report the status.
    ReportStatus,
    /// Nothing can be handed over; the reason.
    Refuse(&'static str),
    SignIn,
    ProvideDaemon,
    /// The daemon is ready; the turn is not over yet.
    WaitForTurn,
    HandOver,
}

pub(crate) const EPHEMERAL: &str =
    "this session is not saved (--no-session), so the SUPER daemon has nothing to resume";
pub(crate) const REMOTE_DISABLED: &str = "remote.enabled is false in the settings, so the apps would not see the session; set it to true first";

pub(crate) fn next_step(facts: Facts) -> Step {
    if facts.attached {
        Step::ReportStatus
    } else if facts.ephemeral {
        Step::Refuse(EPHEMERAL)
    } else if !facts.remote_enabled {
        Step::Refuse(REMOTE_DISABLED)
    } else if !facts.signed_in {
        Step::SignIn
    } else if !facts.daemon_ready {
        Step::ProvideDaemon
    } else if facts.busy {
        Step::WaitForTurn
    } else {
        Step::HandOver
    }
}

/// The operating system, as far as providing a daemon goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum System {
    Linux,
    MacOs,
    Other,
}

impl System {
    pub fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }
}

/// How a daemon becomes available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DaemonRoute {
    /// A daemon answers on the socket.
    Connect,
    /// launchd or systemd has the daemon registered: start it there.
    StartRegistered,
    /// Linux with nothing registered: `super install`.
    Install,
    /// macOS without the SUPER app's agent: the app provides the daemon.
    NeedsApp,
    /// `SUPER_DAEMON_SOCKET` names a socket nothing listens on; only the
    /// default socket is ever started.
    NothingAtSocket,
    /// No daemon on this system.
    Unsupported,
}

pub(crate) fn daemon_route(
    system: System,
    answers: bool,
    default_socket: bool,
    registered: bool,
) -> DaemonRoute {
    if answers {
        return DaemonRoute::Connect;
    }
    if !default_socket {
        return DaemonRoute::NothingAtSocket;
    }
    match system {
        System::Linux | System::MacOs if registered => DaemonRoute::StartRegistered,
        System::Linux => DaemonRoute::Install,
        System::MacOs => DaemonRoute::NeedsApp,
        System::Other => DaemonRoute::Unsupported,
    }
}

pub(crate) const NEEDS_APP: &str = "on macOS the SUPER daemon comes with the SUPER app: install and open SUPER.app, which starts the daemon at login, then run /remote again";
pub(crate) const UNSUPPORTED: &str =
    "the SUPER daemon needs a Unix socket and is not available on this system";

/// Whether a failed `super install` leaves starting a daemon by hand as the
/// way to go: there is no systemd user manager to install into.
pub(crate) fn start_by_hand_after(error: &super_daemon::service::ServiceError) -> bool {
    use super_daemon::service::ServiceError;
    matches!(
        error,
        ServiceError::NoSystemctl | ServiceError::NoUserManager { .. }
    )
}

/// The session the terminal runs, as far as handing it over goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Standalone {
    pub session_id: String,
    /// `None` for `--no-session`.
    pub file: Option<PathBuf>,
    pub cwd: PathBuf,
    pub provider: String,
    pub model: String,
    pub thinking_level: String,
}

/// What the daemon is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// Load the session from its file.
    Resume { session_id: String, file: PathBuf },
    /// Nothing is written yet, so there is no file: start a session in the
    /// same directory with the same model.
    Create {
        cwd: PathBuf,
        provider: String,
        model: String,
        thinking_level: String,
    },
}

pub(crate) fn target(standalone: &Standalone) -> Result<Target, &'static str> {
    match &standalone.file {
        None => Err(EPHEMERAL),
        Some(file) if file.exists() => Ok(Target::Resume {
            session_id: standalone.session_id.clone(),
            file: file.clone(),
        }),
        Some(_) => Ok(Target::Create {
            cwd: standalone.cwd.clone(),
            provider: standalone.provider.clone(),
            model: standalone.model.clone(),
            thinking_level: standalone.thinking_level.clone(),
        }),
    }
}

/// The daemon finds a session by its id under its own session directory, so
/// a file elsewhere (`--session-dir`, a project's `sessionDir`) cannot move.
pub(crate) fn outside_session_dir(file: &Path, daemon_dir: &Path) -> Option<String> {
    let canonical =
        |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if canonical(file).starts_with(canonical(daemon_dir)) {
        None
    } else {
        Some(format!(
            "the session file is in {}, but the SUPER daemon keeps sessions in {}",
            file.parent().unwrap_or(file).display(),
            daemon_dir.display()
        ))
    }
}

/// The confirmation the terminal shows once the daemon has the session.
pub(crate) fn confirmation(session_id: &str, daemon_pid: u32, signed_in: bool) -> String {
    let apps = if signed_in {
        " and shows up in the apps"
    } else {
        ""
    };
    format!(
        "♦ session {} now runs in the SUPER daemon (pid {daemon_pid}){apps}; this terminal is attached to it · /detach leaves it running",
        short_id(session_id)
    )
}

/// What `/remote` says in a terminal that is attached already.
pub(crate) fn status_line(
    session_id: &str,
    daemon_version: &str,
    host_name: &str,
    daemon_pid: u32,
    signed_in: bool,
) -> String {
    let apps = if signed_in {
        "the apps see it"
    } else {
        "not signed in to SUPER, so the apps do not see it: `super login super` signs in"
    };
    format!(
        "♦ attached to session {} in the SUPER daemon {daemon_version} on {host_name} (pid {daemon_pid}); {apps}",
        short_id(session_id)
    )
}

fn short_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}

#[cfg(unix)]
pub(crate) use daemon::*;

#[cfg(unix)]
mod daemon {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use super_coding::session_runner::AgentSession;
    use super_daemon::client::{
        Attached, ClientError, ClientEvent, ClientInfo, DaemonClient, NewSession,
    };
    use super_daemon::paths::DaemonPaths;
    use super_daemon::session_lock::SessionLockKeeper;
    use super_daemon::supervisor;
    use tokio::sync::mpsc::UnboundedReceiver;

    /// How long a daemon that `super install` or the fallback started may
    /// take to answer. It reads the login shell's PATH before it binds.
    const ANSWER_WAIT: Duration = Duration::from_secs(10);

    /// A connection to a daemon, waiting for the turn to end.
    pub(crate) struct Ready {
        pub client: DaemonClient,
        pub events: UnboundedReceiver<ClientEvent>,
    }

    /// How the terminal introduces itself to the daemon.
    pub(crate) fn client_info() -> ClientInfo {
        ClientInfo::new(super_sdk::daemon::ClientKind::Tui, "Terminal")
    }

    /// Connect to the daemon, starting or installing it the way this system
    /// allows. `note` hears about anything that changed the system: a unit
    /// written, a daemon started.
    pub(crate) async fn provide_daemon(
        info: &ClientInfo,
        note: &(dyn Fn(String) + Send + Sync),
    ) -> Result<Ready, String> {
        let paths = DaemonPaths::discover().ok_or("no home directory")?;
        let client = match DaemonClient::connect_to(&paths.socket, info).await {
            Ok(client) => client,
            Err(ClientError::NotRunning { .. }) => start_daemon(&paths, info, note).await?,
            Err(error) => return Err(error.to_string()),
        };
        Ok(ready(client))
    }

    fn ready(mut client: DaemonClient) -> Ready {
        let (tx, empty) = tokio::sync::mpsc::unbounded_channel();
        drop(tx);
        let events = client.take_events().unwrap_or(empty);
        Ready { client, events }
    }

    async fn start_daemon(
        paths: &DaemonPaths,
        info: &ClientInfo,
        note: &(dyn Fn(String) + Send + Sync),
    ) -> Result<DaemonClient, String> {
        let default_socket = paths.is_default();
        let service = if default_socket {
            supervisor::platform_service()
        } else {
            None
        };
        // `launchctl print` and `systemctl --user cat` are quick, but they
        // are processes: keep them off the async workers.
        let (service, registered) = tokio::task::spawn_blocking(move || {
            let registered = service.as_ref().is_some_and(|s| s.is_registered());
            (service, registered)
        })
        .await
        .map_err(|error| error.to_string())?;
        match daemon_route(System::current(), false, default_socket, registered) {
            DaemonRoute::Connect => DaemonClient::connect_to(&paths.socket, info)
                .await
                .map_err(|error| error.to_string()),
            DaemonRoute::StartRegistered => {
                let client = DaemonClient::connect_or_start_with(
                    &paths.socket,
                    info,
                    service.as_deref(),
                    super_daemon::client::START_WAIT,
                )
                .await
                .map_err(|error| error.to_string())?;
                if let Some(service) = &service {
                    note(format!(
                        "started the SUPER daemon through the {}",
                        service.name()
                    ));
                }
                Ok(client)
            }
            DaemonRoute::Install => install_or_start(paths, info, note).await,
            DaemonRoute::NeedsApp => Err(NEEDS_APP.into()),
            DaemonRoute::NothingAtSocket => Err(format!(
                "no SUPER daemon answers on {} (SUPER_DAEMON_SOCKET); start one there with `super daemon run --socket {}`",
                paths.socket.display(),
                paths.socket.display()
            )),
            DaemonRoute::Unsupported => Err(UNSUPPORTED.into()),
        }
    }

    /// Linux: what `super install` does, or, without a systemd user
    /// manager, a daemon started in the background.
    async fn install_or_start(
        paths: &DaemonPaths,
        info: &ClientInfo,
        note: &(dyn Fn(String) + Send + Sync),
    ) -> Result<DaemonClient, String> {
        use super_daemon::service::{self, ServiceEnv, UserSystemctl};
        let exe = std::env::current_exe()
            .map_err(|error| format!("locate the running super binary: {error}"))?;
        let env = ServiceEnv::current().ok_or("no home directory")?;
        let installed = tokio::task::spawn_blocking(move || {
            let exe = service::service_exe(&exe, std::env::var_os("PATH").as_deref());
            service::install(&env, &exe, &mut UserSystemctl)
        })
        .await
        .map_err(|error| error.to_string())?;
        match installed {
            Ok(installed) => {
                note(format!(
                    "installed the SUPER daemon as a systemd user service ({}); it starts at login from now on, `super uninstall` removes it",
                    installed.unit_path.display()
                ));
                connect_within(&paths.socket, info, ANSWER_WAIT).await
            }
            Err(error) if start_by_hand_after(&error) => {
                let pid = spawn_daemon().map_err(|spawn| {
                    format!("{error}; starting a daemon in the background failed too: {spawn}")
                })?;
                note(format!(
                    "started a SUPER daemon in the background (pid {pid}): there is no systemd user manager to install it into, so it runs until `super daemon stop` or a restart"
                ));
                connect_within(&paths.socket, info, ANSWER_WAIT).await
            }
            Err(error) => Err(error.to_string()),
        }
    }

    /// `super daemon run` in its own session, so closing the terminal does
    /// not take it down.
    fn spawn_daemon() -> std::io::Result<u32> {
        use std::os::unix::process::CommandExt as _;
        use std::process::Stdio;
        let exe = std::env::current_exe()?;
        let mut command = std::process::Command::new(exe);
        command
            .args(["daemon", "run"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: setsid is async-signal-safe and touches nothing shared.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let pid = child.id();
        // Reap it whenever it exits, so it never lingers as a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(pid)
    }

    async fn connect_within(
        socket: &Path,
        info: &ClientInfo,
        limit: Duration,
    ) -> Result<DaemonClient, String> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            match DaemonClient::connect_to(socket, info).await {
                Err(ClientError::NotRunning { .. }) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Err(ClientError::NotRunning { .. }) => {
                    return Err(format!(
                        "the SUPER daemon did not answer on {} within {} seconds; ~/.super/agent/logs/daemon.log says why",
                        socket.display(),
                        limit.as_secs()
                    ));
                }
                other => return other.map_err(|error| error.to_string()),
            }
        }
    }

    /// The session as the terminal runs it.
    pub(crate) fn standalone(session: &AgentSession) -> Standalone {
        let model = session.model();
        let manager = session.manager.lock().unwrap();
        Standalone {
            session_id: manager.session_id().to_string(),
            file: manager.session_file().map(Path::to_path_buf),
            cwd: manager.cwd().to_path_buf(),
            provider: model.provider.clone(),
            model: model.id.clone(),
            thinking_level: session.thinking_level().as_str().to_string(),
        }
    }

    /// Everything that can be checked before anything changes hands.
    pub(crate) async fn prepare(
        client: &DaemonClient,
        session: &AgentSession,
    ) -> Result<Target, String> {
        let target = target(&standalone(session)).map_err(str::to_string)?;
        if let Target::Resume { file, .. } = &target {
            let status = client.status().await.map_err(|error| error.to_string())?;
            if let Some(problem) = outside_session_dir(file, Path::new(&status.session_dir)) {
                return Err(problem);
            }
        }
        Ok(target)
    }

    /// Let go of the session file and have the daemon load it, then attach.
    /// On any failure the daemon lets go of the file again and this process
    /// takes its lock back, so the in-process session carries on.
    pub(crate) async fn hand_over(
        client: &DaemonClient,
        target: &Target,
        lock: &mut Option<SessionLockKeeper>,
        session: &Arc<AgentSession>,
    ) -> Result<Attached, String> {
        match target {
            Target::Create {
                cwd,
                provider,
                model,
                thinking_level,
            } => {
                let created = client
                    .create_session(NewSession {
                        cwd: cwd.clone(),
                        model: Some(model.clone()),
                        provider: Some(provider.clone()),
                        thinking_level: Some(thinking_level.clone()),
                        name: None,
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                match client.attach(&created.session_id).await {
                    Ok(attached) => Ok(attached),
                    Err(error) => {
                        let _ = client.stop_session(&created.session_id).await;
                        Err(error.to_string())
                    }
                }
            }
            Target::Resume { session_id, .. } => {
                drop(lock.take());
                let moved = match client.resume_session(session_id).await {
                    Ok(_) => client.attach(session_id).await,
                    Err(error) => Err(error),
                };
                match moved {
                    Ok(attached) => Ok(attached),
                    Err(error) => {
                        // The daemon may have loaded it before `attach` failed.
                        let _ = client.stop_session(session_id).await;
                        match crate::setup::keep_session_lock(session) {
                            Ok(keeper) => *lock = Some(keeper),
                            Err(relock) => {
                                return Err(format!(
                                    "{error}; the session file could not be locked again: {relock:#}"
                                ));
                            }
                        }
                        Err(error.to_string())
                    }
                }
            }
        }
    }

    /// `/remote` in a terminal attached already.
    pub(crate) fn attached_status(client: &DaemonClient, session_id: &str) -> String {
        let hello = client.hello();
        status_line(
            session_id,
            &hello.daemon_version,
            &hello.host_name,
            hello.pid,
            crate::remote::signed_in(),
        )
    }

    /// Reads what the terminal's stdin thread delivers, so the attached front
    /// end continues on the same input the full-screen terminal used. A
    /// second reader of stdin would swallow the first line typed.
    pub(crate) struct ChannelReader {
        rx: UnboundedReceiver<Vec<u8>>,
        pending: Vec<u8>,
        offset: usize,
    }

    impl ChannelReader {
        pub(crate) fn new(rx: UnboundedReceiver<Vec<u8>>) -> Self {
            Self {
                rx,
                pending: Vec::new(),
                offset: 0,
            }
        }
    }

    impl tokio::io::AsyncRead for ChannelReader {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            use std::task::Poll;
            while self.offset >= self.pending.len() {
                match self.rx.poll_recv(cx) {
                    Poll::Ready(Some(bytes)) => {
                        self.pending = bytes;
                        self.offset = 0;
                    }
                    // The sender is gone: end of input.
                    Poll::Ready(None) => return Poll::Ready(Ok(())),
                    Poll::Pending => return Poll::Pending,
                }
            }
            let available = &self.pending[self.offset..];
            let count = available.len().min(buf.remaining());
            buf.put_slice(&available[..count]);
            self.offset += count;
            Poll::Ready(Ok(()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            attached: false,
            ephemeral: false,
            remote_enabled: true,
            signed_in: true,
            daemon_ready: true,
            busy: false,
        }
    }

    #[test]
    fn the_steps_run_in_order_sign_in_daemon_turn_hand_over() {
        let signed_out = Facts {
            signed_in: false,
            daemon_ready: false,
            busy: true,
            ..facts()
        };
        assert_eq!(next_step(signed_out), Step::SignIn);
        let no_daemon = Facts {
            daemon_ready: false,
            busy: true,
            ..facts()
        };
        assert_eq!(next_step(no_daemon), Step::ProvideDaemon);
        let mid_turn = Facts {
            busy: true,
            ..facts()
        };
        assert_eq!(next_step(mid_turn), Step::WaitForTurn);
        assert_eq!(next_step(facts()), Step::HandOver);
    }

    #[test]
    fn attached_reports_and_unsaved_or_hidden_sessions_are_refused() {
        let attached = Facts {
            attached: true,
            signed_in: false,
            ephemeral: true,
            ..facts()
        };
        assert_eq!(next_step(attached), Step::ReportStatus);
        let ephemeral = Facts {
            ephemeral: true,
            signed_in: false,
            ..facts()
        };
        assert_eq!(next_step(ephemeral), Step::Refuse(EPHEMERAL));
        let hidden = Facts {
            remote_enabled: false,
            signed_in: false,
            ..facts()
        };
        assert_eq!(next_step(hidden), Step::Refuse(REMOTE_DISABLED));
    }

    #[test]
    fn a_daemon_is_connected_started_installed_or_explained() {
        use DaemonRoute::*;
        for system in [System::Linux, System::MacOs, System::Other] {
            assert_eq!(daemon_route(system, true, true, false), Connect);
            assert_eq!(daemon_route(system, true, false, false), Connect);
            assert_eq!(
                daemon_route(system, false, false, true),
                NothingAtSocket,
                "a custom socket is never started"
            );
        }
        assert_eq!(
            daemon_route(System::Linux, false, true, true),
            StartRegistered
        );
        assert_eq!(
            daemon_route(System::MacOs, false, true, true),
            StartRegistered
        );
        assert_eq!(daemon_route(System::Linux, false, true, false), Install);
        assert_eq!(daemon_route(System::MacOs, false, true, false), NeedsApp);
        assert_eq!(daemon_route(System::Other, false, true, true), Unsupported);
        assert_eq!(daemon_route(System::Other, false, true, false), Unsupported);
    }

    #[test]
    fn without_a_systemd_user_manager_a_daemon_is_started_by_hand() {
        use super_daemon::service::{Action, Platform, ServiceError};
        assert!(start_by_hand_after(&ServiceError::NoSystemctl));
        assert!(start_by_hand_after(&ServiceError::NoUserManager {
            detail: "Failed to connect to bus".into(),
            user: "felix".into(),
        }));
        assert!(!start_by_hand_after(&ServiceError::Unsupported {
            platform: Platform::MacOs,
            action: Action::Install,
        }));
        assert!(!start_by_hand_after(&ServiceError::Systemctl {
            command: "enable --now super-daemon.service".into(),
            detail: "unit failed".into(),
        }));
    }

    fn standalone(file: Option<PathBuf>) -> Standalone {
        Standalone {
            session_id: "a1b2c3d4-0000".into(),
            file,
            cwd: PathBuf::from("/w"),
            provider: "mock".into(),
            model: "mock-1".into(),
            thinking_level: "off".into(),
        }
    }

    #[test]
    fn a_saved_session_is_resumed_and_an_unwritten_one_is_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("2026_a1b2c3d4-0000.jsonl");
        assert_eq!(target(&standalone(None)), Err(EPHEMERAL));
        assert_eq!(
            target(&standalone(Some(file.clone()))),
            Ok(Target::Create {
                cwd: PathBuf::from("/w"),
                provider: "mock".into(),
                model: "mock-1".into(),
                thinking_level: "off".into(),
            }),
            "no entry written yet means no file to load"
        );
        std::fs::write(&file, "{}\n").unwrap();
        assert_eq!(
            target(&standalone(Some(file.clone()))),
            Ok(Target::Resume {
                session_id: "a1b2c3d4-0000".into(),
                file,
            })
        );
    }

    #[test]
    fn only_files_under_the_daemons_session_dir_can_move() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        let project = sessions.join("--w--");
        std::fs::create_dir_all(&project).unwrap();
        let inside = project.join("s_a1b2.jsonl");
        std::fs::write(&inside, "{}\n").unwrap();
        assert_eq!(outside_session_dir(&inside, &sessions), None);
        let elsewhere = dir.path().join("other/s_a1b2.jsonl");
        let problem = outside_session_dir(&elsewhere, &sessions).expect("refused");
        assert!(problem.contains("keeps sessions in"), "{problem}");
    }

    #[test]
    fn the_confirmation_and_status_are_one_short_line() {
        let line = confirmation("a1b2c3d4-e5f6", 4242, true);
        assert_eq!(
            line,
            "♦ session a1b2c3d4 now runs in the SUPER daemon (pid 4242) and shows up in the apps; this terminal is attached to it · /detach leaves it running"
        );
        assert!(!confirmation("a1b2c3d4", 1, false).contains("apps"));
        assert_eq!(
            status_line("a1b2c3d4-e5f6", "0.0.41", "mbp", 4242, true),
            "♦ attached to session a1b2c3d4 in the SUPER daemon 0.0.41 on mbp (pid 4242); the apps see it"
        );
        assert!(status_line("a1b2", "0.0.41", "mbp", 1, false).contains("super login super"));
    }
}

/// A real daemon on a temporary socket, the mock provider, and a session
/// that a standalone process started: what `/remote` does after the
/// terminal decided to hand over.
#[cfg(all(test, unix))]
mod end_to_end {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use super_daemon::DaemonConfig;
    use super_daemon::client::{ClientEvent, DaemonClient};
    use super_daemon::host::HostConfig;
    use super_daemon::paths::DaemonPaths;
    use super_daemon::session_lock;
    use super_sdk::mock::{MockProvider, MockScript};
    use super_sdk::protocol::Command;
    use super_sdk::{SessionOptions, SessionSource};
    use tokio_util::sync::CancellationToken;

    struct Fixture {
        provider: MockProvider,
        dir: tempfile::TempDir,
        socket: PathBuf,
        shutdown: CancellationToken,
        daemon: tokio::task::JoinHandle<anyhow::Result<i32>>,
    }

    impl Fixture {
        /// A daemon reading sessions from `<dir>/<daemon_sessions>`.
        async fn start(daemon_sessions: &str) -> Fixture {
            let dir = tempfile::Builder::new().prefix("ho").tempdir().unwrap();
            let provider = MockProvider::start(
                dir.path().join("mock"),
                MockScript::text("Hello from the mock provider."),
            )
            .await
            .unwrap();
            std::fs::create_dir_all(dir.path().join("work")).unwrap();
            let socket = dir.path().join("d.sock");
            let config = DaemonConfig {
                paths: DaemonPaths {
                    run_dir: dir.path().to_path_buf(),
                    socket: socket.clone(),
                    lock: dir.path().join("d.lock"),
                    log: dir.path().join("d.log"),
                },
                host: HostConfig {
                    session_dir: dir.path().join(daemon_sessions),
                    idle_unload: Duration::ZERO,
                    models_file: Some(provider.catalog_path()),
                    no_context_files: true,
                },
                host_id: "01TEST".into(),
                host_name: "testbox".into(),
                log_level: super_daemon::log::Level::Debug,
                log_to_stderr: false,
                sweep_interval: Duration::from_secs(3600),
                presence: None,
                protocol_versions: super_sdk::daemon::PROTOCOL_VERSIONS,
            };
            let shutdown = CancellationToken::new();
            let daemon = tokio::spawn(super_daemon::run(config, shutdown.clone()));
            for _ in 0..200 {
                if socket.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert!(socket.exists(), "the daemon created its socket");
            Fixture {
                provider,
                dir,
                socket,
                shutdown,
                daemon,
            }
        }

        /// A session the way a standalone `super` runs it, after one turn.
        async fn standalone_session(&self) -> Arc<super_sdk::Session> {
            let session = super_sdk::Session::create(SessionOptions {
                cwd: self.dir.path().join("work"),
                model: Some("mock/mock-1".into()),
                models_file: Some(self.provider.catalog_path()),
                session: SessionSource::Create,
                session_dir: Some(self.dir.path().join("sessions")),
                no_context_files: true,
                ..Default::default()
            })
            .await
            .unwrap();
            session.prompt("first question").await.unwrap();
            session
        }

        async fn stop(self) {
            self.shutdown.cancel();
            let code = tokio::time::timeout(Duration::from_secs(10), self.daemon)
                .await
                .expect("the daemon stopped in time")
                .expect("the daemon did not panic")
                .expect("the daemon ran");
            assert_eq!(code, 0);
        }
    }

    fn texts(messages: &serde_json::Value) -> Vec<String> {
        messages["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| match &message["content"] {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Array(blocks) => blocks
                    .iter()
                    .filter_map(|block| block["text"].as_str())
                    .collect::<Vec<_>>()
                    .join(""),
                _ => String::new(),
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_standalone_session_moves_to_the_daemon_with_its_transcript() {
        let fixture = Fixture::start("sessions").await;
        let standalone = fixture.standalone_session().await;
        let agent = standalone.agent_session().clone();
        let mut lock = Some(crate::setup::keep_session_lock(&agent).unwrap());
        let file = agent
            .manager
            .lock()
            .unwrap()
            .session_file()
            .unwrap()
            .to_path_buf();
        let session_id = standalone.state().session_id;

        let mut client = DaemonClient::connect_to(&fixture.socket, &client_info())
            .await
            .unwrap();
        let mut events = client.take_events().unwrap();
        // While this process holds the file, the daemon refuses it.
        let refused = client.resume_session(&session_id).await.unwrap_err();
        assert!(
            refused.to_string().contains("open in another process"),
            "{refused}"
        );

        let target = prepare(&client, &agent).await.unwrap();
        assert_eq!(
            target,
            Target::Resume {
                session_id: session_id.clone(),
                file: file.clone(),
            }
        );
        let attached = hand_over(&client, &target, &mut lock, &agent)
            .await
            .unwrap();
        assert!(lock.is_none(), "this process let go of the file");
        assert_eq!(attached.session.session_id, session_id);
        assert!(attached.session.hosted);
        assert!(session_lock::is_held(&file), "the daemon holds it now");

        // The transcript came along.
        let messages = client
            .execute_ok(Command::GetMessages {})
            .await
            .unwrap()
            .unwrap();
        let before = texts(&messages);
        assert_eq!(
            before,
            ["first question", "Hello from the mock provider."],
            "{messages}"
        );

        // The daemon runs the next turn, into the same file.
        client
            .execute_ok(Command::Prompt {
                message: "second question".into(),
                images: Vec::new(),
                streaming_behavior: None,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match events.recv().await {
                    Some(ClientEvent::Session(event)) if event.event_type() == "agent_settled" => {
                        break;
                    }
                    Some(_) => {}
                    None => panic!("the daemon closed the connection"),
                }
            }
        })
        .await
        .expect("the turn settled");
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.contains("first question"));
        assert!(written.contains("second question"));

        let listed = client.list_sessions(None).await.unwrap();
        let hosted = listed
            .iter()
            .find(|session| session.session_id == session_id)
            .expect("listed");
        assert!(hosted.hosted);
        assert_eq!(hosted.attached_clients, 1);

        drop(standalone);
        drop(client);
        fixture.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_hand_over_leaves_the_session_where_it_was() {
        // The daemon reads another session directory than this process
        // writes to.
        let fixture = Fixture::start("daemon-sessions").await;
        let standalone = fixture.standalone_session().await;
        let agent = standalone.agent_session().clone();
        let mut lock = Some(crate::setup::keep_session_lock(&agent).unwrap());
        let file = agent
            .manager
            .lock()
            .unwrap()
            .session_file()
            .unwrap()
            .to_path_buf();
        let client = DaemonClient::connect_to(&fixture.socket, &client_info())
            .await
            .unwrap();

        // Checked before anything changes hands.
        let refused = prepare(&client, &agent).await.unwrap_err();
        assert!(refused.contains("keeps sessions in"), "{refused}");
        assert!(lock.is_some());

        // And when the daemon fails after the file was released, this
        // process takes it back.
        let target = target(&standalone_of(&agent)).unwrap();
        let error = hand_over(&client, &target, &mut lock, &agent)
            .await
            .unwrap_err();
        assert!(error.contains("no session"), "{error}");
        assert!(lock.is_some(), "the lock is back");
        assert!(session_lock::is_held(&file));

        // The in-process session works as before.
        standalone.prompt("still here").await.unwrap();
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("still here")
        );

        drop(standalone);
        drop(client);
        fixture.stop().await;
    }

    fn standalone_of(agent: &Arc<super_coding::session_runner::AgentSession>) -> Standalone {
        super::standalone(agent)
    }
}
