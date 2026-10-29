//! The platform link against the fake S2 and the mock provider.

#![cfg(unix)]

use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use super_daemon::host::{CreateRequest, HostConfig, SessionHost};
use super_daemon::presence::{Link, LinkConfig};
use super_daemon::server::{DaemonInfo, ServerContext};
use super_remote::HostIdentity;
use super_remote::fake::{FakeS2, TestCredentials, serve};
use super_remote::s2::S2Client;
use super_sdk::daemon::EndReason;
use super_sdk::mock::{MockProvider, MockScript};
use tokio_util::sync::CancellationToken;

struct Fixture {
    _provider: MockProvider,
    dir: tempfile::TempDir,
    fake: Arc<FakeS2>,
    ctx: Arc<ServerContext>,
    link: Link,
}

async fn fixture() -> Fixture {
    let dir = tempfile::Builder::new().prefix("sp").tempdir().unwrap();
    let provider = MockProvider::start(dir.path().join("mock"), MockScript::text("Hello."))
        .await
        .unwrap();
    std::fs::create_dir_all(dir.path().join("work")).unwrap();
    let host = SessionHost::new(HostConfig {
        session_dir: dir.path().join("sessions"),
        idle_unload: Duration::ZERO,
        models_file: Some(provider.catalog_path()),
        no_context_files: true,
    });
    // A stored session from before the link started.
    let stored = host
        .create(CreateRequest {
            cwd: dir.path().join("work"),
            model: Some("mock/mock-1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    stored.session.prompt("old").await.unwrap();
    let stored_id = stored.session_id.clone();
    drop(stored);
    host.stop(&stored_id, EndReason::Stopped).await.unwrap();

    let ctx = ServerContext::new(
        host,
        DaemonInfo {
            version: "test".into(),
            host_id: "01H".into(),
            host_name: "mbp".into(),
            platform: "macos".into(),
            socket_path: dir.path().join("d.sock"),
        },
        CancellationToken::new(),
    );
    let fake = Arc::new(FakeS2::default());
    let base = serve(fake.clone()).await;
    let client = Arc::new(S2Client::new(
        reqwest::Client::new(),
        base,
        Arc::new(TestCredentials),
    ));
    let identity = HostIdentity {
        host_id: "01H".into(),
        name: "mbp".into(),
        platform: "macos".into(),
    };
    let link = Link::start(
        client,
        ctx.clone(),
        identity,
        "device-1".into(),
        LinkConfig {
            cursor_dir: Some(dir.path().join("cursors")),
            republish_interval: Duration::from_secs(3600),
            ..LinkConfig::default()
        },
    );
    Fixture {
        _provider: provider,
        dir,
        fake,
        ctx,
        link,
    }
}

fn bodies(fake: &FakeS2, stream: &str) -> Vec<Value> {
    fake.records(stream)
        .iter()
        .map(|r| serde_json::from_str(&r.body).unwrap())
        .collect()
}

async fn wait_until(mut check: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met in time");
}

fn command(fake: &FakeS2, id: &str, body: Value) {
    let mut body = body;
    body["id"] = Value::String(id.into());
    fake.append(
        "hosts/01H/commands",
        vec![(
            vec![
                ("t".to_string(), body["type"].as_str().unwrap().to_string()),
                ("from".to_string(), "device-ios".to_string()),
                ("name".to_string(), "iPhone".to_string()),
            ],
            body.to_string(),
        )],
    );
}

#[tokio::test]
async fn the_host_and_every_stored_session_are_announced_at_start() {
    let fixture = fixture().await;
    wait_until(|| bodies(&fixture.fake, "registry").len() >= 2).await;
    let registry = bodies(&fixture.fake, "registry");
    assert_eq!(registry[0]["kind"], "host");
    assert_eq!(registry[0]["name"], "mbp");
    assert_eq!(registry[1]["kind"], "session");
    assert_eq!(registry[1]["state"], "ended");
    assert_eq!(registry[1]["daemon"], true);
    assert_eq!(registry[1]["resumeArgv"][1], "--session");
    fixture.link.stop().await;
}

#[tokio::test]
async fn a_create_session_over_the_platform_is_answered_mirrored_and_registered() {
    let fixture = fixture().await;
    wait_until(|| bodies(&fixture.fake, "registry").len() >= 2).await;
    let work = fixture.dir.path().join("work").display().to_string();
    command(
        &fixture.fake,
        "c1",
        json!({"type": "create_session", "cwd": work, "model": "mock/mock-1"}),
    );

    wait_until(|| {
        bodies(&fixture.fake, "hosts/01H/events")
            .iter()
            .any(|b| b["type"] == "response" && b["id"] == "c1")
    })
    .await;
    let events = bodies(&fixture.fake, "hosts/01H/events");
    let response = events.iter().find(|b| b["type"] == "response").unwrap();
    assert_eq!(response["success"], true, "{response}");
    let id = response["data"]["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(events.iter().any(|b| b["type"] == "session_created"));

    // The new session is hosted, mirrored, and registered as idle.
    wait_until(|| {
        bodies(&fixture.fake, "registry")
            .iter()
            .any(|b| b["sessionId"] == id && b["state"] == "idle")
    })
    .await;
    let hosted = fixture.ctx.host.get(&id).expect("hosted");
    hosted.session.prompt("hi").await.unwrap();
    let stream = format!("sessions/01H/{id}/events");
    wait_until(|| {
        bodies(&fixture.fake, &stream)
            .iter()
            .any(|b| b["type"] == "agent_settled")
    })
    .await;
    wait_until(|| {
        bodies(&fixture.fake, "registry")
            .iter()
            .any(|b| b["sessionId"] == id && b["state"] == "working")
    })
    .await;
    let entries = format!("sessions/01H/{id}/entries");
    wait_until(|| bodies(&fixture.fake, &entries).len() >= 2).await;

    // Stopping over the host stream unloads it and announces the end.
    command(
        &fixture.fake,
        "c2",
        json!({"type": "stop_session", "sessionId": id}),
    );
    wait_until(|| {
        bodies(&fixture.fake, "hosts/01H/events")
            .iter()
            .any(|b| b["type"] == "session_ended" && b["sessionId"] == id)
    })
    .await;
    wait_until(|| {
        bodies(&fixture.fake, "registry")
            .iter()
            .rev()
            .find(|b| b["sessionId"] == id)
            .is_some_and(|b| b["state"] == "ended")
    })
    .await;
    assert!(fixture.ctx.host.get(&id).is_none());
    fixture.link.stop().await;
}

#[tokio::test]
async fn connection_bound_and_malformed_commands_are_refused_and_expired_ones_dropped() {
    let fixture = fixture().await;
    wait_until(|| bodies(&fixture.fake, "registry").len() >= 2).await;
    command(
        &fixture.fake,
        "c1",
        json!({"type": "attach", "sessionId": "x"}),
    );
    fixture.fake.append(
        "hosts/01H/commands",
        vec![(
            vec![
                ("t".into(), "list_sessions".into()),
                ("exp".into(), "1".into()),
            ],
            json!({"id": "c2", "type": "list_sessions"}).to_string(),
        )],
    );
    command(&fixture.fake, "c3", json!({"type": "teleport"}));
    command(&fixture.fake, "c4", json!({"type": "list_workspaces"}));
    wait_until(|| {
        bodies(&fixture.fake, "hosts/01H/events")
            .iter()
            .any(|b| b["id"] == "c4")
    })
    .await;
    let events = bodies(&fixture.fake, "hosts/01H/events");
    let by_id = |id: &str| events.iter().find(|b| b["id"] == id).cloned();
    assert_eq!(
        by_id("c1").unwrap()["error"],
        "not available over the platform"
    );
    assert!(by_id("c2").is_none(), "an expired command is dropped");
    assert_eq!(by_id("c3").unwrap()["success"], false);
    assert!(
        by_id("c3").unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("teleport")
    );
    assert_eq!(
        by_id("c4").unwrap()["data"]["workspaces"][0]["name"],
        "work"
    );
    fixture.link.stop().await;
}
