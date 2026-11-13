//! The platform link against the fake S2 and the mock provider.

#![cfg(unix)]

use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use super_daemon::host::{CreateRequest, HostConfig, SessionHost};
use super_daemon::presence::{Link, LinkConfig};
use super_daemon::server::{DaemonInfo, ServerContext};
use super_remote::MachineIdentity;
use super_remote::fake::{FakeS2, TestCredentials, serve};
use super_remote::s2::S2Client;
use super_sdk::daemon::EndReason;
use super_sdk::mock::{MockProvider, MockScript};
use tokio_util::sync::CancellationToken;

struct Fixture {
    _provider: MockProvider,
    dir: tempfile::TempDir,
    fake: Arc<FakeS2>,
    client: Arc<S2Client>,
    ctx: Arc<ServerContext>,
    link: Link,
}

fn identity() -> MachineIdentity {
    MachineIdentity {
        machine_id: "01H".into(),
        name: "mbp".into(),
        platform: "macos".into(),
    }
}

fn link_config(dir: &std::path::Path) -> LinkConfig {
    LinkConfig {
        cursor_dir: Some(dir.join("cursors")),
        republish_interval: Duration::from_secs(3600),
        ..LinkConfig::default()
    }
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
            machine_id: "01H".into(),
            machine_name: "mbp".into(),
            platform: "macos".into(),
            socket_path: dir.path().join("d.sock"),
            protocol_versions: super_sdk::daemon::PROTOCOL_VERSIONS,
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
    let link = Link::start(
        client.clone(),
        ctx.clone(),
        identity(),
        "device-1".into(),
        link_config(dir.path()),
    );
    Fixture {
        _provider: provider,
        dir,
        fake,
        client,
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
        "machines/01H/commands",
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
    assert_eq!(
        registry[0]["daemon"], true,
        "the host record names the daemon"
    );
    assert_eq!(registry[1]["kind"], "session");
    assert_eq!(registry[1]["state"], "ended");
    assert_eq!(registry[1]["daemon"], true);
    assert_eq!(registry[1]["resumeArgv"][1], "--session");
    fixture.link.stop().await;
}

/// When a file was last written, in Unix milliseconds to the second, as the
/// daemon's listing names it (`modifiedAt` has no fraction).
fn modified_ms(file: &std::path::Path) -> i64 {
    let modified = std::fs::metadata(file).unwrap().modified().unwrap();
    let since = modified
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    since as i64 * 1000
}

#[tokio::test]
async fn a_record_sent_again_keeps_the_time_of_the_last_activity() {
    let fixture = fixture().await;
    wait_until(|| bodies(&fixture.fake, "registry").len() >= 2).await;
    fixture.link.stop().await;
    let before = bodies(&fixture.fake, "registry").len();

    // The stored session was last used two days ago.
    let announced = bodies(&fixture.fake, "registry");
    let file = std::path::PathBuf::from(announced[1]["resumeArgv"][2].as_str().unwrap());
    let two_days_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 86_400);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(two_days_ago)
        .unwrap();
    let activity = modified_ms(&file);

    // A link that sends everything again after 300 ms instead of six hours.
    let link = Link::start(
        fixture.client.clone(),
        fixture.ctx.clone(),
        identity(),
        "device-1".into(),
        LinkConfig {
            republish_interval: Duration::from_millis(300),
            ..link_config(fixture.dir.path())
        },
    );
    let stored = |fake: &FakeS2| -> Vec<Value> {
        bodies(fake, "registry")
            .into_iter()
            .skip(before)
            .filter(|b| b["kind"] == "session")
            .collect()
    };
    wait_until(|| stored(&fixture.fake).len() >= 2).await;
    let records = stored(&fixture.fake);
    let (first, again) = (&records[0], &records[1]);
    assert_eq!(first["sessionId"], again["sessionId"]);
    assert_eq!(again["state"], "ended");

    // That is the session's last activity, however often its record is
    // written; `at` is when each record was written.
    assert_eq!(first["lastActivityAt"], activity, "{first}");
    assert_eq!(again["lastActivityAt"], activity, "{again}");
    let written = |record: &Value| record["at"].as_i64().unwrap();
    assert!(written(again) >= written(first) + 250, "{first} {again}");
    assert!(written(first) - activity > 86_400_000, "{first}");
    link.stop().await;
}

#[tokio::test]
async fn a_turn_moves_the_time_of_the_last_activity() {
    let fixture = fixture().await;
    wait_until(|| bodies(&fixture.fake, "registry").len() >= 2).await;
    let hosted = fixture
        .ctx
        .host
        .create(CreateRequest {
            cwd: fixture.dir.path().join("work"),
            model: Some("mock/mock-1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let id = hosted.session_id.clone();
    let records = |fake: &FakeS2| -> Vec<Value> {
        bodies(fake, "registry")
            .into_iter()
            .filter(|b| b["sessionId"] == id)
            .collect()
    };
    wait_until(|| records(&fixture.fake).len() == 1).await;

    // Pretend the session was started two days ago and left alone since.
    let file =
        std::path::PathBuf::from(records(&fixture.fake)[0]["resumeArgv"][2].as_str().unwrap());
    let two_days_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 86_400);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(two_days_ago)
        .unwrap();
    let started = modified_ms(&file);

    hosted.session.prompt("hi").await.unwrap();
    wait_until(|| {
        records(&fixture.fake)
            .last()
            .is_some_and(|b| b["state"] == "idle" && b["lastActivityAt"] != started)
    })
    .await;
    // The record after the turn names the time of the reply, which is the
    // last thing written to the session's file.
    let settled = records(&fixture.fake).pop().unwrap();
    assert_eq!(settled["lastActivityAt"], modified_ms(&file), "{settled}");
    assert!(
        settled["lastActivityAt"].as_i64().unwrap() - started > 86_400_000,
        "{settled}"
    );
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
        bodies(&fixture.fake, "machines/01H/events")
            .iter()
            .any(|b| b["type"] == "response" && b["id"] == "c1")
    })
    .await;
    let events = bodies(&fixture.fake, "machines/01H/events");
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
        bodies(&fixture.fake, "machines/01H/events")
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
    // The ended record says what a listing of the stored session says, so a
    // resync does not send it again: no model, no thinking level.
    let ended = bodies(&fixture.fake, "registry")
        .into_iter()
        .rev()
        .find(|b| b["sessionId"] == id)
        .unwrap();
    assert_eq!(ended["model"], "");
    assert_eq!(ended["thinkingLevel"], "");
    assert_eq!(ended["daemon"], true);
    fixture.link.stop().await;
}

#[tokio::test]
async fn a_restarted_link_resumes_from_its_cursor_and_runs_nothing_twice() {
    let fixture = fixture().await;
    wait_until(|| bodies(&fixture.fake, "registry").len() >= 2).await;
    command(&fixture.fake, "c1", json!({"type": "status"}));
    wait_until(|| {
        bodies(&fixture.fake, "machines/01H/events")
            .iter()
            .any(|b| b["id"] == "c1")
    })
    .await;
    fixture.link.stop().await;

    // Appended while no link runs: the next link must run it, and only it.
    command(&fixture.fake, "c2", json!({"type": "list_workspaces"}));
    let link = Link::start(
        fixture.client.clone(),
        fixture.ctx.clone(),
        identity(),
        "device-1".into(),
        link_config(fixture.dir.path()),
    );
    wait_until(|| {
        bodies(&fixture.fake, "machines/01H/events")
            .iter()
            .any(|b| b["id"] == "c2")
    })
    .await;
    let events = bodies(&fixture.fake, "machines/01H/events");
    let answers = |id: &str| events.iter().filter(|b| b["id"] == id).count();
    assert_eq!(answers("c1"), 1, "{events:?}");
    assert_eq!(answers("c2"), 1, "{events:?}");
    assert_eq!(
        bodies(&fixture.fake, "registry")
            .iter()
            .filter(|b| b["kind"] == "host")
            .count(),
        2,
        "every link start announces the host"
    );
    link.stop().await;
}

#[tokio::test]
async fn stopping_the_link_announces_hosted_sessions_ended_and_stops_mirroring() {
    let fixture = fixture().await;
    wait_until(|| bodies(&fixture.fake, "registry").len() >= 2).await;
    let hosted = fixture
        .ctx
        .host
        .create(CreateRequest {
            cwd: fixture.dir.path().join("work"),
            model: Some("mock/mock-1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let id = hosted.session_id.clone();
    wait_until(|| {
        bodies(&fixture.fake, "registry")
            .iter()
            .any(|b| b["sessionId"] == id && b["state"] == "idle")
    })
    .await;
    let stream = format!("sessions/01H/{id}/events");
    hosted.session.prompt("hi").await.unwrap();
    wait_until(|| {
        bodies(&fixture.fake, &stream)
            .iter()
            .any(|b| b["type"] == "agent_settled")
    })
    .await;

    // Signing out: the link stops while the session stays hosted. The
    // goodbye is on the registry before `stop` returns.
    fixture.link.stop().await;
    let last = bodies(&fixture.fake, "registry")
        .into_iter()
        .rev()
        .find(|b| b["sessionId"] == id)
        .unwrap();
    assert_eq!(last["state"], "ended", "{last}");
    assert!(
        fixture.ctx.host.get(&id).is_some(),
        "the session keeps running"
    );

    // Nothing of the session reaches the platform any more.
    let mirrored = bodies(&fixture.fake, &stream).len();
    hosted.session.prompt("again").await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(bodies(&fixture.fake, &stream).len(), mirrored);
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
        "machines/01H/commands",
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
        bodies(&fixture.fake, "machines/01H/events")
            .iter()
            .any(|b| b["id"] == "c4")
    })
    .await;
    let events = bodies(&fixture.fake, "machines/01H/events");
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
