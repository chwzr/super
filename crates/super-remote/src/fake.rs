//! An in-memory platform for tests: the S2 proxy (streams in memory, SSE
//! tails that deliver whatever is appended after the reader connected) and
//! the registry endpoints (machines, heartbeats, sessions, `GET
//! /api/registry`), all of which a test can inspect. Enabled with the `fake`
//! feature; never built into a release.

use crate::RemoteCredentials;
use crate::registry::RegistryClient;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct StoredRecord {
    pub seq: u64,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// A machine as the fake registry holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeMachine {
    pub id: String,
    pub name: String,
    pub platform: String,
    /// An announcement or a heartbeat said `"daemon": true`. The real
    /// platform turns this off 90 seconds after the last one that did; here
    /// a test does with [`FakePlatform::set_daemon`].
    pub daemon: bool,
    /// On from the first announcement; a test turns it off with
    /// [`FakePlatform::set_online`], as the platform's sweep would.
    pub online: bool,
    pub announcements: usize,
    pub heartbeats: usize,
    last_seen_at: String,
    created_at: String,
}

/// One request to a registry endpoint, as it arrived.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiRequest {
    pub method: String,
    /// The path without the query, for example `/api/machines/01H/heartbeat`.
    pub path: String,
    /// The JSON body; `Null` when there was none.
    pub body: Value,
}

#[derive(Default)]
struct FakeRegistry {
    version: u64,
    machines: BTreeMap<String, FakeMachine>,
    /// `(machineId, sessionId)` to the session as `GET /api/registry` lists it.
    sessions: BTreeMap<(String, String), Value>,
}

/// A fake platform: streams in memory with SSE tails that deliver whatever
/// is appended after the reader connected, and the registry in memory.
#[derive(Default)]
pub struct FakePlatform {
    streams: Mutex<BTreeMap<String, Vec<StoredRecord>>>,
    waiters: Mutex<Vec<tokio::sync::mpsc::UnboundedSender<()>>>,
    seen_tokens: Mutex<Vec<String>>,
    url: Mutex<Option<String>>,
    registry: Mutex<FakeRegistry>,
    requests: Mutex<Vec<ApiRequest>>,
    api_failure: Mutex<Option<u16>>,
}

impl FakePlatform {
    pub fn append(
        &self,
        stream: &str,
        records: Vec<(Vec<(String, String)>, String)>,
    ) -> (u64, u64) {
        let mut streams = self.streams.lock().unwrap();
        let entries = streams.entry(stream.to_string()).or_default();
        let start = entries.len() as u64;
        for (headers, body) in records {
            let seq = entries.len() as u64;
            entries.push(StoredRecord { seq, headers, body });
        }
        let end = entries.len() as u64;
        drop(streams);
        for waiter in self.waiters.lock().unwrap().iter() {
            let _ = waiter.send(());
        }
        (start, end)
    }

    /// Every bearer token the server has seen.
    pub fn seen_tokens(&self) -> Vec<String> {
        self.seen_tokens.lock().unwrap().clone()
    }

    pub fn records(&self, stream: &str) -> Vec<StoredRecord> {
        self.streams
            .lock()
            .unwrap()
            .get(stream)
            .cloned()
            .unwrap_or_default()
    }

    /// The platform's base URL, `http://127.0.0.1:<port>`, once [`serve`]
    /// runs.
    pub fn url(&self) -> String {
        self.url
            .lock()
            .unwrap()
            .clone()
            .expect("the fake platform is not served yet")
    }

    /// Every request to a registry endpoint so far, in the order they came.
    pub fn requests(&self) -> Vec<ApiRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// Answer every registry request with this status from now on (`401`
    /// for a rejected token, `503` for a platform out of order), or as
    /// usual again with `None`.
    pub fn fail_api(&self, status: Option<u16>) {
        *self.api_failure.lock().unwrap() = status;
    }

    pub fn machine(&self, machine_id: &str) -> Option<FakeMachine> {
        self.registry
            .lock()
            .unwrap()
            .machines
            .get(machine_id)
            .cloned()
    }

    /// The sessions of a machine as `GET /api/registry` lists them, by id.
    pub fn sessions(&self, machine_id: &str) -> Vec<Value> {
        self.registry
            .lock()
            .unwrap()
            .sessions
            .iter()
            .filter(|((machine, _), _)| machine == machine_id)
            .map(|(_, session)| session.clone())
            .collect()
    }

    pub fn session(&self, machine_id: &str, session_id: &str) -> Option<Value> {
        self.registry
            .lock()
            .unwrap()
            .sessions
            .get(&(machine_id.to_string(), session_id.to_string()))
            .cloned()
    }

    /// What the platform's sweep does when a machine's heartbeats stop, or
    /// undoes when they come back.
    pub fn set_online(&self, machine_id: &str, online: bool) {
        let mut registry = self.registry.lock().unwrap();
        if let Some(machine) = registry.machines.get_mut(machine_id) {
            machine.online = online;
            if !online {
                machine.daemon = false;
            }
        }
        registry.version += 1;
    }

    /// What the platform's sweep does when a machine's heartbeats with
    /// `"daemon": true` stop while others go on.
    pub fn set_daemon(&self, machine_id: &str, daemon: bool) {
        let mut registry = self.registry.lock().unwrap();
        if let Some(machine) = registry.machines.get_mut(machine_id) {
            machine.daemon = daemon;
        }
        registry.version += 1;
    }

    /// Remove a machine and its sessions, as `DELETE /api/machines/{id}`
    /// does for one that is offline. Its next call is answered 404.
    pub fn remove_machine(&self, machine_id: &str) {
        let mut registry = self.registry.lock().unwrap();
        registry.machines.remove(machine_id);
        registry
            .sessions
            .retain(|(machine, _), _| machine != machine_id);
        registry.version += 1;
    }

    /// One registry request: the status and the JSON to answer with.
    fn api(&self, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
        let body: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        self.requests.lock().unwrap().push(ApiRequest {
            method: method.to_string(),
            path: path.to_string(),
            body: body.clone(),
        });
        if let Some(status) = *self.api_failure.lock().unwrap() {
            let code = if status == 401 {
                "unauthorized"
            } else {
                "unavailable"
            };
            return (status, api_error(code));
        }
        let segments: Vec<String> = path
            .trim_start_matches("/api/")
            .split('/')
            .map(percent_decode)
            .collect();
        let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
        let mut registry = self.registry.lock().unwrap();
        match (method, segments.as_slice()) {
            ("GET", ["registry"]) => (200, registry.snapshot()),
            (_, ["machines", id, ..]) if !valid_machine_id(id) => {
                (400, api_error("invalid_machine_id"))
            }
            ("PUT", ["machines", id]) => (200, registry.announce(id, &body)),
            ("DELETE", ["machines", id]) => {
                if registry.machines.get(*id).is_some_and(|m| m.online) {
                    return (409, api_error("machine_online"));
                }
                registry.machines.remove(*id);
                registry.sessions.retain(|(machine, _), _| machine != id);
                registry.version += 1;
                (204, Value::Null)
            }
            (_, ["machines", id, ..]) if !registry.machines.contains_key(*id) => {
                (404, api_error("not_found"))
            }
            ("POST", ["machines", id, "heartbeat"]) => {
                registry.seen(id, body["daemon"] == true);
                if let Some(machine) = registry.machines.get_mut(*id) {
                    machine.heartbeats += 1;
                }
                (204, Value::Null)
            }
            ("PUT", ["machines", id, "sessions"]) => {
                let Some(sessions) = body["sessions"].as_array() else {
                    return (400, api_error("invalid_request"));
                };
                if sessions.len() > 10_000 {
                    return (400, api_error("too_many_sessions"));
                }
                let mut listed = Vec::new();
                for session in sessions {
                    let Some(session_id) = session["id"].as_str() else {
                        return (400, api_error("invalid_request"));
                    };
                    registry.upsert(id, session_id, session);
                    listed.push(session_id.to_string());
                }
                let before = registry.sessions.len();
                registry.sessions.retain(|(machine, session_id), session| {
                    machine != id || session["daemon"] != true || listed.contains(session_id)
                });
                let removed = before - registry.sessions.len();
                registry.version += 1;
                (200, json!({"sessions": listed.len(), "removed": removed}))
            }
            ("PUT", ["machines", id, "sessions", session_id]) => {
                (200, registry.upsert(id, session_id, &body))
            }
            ("DELETE", ["machines", id, "sessions", session_id]) => {
                registry
                    .sessions
                    .remove(&(id.to_string(), session_id.to_string()));
                registry.version += 1;
                (204, Value::Null)
            }
            _ => (404, api_error("not_found")),
        }
    }
}

impl FakeRegistry {
    /// A heartbeat or an announcement arrived.
    fn seen(&mut self, machine_id: &str, daemon: bool) {
        if let Some(machine) = self.machines.get_mut(machine_id) {
            machine.online = true;
            machine.daemon |= daemon;
            machine.last_seen_at = now_rfc3339();
        }
    }

    fn announce(&mut self, machine_id: &str, body: &Value) -> Value {
        let now = now_rfc3339();
        let machine = self
            .machines
            .entry(machine_id.to_string())
            .or_insert_with(|| FakeMachine {
                id: machine_id.to_string(),
                name: String::new(),
                platform: String::new(),
                daemon: false,
                online: true,
                announcements: 0,
                heartbeats: 0,
                last_seen_at: now.clone(),
                created_at: now,
            });
        machine.name = body["name"].as_str().unwrap_or_default().to_string();
        machine.platform = body["platform"].as_str().unwrap_or_default().to_string();
        machine.announcements += 1;
        self.seen(machine_id, body["daemon"] == true);
        self.version += 1;
        machine_json(&self.machines[machine_id])
    }

    /// Create or replace a session from the fields a writer sent. A writer
    /// that leaves `lastActivityAt` out gets the time of the request for a
    /// new session and keeps the stored time for an existing one.
    fn upsert(&mut self, machine_id: &str, session_id: &str, fields: &Value) -> Value {
        let now = now_rfc3339();
        let key = (machine_id.to_string(), session_id.to_string());
        let existing = self.sessions.get(&key);
        let stored = |name: &str| existing.and_then(|session| session.get(name).cloned());
        let field = |name: &str, default: Value| match fields.get(name) {
            Some(value) if !value.is_null() => value.clone(),
            _ => default,
        };
        let session = json!({
            "machineId": machine_id,
            "id": session_id,
            "workspace": field("workspace", json!("")),
            "cwd": field("cwd", json!("")),
            "state": field("state", json!("idle")),
            "blockedReason": field("blockedReason", Value::Null),
            "model": field("model", json!("")),
            "thinkingLevel": field("thinkingLevel", json!("")),
            "name": field("name", Value::Null),
            "firstMessage": field("firstMessage", Value::Null),
            "resumeArgv": field("resumeArgv", json!([])),
            "daemon": field("daemon", json!(false)),
            "headless": field("headless", json!(false)),
            "lastActivityAt": field(
                "lastActivityAt",
                stored("lastActivityAt").unwrap_or_else(|| json!(now)),
            ),
            "createdAt": stored("createdAt").unwrap_or_else(|| json!(now)),
            "updatedAt": now,
        });
        self.sessions.insert(key, session.clone());
        self.version += 1;
        session
    }

    /// The answer to `GET /api/registry` without `since`: everything.
    fn snapshot(&self) -> Value {
        json!({
            "version": self.version,
            "reset": true,
            "machines": self.machines.values().map(machine_json).collect::<Vec<_>>(),
            "sessions": self.sessions.values().collect::<Vec<_>>(),
            "projects": [],
            "chats": [],
            "deleted": [],
        })
    }
}

fn machine_json(machine: &FakeMachine) -> Value {
    json!({
        "id": machine.id,
        "name": machine.name,
        "platform": machine.platform,
        "daemon": machine.daemon,
        "online": machine.online,
        "lastSeenAt": machine.last_seen_at,
        "createdAt": machine.created_at,
    })
}

fn api_error(code: &str) -> Value {
    json!({"error": {"code": code, "message": code.replace('_', " ")}})
}

/// 1 to 128 characters of `A-Z a-z 0-9 _ -`, and never `cloud`.
fn valid_machine_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id != "cloud"
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn now_rfc3339() -> String {
    crate::registry::timestamp(std::time::SystemTime::now())
}

/// A registry client for a served fake, with the credentials every test
/// uses.
pub fn registry_client(fake: &FakePlatform) -> Arc<RegistryClient> {
    Arc::new(RegistryClient::new(
        reqwest::Client::new(),
        fake.url(),
        Arc::new(TestCredentials),
    ))
}

/// Serve `fake` on a loopback port; returns the S2 base URL, `http://…/v1`.
/// The registry endpoints are served on the same port, under
/// [`FakePlatform::url`].
pub async fn serve(fake: Arc<FakePlatform>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    *fake.url.lock().unwrap() = Some(format!("http://{address}"));
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            let fake = fake.clone();
            tokio::spawn(async move {
                let _ = handle(socket, fake).await;
            });
        }
    });
    format!("http://{address}/v1")
}

async fn handle(socket: tokio::net::TcpStream, fake: Arc<FakePlatform>) -> std::io::Result<()> {
    let (reader, mut writer) = socket.into_split();
    let mut reader = BufReader::new(reader);
    loop {
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).await? == 0 {
            return Ok(());
        }
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("").to_string();
        let mut headers = BTreeMap::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await?;
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }
        if let Some(auth) = headers.get("authorization") {
            fake.seen_tokens.lock().unwrap().push(auth.clone());
        }
        let length: usize = headers
            .get("content-length")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0_u8; length];
        reader.read_exact(&mut body).await?;
        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        let query: BTreeMap<String, String> = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .filter_map(|pair| pair.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        if path.starts_with("/api/") {
            let (status, answer) = fake.api(&method, path, &body);
            let answer = match answer {
                Value::Null => String::new(),
                answer => answer.to_string(),
            };
            write_response(&mut writer, status, "application/json", answer.as_bytes()).await?;
            continue;
        }
        let Some(rest) = path.strip_prefix("/v1/streams/") else {
            write_response(&mut writer, 404, "application/json", b"{}").await?;
            continue;
        };
        let (stream, tail) = rest.rsplit_once("/records").unwrap_or((rest, ""));
        let stream = percent_decode(stream);
        match (method.as_str(), tail) {
            ("POST", "") => {
                let input: Value = serde_json::from_slice(&body).unwrap();
                let records = input["records"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|record| {
                        let headers = record["headers"]
                            .as_array()
                            .map(|pairs| {
                                pairs
                                    .iter()
                                    .map(|pair| {
                                        (
                                            pair[0].as_str().unwrap_or("").to_string(),
                                            pair[1].as_str().unwrap_or("").to_string(),
                                        )
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        (headers, record["body"].as_str().unwrap_or("").to_string())
                    })
                    .collect();
                let (start, end) = fake.append(&stream, records);
                let ack = json!({
                    "start": {"seq_num": start, "timestamp": 1},
                    "end": {"seq_num": end, "timestamp": 1},
                    "tail": {"seq_num": end, "timestamp": 1},
                });
                write_response(
                    &mut writer,
                    200,
                    "application/json",
                    ack.to_string().as_bytes(),
                )
                .await?;
            }
            ("GET", "/tail") => {
                let tail = fake.records(&stream).len();
                let value = json!({"tail": {"seq_num": tail, "timestamp": 1}});
                write_response(
                    &mut writer,
                    200,
                    "application/json",
                    value.to_string().as_bytes(),
                )
                .await?;
            }
            ("GET", "")
                if headers.get("accept").map(String::as_str) == Some("text/event-stream") =>
            {
                let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();
                fake.waiters.lock().unwrap().push(notify_tx);
                let existing = fake.records(&stream);
                let mut next = match (query.get("seq_num"), query.get("tail_offset")) {
                    (Some(seq), _) => seq.parse::<u64>().unwrap(),
                    (None, Some(offset)) => {
                        (existing.len() as u64).saturating_sub(offset.parse::<u64>().unwrap())
                    }
                    _ => 0,
                };
                writer
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                    )
                    .await?;
                loop {
                    let records = fake.records(&stream);
                    let batch: Vec<Value> = records
                        .iter()
                        .filter(|record| record.seq >= next)
                        .map(|record| {
                            json!({
                                "seq_num": record.seq,
                                "timestamp": now_ms(),
                                "headers": record.headers,
                                "body": record.body,
                            })
                        })
                        .collect();
                    if let Some(last) = records.last()
                        && !batch.is_empty()
                    {
                        next = last.seq + 1;
                        let data =
                            json!({"records": batch, "tail": {"seq_num": next, "timestamp": 1}});
                        writer
                            .write_all(
                                format!("event: batch\nid: {next},1,1\ndata: {data}\n\n")
                                    .as_bytes(),
                            )
                            .await?;
                    }
                    tokio::select! {
                        changed = notify_rx.recv() => {
                            if changed.is_none() { return Ok(()); }
                        }
                        _ = tokio::time::sleep(Duration::from_millis(200)) => {
                            let ping = json!({"timestamp": now_ms(), "tail": {"seq_num": next, "timestamp": 1}});
                            writer.write_all(format!("event: ping\ndata: {ping}\n\n").as_bytes()).await?;
                        }
                    }
                }
            }
            ("GET", "") => {
                let records = fake.records(&stream);
                let last: Vec<Value> = records
                    .last()
                    .map(|record| {
                        vec![json!({
                            "seq_num": record.seq,
                            "timestamp": 1,
                            "headers": record.headers,
                            "body": record.body,
                        })]
                    })
                    .unwrap_or_default();
                let value = json!({"records": last});
                write_response(
                    &mut writer,
                    200,
                    "application/json",
                    value.to_string().as_bytes(),
                )
                .await?;
            }
            _ => write_response(&mut writer, 404, "application/json", b"{}").await?,
        }
    }
}

/// The clients send stream names and ids as one percent-encoded path segment.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(value) = u8::from_str_radix(&text[i + 1..i + 3], 16)
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn write_response(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    writer.write_all(head.as_bytes()).await?;
    writer.write_all(body).await
}

/// Unix milliseconds now, for `exp` headers in tests.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Credentials every test uses: a fixed token, a device named `test-machine`.
pub struct TestCredentials;

impl RemoteCredentials for TestCredentials {
    fn platform_url(&self) -> String {
        "http://127.0.0.1:1".into()
    }

    fn device_name(&self) -> String {
        "test-machine".into()
    }

    fn access_token(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + '_>> {
        Box::pin(async { Ok("secret-token".to_string()) })
    }
}
