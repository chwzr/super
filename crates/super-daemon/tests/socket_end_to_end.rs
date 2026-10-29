//! The daemon as a client sees it: a real Unix socket, the mock provider.

#![cfg(unix)]

use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use super_daemon::DaemonConfig;
use super_daemon::host::HostConfig;
use super_daemon::log::Level;
use super_daemon::paths::DaemonPaths;
use super_sdk::mock::{MockProvider, MockScript};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio_util::sync::CancellationToken;

struct Daemon {
    _provider: MockProvider,
    dir: tempfile::TempDir,
    socket: PathBuf,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<anyhow::Result<i32>>,
}

impl Daemon {
    async fn start(idle_unload: Duration, sweep_interval: Duration) -> Daemon {
        // `sun_path` is short; keep the temp dir's own name and a short file name.
        let dir = tempfile::Builder::new()
            .prefix("sd")
            .tempdir()
            .expect("tempdir");
        let provider = MockProvider::start(
            dir.path().join("mock"),
            MockScript::text("Hello from the mock provider."),
        )
        .await
        .expect("mock provider");
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
                idle_unload,
                models_file: Some(provider.catalog_path()),
                no_context_files: true,
            },
            host_id: "01TEST".into(),
            host_name: "testbox".into(),
            log_level: Level::Debug,
            log_to_stderr: false,
            sweep_interval,
            presence: None,
        };
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(super_daemon::run(config, shutdown.clone()));
        for _ in 0..200 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(socket.exists(), "the daemon created its socket");
        Daemon {
            _provider: provider,
            dir,
            socket,
            shutdown,
            task,
        }
    }

    fn work_dir(&self) -> String {
        self.dir.path().join("work").display().to_string()
    }

    async fn raw_client(&self) -> Client {
        let stream = UnixStream::connect(&self.socket).await.expect("connect");
        let (read, write) = stream.into_split();
        Client {
            read: BufReader::new(read),
            write,
            counter: 0,
        }
    }

    /// A connection that already said hello as `name`.
    async fn client(&self, name: &str) -> Client {
        let mut client = self.raw_client().await;
        let reply = client
            .call(json!({
                "type": "hello", "client": "other", "name": name,
                "version": "test", "protocolVersions": [1, 1]
            }))
            .await;
        assert_eq!(reply["success"], true, "{reply}");
        assert_eq!(reply["data"]["protocolVersion"], 1);
        assert_eq!(reply["data"]["hostName"], "testbox");
        client
    }

    async fn stop(self) -> i32 {
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), self.task)
            .await
            .expect("run returned in time")
            .expect("run did not panic")
            .expect("run succeeded")
    }
}

struct Client {
    read: BufReader<OwnedReadHalf>,
    write: OwnedWriteHalf,
    counter: u64,
}

impl Client {
    async fn send(&mut self, mut command: Value) -> String {
        self.counter += 1;
        let id = format!("c{}", self.counter);
        command["id"] = Value::String(id.clone());
        let line = format!("{command}\n");
        self.write.write_all(line.as_bytes()).await.expect("write");
        self.write.flush().await.expect("flush");
        id
    }

    /// The next line, or `None` when the daemon closed the connection.
    async fn next(&mut self) -> Option<Value> {
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(20), self.read.read_line(&mut line))
            .await
            .expect("the daemon answered in time")
            .expect("read a line");
        if read == 0 {
            return None;
        }
        Some(serde_json::from_str(line.trim_end()).expect("JSON line"))
    }

    async fn read_until(&mut self, mut predicate: impl FnMut(&Value) -> bool) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let value = self.next().await.expect("the connection stays open");
            let done = predicate(&value);
            seen.push(value);
            if done {
                return seen;
            }
        }
    }

    /// Send a command and return its response, skipping events on the way.
    async fn call(&mut self, command: Value) -> Value {
        let id = self.send(command).await;
        self.read_until(|value| value["type"] == "response" && value["id"] == id)
            .await
            .pop()
            .unwrap()
    }
}

fn text_of(lines: &[Value]) -> String {
    lines
        .iter()
        .filter(|value| value["type"] == "message_update")
        .filter_map(|value| {
            let inner = &value["assistantMessageEvent"];
            (inner["type"] == "text_delta")
                .then(|| inner["delta"].as_str().unwrap_or_default().to_string())
        })
        .collect()
}

#[tokio::test]
async fn hello_must_come_first() {
    let daemon = Daemon::start(Duration::ZERO, Duration::from_secs(60)).await;
    let mut client = daemon.raw_client().await;
    let reply = client.call(json!({"type": "ping"})).await;
    assert_eq!(reply["command"], "hello");
    assert_eq!(reply["success"], false);
    assert!(
        reply["error"].as_str().unwrap().contains("say hello first"),
        "{reply}"
    );
    assert!(
        client.next().await.is_none(),
        "the daemon closed the connection"
    );
    daemon.stop().await;
}

#[tokio::test]
async fn a_protocol_mismatch_names_both_ranges() {
    let daemon = Daemon::start(Duration::ZERO, Duration::from_secs(60)).await;
    let mut client = daemon.raw_client().await;
    let reply = client
        .call(json!({
            "type": "hello", "client": "cli", "version": "9.9.9",
            "protocolVersions": [9, 9]
        }))
        .await;
    assert_eq!(reply["success"], false);
    assert_eq!(
        reply["error"],
        "this SUPER daemon speaks protocol 1; this client speaks 9"
    );
    assert_eq!(reply["data"]["protocolVersions"], json!([1, 1]));
    assert!(client.next().await.is_none());
    daemon.stop().await;
}

#[tokio::test]
async fn two_clients_share_one_session_until_it_is_stopped_and_resumed() {
    let daemon = Daemon::start(Duration::ZERO, Duration::from_secs(60)).await;
    let mut a = daemon.client("A").await;
    let mut b = daemon.client("B").await;

    let created = a
        .call(json!({"type": "create_session", "cwd": daemon.work_dir(), "model": "mock/mock-1"}))
        .await;
    assert_eq!(created["success"], true, "{created}");
    let session = &created["data"]["session"];
    assert_eq!(session["hosted"], true);
    assert_eq!(session["state"], "idle");
    let id = session["sessionId"].as_str().unwrap().to_string();

    // B heard about the new session without being attached to anything.
    let seen = b.read_until(|v| v["type"] == "session_created").await;
    assert_eq!(seen.last().unwrap()["session"]["sessionId"], id);

    // A session command before attaching is refused.
    let refused = a.call(json!({"type": "get_state"})).await;
    assert_eq!(refused["success"], false);
    assert_eq!(refused["error"], "not attached to a session");

    let attached = a.call(json!({"type": "attach", "sessionId": id})).await;
    assert_eq!(attached["success"], true, "{attached}");
    assert_eq!(attached["data"]["session"]["attachedClients"], 1);
    let attached = b.call(json!({"type": "attach", "sessionId": id})).await;
    assert_eq!(attached["data"]["session"]["attachedClients"], 2);

    let accepted = a.call(json!({"type": "prompt", "message": "say hi"})).await;
    assert_eq!(accepted["data"]["disposition"], "started");
    let a_lines = a.read_until(|v| v["type"] == "agent_settled").await;
    let b_lines = b.read_until(|v| v["type"] == "agent_settled").await;
    assert_eq!(text_of(&a_lines), "Hello from the mock provider.");
    assert_eq!(text_of(&b_lines), "Hello from the mock provider.");
    // The daemon-level updates come from another task than the session
    // events, so collect until the idle update is in and then look.
    let state_update =
        |v: &Value, state: &str| v["type"] == "session_updated" && v["session"]["state"] == state;
    let mut b_lines = b_lines;
    if !b_lines.iter().any(|v| state_update(v, "idle")) {
        b_lines.extend(b.read_until(|v| state_update(v, "idle")).await);
    }
    assert!(
        b_lines.iter().any(|v| state_update(v, "working")),
        "B saw the session go to work"
    );

    // The transcript is the same from both pipes.
    let entries = b.call(json!({"type": "get_entries"})).await;
    let entry_count = entries["data"]["entries"].as_array().unwrap().len();
    assert!(entry_count >= 2, "a user and an assistant entry: {entries}");

    let listed = a.call(json!({"type": "list_sessions"})).await;
    let sessions = listed["data"]["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["attachedClients"], 2);
    assert_eq!(sessions[0]["firstMessage"], "say hi");

    let detached = b.call(json!({"type": "detach"})).await;
    assert_eq!(detached["data"]["sessionId"], id);

    let stopped = a
        .call(json!({"type": "stop_session", "sessionId": id}))
        .await;
    assert_eq!(stopped["success"], true, "{stopped}");
    let ended = a.read_until(|v| v["type"] == "session_ended").await;
    assert_eq!(ended.last().unwrap()["reason"], "stopped");
    let ended = b.read_until(|v| v["type"] == "session_ended").await;
    assert_eq!(ended.last().unwrap()["sessionId"], id);
    let refused = a.call(json!({"type": "get_state"})).await;
    assert_eq!(refused["error"], "not attached to a session");

    let listed = a.call(json!({"type": "list_sessions"})).await;
    let sessions = listed["data"]["sessions"].as_array().unwrap();
    assert_eq!(sessions[0]["hosted"], false);
    assert_eq!(sessions[0]["state"], "ended");

    // Attaching to a stored session loads it again with its history.
    let attached = a.call(json!({"type": "attach", "sessionId": id})).await;
    assert_eq!(attached["success"], true, "{attached}");
    assert_eq!(attached["data"]["session"]["hosted"], true);
    let entries = a.call(json!({"type": "get_entries"})).await;
    assert_eq!(
        entries["data"]["entries"].as_array().unwrap().len(),
        entry_count
    );

    let workspaces = a.call(json!({"type": "list_workspaces"})).await;
    assert_eq!(workspaces["data"]["workspaces"][0]["name"], "work");
    assert_eq!(workspaces["data"]["workspaces"][0]["sessionCount"], 1);

    let status = a.call(json!({"type": "status"})).await;
    assert_eq!(status["data"]["hostedSessions"], 1);
    assert_eq!(status["data"]["clients"], 2);
    assert_eq!(status["data"]["hostId"], "01TEST");

    daemon.stop().await;
}

#[tokio::test]
async fn the_sweep_unloads_a_session_nobody_watches() {
    let daemon = Daemon::start(Duration::from_millis(50), Duration::from_millis(20)).await;
    let mut a = daemon.client("A").await;
    let created = a
        .call(json!({"type": "create_session", "cwd": daemon.work_dir(), "model": "mock/mock-1"}))
        .await;
    let id = created["data"]["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    a.call(json!({"type": "attach", "sessionId": id})).await;
    a.call(json!({"type": "prompt", "message": "hi"})).await;
    a.read_until(|v| v["type"] == "agent_settled").await;
    a.call(json!({"type": "detach"})).await;
    let ended = a.read_until(|v| v["type"] == "session_ended").await;
    assert_eq!(ended.last().unwrap()["reason"], "idle");
    daemon.stop().await;
}

#[tokio::test]
async fn list_directories_skips_hidden_entries_and_sorts() {
    let daemon = Daemon::start(Duration::ZERO, Duration::from_secs(60)).await;
    let root = daemon.dir.path().join("tree");
    for name in ["beta", "alpha", ".hidden"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    std::fs::write(root.join("file.txt"), "x").unwrap();
    let mut a = daemon.client("A").await;
    let listed = a
        .call(json!({"type": "list_directories", "path": root.display().to_string()}))
        .await;
    let names: Vec<&str> = listed["data"]["directories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["alpha", "beta"]);
    let missing = a
        .call(json!({
            "type": "list_directories",
            "path": root.join("absent").display().to_string()
        }))
        .await;
    assert_eq!(missing["success"], false);
    daemon.stop().await;
}

#[tokio::test]
async fn shutdown_over_the_socket_ends_the_daemon_with_zero() {
    let daemon = Daemon::start(Duration::ZERO, Duration::from_secs(60)).await;
    let mut a = daemon.client("A").await;
    let reply = a.call(json!({"type": "shutdown"})).await;
    assert_eq!(reply["success"], true);
    let seen = a.read_until(|v| v["type"] == "daemon_shutdown").await;
    assert!(!seen.is_empty());
    let socket = daemon.socket.clone();
    assert_eq!(daemon.stop().await, 0);
    assert!(!socket.exists(), "the socket file is removed on exit");
}

#[tokio::test]
async fn a_second_daemon_on_the_same_lock_exits_with_one() {
    let daemon = Daemon::start(Duration::ZERO, Duration::from_secs(60)).await;
    let config = DaemonConfig {
        paths: DaemonPaths {
            run_dir: daemon.dir.path().to_path_buf(),
            socket: daemon.dir.path().join("other.sock"),
            lock: daemon.dir.path().join("d.lock"),
            log: daemon.dir.path().join("other.log"),
        },
        host: HostConfig {
            session_dir: daemon.dir.path().join("sessions"),
            idle_unload: Duration::ZERO,
            models_file: None,
            no_context_files: true,
        },
        host_id: "01TEST".into(),
        host_name: "testbox".into(),
        log_level: Level::Info,
        log_to_stderr: false,
        sweep_interval: Duration::from_secs(60),
        presence: None,
    };
    let code = super_daemon::run(config, CancellationToken::new())
        .await
        .expect("run");
    assert_eq!(code, 1);
    daemon.stop().await;
}
