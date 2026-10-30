//! `DaemonClient` against a real daemon on a Unix socket with the mock
//! provider: the calls a terminal makes, and a daemon from another protocol
//! range.

#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;
use super_daemon::DaemonConfig;
use super_daemon::client::{ClientError, ClientEvent, ClientInfo, DaemonClient, NewSession};
use super_daemon::host::HostConfig;
use super_daemon::log::Level;
use super_daemon::paths::DaemonPaths;
use super_sdk::daemon::{ClientKind, DaemonEvent, PROTOCOL_VERSIONS, SessionState};
use super_sdk::mock::{MockProvider, MockScript};
use super_sdk::protocol::Command;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

struct Daemon {
    _provider: MockProvider,
    dir: tempfile::TempDir,
    socket: PathBuf,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<anyhow::Result<i32>>,
}

impl Daemon {
    async fn start(protocol_versions: [u32; 2]) -> Daemon {
        // `sun_path` is short; keep the temp dir's own name and a short file name.
        let dir = tempfile::Builder::new()
            .prefix("ce")
            .tempdir()
            .expect("tempdir");
        let provider = MockProvider::start(
            dir.path().join("mock"),
            MockScript::text("Hello through the client."),
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
                idle_unload: Duration::ZERO,
                models_file: Some(provider.catalog_path()),
                no_context_files: true,
            },
            host_id: "01TEST".into(),
            host_name: "testbox".into(),
            log_level: Level::Debug,
            log_to_stderr: false,
            sweep_interval: Duration::from_secs(60),
            presence: None,
            protocol_versions,
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

    fn work_dir(&self) -> PathBuf {
        self.dir.path().join("work")
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

fn terminal() -> ClientInfo {
    ClientInfo::new(ClientKind::Tui, "Terminal")
}

/// Events until `done` matches one, which is included.
async fn events_until(
    events: &mut UnboundedReceiver<ClientEvent>,
    mut done: impl FnMut(&ClientEvent) -> bool,
) -> Vec<ClientEvent> {
    let mut seen = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), events.recv())
            .await
            .expect("an event in time")
            .expect("the connection stays open");
        let finished = done(&event);
        seen.push(event);
        if finished {
            return seen;
        }
    }
}

fn session_event(event: &ClientEvent, kind: &str) -> bool {
    matches!(event, ClientEvent::Session(inner) if inner.event_type() == kind)
}

fn streamed_text(events: &[ClientEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            ClientEvent::Session(inner) if inner.event_type() == "message_update" => {
                let delta = &inner.0["assistantMessageEvent"];
                (delta["type"] == "text_delta")
                    .then(|| delta["delta"].as_str().unwrap_or_default().to_string())
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_terminal_creates_attaches_prompts_and_detaches() {
    let daemon = Daemon::start(PROTOCOL_VERSIONS).await;
    let mut client = DaemonClient::connect_to(&daemon.socket, &terminal())
        .await
        .expect("hello");
    assert_eq!(client.protocol_version(), 1);
    assert_eq!(client.hello().host_name, "testbox");
    assert_eq!(client.hello().pid, std::process::id());
    let mut events = client.take_events().expect("the event stream");
    assert!(client.take_events().is_none(), "the stream is taken once");

    let status = client.status().await.expect("status");
    assert_eq!(status.clients, 1);
    assert_eq!(status.protocol_versions, PROTOCOL_VERSIONS);
    assert!(
        client
            .list_sessions(Some(&daemon.work_dir()))
            .await
            .unwrap()
            .is_empty()
    );

    let created = client
        .create_session(NewSession {
            cwd: daemon.work_dir(),
            model: Some("mock/mock-1".into()),
            ..Default::default()
        })
        .await
        .expect("create_session");
    assert!(created.hosted);
    assert_eq!(created.state, SessionState::Idle);
    let seen = events_until(&mut events, |event| {
        matches!(
            event,
            ClientEvent::Daemon(DaemonEvent::SessionCreated { .. })
        )
    })
    .await;
    assert!(!seen.is_empty());

    // A session command before attaching is the daemon's error, typed.
    let refused = client
        .execute_ok(Command::GetState {})
        .await
        .expect_err("not attached");
    assert!(
        matches!(&refused, ClientError::Command { message, .. } if message == "not attached to a session"),
        "{refused}"
    );

    let attached = client.attach(&created.session_id).await.expect("attach");
    assert_eq!(attached.session.attached_clients, 1);
    assert_eq!(attached.previous_session_id, None);

    let started = client
        .execute_ok(Command::Prompt {
            message: "say hi".into(),
            images: Vec::new(),
            streaming_behavior: None,
        })
        .await
        .expect("prompt");
    assert_eq!(started.unwrap()["disposition"], "started");
    let turn = events_until(&mut events, |event| session_event(event, "agent_settled")).await;
    assert!(turn.iter().any(|event| session_event(event, "agent_start")));
    assert_eq!(streamed_text(&turn), "Hello through the client.");

    let state = client
        .execute_ok(Command::GetState {})
        .await
        .expect("get_state")
        .unwrap();
    assert_eq!(state["sessionId"], created.session_id);
    assert_eq!(state["isStreaming"], false);

    let listed = client
        .list_sessions(Some(&daemon.work_dir()))
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].first_message.as_deref(), Some("say hi"));
    assert_eq!(listed[0].attached_clients, 1);
    let workspaces = client.list_workspaces().await.unwrap();
    assert_eq!(workspaces[0].name, "work");
    let directories = client.list_directories(daemon.dir.path()).await.unwrap();
    assert!(directories.iter().any(|entry| entry.name == "work"));

    let detached = client.detach().await.expect("detach");
    assert_eq!(detached.as_deref(), Some(created.session_id.as_str()));
    assert!(client.execute_ok(Command::GetState {}).await.is_err());

    // Resuming a session that is still hosted changes nothing.
    let resumed = client.resume_session(&created.session_id).await.unwrap();
    assert!(resumed.hosted);
    client
        .stop_session(&created.session_id)
        .await
        .expect("stop_session");
    events_until(&mut events, |event| {
        matches!(event, ClientEvent::Daemon(DaemonEvent::SessionEnded { .. }))
    })
    .await;

    // `shutdown` closes the connection on its way out.
    client.shutdown().await.expect("shutdown");
    tokio::time::timeout(Duration::from_secs(10), client.closed())
        .await
        .expect("the daemon closed the connection");
    assert!(client.is_closed());
    assert!(matches!(client.status().await, Err(ClientError::Closed)));
    assert_eq!(daemon.stop().await, 0);
}

#[tokio::test]
async fn a_daemon_on_another_protocol_range_is_a_skew_error() {
    let daemon = Daemon::start([2, 3]).await;
    let error = DaemonClient::connect_to(&daemon.socket, &terminal())
        .await
        .err()
        .expect("refused");
    let ClientError::Skew(skew) = &error else {
        panic!("expected a skew error, got {error}");
    };
    assert_eq!(skew.daemon, [2, 3]);
    assert_eq!(skew.client, PROTOCOL_VERSIONS);
    assert_eq!(
        skew.daemon_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    let message = error.to_string();
    assert!(
        message.starts_with("the SUPER daemon speaks protocol 2 to 3, this build speaks 1: "),
        "{message}"
    );

    // A client from the future meets the same daemon's older side.
    let mut newer = terminal();
    newer.protocol_versions = [4, 4];
    let error = DaemonClient::connect_to(&daemon.socket, &newer)
        .await
        .err()
        .expect("refused");
    assert!(
        error
            .to_string()
            .starts_with("the SUPER daemon speaks protocol 2 to 3, this build speaks 4: "),
        "{error}"
    );
    assert_eq!(daemon.stop().await, 0);
}

#[tokio::test]
async fn a_dead_socket_is_not_running() {
    let dir = tempfile::Builder::new().prefix("ce").tempdir().unwrap();
    let error = DaemonClient::connect_to(&dir.path().join("none.sock"), &terminal())
        .await
        .err()
        .expect("no daemon");
    assert!(matches!(error, ClientError::NotRunning { .. }), "{error}");
}
