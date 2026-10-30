//! The client side of the daemon socket.
//!
//! [`DaemonClient::connect`] finds the socket (`SUPER_DAEMON_SOCKET`, else
//! `~/.super/run/daemon.sock`), says `hello` and keeps the negotiated
//! protocol version. Every daemon-level command has a typed call; after
//! [`attach`](DaemonClient::attach), [`execute`](DaemonClient::execute) sends
//! session commands to the attached session and its events arrive on the
//! stream from [`take_events`](DaemonClient::take_events), next to the
//! daemon-level events every connection receives.
//!
//! [`DaemonClient::connect_or_start`] also starts a registered daemon when
//! the socket is dead (see [`crate::service`]). A daemon that speaks another
//! protocol range is a [`SkewError`], whose message names the command that
//! updates whatever is out of date.

use crate::installation::InstallKind;
use crate::paths::DaemonPaths;
use crate::service::{self, DaemonService};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_sdk::daemon::{
    ClientKind, DaemonCommand, DaemonEvent, DaemonStatus, DirectoryEntry, HelloReply,
    PROTOCOL_VERSIONS, SessionSummary, WorkspaceSummary, describe_range, negotiate,
};
use super_sdk::protocol::{Command, Event, Request, Response};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// How long `hello` may take before the daemon counts as unresponsive.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a daemon-level command may take. `create_session` builds a
/// session, which can take a moment; nothing takes this long when healthy.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// How long [`DaemonClient::connect_or_start`] waits for a started daemon.
pub const START_WAIT: Duration = Duration::from_secs(5);

/// Who is connecting, as `hello` tells the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    pub kind: ClientKind,
    /// What other clients see as the origin of this client's commands.
    pub name: String,
    pub version: String,
    /// The protocol range this client speaks; [`PROTOCOL_VERSIONS`] outside
    /// tests.
    pub protocol_versions: [u32; 2],
}

impl ClientInfo {
    pub fn new(kind: ClientKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_versions: PROTOCOL_VERSIONS,
        }
    }
}

/// The daemon and this build share no protocol version.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.message())]
pub struct SkewError {
    /// The range the daemon speaks.
    pub daemon: [u32; 2],
    /// The range this build speaks.
    pub client: [u32; 2],
    pub daemon_version: Option<String>,
    /// How this build was installed, for the update hint.
    pub install: InstallKind,
    /// `std::env::consts::OS`; on macOS the daemon comes with the app.
    pub os: String,
}

impl SkewError {
    pub fn new(daemon: [u32; 2], client: [u32; 2], daemon_version: Option<String>) -> Self {
        Self {
            daemon,
            client,
            daemon_version,
            install: InstallKind::current(),
            os: std::env::consts::OS.to_string(),
        }
    }

    /// Whether this build is the older side.
    pub fn client_is_older(&self) -> bool {
        self.client[1] < self.daemon[0]
    }

    /// "the SUPER daemon speaks protocol 2 to 3, this build speaks 1: run
    /// `brew upgrade chwzr/tap/super`".
    pub fn message(&self) -> String {
        let hint = if self.client_is_older() {
            self.install.update_hint()
        } else if self.os == "macos" {
            "update the SUPER app, which runs the daemon".to_string()
        } else {
            "restart the daemon on this build: `super daemon stop`, then start it again".to_string()
        };
        format!(
            "the SUPER daemon speaks protocol {}, this build speaks {}: {hint}",
            describe_range(self.daemon),
            describe_range(self.client)
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Nothing listens on the socket.
    #[error("no SUPER daemon is running on {}", .socket.display())]
    NotRunning {
        socket: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// No daemon answers and none could be started.
    #[error("no SUPER daemon is available: {0}")]
    Unavailable(String),
    #[error(transparent)]
    Skew(#[from] SkewError),
    /// `hello` failed for another reason than the protocol range.
    #[error("the SUPER daemon refused the connection: {0}")]
    Refused(String),
    #[error("the SUPER daemon closed the connection")]
    Closed,
    #[error("the SUPER daemon did not answer `{0}` in time")]
    Timeout(String),
    /// A command came back with `success: false`.
    #[error("{command}: {message}")]
    Command { command: String, message: String },
    #[error("unexpected answer from the SUPER daemon: {0}")]
    Protocol(String),
    /// The process listening on the socket runs as another user.
    #[error(
        "the socket {} belongs to another user (uid {uid}); not connecting to it",
        .socket.display()
    )]
    ForeignSocket { socket: PathBuf, uid: u32 },
    #[error(transparent)]
    Io(std::io::Error),
}

/// What arrives on a connection besides responses.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    /// A daemon-level event; every connection receives these.
    Daemon(DaemonEvent),
    /// An event of the attached session, exactly as `--mode rpc` emits it.
    Session(Event),
}

/// The answer to `attach`.
#[derive(Debug, Clone, PartialEq)]
pub struct Attached {
    pub session: SessionSummary,
    /// The session this connection was attached to before, if any.
    pub previous_session_id: Option<String>,
}

/// What `create_session` takes besides the directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewSession {
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub thinking_level: Option<String>,
    pub name: Option<String>,
}

struct Shared {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    pending: Mutex<HashMap<String, oneshot::Sender<Response>>>,
    next_id: AtomicU64,
    closed: CancellationToken,
}

/// One connection to the daemon, after a successful `hello`.
pub struct DaemonClient {
    socket: PathBuf,
    hello: HelloReply,
    shared: Arc<Shared>,
    events: Option<mpsc::UnboundedReceiver<ClientEvent>>,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for DaemonClient {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl DaemonClient {
    /// The socket a client connects to: `SUPER_DAEMON_SOCKET`, else
    /// `~/.super/run/daemon.sock`.
    pub fn socket_path() -> Option<PathBuf> {
        DaemonPaths::discover().map(|paths| paths.socket)
    }

    /// Connect to the discovered socket and say hello.
    pub async fn connect(info: &ClientInfo) -> Result<Self, ClientError> {
        let socket = Self::socket_path()
            .ok_or_else(|| ClientError::Unavailable("no home directory".into()))?;
        Self::connect_to(&socket, info).await
    }

    /// Connect, or start the daemon when the socket is dead and a service
    /// manager knows it: the launchd agent the SUPER app registers on macOS,
    /// the systemd user unit `super install` writes on Linux. Only the default
    /// socket is started this way; `SUPER_DAEMON_SOCKET` pointing elsewhere
    /// means connect or nothing.
    pub async fn connect_or_start(info: &ClientInfo) -> Result<Self, ClientError> {
        let paths = DaemonPaths::discover()
            .ok_or_else(|| ClientError::Unavailable("no home directory".into()))?;
        let service = if paths.is_default() {
            service::platform_service()
        } else {
            None
        };
        Self::connect_or_start_with(&paths.socket, info, service.as_deref(), START_WAIT).await
    }

    /// [`connect_or_start`](Self::connect_or_start) with the socket, the
    /// service manager and the wait spelled out.
    pub async fn connect_or_start_with(
        socket: &Path,
        info: &ClientInfo,
        service: Option<&dyn DaemonService>,
        wait: Duration,
    ) -> Result<Self, ClientError> {
        match Self::connect_to(socket, info).await {
            Err(ClientError::NotRunning { .. }) => {}
            other => return other,
        }
        let service = match service::start_decision(service) {
            service::StartDecision::Start(service) => service,
            service::StartDecision::Unavailable(reason) => {
                return Err(ClientError::Unavailable(reason));
            }
        };
        if let Err(error) = service.start() {
            return Err(ClientError::Unavailable(format!(
                "could not start the {}: {error}",
                service.name()
            )));
        }
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            match Self::connect_to(socket, info).await {
                Err(ClientError::NotRunning { .. }) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(ClientError::NotRunning { .. }) => {
                    return Err(ClientError::Unavailable(format!(
                        "started the {}, but it did not answer within {}s",
                        service.name(),
                        wait.as_secs_f32()
                    )));
                }
                other => return other,
            }
        }
    }

    /// Connect to `socket` and say hello.
    pub async fn connect_to(socket: &Path, info: &ClientInfo) -> Result<Self, ClientError> {
        let stream =
            UnixStream::connect(socket)
                .await
                .map_err(|source| ClientError::NotRunning {
                    socket: socket.to_path_buf(),
                    source,
                })?;
        // Prompts and file contents go over this socket: only to a daemon
        // of this user, wherever `SUPER_DAEMON_SOCKET` points.
        let peer = stream.peer_cred().map_err(ClientError::Io)?.uid();
        // SAFETY: getuid has no preconditions and cannot fail.
        same_user(socket, peer, unsafe { libc::getuid() })?;
        let (read, write) = stream.into_split();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            writer: tokio::sync::Mutex::new(write),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            closed: CancellationToken::new(),
        });
        let reader = tokio::spawn(read_loop(BufReader::new(read), shared.clone(), events_tx));
        let mut client = Self {
            socket: socket.to_path_buf(),
            hello: placeholder_hello(),
            shared,
            events: Some(events_rx),
            reader,
        };
        let response = client
            .round_trip(
                serde_json::to_value(DaemonCommand::Hello {
                    client: info.kind,
                    name: Some(info.name.clone()),
                    version: info.version.clone(),
                    protocol_versions: info.protocol_versions,
                })
                .map_err(|error| ClientError::Protocol(error.to_string()))?,
                "hello",
                Some(HELLO_TIMEOUT),
            )
            .await?;
        if !response.success {
            return Err(hello_refusal(&response, info.protocol_versions));
        }
        client.hello = serde_json::from_value(response.data.unwrap_or(Value::Null))
            .map_err(|error| ClientError::Protocol(format!("hello: {error}")))?;
        Ok(client)
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// What the daemon said in answer to `hello`.
    pub fn hello(&self) -> &HelloReply {
        &self.hello
    }

    /// The protocol version both sides agreed on.
    pub fn protocol_version(&self) -> u32 {
        self.hello.protocol_version
    }

    /// The events of this connection: daemon-level events from the start,
    /// session events once attached. Take it once, before `attach`, so no
    /// event is missed; events queue up until it is taken.
    pub fn take_events(&mut self) -> Option<mpsc::UnboundedReceiver<ClientEvent>> {
        self.events.take()
    }

    /// Resolves when the daemon closed the connection.
    pub async fn closed(&self) {
        self.shared.closed.cancelled().await;
    }

    pub fn is_closed(&self) -> bool {
        self.shared.closed.is_cancelled()
    }

    // --- daemon-level commands ---

    pub async fn status(&self) -> Result<DaemonStatus, ClientError> {
        self.request(DaemonCommand::Status {}).await
    }

    /// Hosted and stored sessions, newest first; `cwd` limits the listing.
    pub async fn list_sessions(
        &self,
        cwd: Option<&Path>,
    ) -> Result<Vec<SessionSummary>, ClientError> {
        let data: Value = self
            .request(DaemonCommand::ListSessions {
                cwd: cwd.map(|cwd| cwd.display().to_string()),
            })
            .await?;
        field(data, "sessions")
    }

    pub async fn list_workspaces(&self) -> Result<Vec<WorkspaceSummary>, ClientError> {
        let data: Value = self.request(DaemonCommand::ListWorkspaces {}).await?;
        field(data, "workspaces")
    }

    pub async fn list_directories(&self, path: &Path) -> Result<Vec<DirectoryEntry>, ClientError> {
        let data: Value = self
            .request(DaemonCommand::ListDirectories {
                path: path.display().to_string(),
            })
            .await?;
        field(data, "directories")
    }

    pub async fn create_session(&self, new: NewSession) -> Result<SessionSummary, ClientError> {
        let data: Value = self
            .request(DaemonCommand::CreateSession {
                cwd: new.cwd.display().to_string(),
                model: new.model,
                provider: new.provider,
                thinking_level: new.thinking_level,
                name: new.name,
            })
            .await?;
        field(data, "session")
    }

    /// Load a stored session; a no-op for a hosted one.
    pub async fn resume_session(&self, session_id: &str) -> Result<SessionSummary, ClientError> {
        let data: Value = self
            .request(DaemonCommand::ResumeSession {
                session_id: session_id.to_string(),
            })
            .await?;
        field(data, "session")
    }

    /// Attach this connection to a session, loading it when stored. Its
    /// events arrive on [`take_events`](Self::take_events) from now on.
    pub async fn attach(&self, session_id: &str) -> Result<Attached, ClientError> {
        let data: Value = self
            .request(DaemonCommand::Attach {
                session_id: session_id.to_string(),
            })
            .await?;
        let previous_session_id = data
            .get("previousSessionId")
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok(Attached {
            session: field(data, "session")?,
            previous_session_id,
        })
    }

    /// Detach from the attached session; returns its id, if there was one.
    pub async fn detach(&self) -> Result<Option<String>, ClientError> {
        let data: Value = self.request(DaemonCommand::Detach {}).await?;
        Ok(data
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// Abort a session's turn and unload it; its file stays.
    pub async fn stop_session(&self, session_id: &str) -> Result<(), ClientError> {
        let _: Value = self
            .request(DaemonCommand::StopSession {
                session_id: session_id.to_string(),
            })
            .await?;
        Ok(())
    }

    /// Ask the daemon to exit. Await [`closed`](Self::closed) to see it go.
    pub async fn shutdown(&self) -> Result<(), ClientError> {
        let _: Value = self.request(DaemonCommand::Shutdown {}).await?;
        Ok(())
    }

    /// Any daemon-level command, with its raw response.
    pub async fn call(&self, command: DaemonCommand) -> Result<Response, ClientError> {
        let name = command.name();
        let value = serde_json::to_value(&command)
            .map_err(|error| ClientError::Protocol(error.to_string()))?;
        self.round_trip(value, name, Some(COMMAND_TIMEOUT)).await
    }

    /// A session command (`prompt`, `abort`, `get_state`, ...) for the
    /// attached session, with its raw response. No timeout: `bash` answers
    /// when its command finishes.
    pub async fn execute(&self, command: Command) -> Result<Response, ClientError> {
        let name = command.name();
        let value = serde_json::to_value(Request::new(command))
            .map_err(|error| ClientError::Protocol(error.to_string()))?;
        self.round_trip(value, name, None).await
    }

    /// [`execute`](Self::execute), turning `success: false` into an error.
    pub async fn execute_ok(&self, command: Command) -> Result<Option<Value>, ClientError> {
        let response = self.execute(command).await?;
        if response.success {
            Ok(response.data)
        } else {
            Err(command_error(response))
        }
    }

    async fn request<T: DeserializeOwned>(&self, command: DaemonCommand) -> Result<T, ClientError> {
        let response = self.call(command).await?;
        if !response.success {
            return Err(command_error(response));
        }
        serde_json::from_value(response.data.unwrap_or(Value::Null))
            .map_err(|error| ClientError::Protocol(error.to_string()))
    }

    /// Send one command object, adding an id, and wait for its response.
    async fn round_trip(
        &self,
        mut command: Value,
        name: &str,
        timeout: Option<Duration>,
    ) -> Result<Response, ClientError> {
        if self.shared.closed.is_cancelled() {
            return Err(ClientError::Closed);
        }
        let id = format!(
            "c{}",
            self.shared.next_id.fetch_add(1, Ordering::SeqCst) + 1
        );
        command["id"] = Value::String(id.clone());
        let (sender, receiver) = oneshot::channel();
        self.shared
            .pending
            .lock()
            .unwrap()
            .insert(id.clone(), sender);
        let line = format!("{command}\n");
        let written = {
            let mut writer = self.shared.writer.lock().await;
            match writer.write_all(line.as_bytes()).await {
                Ok(()) => writer.flush().await,
                Err(error) => Err(error),
            }
        };
        if let Err(error) = written {
            self.shared.pending.lock().unwrap().remove(&id);
            return Err(match error.kind() {
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset => {
                    ClientError::Closed
                }
                _ => ClientError::Io(error),
            });
        }
        let answer = match timeout {
            Some(limit) => match tokio::time::timeout(limit, receiver).await {
                Ok(answer) => answer,
                Err(_) => {
                    self.shared.pending.lock().unwrap().remove(&id);
                    return Err(ClientError::Timeout(name.to_string()));
                }
            },
            None => receiver.await,
        };
        answer.map_err(|_| ClientError::Closed)
    }
}

/// Route every line: responses to their waiting call, events to the stream.
async fn read_loop(
    mut reader: BufReader<OwnedReadHalf>,
    shared: Arc<Shared>,
    events: mpsc::UnboundedSender<ClientEvent>,
) {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = line.trim_end_matches('\n').trim_end_matches('\r');
        if text.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("response") => {
                let Ok(response) = serde_json::from_value::<Response>(value) else {
                    continue;
                };
                let waiting = response
                    .id
                    .as_ref()
                    .and_then(|id| shared.pending.lock().unwrap().remove(id));
                if let Some(waiting) = waiting {
                    let _ = waiting.send(response);
                }
            }
            Some(_) => {
                let _ = events.send(classify_event(value));
            }
            None => {}
        }
    }
    shared.closed.cancel();
    // Dropping the senders fails every call still waiting with `Closed`.
    shared.pending.lock().unwrap().clear();
}

/// Daemon-level events by their type; everything else belongs to the session.
fn classify_event(value: Value) -> ClientEvent {
    let daemon_level = matches!(
        value.get("type").and_then(Value::as_str),
        Some("session_created" | "session_updated" | "session_ended" | "daemon_shutdown")
    );
    if daemon_level && let Ok(event) = serde_json::from_value::<DaemonEvent>(value.clone()) {
        return ClientEvent::Daemon(event);
    }
    ClientEvent::Session(Event(value))
}

/// Turn a refused `hello` into a skew error when the ranges do not meet.
fn hello_refusal(response: &Response, client: [u32; 2]) -> ClientError {
    let data = response.data.as_ref();
    let daemon_range = data
        .and_then(|data| data.get("protocolVersions"))
        .and_then(|range| serde_json::from_value::<[u32; 2]>(range.clone()).ok());
    if let Some(daemon) = daemon_range
        && negotiate(client, daemon).is_err()
    {
        let daemon_version = data
            .and_then(|data| data.get("daemonVersion"))
            .and_then(Value::as_str)
            .map(str::to_string);
        return ClientError::Skew(SkewError::new(daemon, client, daemon_version));
    }
    ClientError::Refused(
        response
            .error
            .clone()
            .unwrap_or_else(|| "hello refused".into()),
    )
}

/// Refuse a socket whose listener runs as another user than this process.
fn same_user(socket: &Path, peer: u32, own: u32) -> Result<(), ClientError> {
    if peer == own {
        Ok(())
    } else {
        Err(ClientError::ForeignSocket {
            socket: socket.to_path_buf(),
            uid: peer,
        })
    }
}

fn command_error(response: Response) -> ClientError {
    ClientError::Command {
        command: response.command,
        message: response.error.unwrap_or_else(|| "failed".into()),
    }
}

fn field<T: DeserializeOwned>(mut data: Value, name: &str) -> Result<T, ClientError> {
    let value = data.get_mut(name).map(Value::take).unwrap_or(Value::Null);
    serde_json::from_value(value).map_err(|error| ClientError::Protocol(format!("{name}: {error}")))
}

fn placeholder_hello() -> HelloReply {
    HelloReply {
        protocol_version: 0,
        daemon_version: String::new(),
        host_id: String::new(),
        host_name: String::new(),
        platform: String::new(),
        pid: 0,
        started_at: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::net::UnixListener;

    /// A stand-in daemon on `socket` that answers one `hello` per connection:
    /// accepted when `range` meets the client's, refused like the real
    /// server otherwise.
    fn fake_daemon(socket: &Path, range: [u32; 2]) -> tokio::task::JoinHandle<()> {
        let listener = UnixListener::bind(socket).expect("bind");
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut reader = BufReader::new(read);
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let hello: Value = serde_json::from_str(&line).unwrap();
                    let client: [u32; 2] =
                        serde_json::from_value(hello["protocolVersions"].clone()).unwrap();
                    let reply = match negotiate(client, range) {
                        Ok(version) => serde_json::json!({
                            "type": "response", "id": hello["id"], "command": "hello",
                            "success": true,
                            "data": {"protocolVersion": version, "daemonVersion": "9.9.9",
                                     "hostId": "h", "hostName": "box", "platform": "macos",
                                     "pid": 7, "startedAt": "2026-10-30T00:00:00Z"}
                        }),
                        Err(error) => serde_json::json!({
                            "type": "response", "id": hello["id"], "command": "hello",
                            "success": false, "error": error,
                            "data": {"daemonVersion": "9.9.9", "protocolVersions": range}
                        }),
                    };
                    let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
                    // Keep the connection open until the client goes.
                    let mut rest = String::new();
                    let _ = reader.read_line(&mut rest).await;
                });
            }
        })
    }

    struct FakeService {
        registered: bool,
        fail: bool,
        socket: Option<PathBuf>,
        starts: AtomicUsize,
        daemon: Mutex<Option<tokio::task::JoinHandle<()>>>,
    }

    impl FakeService {
        fn new(registered: bool) -> Self {
            Self {
                registered,
                fail: false,
                socket: None,
                starts: AtomicUsize::new(0),
                daemon: Mutex::new(None),
            }
        }
    }

    impl DaemonService for FakeService {
        fn name(&self) -> String {
            "fake service".into()
        }

        fn is_registered(&self) -> bool {
            self.registered
        }

        fn start(&self) -> std::io::Result<()> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(std::io::Error::other("launchctl said no"));
            }
            if let Some(socket) = &self.socket {
                *self.daemon.lock().unwrap() = Some(fake_daemon(socket, PROTOCOL_VERSIONS));
            }
            Ok(())
        }
    }

    fn info() -> ClientInfo {
        ClientInfo::new(ClientKind::Cli, "test")
    }

    fn socket_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("d.sock")
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("dc").tempdir().unwrap()
    }

    #[tokio::test]
    async fn a_live_socket_is_used_without_starting_anything() {
        let dir = tempdir();
        let socket = socket_in(&dir);
        let _daemon = fake_daemon(&socket, PROTOCOL_VERSIONS);
        let service = FakeService::new(true);
        let client =
            DaemonClient::connect_or_start_with(&socket, &info(), Some(&service), START_WAIT)
                .await
                .expect("connected");
        assert_eq!(client.protocol_version(), 1);
        assert_eq!(client.hello().host_name, "box");
        assert_eq!(service.starts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_dead_socket_starts_the_registered_service_and_waits_for_it() {
        let dir = tempdir();
        let socket = socket_in(&dir);
        let mut service = FakeService::new(true);
        service.socket = Some(socket.clone());
        let client =
            DaemonClient::connect_or_start_with(&socket, &info(), Some(&service), START_WAIT)
                .await
                .expect("started and connected");
        assert_eq!(service.starts.load(Ordering::SeqCst), 1);
        assert_eq!(client.hello().daemon_version, "9.9.9");
    }

    #[tokio::test]
    async fn nothing_registered_means_no_daemon_and_no_start() {
        let dir = tempdir();
        let socket = socket_in(&dir);
        let service = FakeService::new(false);
        let error =
            DaemonClient::connect_or_start_with(&socket, &info(), Some(&service), START_WAIT)
                .await
                .err()
                .expect("unavailable");
        assert!(matches!(error, ClientError::Unavailable(_)), "{error}");
        assert_eq!(service.starts.load(Ordering::SeqCst), 0);
        assert!(
            error.to_string().contains("fake service is not registered"),
            "{error}"
        );

        let error = DaemonClient::connect_or_start_with(&socket, &info(), None, START_WAIT)
            .await
            .err()
            .expect("unavailable");
        assert!(matches!(error, ClientError::Unavailable(_)), "{error}");
    }

    #[tokio::test]
    async fn a_failed_or_silent_start_is_reported() {
        let dir = tempdir();
        let socket = socket_in(&dir);
        let mut failing = FakeService::new(true);
        failing.fail = true;
        let error =
            DaemonClient::connect_or_start_with(&socket, &info(), Some(&failing), START_WAIT)
                .await
                .err()
                .expect("unavailable");
        assert!(error.to_string().contains("launchctl said no"), "{error}");

        // Registered and started, but nothing ever listens.
        let silent = FakeService::new(true);
        let error = DaemonClient::connect_or_start_with(
            &socket,
            &info(),
            Some(&silent),
            Duration::from_millis(250),
        )
        .await
        .err()
        .expect("unavailable");
        assert!(error.to_string().contains("did not answer"), "{error}");
        assert_eq!(silent.starts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_daemon_with_another_range_is_a_skew_error() {
        let dir = tempdir();
        let socket = socket_in(&dir);
        let _daemon = fake_daemon(&socket, [2, 3]);
        let service = FakeService::new(true);
        let error =
            DaemonClient::connect_or_start_with(&socket, &info(), Some(&service), START_WAIT)
                .await
                .err()
                .expect("skew");
        let ClientError::Skew(skew) = error else {
            panic!("expected a skew error, got {error}");
        };
        assert_eq!(skew.daemon, [2, 3]);
        assert_eq!(skew.client, [1, 1]);
        assert_eq!(skew.daemon_version.as_deref(), Some("9.9.9"));
        assert!(
            skew.to_string()
                .starts_with("the SUPER daemon speaks protocol 2 to 3, this build speaks 1: "),
            "{skew}"
        );
        assert_eq!(
            service.starts.load(Ordering::SeqCst),
            0,
            "a skew is not a dead socket"
        );
    }

    #[test]
    fn the_skew_message_names_the_update_for_each_install() {
        let skew = |install, os: &str| SkewError {
            daemon: [2, 3],
            client: [1, 1],
            daemon_version: Some("0.0.50".into()),
            install,
            os: os.into(),
        };
        assert_eq!(
            skew(InstallKind::Homebrew, "macos").to_string(),
            "the SUPER daemon speaks protocol 2 to 3, this build speaks 1: run `brew upgrade chwzr/tap/super`"
        );
        assert_eq!(
            skew(InstallKind::Script, "linux").to_string(),
            "the SUPER daemon speaks protocol 2 to 3, this build speaks 1: run `super update`"
        );
        assert_eq!(
            skew(InstallKind::App, "macos").to_string(),
            "the SUPER daemon speaks protocol 2 to 3, this build speaks 1: managed by the SUPER app: update the app"
        );
        assert_eq!(
            skew(InstallKind::Cargo, "linux").to_string(),
            "the SUPER daemon speaks protocol 2 to 3, this build speaks 1: run `cargo install --git https://github.com/chwzr/super super`"
        );

        // An older daemon is the daemon's update, not this build's.
        let older_daemon = |os: &str| SkewError {
            daemon: [1, 1],
            client: [2, 2],
            daemon_version: None,
            install: InstallKind::Homebrew,
            os: os.into(),
        };
        assert_eq!(
            older_daemon("macos").to_string(),
            "the SUPER daemon speaks protocol 1, this build speaks 2: update the SUPER app, which runs the daemon"
        );
        assert!(older_daemon("linux").to_string().ends_with(
            "restart the daemon on this build: `super daemon stop`, then start it again"
        ));
    }

    #[test]
    fn a_socket_of_another_user_is_refused() {
        let socket = Path::new("/tmp/shared.sock");
        assert!(same_user(socket, 501, 501).is_ok());
        let error = same_user(socket, 0, 501).expect_err("another user");
        assert!(
            matches!(&error, ClientError::ForeignSocket { uid: 0, .. }),
            "{error}"
        );
        assert_eq!(
            error.to_string(),
            "the socket /tmp/shared.sock belongs to another user (uid 0); not connecting to it"
        );
    }

    #[test]
    fn events_are_sorted_by_level() {
        let daemon =
            classify_event(serde_json::json!({"type": "daemon_shutdown", "reason": "stopped"}));
        assert!(matches!(
            daemon,
            ClientEvent::Daemon(DaemonEvent::DaemonShutdown { .. })
        ));
        let session = classify_event(serde_json::json!({"type": "agent_start"}));
        assert!(
            matches!(session, ClientEvent::Session(event) if event.event_type() == "agent_start")
        );
    }
}
