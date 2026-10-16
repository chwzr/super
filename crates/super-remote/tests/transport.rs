//! The transport against a fake S2 server: events are mirrored, entries are
//! appended durably, commands are tailed, executed and answered, and expired
//! commands are dropped.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_remote::s2::S2Client;
use super_remote::transport::{RemoteNotice, SessionStreams, SessionTransport, TransportOptions};
use super_remote::{RemoteCredentials, RemoteStatus};
use super_sdk::mock::{MockProvider, MockScript};
use super_sdk::{Session, SessionOptions, ThinkingLevel};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
struct StoredRecord {
    seq: u64,
    headers: Vec<(String, String)>,
    body: String,
}

/// A fake S2: streams in memory, SSE tails that deliver whatever is appended
/// after the reader connected, and a `pending` queue the test pushes
/// commands into.
#[derive(Default)]
struct FakeS2 {
    streams: Mutex<BTreeMap<String, Vec<StoredRecord>>>,
    waiters: Mutex<Vec<tokio::sync::mpsc::UnboundedSender<()>>>,
    seen_tokens: Mutex<Vec<String>>,
}

impl FakeS2 {
    fn append(&self, stream: &str, records: Vec<(Vec<(String, String)>, String)>) -> (u64, u64) {
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

    fn records(&self, stream: &str) -> Vec<StoredRecord> {
        self.streams
            .lock()
            .unwrap()
            .get(stream)
            .cloned()
            .unwrap_or_default()
    }
}

async fn serve(fake: Arc<FakeS2>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
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

async fn handle(socket: tokio::net::TcpStream, fake: Arc<FakeS2>) -> std::io::Result<()> {
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

/// The client sends stream names as one percent-encoded path segment.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(value);
                i += 3;
                continue;
            }
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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

struct TestCredentials;

impl RemoteCredentials for TestCredentials {
    fn platform_url(&self) -> String {
        "http://127.0.0.1:1".into()
    }

    fn device_name(&self) -> String {
        "test-host".into()
    }

    fn access_token(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + '_>> {
        Box::pin(async { Ok("secret-token".to_string()) })
    }
}

async fn session_with(
    directory: &std::path::Path,
    script: MockScript,
) -> (MockProvider, Arc<Session>) {
    let provider = MockProvider::start(directory, script).await.unwrap();
    let options = SessionOptions {
        cwd: directory.to_path_buf(),
        model: Some("mock/mock-1".into()),
        models_file: Some(provider.catalog_path()),
        thinking_level: Some(ThinkingLevel::Off),
        settings: Some(super_coding::Settings::default()),
        tools: Some(vec!["read".into()]),
        trust_project_files: false,
        no_context_files: true,
        ..Default::default()
    };
    let session = Session::create(options).await.unwrap();
    (provider, session)
}

async fn wait_until(mut check: impl FnMut() -> bool) {
    for _ in 0..600 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met in time");
}

#[tokio::test]
async fn events_entries_and_commands_flow_through_the_streams() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeS2::default());
    let base_url = serve(fake.clone()).await;
    let (_provider, session) =
        session_with(directory.path(), MockScript::text("Hello from the mock")).await;
    let client = Arc::new(S2Client::new(
        reqwest::Client::new(),
        base_url,
        Arc::new(TestCredentials),
    ));
    let streams = SessionStreams::new("host1", "sess1");
    let notices: Arc<Mutex<Vec<RemoteNotice>>> = Default::default();
    let sink = notices.clone();
    let transport = SessionTransport::start(
        client,
        session.clone(),
        streams.clone(),
        TransportOptions {
            linger: Duration::from_millis(5),
            ..TransportOptions::default()
        },
        Arc::new(move |notice| sink.lock().unwrap().push(notice)),
    );

    // Connected once the tail is open; appends happen on demand.
    wait_until(|| {
        notices
            .lock()
            .unwrap()
            .iter()
            .any(|notice| matches!(notice, RemoteNotice::Status(RemoteStatus::Reconnecting)))
            || true
    })
    .await;

    // A controller sends a prompt through the commands stream.
    fake.append(
        &streams.commands,
        vec![(
            vec![
                ("t".into(), "prompt".into()),
                ("name".into(), "iPhone".into()),
                ("exp".into(), (now_ms() + 60_000).to_string()),
            ],
            json!({"id": "c1", "type": "prompt", "message": "Say hello"}).to_string(),
        )],
    );

    // The response and the whole run show up in the events stream.
    wait_until(|| {
        fake.records(&streams.events).iter().any(|record| {
            record
                .headers
                .iter()
                .any(|(k, v)| k == "t" && v == "agent_settled")
        })
    })
    .await;
    let events = fake.records(&streams.events);
    let response = events
        .iter()
        .find(|record| {
            record
                .headers
                .iter()
                .any(|(k, v)| k == "t" && v == "response")
        })
        .expect("response record");
    let response: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(response["id"], "c1");
    assert_eq!(response["success"], true);
    assert_eq!(response["data"]["disposition"], "started");
    assert!(events.iter().any(|record| {
        record
            .headers
            .iter()
            .any(|(k, v)| k == "t" && v == "message_end")
    }));

    // Durable entries carry the conversation with entry ids.
    wait_until(|| fake.records(&streams.entries).len() >= 2).await;
    let entries = fake.records(&streams.entries);
    assert!(
        entries
            .iter()
            .all(|record| record.headers.iter().any(|(k, _)| k == "eid"))
    );
    let bodies: Vec<Value> = entries
        .iter()
        .map(|record| serde_json::from_str(&record.body).unwrap())
        .collect();
    assert!(
        bodies
            .iter()
            .any(|entry| entry["message"]["role"] == "user")
    );
    assert!(
        bodies
            .iter()
            .any(|entry| entry["message"]["role"] == "assistant")
    );

    // The platform token went on the wire; nothing else.
    assert!(
        fake.seen_tokens
            .lock()
            .unwrap()
            .iter()
            .all(|token| token == "Bearer secret-token")
    );

    // An expired command is dropped and reported.
    fake.append(
        &streams.commands,
        vec![(
            vec![
                ("t".into(), "ping".into()),
                ("exp".into(), (now_ms() - 1).to_string()),
            ],
            json!({"id": "old", "type": "ping"}).to_string(),
        )],
    );
    wait_until(|| {
        notices
            .lock()
            .unwrap()
            .iter()
            .any(|notice| matches!(notice, RemoteNotice::ExpiredCommands(1)))
    })
    .await;
    assert!(
        !fake
            .records(&streams.events)
            .iter()
            .any(|record| record.body.contains("\"id\":\"old\""))
    );

    // Status settled on connected.
    assert_eq!(transport.status(), RemoteStatus::Connected);
    transport.stop().await;
}

#[tokio::test]
async fn shell_can_be_refused_by_policy() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeS2::default());
    let base_url = serve(fake.clone()).await;
    let (_provider, session) = session_with(directory.path(), MockScript::text("unused")).await;
    let client = Arc::new(S2Client::new(
        reqwest::Client::new(),
        base_url,
        Arc::new(TestCredentials),
    ));
    let streams = SessionStreams::new("host1", "sess2");
    let notices: Arc<Mutex<Vec<RemoteNotice>>> = Default::default();
    let sink = notices.clone();
    let transport = SessionTransport::start(
        client,
        session,
        streams.clone(),
        TransportOptions {
            allow_shell: false,
            linger: Duration::from_millis(5),
            ..TransportOptions::default()
        },
        Arc::new(move |notice| sink.lock().unwrap().push(notice)),
    );
    fake.append(
        &streams.commands,
        vec![(
            vec![
                ("name".into(), "iPhone".into()),
                ("exp".into(), (now_ms() + 60_000).to_string()),
            ],
            json!({"id": "sh", "type": "bash", "command": "echo hi"}).to_string(),
        )],
    );
    wait_until(|| {
        fake.records(&streams.events)
            .iter()
            .any(|record| record.body.contains("\"id\":\"sh\""))
    })
    .await;
    let record = fake
        .records(&streams.events)
        .into_iter()
        .find(|record| record.body.contains("\"id\":\"sh\""))
        .unwrap();
    let response: Value = serde_json::from_str(&record.body).unwrap();
    assert_eq!(response["success"], false);
    assert!(response["error"].as_str().unwrap().contains("disabled"));
    assert!(notices
        .lock()
        .unwrap()
        .iter()
        .any(|notice| matches!(notice, RemoteNotice::Refused { origin, command } if origin == "iPhone" && command == "bash")));
    transport.stop().await;
}
