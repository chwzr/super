//! A socket client follows an agent's events live: `subscribe_agent` names
//! the agent's path, and its events come as `agent_event` lines, in the
//! shape its nested `events` stream carries them.
//! Own test binary: it points HOME at a temporary directory whose settings
//! switch Subagents on, so the settings of the machine running the tests
//! play no part. Unix only, like the socket.

#![cfg(unix)]

use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use super_daemon::DaemonConfig;
use super_daemon::host::HostConfig;
use super_daemon::log::Level;
use super_daemon::paths::DaemonPaths;
use super_sdk::mock::{MockProvider, MockScript, MockTurn};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio_util::sync::CancellationToken;

struct Client {
    read: BufReader<OwnedReadHalf>,
    write: OwnedWriteHalf,
    counter: u64,
}

impl Client {
    async fn connect(socket: &PathBuf, name: &str) -> Client {
        let stream = UnixStream::connect(socket).await.expect("connect");
        let (read, write) = stream.into_split();
        let mut client = Client {
            read: BufReader::new(read),
            write,
            counter: 0,
        };
        let reply = client
            .call(json!({
                "type": "hello", "client": "macos", "name": name,
                "version": "test", "protocolVersions": [2, 2], "taskControls": true
            }))
            .await;
        assert_eq!(reply["success"], true, "{reply}");
        client
    }

    async fn send(&mut self, mut command: Value) -> String {
        self.counter += 1;
        let id = format!("c{}", self.counter);
        command["id"] = Value::String(id.clone());
        let line = format!("{command}\n");
        self.write.write_all(line.as_bytes()).await.expect("write");
        self.write.flush().await.expect("flush");
        id
    }

    async fn next(&mut self) -> Value {
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(20), self.read.read_line(&mut line))
            .await
            .expect("the daemon answered in time")
            .expect("read a line");
        assert!(read > 0, "the connection stays open");
        serde_json::from_str(line.trim_end()).expect("JSON line")
    }

    async fn read_until(&mut self, mut predicate: impl FnMut(&Value) -> bool) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let value = self.next().await;
            let done = predicate(&value);
            seen.push(value);
            if done {
                return seen;
            }
        }
    }

    /// Send a command; the lines up to its response, the response last.
    async fn exchange(&mut self, command: Value) -> Vec<Value> {
        let id = self.send(command).await;
        self.read_until(|value| value["type"] == "response" && value["id"] == id)
            .await
    }

    async fn call(&mut self, command: Value) -> Value {
        self.exchange(command).await.pop().unwrap()
    }
}

#[tokio::test]
async fn a_client_follows_an_agents_events_live_by_its_path() {
    let home = tempfile::Builder::new().prefix("sh").tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".super/agent")).unwrap();
    std::fs::write(
        home.path().join(".super/agent/settings.json"),
        r#"{"subagents": {"enabled": true}}"#,
    )
    .unwrap();
    // SAFETY: the only test in this binary; set before any thread reads the
    // environment.
    unsafe { std::env::set_var("HOME", home.path()) };

    let dir = tempfile::Builder::new().prefix("sd").tempdir().unwrap();
    // The agent the first prompt waits for, then a message that resumes it
    // in the background; after that every answer, the agent's and the main
    // session's, is the same.
    let provider = MockProvider::start(
        dir.path().join("mock"),
        MockScript::default()
            .then(vec![MockTurn::ToolCall {
                id: "call_1".into(),
                name: "Agent".into(),
                arguments: json!({"description": "Count", "prompt": "count the files", "run_in_background": false}),
            }])
            .then(vec![MockTurn::Text("three files".into())])
            .then(vec![MockTurn::Text("done".into())])
            .then(vec![MockTurn::ToolCall {
                id: "call_2".into(),
                name: "SendMessage".into(),
                arguments: json!({"to": "Count", "message": "count again"}),
            }])
            .then(vec![MockTurn::Text("ok".into())]),
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
            session_dir: dir.path().join("sessions"),
            idle_unload: Duration::ZERO,
            models_file: Some(provider.catalog_path()),
            no_context_files: true,
        },
        machine_id: "01TEST".into(),
        machine_name: "testbox".into(),
        log_level: Level::Debug,
        log_to_stderr: false,
        sweep_interval: Duration::from_secs(60),
        presence: None,
        protocol_versions: super_sdk::daemon::PROTOCOL_VERSIONS,
    };
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(super_daemon::run(config, shutdown.clone()));
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let mut app = Client::connect(&socket, "Mac").await;
    let mut other = Client::connect(&socket, "Other").await;
    // Subscribing needs an attached session.
    let refused = app
        .call(json!({"type": "subscribe_agent", "agent": "a1b2c3d4e"}))
        .await;
    assert_eq!(refused["success"], false);
    assert_eq!(refused["error"], "not attached to a session");

    let created = app
        .call(json!({"type": "create_session", "cwd": dir.path().join("work"), "model": "mock/mock-1"}))
        .await;
    assert_eq!(created["success"], true, "{created}");
    let session_id = created["data"]["session"]["sessionId"].clone();
    for client in [&mut app, &mut other] {
        let attached = client
            .call(json!({"type": "attach", "sessionId": session_id}))
            .await;
        assert_eq!(attached["success"], true, "{attached}");
    }
    let accepted = app
        .call(json!({"type": "prompt", "message": "count with an agent"}))
        .await;
    assert_eq!(accepted["success"], true, "{accepted}");
    app.read_until(|v| v["type"] == "agent_settled").await;
    let listed = app.call(json!({"type": "get_agents"})).await;
    let agent = listed["data"]["agents"][0].clone();
    assert_eq!(agent["name"], "Count", "{listed}");
    let path = agent["path"].as_str().unwrap().to_string();

    let unknown = app
        .call(json!({"type": "subscribe_agent", "agent": "nobody"}))
        .await;
    assert_eq!(unknown["success"], false);
    assert_eq!(
        unknown["error"],
        "No agent of this session has the id or name 'nobody'; ListAgents lists them."
    );
    let subscribed = app
        .call(json!({"type": "subscribe_agent", "agent": format!("/{path}/")}))
        .await;
    assert_eq!(subscribed["success"], true, "{subscribed}");
    assert_eq!(subscribed["data"]["agent"], json!(path));
    let entries = app
        .call(json!({"type": "get_agent_entries", "agent": path}))
        .await;
    assert_eq!(
        subscribed["data"]["sessionId"],
        entries["data"]["sessionId"]
    );

    // A message resumes the agent; its run comes to the subscriber live.
    let accepted = app
        .call(json!({"type": "prompt", "message": "again"}))
        .await;
    assert_eq!(accepted["success"], true, "{accepted}");
    let lines = app
        .read_until(|v| v["type"] == "agent_event" && v["event"]["type"] == "agent_settled")
        .await;
    let events: Vec<&Value> = lines
        .iter()
        .filter(|v| v["type"] == "agent_event")
        .collect();
    assert!(events.iter().all(|v| v["agent"] == json!(path)));
    let kinds: Vec<&str> = events
        .iter()
        .map(|v| v["event"]["type"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(kinds.first(), Some(&"agent_start"), "{kinds:?}");
    assert!(kinds.contains(&"message_end"), "{kinds:?}");
    assert!(
        events.iter().any(|v| v["event"]["type"] == "message_end"
            && v["event"]["message"]["role"] == "assistant"
            && v["event"]["message"]["content"][0]["text"] == "ok"),
        "the agent's answer: {events:?}"
    );

    // Unsubscribed, nothing more comes; a client that never subscribed got
    // none of it.
    let gone = app
        .call(json!({"type": "unsubscribe_agent", "agent": path}))
        .await;
    assert_eq!(gone["data"], json!({"agent": path, "subscribed": true}));
    let again = app
        .call(json!({"type": "unsubscribe_agent", "agent": path}))
        .await;
    assert_eq!(again["data"]["subscribed"], false);
    let seen = other.exchange(json!({"type": "get_state"})).await;
    assert!(seen.iter().all(|v| v["type"] != "agent_event"), "{seen:?}");

    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(10), task).await;
}
