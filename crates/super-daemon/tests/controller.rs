//! A terminal on another device drives this daemon through the platform:
//! the registry reader finds the host and its sessions, host commands list
//! directories and create a session, and a session controller prompts it and
//! follows its events. The fake S2 stands in for the platform, the mock
//! provider for the model.

#![cfg(unix)]

use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use super_daemon::host::{CreateRequest, HostConfig, SessionHost};
use super_daemon::presence::{Link, LinkConfig};
use super_daemon::server::{DaemonInfo, ServerContext};
use super_remote::HostIdentity;
use super_remote::controller::{HostCommands, Sender, SessionController};
use super_remote::fake::{FakeS2, TestCredentials, serve};
use super_remote::registry::{RegistryView, read_registry};
use super_remote::s2::S2Client;
use super_sdk::daemon::{DaemonCommand, EndReason, SessionState};
use super_sdk::mock::{MockProvider, MockScript};
use super_sdk::protocol::Command;
use tokio_util::sync::CancellationToken;

struct Fixture {
    _provider: MockProvider,
    dir: tempfile::TempDir,
    client: Arc<S2Client>,
    ctx: Arc<ServerContext>,
    link: Link,
    stored_id: String,
}

async fn fixture() -> Fixture {
    let dir = tempfile::Builder::new().prefix("sc").tempdir().unwrap();
    let provider = MockProvider::start(dir.path().join("mock"), MockScript::text("Hello."))
        .await
        .unwrap();
    std::fs::create_dir_all(dir.path().join("work/app")).unwrap();
    let host = SessionHost::new(HostConfig {
        session_dir: dir.path().join("sessions"),
        idle_unload: Duration::ZERO,
        models_file: Some(provider.catalog_path()),
        no_context_files: true,
    });
    let stored = host
        .create(CreateRequest {
            cwd: dir.path().join("work"),
            model: Some("mock/mock-1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    stored.session.prompt("an older question").await.unwrap();
    let stored_id = stored.session_id.clone();
    drop(stored);
    host.stop(&stored_id, EndReason::Stopped).await.unwrap();

    let ctx = ServerContext::new(
        host,
        DaemonInfo {
            version: "test".into(),
            host_id: "01L".into(),
            host_name: "linuxbox".into(),
            platform: "linux".into(),
            socket_path: dir.path().join("d.sock"),
            protocol_versions: super_sdk::daemon::PROTOCOL_VERSIONS,
        },
        CancellationToken::new(),
    );
    let fake = Arc::new(FakeS2::default());
    let base = serve(fake).await;
    let client = Arc::new(S2Client::new(
        reqwest::Client::new(),
        base,
        Arc::new(TestCredentials),
    ));
    let link = Link::start(
        client.clone(),
        ctx.clone(),
        HostIdentity {
            host_id: "01L".into(),
            name: "linuxbox".into(),
            platform: "linux".into(),
        },
        "device-linux".into(),
        LinkConfig {
            cursor_dir: Some(dir.path().join("cursors")),
            republish_interval: Duration::from_secs(3600),
            ..LinkConfig::default()
        },
    );
    Fixture {
        _provider: provider,
        dir,
        client,
        ctx,
        link,
        stored_id,
    }
}

fn terminal() -> Sender {
    Sender {
        device_id: "device-mac".into(),
        name: "Terminal on mbp".into(),
    }
}

async fn registry_until(
    client: &Arc<S2Client>,
    check: impl Fn(&RegistryView) -> bool,
) -> RegistryView {
    for _ in 0..500 {
        let view = read_registry(client, Duration::from_secs(5)).await.unwrap();
        if check(&view) {
            return view;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the registry never showed what the test waits for");
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[tokio::test]
async fn the_registry_reader_sees_the_daemon_host_and_its_stored_session() {
    let fixture = fixture().await;
    let view = registry_until(&fixture.client, |view| view.sessions().count() == 1).await;
    let host = view.host_by_id("01L").expect("the host");
    assert_eq!(host.name, "linuxbox");
    assert_eq!(host.platform, "linux");
    assert!(host.daemon);
    assert!(view.is_online("01L", now_ms()));
    let session = view.sessions().next().unwrap();
    assert_eq!(session.record.session_id, fixture.stored_id);
    assert_eq!(session.record.state, SessionState::Ended);
    assert_eq!(
        session.record.first_message.as_deref(),
        Some("an older question")
    );
    fixture.link.stop().await;
}

#[tokio::test]
async fn a_controller_creates_prompts_and_follows_a_session_on_another_host() {
    let fixture = fixture().await;
    registry_until(&fixture.client, |view| view.host_by_id("01L").is_some()).await;
    let host = HostCommands::new(fixture.client.clone(), "01L", terminal());

    // "New workspace on device…": the device's directories, then a session
    // in the one picked.
    let work = fixture.dir.path().join("work");
    let listing = host
        .call(&DaemonCommand::ListDirectories {
            path: work.display().to_string(),
        })
        .await
        .unwrap();
    assert!(listing.success, "{listing:?}");
    let names: Vec<&str> = listing.data.as_ref().unwrap()["directories"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert_eq!(names, ["app"]);
    let created = host
        .call(&DaemonCommand::CreateSession {
            cwd: work.join("app").display().to_string(),
            model: Some("mock/mock-1".into()),
            provider: None,
            thinking_level: None,
            name: None,
        })
        .await
        .unwrap();
    assert!(created.success, "{created:?}");
    let session_id = created.data.unwrap()["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(fixture.ctx.host.get(&session_id).is_some());

    // The same commands a local client sends, through the session's streams.
    let (controller, mut events) =
        SessionController::open(fixture.client.clone(), "01L", &session_id, terminal())
            .await
            .unwrap();
    let response = controller
        .execute(Command::Prompt {
            message: "hi from the Mac".into(),
            images: Vec::new(),
            streaming_behavior: None,
        })
        .await
        .unwrap();
    assert!(response.success, "{response:?}");
    let mut kinds: Vec<String> = Vec::new();
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = events.recv().await {
            let kind = event["type"].as_str().unwrap_or("").to_string();
            if kind == "message_update"
                && let Some(delta) = event["assistantMessageEvent"]["delta"].as_str()
            {
                text.push_str(delta);
            }
            assert_ne!(kind, "response", "answers go to the waiting call");
            kinds.push(kind.clone());
            if kind == "agent_settled" {
                break;
            }
        }
    })
    .await
    .expect("the turn settled");
    assert!(kinds.contains(&"agent_start".to_string()), "{kinds:?}");
    assert_eq!(text, "Hello.");

    let messages = controller.execute(Command::GetMessages {}).await.unwrap();
    let data: Value = messages.data.unwrap();
    let roles: Vec<&str> = data["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["role"].as_str())
        .collect();
    assert_eq!(roles, ["user", "assistant"]);

    // Resuming the stored session over the host stream hosts it again.
    let resumed = host
        .call(&DaemonCommand::ResumeSession {
            session_id: fixture.stored_id.clone(),
        })
        .await
        .unwrap();
    assert!(resumed.success, "{resumed:?}");
    assert!(fixture.ctx.host.get(&fixture.stored_id).is_some());

    // A command for an unknown session answers with the daemon's error.
    let missing = host
        .call(&DaemonCommand::StopSession {
            session_id: "nope".into(),
        })
        .await
        .unwrap();
    assert!(!missing.success);

    controller.close();
    fixture.link.stop().await;
}
