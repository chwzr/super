//! The platform link against the fake platform and the mock provider.

#![cfg(unix)]

use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use super_daemon::host::{CreateRequest, HostConfig, SessionHost};
use super_daemon::presence::{Link, LinkConfig};
use super_daemon::server::{DaemonInfo, ServerContext};
use super_remote::fake::{ApiRequest, FakePlatform, TestCredentials, registry_client, serve};
use super_remote::s2::S2Client;
use super_remote::{MachineIdentity, RegistryClient, SessionRecord, SessionState};
use super_sdk::daemon::EndReason;
use super_sdk::mock::{MockProvider, MockScript};
use tokio_util::sync::CancellationToken;

struct Fixture {
    _provider: MockProvider,
    dir: tempfile::TempDir,
    fake: Arc<FakePlatform>,
    client: Arc<S2Client>,
    registry: Arc<RegistryClient>,
    ctx: Arc<ServerContext>,
    link: Link,
    /// The session stored before the link started.
    stored_id: String,
}

/// A link of the fixture's daemon; another one once the first was stopped,
/// as after a new sign-in.
fn start_link(
    client: &Arc<S2Client>,
    registry: &Arc<RegistryClient>,
    ctx: &Arc<ServerContext>,
    dir: &std::path::Path,
) -> Link {
    Link::start(
        client.clone(),
        registry.clone(),
        ctx.clone(),
        identity(),
        link_config(dir),
    )
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
    let fake = Arc::new(FakePlatform::default());
    let base = serve(fake.clone()).await;
    let client = Arc::new(S2Client::new(
        reqwest::Client::new(),
        base,
        Arc::new(TestCredentials),
    ));
    let registry = registry_client(&fake);
    let link = start_link(&client, &registry, &ctx, dir.path());
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

fn bodies(fake: &FakePlatform, stream: &str) -> Vec<Value> {
    fake.records(stream)
        .iter()
        .map(|r| serde_json::from_str(&r.body).unwrap())
        .collect()
}

/// `METHOD path` of every registry request so far.
fn calls(fake: &FakePlatform) -> Vec<String> {
    fake.requests()
        .iter()
        .map(|request| format!("{} {}", request.method, request.path))
        .collect()
}

/// The complete lists the daemon sent so far.
fn lists(fake: &FakePlatform) -> Vec<ApiRequest> {
    fake.requests()
        .into_iter()
        .filter(|request| request.method == "PUT" && request.path == "/api/machines/01H/sessions")
        .collect()
}

/// A session of the daemon's machine as the registry has it.
fn listed(fake: &FakePlatform, id: &str) -> Option<Value> {
    fake.session("01H", id)
}

fn has_state(fake: &FakePlatform, id: &str, state: &str) -> bool {
    listed(fake, id).is_some_and(|session| session["state"] == state)
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

/// The link is up once the registry has the daemon's first complete list.
async fn started(fixture: &Fixture) {
    wait_until(|| !lists(&fixture.fake).is_empty() && !fixture.fake.sessions("01H").is_empty())
        .await;
}

fn command(fake: &FakePlatform, id: &str, body: Value) {
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

/// When a file was last written, as the registry names times: RFC 3339,
/// seconds, UTC.
fn modified(file: &std::path::Path) -> String {
    super_remote::registry::modified_at(file).unwrap()
}

fn seconds(rfc3339: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .unwrap()
        .timestamp()
}

/// Pretend `file` was last written two days ago.
fn age_two_days(file: &std::path::Path) -> String {
    let two_days_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 86_400);
    std::fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(two_days_ago)
        .unwrap();
    modified(file)
}

#[tokio::test]
async fn the_machine_is_announced_and_every_stored_session_listed_at_start() {
    let fixture = fixture().await;
    started(&fixture).await;
    assert_eq!(
        calls(&fixture.fake)[..2],
        ["PUT /api/machines/01H", "PUT /api/machines/01H/sessions"],
        "the announcement, then the complete list in one request"
    );
    let requests = fixture.fake.requests();
    assert_eq!(
        requests[0].body,
        json!({"name": "mbp", "platform": "macos", "daemon": true}),
        "the announcement names the daemon"
    );
    let sessions = requests[1].body["sessions"].as_array().unwrap().clone();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], fixture.stored_id.as_str());
    assert_eq!(sessions[0]["state"], "ended");
    assert_eq!(sessions[0]["daemon"], true);
    assert_eq!(sessions[0]["resumeArgv"][1], "--session");
    assert_eq!(sessions[0]["firstMessage"], "old");

    let machine = fixture.fake.machine("01H").expect("announced");
    assert!(machine.online && machine.daemon);
    assert!(has_state(&fixture.fake, &fixture.stored_id, "ended"));
    fixture.link.stop().await;
}

#[tokio::test]
async fn the_complete_list_at_link_start_removes_only_what_the_daemon_no_longer_lists() {
    let fixture = fixture().await;
    started(&fixture).await;
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
    hosted.session.prompt("hi").await.unwrap();
    let hosted_id = hosted.session_id.clone();
    // Idle again after its turn, not idle as before it.
    wait_until(|| {
        listed(&fixture.fake, &hosted_id)
            .is_some_and(|session| session["state"] == "idle" && session["firstMessage"] == "hi")
    })
    .await;
    fixture.link.stop().await;
    assert!(has_state(&fixture.fake, &hosted_id, "ended"), "the goodbye");

    // While the daemon was signed out, the registry kept a session the
    // daemon has since lost, and a terminal `super` on the same machine
    // announced one of its own.
    let record = |id: &str, daemon: bool| SessionRecord {
        session_id: id.into(),
        workspace: "/w".into(),
        cwd: "/w".into(),
        state: SessionState::Ended,
        blocked_reason: None,
        model: String::new(),
        thinking_level: String::new(),
        name: None,
        first_message: None,
        resume_argv: Vec::new(),
        headless: false,
        daemon,
        last_activity_at: None,
    };
    fixture
        .registry
        .put_session("01H", &record("lost", true))
        .await
        .unwrap();
    fixture
        .registry
        .put_session("01H", &record("terminal", false))
        .await
        .unwrap();
    assert_eq!(fixture.fake.sessions("01H").len(), 4);

    let before = lists(&fixture.fake).len();
    let link = start_link(
        &fixture.client,
        &fixture.registry,
        &fixture.ctx,
        fixture.dir.path(),
    );
    wait_until(|| lists(&fixture.fake).len() > before).await;
    wait_until(|| listed(&fixture.fake, "lost").is_none()).await;
    let list = lists(&fixture.fake).pop().unwrap();
    let mut ids: Vec<&str> = list.body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|session| session["id"].as_str())
        .collect();
    ids.sort_unstable();
    let mut expected = [fixture.stored_id.as_str(), hosted_id.as_str()];
    expected.sort_unstable();
    assert_eq!(ids, expected);
    // Everything the daemon lists is still there, in its state of now; the
    // session of the terminal is not the daemon's to remove.
    assert!(has_state(&fixture.fake, &fixture.stored_id, "ended"));
    assert!(has_state(&fixture.fake, &hosted_id, "idle"));
    assert!(listed(&fixture.fake, "terminal").is_some());
    assert_eq!(fixture.fake.sessions("01H").len(), 3);
    link.stop().await;
}

#[tokio::test]
async fn events_the_link_lost_are_made_up_for_with_the_complete_list() {
    let fixture = fixture().await;
    started(&fixture).await;
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
    wait_until(|| has_state(&fixture.fake, &id, "idle")).await;
    assert_eq!(lists(&fixture.fake).len(), 1);

    // More events than the daemon keeps for a subscriber, before the link
    // gets to read one of them.
    for _ in 0..200 {
        fixture.ctx.host.attach(&hosted);
        fixture.ctx.host.detach(&hosted);
    }
    wait_until(|| lists(&fixture.fake).len() == 2).await;
    let list = lists(&fixture.fake).pop().unwrap();
    let mut ids: Vec<&str> = list.body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|session| session["id"].as_str())
        .collect();
    ids.sort_unstable();
    let mut expected = [fixture.stored_id.as_str(), id.as_str()];
    expected.sort_unstable();
    assert_eq!(ids, expected);
    assert!(has_state(&fixture.fake, &id, "idle"));
    assert!(has_state(&fixture.fake, &fixture.stored_id, "ended"));
    fixture.link.stop().await;
}

#[tokio::test]
async fn a_listed_session_names_the_time_of_its_last_activity() {
    let fixture = fixture().await;
    started(&fixture).await;
    fixture.link.stop().await;

    // The stored session was last used two days ago.
    let announced = listed(&fixture.fake, &fixture.stored_id).unwrap();
    let file = std::path::PathBuf::from(announced["resumeArgv"][2].as_str().unwrap());
    let activity = age_two_days(&file);

    // A new link lists it again, today.
    let before = lists(&fixture.fake).len();
    let link = start_link(
        &fixture.client,
        &fixture.registry,
        &fixture.ctx,
        fixture.dir.path(),
    );
    wait_until(|| lists(&fixture.fake).len() > before).await;
    let list = lists(&fixture.fake).pop().unwrap();
    assert_eq!(
        list.body["sessions"][0]["lastActivityAt"],
        activity.as_str()
    );
    wait_until(|| {
        listed(&fixture.fake, &fixture.stored_id)
            .is_some_and(|session| session["lastActivityAt"] == activity.as_str())
    })
    .await;
    // That is the session's last activity, whenever the registry heard of it.
    let session = listed(&fixture.fake, &fixture.stored_id).unwrap();
    assert_eq!(session["state"], "ended");
    let heard = seconds(session["updatedAt"].as_str().unwrap());
    assert!(heard - seconds(&activity) > 86_400, "{session}");
    link.stop().await;
}

#[tokio::test]
async fn a_turn_moves_the_time_of_the_last_activity() {
    let fixture = fixture().await;
    started(&fixture).await;
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
    wait_until(|| has_state(&fixture.fake, &id, "idle")).await;

    // Pretend the session was started two days ago and left alone since.
    let created = listed(&fixture.fake, &id).unwrap();
    let file = std::path::PathBuf::from(created["resumeArgv"][2].as_str().unwrap());
    let started = age_two_days(&file);

    hosted.session.prompt("hi").await.unwrap();
    wait_until(|| {
        listed(&fixture.fake, &id)
            .is_some_and(|session| session["state"] == "idle" && session["firstMessage"] == "hi")
    })
    .await;
    // The session as it is after the turn names the time of the reply,
    // which is the last thing written to its file.
    let settled = listed(&fixture.fake, &id).unwrap();
    assert_eq!(
        settled["lastActivityAt"],
        modified(&file).as_str(),
        "{settled}"
    );
    assert!(
        seconds(settled["lastActivityAt"].as_str().unwrap()) - seconds(&started) > 86_400,
        "{settled}"
    );
    fixture.link.stop().await;
}

#[tokio::test]
async fn a_session_that_ends_with_nothing_said_leaves_the_registry() {
    let fixture = fixture().await;
    started(&fixture).await;
    let create = || async {
        fixture
            .ctx
            .host
            .create(CreateRequest {
                cwd: fixture.dir.path().join("work"),
                model: Some("mock/mock-1".into()),
                ..Default::default()
            })
            .await
            .unwrap()
    };
    // Two sessions are opened; something is said in one of them only.
    let empty = create().await;
    let used = create().await;
    let (empty_id, used_id) = (empty.session_id.clone(), used.session_id.clone());
    wait_until(|| {
        has_state(&fixture.fake, &empty_id, "idle") && has_state(&fixture.fake, &used_id, "idle")
    })
    .await;
    used.session.prompt("hi").await.unwrap();
    wait_until(|| {
        listed(&fixture.fake, &used_id)
            .is_some_and(|session| session["state"] == "idle" && session["firstMessage"] == "hi")
    })
    .await;
    drop((empty, used));

    // Both end. The one that was used stays listed, to be opened again; the
    // other one has nothing to open and is gone.
    fixture
        .ctx
        .host
        .stop(&empty_id, EndReason::Stopped)
        .await
        .unwrap();
    fixture
        .ctx
        .host
        .stop(&used_id, EndReason::Stopped)
        .await
        .unwrap();
    wait_until(|| has_state(&fixture.fake, &used_id, "ended")).await;
    wait_until(|| listed(&fixture.fake, &empty_id).is_none()).await;
    fixture.link.stop().await;
}

#[tokio::test]
async fn a_create_session_over_the_platform_is_answered_mirrored_and_registered() {
    let fixture = fixture().await;
    started(&fixture).await;
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

    // The new session is hosted, mirrored, and registered as idle, with one
    // request for it alone.
    wait_until(|| has_state(&fixture.fake, &id, "idle")).await;
    let session_path = format!("/api/machines/01H/sessions/{id}");
    let puts = |fake: &FakePlatform| -> Vec<ApiRequest> {
        fake.requests()
            .into_iter()
            .filter(|request| request.method == "PUT" && request.path == session_path)
            .collect()
    };
    assert_eq!(puts(&fixture.fake)[0].body["state"], "idle");
    assert_eq!(lists(&fixture.fake).len(), 1, "no second complete list");
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
        puts(&fixture.fake)
            .iter()
            .any(|request| request.body["state"] == "working")
    })
    .await;
    let entries = format!("sessions/01H/{id}/entries");
    wait_until(|| bodies(&fixture.fake, &entries).len() >= 2).await;

    // Stopping over the machine's stream unloads it and announces the end.
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
    wait_until(|| has_state(&fixture.fake, &id, "ended")).await;
    assert!(fixture.ctx.host.get(&id).is_none());
    // The ended session says what a listing of the stored session says: no
    // model, no thinking level.
    let ended = listed(&fixture.fake, &id).unwrap();
    assert_eq!(ended["model"], "");
    assert_eq!(ended["thinkingLevel"], "");
    assert_eq!(ended["daemon"], true);
    fixture.link.stop().await;
}

#[tokio::test]
async fn a_restarted_link_resumes_from_its_cursor_and_runs_nothing_twice() {
    let fixture = fixture().await;
    started(&fixture).await;
    command(&fixture.fake, "c1", json!({"type": "status"}));
    wait_until(|| {
        bodies(&fixture.fake, "machines/01H/events")
            .iter()
            .any(|b| b["id"] == "c1")
    })
    .await;
    let status = bodies(&fixture.fake, "machines/01H/events")
        .into_iter()
        .find(|b| b["id"] == "c1")
        .unwrap();
    assert_eq!(status["data"]["machineId"], "01H", "{status}");
    assert_eq!(status["data"]["machineName"], "mbp");
    fixture.link.stop().await;

    // Appended while no link runs: the next link must run it, and only it.
    command(&fixture.fake, "c2", json!({"type": "list_workspaces"}));
    let link = start_link(
        &fixture.client,
        &fixture.registry,
        &fixture.ctx,
        fixture.dir.path(),
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
    wait_until(|| lists(&fixture.fake).len() == 2).await;
    assert_eq!(
        calls(&fixture.fake)
            .iter()
            .filter(|call| *call == "PUT /api/machines/01H")
            .count(),
        2,
        "every link start announces the machine"
    );
    link.stop().await;
}

#[tokio::test]
async fn stopping_the_link_marks_hosted_sessions_ended_and_stops_mirroring() {
    let fixture = fixture().await;
    started(&fixture).await;
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
    wait_until(|| has_state(&fixture.fake, &id, "idle")).await;
    let stream = format!("sessions/01H/{id}/events");
    hosted.session.prompt("hi").await.unwrap();
    wait_until(|| {
        bodies(&fixture.fake, &stream)
            .iter()
            .any(|b| b["type"] == "agent_settled")
    })
    .await;

    // Signing out: the link stops while the session stays hosted. The
    // goodbye is in the registry before `stop` returns.
    fixture.link.stop().await;
    let last = listed(&fixture.fake, &id).unwrap();
    assert_eq!(last["state"], "ended", "{last}");
    assert!(
        fixture.ctx.host.get(&id).is_some(),
        "the session keeps running"
    );

    // Nothing of the session reaches the platform any more.
    let mirrored = bodies(&fixture.fake, &stream).len();
    let requests = fixture.fake.requests().len();
    hosted.session.prompt("again").await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(bodies(&fixture.fake, &stream).len(), mirrored);
    assert_eq!(fixture.fake.requests().len(), requests);
}

#[tokio::test]
async fn a_token_the_registry_rejects_ends_the_link() {
    let fixture = fixture().await;
    started(&fixture).await;
    assert!(!fixture.link.unauthorized());
    // The next thing the daemon tells the registry is answered 401.
    fixture.fake.fail_api(Some(401));
    fixture
        .ctx
        .host
        .create(CreateRequest {
            cwd: fixture.dir.path().join("work"),
            model: Some("mock/mock-1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    wait_until(|| fixture.link.unauthorized()).await;
    fixture.link.stop().await;
}

#[tokio::test]
async fn connection_bound_and_malformed_commands_are_refused_and_expired_ones_dropped() {
    let fixture = fixture().await;
    started(&fixture).await;
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
