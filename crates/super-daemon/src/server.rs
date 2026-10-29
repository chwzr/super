//! The socket server: one task per connection, at most one attached session
//! per connection, daemon-level commands on any connection.

use crate::host::{CreateRequest, Hosted, SessionHost};
use crate::log;
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use super_sdk::daemon::{
    ClientLine, DaemonCommand, DaemonEvent, DaemonRequest, DaemonStatus, DirectoryEntry, EndReason,
    HelloReply, PROTOCOL_VERSIONS, decode_client_line, negotiate,
};
use super_sdk::protocol::Response;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::unix::OwnedReadHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

/// What `hello` and `status` report about this daemon.
#[derive(Debug, Clone)]
pub struct DaemonInfo {
    pub version: String,
    pub host_id: String,
    pub host_name: String,
    pub platform: String,
    pub socket_path: PathBuf,
}

pub struct ServerContext {
    pub host: Arc<SessionHost>,
    pub info: DaemonInfo,
    pub shutdown: CancellationToken,
    started: Instant,
    started_at: String,
    clients: AtomicUsize,
    next_client: AtomicU64,
}

impl ServerContext {
    pub fn new(host: Arc<SessionHost>, info: DaemonInfo, shutdown: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            host,
            info,
            shutdown,
            started: Instant::now(),
            started_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            clients: AtomicUsize::new(0),
            next_client: AtomicU64::new(1),
        })
    }

    pub fn clients(&self) -> usize {
        self.clients.load(Ordering::SeqCst)
    }

    fn hello_reply(&self, protocol_version: u32) -> HelloReply {
        HelloReply {
            protocol_version,
            daemon_version: self.info.version.clone(),
            host_id: self.info.host_id.clone(),
            host_name: self.info.host_name.clone(),
            platform: self.info.platform.clone(),
            pid: std::process::id(),
            started_at: self.started_at.clone(),
        }
    }

    async fn status(&self) -> DaemonStatus {
        let sessions = self.host.list(None).await;
        let hosted = sessions.iter().filter(|session| session.hosted).count();
        DaemonStatus {
            daemon_version: self.info.version.clone(),
            protocol_versions: PROTOCOL_VERSIONS,
            host_id: self.info.host_id.clone(),
            host_name: self.info.host_name.clone(),
            platform: self.info.platform.clone(),
            pid: std::process::id(),
            started_at: self.started_at.clone(),
            uptime_seconds: self.started.elapsed().as_secs(),
            hosted_sessions: hosted,
            stored_sessions: sessions.len() - hosted,
            clients: self.clients(),
            socket_path: self.info.socket_path.display().to_string(),
            session_dir: self.host.config().session_dir.display().to_string(),
        }
    }
}

/// Accept connections until `shutdown` fires.
pub async fn serve(listener: UnixListener, ctx: Arc<ServerContext>) {
    loop {
        let accepted = tokio::select! {
            _ = ctx.shutdown.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, _)) => {
                let ctx = ctx.clone();
                tokio::spawn(async move { connection(stream, ctx).await });
            }
            Err(error) => {
                log::warn("server", format!("accept failed: {error}"));
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

fn peer_is_this_user(stream: &UnixStream) -> bool {
    match stream.peer_cred() {
        // SAFETY: getuid has no preconditions and cannot fail.
        Ok(credentials) => credentials.uid() == unsafe { libc::getuid() },
        Err(error) => {
            log::warn("server", format!("peer credentials unavailable: {error}"));
            false
        }
    }
}

struct Attachment {
    hosted: Arc<Hosted>,
    pump: tokio::task::JoinHandle<()>,
}

/// One connection after a successful hello.
struct Client {
    id: u64,
    origin: String,
    attachment: Mutex<Option<Attachment>>,
    out: mpsc::UnboundedSender<String>,
}

impl Client {
    /// Attach to `hosted`, detaching from the previous session first. The
    /// event subscription starts before the caller responds, so no event
    /// between the response and the first read is lost.
    fn attach(&self, ctx: &ServerContext, hosted: Arc<Hosted>) -> Option<String> {
        let previous = self.detach(ctx);
        let mut events = hosted.session.events();
        let out = self.out.clone();
        let pump = tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                if out.send(event.to_line()).is_err() {
                    break;
                }
            }
        });
        ctx.host.attach(&hosted);
        log::info(
            "server",
            format!("client {} attached {}", self.id, hosted.session_id),
        );
        *self.attachment.lock().unwrap() = Some(Attachment { hosted, pump });
        previous
    }

    fn detach(&self, ctx: &ServerContext) -> Option<String> {
        let taken = self.attachment.lock().unwrap().take()?;
        taken.pump.abort();
        ctx.host.detach(&taken.hosted);
        log::info(
            "server",
            format!("client {} detached {}", self.id, taken.hosted.session_id),
        );
        Some(taken.hosted.session_id.clone())
    }

    /// The attached session was unloaded: forget it without touching the host.
    fn forget_if(&self, session_id: &str) {
        let mut guard = self.attachment.lock().unwrap();
        let matches = guard
            .as_ref()
            .is_some_and(|attachment| attachment.hosted.session_id == session_id);
        if matches && let Some(attachment) = guard.take() {
            attachment.pump.abort();
        }
    }

    fn attached(&self) -> Option<Arc<Hosted>> {
        self.attachment
            .lock()
            .unwrap()
            .as_ref()
            .map(|attachment| attachment.hosted.clone())
    }
}

async fn connection(stream: UnixStream, ctx: Arc<ServerContext>) {
    if !peer_is_this_user(&stream) {
        log::warn("server", "refused a connection from another user");
        return;
    }
    let client_id = ctx.next_client.fetch_add(1, Ordering::SeqCst);
    ctx.clients.fetch_add(1, Ordering::SeqCst);
    let (read, mut write) = stream.into_split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
    // One writer owns the socket so lines never interleave.
    let writer = tokio::spawn(async move {
        while let Some(line) = out_rx.recv().await {
            if write.write_all(line.as_bytes()).await.is_err()
                || write.write_all(b"\n").await.is_err()
                || write.flush().await.is_err()
            {
                break;
            }
        }
        let _ = write.shutdown().await;
    });
    let mut reader = BufReader::new(read);
    let mut buffer = String::new();

    let origin = match read_line(&mut reader, &mut buffer).await {
        Some(first) => handshake(&ctx, client_id, &first, &out),
        None => None,
    };
    let Some(origin) = origin else {
        drop(out);
        let _ = writer.await;
        ctx.clients.fetch_sub(1, Ordering::SeqCst);
        return;
    };
    let client = Arc::new(Client {
        id: client_id,
        origin,
        attachment: Mutex::new(None),
        out: out.clone(),
    });

    // Daemon-level events go to every connection.
    let events_pump = {
        let client = client.clone();
        let mut events = ctx.host.subscribe();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        if let DaemonEvent::SessionEnded { session_id, .. } = &event {
                            client.forget_if(session_id);
                        }
                        if client.out.send(event.to_line()).is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    };

    while let Some(line) = read_line(&mut reader, &mut buffer).await {
        dispatch(&ctx, &client, &line);
    }

    events_pump.abort();
    client.detach(&ctx);
    drop(client);
    drop(out);
    let _ = writer.await;
    ctx.clients.fetch_sub(1, Ordering::SeqCst);
    log::debug("server", format!("client {client_id} disconnected"));
}

/// The first line must be `hello`. Returns the client's origin name.
fn handshake(
    ctx: &ServerContext,
    client_id: u64,
    line: &str,
    out: &mpsc::UnboundedSender<String>,
) -> Option<String> {
    let refuse = |id: Option<String>, message: String, data: Option<Value>| {
        let response = Response {
            kind: "response".into(),
            id,
            command: "hello".into(),
            success: false,
            data,
            error: Some(message),
        };
        let _ = out.send(encode(&response));
        None
    };
    match decode_client_line(line) {
        Ok(ClientLine::Daemon(DaemonRequest {
            id,
            command:
                DaemonCommand::Hello {
                    client,
                    name,
                    version,
                    protocol_versions,
                },
        })) => match negotiate(protocol_versions, PROTOCOL_VERSIONS) {
            Ok(protocol_version) => {
                let reply = ctx.hello_reply(protocol_version);
                let response =
                    Response::ok_data("hello", serde_json::to_value(reply).unwrap_or(Value::Null))
                        .with_id(id);
                let _ = out.send(encode(&response));
                let origin = name
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| client.as_str().to_string());
                log::info(
                    "server",
                    format!(
                        "client {client_id} hello: {} {origin} {version} protocol {protocol_version}",
                        client.as_str()
                    ),
                );
                Some(origin)
            }
            Err(message) => {
                log::warn("server", format!("client {client_id} refused: {message}"));
                refuse(
                    id,
                    message,
                    Some(json!({
                        "daemonVersion": ctx.info.version,
                        "protocolVersions": PROTOCOL_VERSIONS,
                    })),
                )
            }
        },
        Ok(ClientLine::Daemon(DaemonRequest { id, .. })) => {
            refuse(id, "say hello first".into(), None)
        }
        Ok(ClientLine::Session(request)) => refuse(request.id, "say hello first".into(), None),
        Err(error) => refuse(None, format!("say hello first: {error}"), None),
    }
}

fn dispatch(ctx: &Arc<ServerContext>, client: &Arc<Client>, line: &str) {
    match decode_client_line(line) {
        Err(error) => {
            let _ = client.out.send(error_line("parse", error));
        }
        Ok(ClientLine::Daemon(request)) => {
            let ctx = ctx.clone();
            let client = client.clone();
            tokio::spawn(async move {
                let is_shutdown = matches!(request.command, DaemonCommand::Shutdown {});
                let response = daemon_command(&ctx, &client, request).await;
                if !is_shutdown {
                    let _ = client.out.send(encode(&response));
                }
            });
        }
        Ok(ClientLine::Session(request)) => match client.attached() {
            None => {
                let response = Response::err(request.command.name(), "not attached to a session")
                    .with_id(request.id);
                let _ = client.out.send(encode(&response));
            }
            Some(hosted) => {
                hosted.touch();
                let origin = client.origin.clone();
                let out = client.out.clone();
                tokio::spawn(async move {
                    let response = hosted
                        .session
                        .execute_from(request.command, request.id, Some(origin))
                        .await;
                    let _ = out.send(encode(&response));
                });
            }
        },
    }
}

async fn daemon_command(
    ctx: &Arc<ServerContext>,
    client: &Arc<Client>,
    request: DaemonRequest,
) -> Response {
    let name = request.command.name();
    let id = request.id;
    log::debug("server", format!("client {} {name}", client.id));
    let result: Result<Value, String> = match request.command {
        DaemonCommand::Hello { .. } => Err("already said hello".into()),
        DaemonCommand::Status {} => {
            serde_json::to_value(ctx.status().await).map_err(|error| error.to_string())
        }
        DaemonCommand::ListSessions { cwd } => {
            let cwd = cwd.map(PathBuf::from);
            Ok(json!({"sessions": ctx.host.list(cwd.as_deref()).await}))
        }
        DaemonCommand::ListWorkspaces {} => Ok(json!({"workspaces": ctx.host.workspaces().await})),
        DaemonCommand::ListDirectories { path } => list_directories(&path)
            .map(|directories| json!({"path": path, "directories": directories})),
        DaemonCommand::CreateSession {
            cwd,
            model,
            provider,
            thinking_level,
            name,
        } => ctx
            .host
            .create(CreateRequest {
                cwd: PathBuf::from(cwd),
                model,
                provider,
                thinking_level,
                name,
            })
            .await
            .map(|hosted| json!({"session": ctx.host.summary(&hosted)}))
            .map_err(|error| error.to_string()),
        DaemonCommand::ResumeSession { session_id } => ctx
            .host
            .load(&session_id)
            .await
            .map(|hosted| json!({"session": ctx.host.summary(&hosted)}))
            .map_err(|error| error.to_string()),
        DaemonCommand::Attach { session_id } => match ctx.host.load(&session_id).await {
            Ok(hosted) => {
                let previous = client.attach(ctx, hosted.clone());
                Ok(json!({
                    "session": ctx.host.summary(&hosted),
                    "previousSessionId": previous,
                }))
            }
            Err(error) => Err(error.to_string()),
        },
        DaemonCommand::Detach {} => Ok(json!({"sessionId": client.detach(ctx)})),
        DaemonCommand::StopSession { session_id } => ctx
            .host
            .stop(&session_id, EndReason::Stopped)
            .await
            .map(|()| json!({"sessionId": session_id}))
            .map_err(|error| error.to_string()),
        DaemonCommand::Shutdown {} => {
            log::info("server", format!("client {} asked for shutdown", client.id));
            let response = Response::ok("shutdown").with_id(id);
            // Queue the response before cancelling so it still goes out; the
            // caller does not send a second one for `shutdown`.
            let _ = client.out.send(encode(&response));
            ctx.shutdown.cancel();
            return response;
        }
    };
    match result {
        Ok(data) => Response::ok_data(name, data).with_id(id),
        Err(error) => Response::err(name, error).with_id(id),
    }
}

fn list_directories(path: &str) -> Result<Vec<DirectoryEntry>, String> {
    let dir = Path::new(path);
    if !dir.is_absolute() {
        return Err(format!("path must be absolute: {path}"));
    }
    let entries = std::fs::read_dir(dir).map_err(|error| format!("cannot read {path}: {error}"))?;
    let mut out: Vec<DirectoryEntry> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            (!name.starts_with('.')).then(|| DirectoryEntry {
                path: entry.path().display().to_string(),
                name,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The next non-empty line without its terminator, or `None` at EOF.
async fn read_line(reader: &mut BufReader<OwnedReadHalf>, buffer: &mut String) -> Option<String> {
    loop {
        buffer.clear();
        match reader.read_line(buffer).await {
            Ok(0) | Err(_) => return None,
            Ok(_) => {
                let line = buffer.trim_end_matches('\n').trim_end_matches('\r');
                if line.trim().is_empty() {
                    continue;
                }
                return Some(line.to_string());
            }
        }
    }
}

fn encode(response: &Response) -> String {
    serde_json::to_string(response).unwrap_or_else(|error| error_line("parse", error))
}

fn error_line(command: &str, error: impl std::fmt::Display) -> String {
    let message = serde_json::to_string(&error.to_string())
        .unwrap_or_else(|_| "\"internal error\"".to_string());
    format!(r#"{{"type":"response","command":"{command}","success":false,"error":{message}}}"#)
}
