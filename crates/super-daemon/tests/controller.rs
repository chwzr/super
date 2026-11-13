//! A terminal on another machine drives this daemon through the platform:
//! the registry lists the machine and its sessions, machine commands list
//! directories and create a session, and a session controller prompts it and
//! follows its events. The fake stands in for the platform, the mock
//! provider for the model.

#![cfg(unix)]

use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use super_daemon::host::{CreateRequest, HostConfig, SessionHost};
use super_daemon::presence::{Link, LinkConfig};
use super_daemon::server::{DaemonInfo, ServerContext};
use super_remote::MachineIdentity;
use super_remote::controller::{MachineCommands, Sender, SessionController};
use super_remote::fake::{FakePlatform, TestCredentials, registry_client, serve};
use super_remote::registry::{Registry, RegistryClient};
use super_remote::s2::S2Client;
use super_sdk::daemon::{DaemonCommand, EndReason, SessionState};
use super_sdk::mock::{MockProvider, MockScript};
use super_sdk::protocol::Command;
use tokio_util::sync::CancellationToken;

struct Fixture {
    _provider: MockProvider,
    dir: tempfile::TempDir,
    fake: Arc<FakePlatform>,
    client: Arc<S2Client>,
    registry: Arc<RegistryClient>,
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
            machine_id: "01L".into(),
            machine_name: "linuxbox".into(),
            platform: "linux".into(),
            socket_path: dir.path().join("d.sock"),
            protocol_versions: super_sdk::daemon::PROTOCOL_VERSIONS,
        },
        CancellationToken::new(),
    );
    let fake = Arc::new(FakePlatform::default());
    let base = serve(fake.clone()).await;
    let client = Arc::new(S2Client::new(
        reqwest::Client::new(),
        base,
        Arc::new(TestCredentials),
    ));
    let registry = registry_client(&fake);
    let link = Link::start(
        client.clone(),
        registry.clone(),
        ctx.clone(),
        MachineIdentity {
            machine_id: "01L".into(),
            name: "linuxbox".into(),
            platform: "linux".into(),
        },
        LinkConfig {
            cursor_dir: Some(dir.path().join("cursors")),
            ..LinkConfig::default()
        },
    );
    Fixture {
        _provider: provider,
        dir,
        fake,
        client,
        registry,
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

async fn registry_until(client: &RegistryClient, check: impl Fn(&Registry) -> bool) -> Registry {
    for _ in 0..500 {
        let registry = client.read().await.unwrap();
        if check(&registry) {
            return registry;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the registry never showed what the test waits for");
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The headers of the newest record on `stream`, and its body.
fn newest(
    fake: &FakePlatform,
    stream: &str,
) -> (std::collections::BTreeMap<String, String>, Value) {
    let record = fake.records(stream).pop().expect("a record");
    (
        record.headers.into_iter().collect(),
        serde_json::from_str(&record.body).unwrap(),
    )
}

/// What the apps put on every command: its type, the device that sends it,
/// the name the others see, and an expiry in the future.
fn assert_command_headers(headers: &std::collections::BTreeMap<String, String>, kind: &str) {
    assert_eq!(headers["t"], kind, "{headers:?}");
    assert_eq!(headers["from"], "device-mac");
    assert_eq!(headers["name"], "Terminal on mbp");
    let expires: i64 = headers["exp"].parse().unwrap();
    assert!(expires > now_ms(), "{headers:?}");
}

#[tokio::test]
async fn the_registry_lists_the_daemon_machine_and_its_stored_session() {
    let fixture = fixture().await;
    let registry = registry_until(&fixture.registry, |registry| registry.sessions.len() == 1).await;
    let machine = registry.machine("01L").expect("the machine");
    assert_eq!(machine.name, "linuxbox");
    assert_eq!(machine.platform, "linux");
    assert!(machine.daemon);
    assert!(machine.online);
    let session = registry.sessions_on("01L").next().unwrap();
    assert_eq!(session.record.session_id, fixture.stored_id);
    assert!(session.record.daemon);
    assert_eq!(session.record.state, SessionState::Ended);
    assert_eq!(
        session.record.first_message.as_deref(),
        Some("an older question")
    );
    fixture.link.stop().await;
}

#[tokio::test]
async fn a_controller_creates_prompts_and_follows_a_session_on_another_machine() {
    let fixture = fixture().await;
    registry_until(&fixture.registry, |registry| {
        registry.machine("01L").is_some()
    })
    .await;
    let machine = MachineCommands::new(fixture.client.clone(), "01L", terminal());

    // "New workspace on machine…": the machine's directories, then a session
    // in the one picked.
    let work = fixture.dir.path().join("work");
    let listing = machine
        .call(&DaemonCommand::ListDirectories {
            path: work.display().to_string(),
        })
        .await
        .unwrap();
    assert!(listing.success, "{listing:?}");
    // The command went to the machine's commands stream with the apps'
    // headers, and its answer, matched by id, came from its events stream.
    let (headers, body) = newest(&fixture.fake, "machines/01L/commands");
    assert_command_headers(&headers, "list_directories");
    assert_eq!(body["type"], "list_directories");
    let answered = fixture
        .fake
        .records("machines/01L/events")
        .into_iter()
        .any(|record| {
            serde_json::from_str::<Value>(&record.body)
                .is_ok_and(|event| event["type"] == "response" && event["id"] == body["id"])
        });
    assert!(answered, "the answer is on machines/01L/events");
    let names: Vec<&str> = listing.data.as_ref().unwrap()["directories"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert_eq!(names, ["app"]);
    let created = machine
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
    // A prompt goes to that session's own commands stream.
    let (headers, body) = newest(
        &fixture.fake,
        &format!("sessions/01L/{session_id}/commands"),
    );
    assert_command_headers(&headers, "prompt");
    assert_eq!(body["message"], "hi from the Mac");
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

    // Resuming the stored session over the machine's stream hosts it again.
    let resumed = machine
        .call(&DaemonCommand::ResumeSession {
            session_id: fixture.stored_id.clone(),
        })
        .await
        .unwrap();
    assert!(resumed.success, "{resumed:?}");
    assert!(fixture.ctx.host.get(&fixture.stored_id).is_some());

    // A command for an unknown session answers with the daemon's error.
    let missing = machine
        .call(&DaemonCommand::StopSession {
            session_id: "nope".into(),
        })
        .await
        .unwrap();
    assert!(!missing.success);

    controller.close();
    fixture.link.stop().await;
}
