//! The registry writer and reader against the fake platform: the machine is
//! announced first, every change to a session is sent once, heartbeats keep
//! their own pace, a daemon sends its complete list, a forgotten machine is
//! announced again, failed sends are retried, and a goodbye marks every live
//! session as ended.

use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use super_remote::fake::{ApiRequest, FakePlatform, registry_client, serve};
use super_remote::registry::{RegistryClient, WriterOptions};
use super_remote::{MachineIdentity, RegistryWriter, SessionRecord, SessionState};

fn record(id: &str, state: SessionState) -> SessionRecord {
    SessionRecord {
        session_id: id.into(),
        workspace: "/w".into(),
        cwd: "/w".into(),
        state,
        blocked_reason: None,
        model: "mock/mock-1".into(),
        thinking_level: "off".into(),
        name: None,
        first_message: None,
        resume_argv: vec!["super".into(), "--session".into(), format!("/s/{id}.jsonl")],
        headless: false,
        daemon: false,
        last_activity_at: None,
    }
}

fn daemon_record(id: &str, state: SessionState) -> SessionRecord {
    SessionRecord {
        daemon: true,
        ..record(id, state)
    }
}

async fn fake_platform() -> (Arc<FakePlatform>, Arc<RegistryClient>) {
    let fake = Arc::new(FakePlatform::default());
    serve(fake.clone()).await;
    let client = registry_client(&fake);
    (fake, client)
}

fn machine() -> MachineIdentity {
    MachineIdentity {
        machine_id: "01H".into(),
        name: "mbp".into(),
        platform: "macos".into(),
    }
}

/// A writer whose heartbeats and retries do not wait for half a minute.
fn quick() -> WriterOptions {
    WriterOptions {
        heartbeat: Duration::from_millis(100),
        retry: Duration::from_millis(50),
    }
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

/// `METHOD path` of every registry request so far.
fn calls(fake: &FakePlatform) -> Vec<String> {
    fake.requests()
        .iter()
        .map(|request| format!("{} {}", request.method, request.path))
        .collect()
}

fn session_puts(fake: &FakePlatform, id: &str) -> Vec<ApiRequest> {
    let path = format!("/api/machines/01H/sessions/{id}");
    fake.requests()
        .into_iter()
        .filter(|request| request.method == "PUT" && request.path == path)
        .collect()
}

fn state_of(fake: &FakePlatform, id: &str) -> Option<String> {
    fake.session("01H", id)
        .and_then(|session| session["state"].as_str().map(str::to_string))
}

#[tokio::test]
async fn the_machine_is_announced_before_the_first_session() {
    let (fake, client) = fake_platform().await;
    let writer = RegistryWriter::start(client, machine());
    // Published before the writer said anything to the platform.
    writer.publish(record("a", SessionState::Idle));
    wait_until(|| fake.session("01H", "a").is_some()).await;

    let requests = fake.requests();
    assert_eq!(
        calls(&fake),
        ["PUT /api/machines/01H", "PUT /api/machines/01H/sessions/a"]
    );
    // A standalone `super` is no daemon: the announcement leaves the flag out.
    assert_eq!(
        requests[0].body,
        json!({"name": "mbp", "platform": "macos"})
    );
    assert_eq!(
        requests[1].body,
        json!({
            "workspace": "/w", "cwd": "/w", "state": "idle", "blockedReason": null,
            "model": "mock/mock-1", "thinkingLevel": "off", "name": null,
            "firstMessage": null, "resumeArgv": ["super", "--session", "/s/a.jsonl"],
            "headless": false, "daemon": false
        })
    );
    let listed = fake.machine("01H").expect("announced");
    assert_eq!(
        (listed.name.as_str(), listed.platform.as_str()),
        ("mbp", "macos")
    );
    assert!(listed.online && !listed.daemon);
    assert!(
        fake.seen_tokens()
            .iter()
            .all(|token| token == "Bearer secret-token"),
        "every call carries the platform token"
    );
    writer.stop().await;
}

#[tokio::test]
async fn a_session_is_sent_when_it_changed_and_not_for_a_newer_time_alone() {
    let (fake, client) = fake_platform().await;
    let writer = RegistryWriter::start(client, machine());
    let at = |time: &str, state| SessionRecord {
        last_activity_at: Some(time.into()),
        ..record("a", state)
    };
    writer.publish(at("2026-11-12T16:00:00Z", SessionState::Idle));
    wait_until(|| session_puts(&fake, "a").len() == 1).await;

    // The same again, and the same with a newer time of last activity:
    // neither is worth a request.
    writer.publish(at("2026-11-12T16:00:00Z", SessionState::Idle));
    writer.publish(at("2026-11-12T16:05:00Z", SessionState::Idle));
    // Another session goes through the writer after them, so they were seen.
    writer.publish(record("b", SessionState::Ended));
    wait_until(|| fake.session("01H", "b").is_some()).await;
    assert_eq!(session_puts(&fake, "a").len(), 1, "{:?}", calls(&fake));
    assert_eq!(
        fake.session("01H", "a").unwrap()["lastActivityAt"],
        "2026-11-12T16:00:00Z"
    );

    // A change of state is sent, with the time it names.
    writer.publish(at("2026-11-12T16:10:00Z", SessionState::Working));
    wait_until(|| state_of(&fake, "a").as_deref() == Some("working")).await;
    assert_eq!(session_puts(&fake, "a").len(), 2);
    assert_eq!(
        fake.session("01H", "a").unwrap()["lastActivityAt"],
        "2026-11-12T16:10:00Z"
    );

    // A session without a time gets the platform's, and keeps it.
    let first = fake.session("01H", "b").unwrap()["lastActivityAt"].clone();
    assert!(first.is_string(), "{first}");
    writer.publish(record("b", SessionState::Idle));
    wait_until(|| state_of(&fake, "b").as_deref() == Some("idle")).await;
    assert_eq!(fake.session("01H", "b").unwrap()["lastActivityAt"], first);
    writer.stop().await;
}

#[tokio::test]
async fn heartbeats_keep_their_pace_while_session_updates_keep_coming() {
    let (fake, client) = fake_platform().await;
    let writer = RegistryWriter::start_with(client, machine(), false, quick());
    // A session that changes every 10 ms for 600 ms: six heartbeats' time.
    for round in 0..60 {
        let state = if round % 2 == 0 {
            SessionState::Working
        } else {
            SessionState::Idle
        };
        writer.publish(record("a", state));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let calls = calls(&fake);
    let beats: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| *call == "POST /api/machines/01H/heartbeat")
        .map(|(index, _)| index)
        .collect();
    assert!(beats.len() >= 3, "{} heartbeats in {calls:?}", beats.len());
    // They went out between the session's updates, not after them.
    let last_update = calls
        .iter()
        .rposition(|call| call == "PUT /api/machines/01H/sessions/a")
        .expect("updates");
    assert!(beats[1] < last_update, "{calls:?}");
    let heartbeat = fake
        .requests()
        .into_iter()
        .find(|request| request.path.ends_with("/heartbeat"))
        .unwrap();
    assert_eq!(heartbeat.body, json!({}), "no daemon here");
    assert_eq!(fake.machine("01H").unwrap().announcements, 1);
    writer.stop().await;
}

#[tokio::test]
async fn a_daemon_says_so_and_sends_its_complete_list_in_one_request() {
    let (fake, client) = fake_platform().await;
    // A terminal `super` on the same machine has a session of its own.
    let terminal = RegistryWriter::start(client.clone(), machine());
    terminal.publish(record("t", SessionState::Idle));
    wait_until(|| fake.session("01H", "t").is_some()).await;
    assert!(!fake.machine("01H").unwrap().daemon);

    let writer = RegistryWriter::start_with(client, machine(), true, quick());
    wait_until(|| fake.machine("01H").is_some_and(|machine| machine.daemon)).await;
    let announced = fake
        .requests()
        .into_iter()
        .rfind(|request| request.path == "/api/machines/01H")
        .unwrap();
    assert_eq!(
        announced.body,
        json!({"name": "mbp", "platform": "macos", "daemon": true})
    );

    writer.publish_all(vec![
        daemon_record("a", SessionState::Ended),
        daemon_record("b", SessionState::Idle),
    ]);
    wait_until(|| fake.sessions("01H").len() == 3).await;
    let lists = |fake: &FakePlatform| -> Vec<ApiRequest> {
        fake.requests()
            .into_iter()
            .filter(|request| request.path == "/api/machines/01H/sessions")
            .collect()
    };
    assert_eq!(lists(&fake).len(), 1);
    assert_eq!(lists(&fake)[0].method, "PUT");
    let listed = lists(&fake)[0].body["sessions"].clone();
    assert_eq!(listed[0]["id"], "a");
    assert_eq!(listed[1]["id"], "b");
    assert_eq!(listed[1]["daemon"], true);
    assert!(
        session_puts(&fake, "a").is_empty(),
        "no request per session"
    );

    // What the list said is what the platform has: the same state again is
    // not sent, a change is.
    writer.publish(daemon_record("b", SessionState::Idle));
    writer.publish(daemon_record("b", SessionState::Working));
    wait_until(|| state_of(&fake, "b").as_deref() == Some("working")).await;
    assert_eq!(session_puts(&fake, "b").len(), 1);

    // The next complete list no longer has `a`: the platform drops it, and
    // keeps what the daemon still lists and what is not the daemon's.
    writer.publish_all(vec![daemon_record("b", SessionState::Working)]);
    wait_until(|| fake.session("01H", "a").is_none()).await;
    assert_eq!(state_of(&fake, "b").as_deref(), Some("working"));
    assert_eq!(state_of(&fake, "t").as_deref(), Some("idle"));

    // The daemon's heartbeats say that it is one.
    wait_until(|| {
        fake.requests()
            .iter()
            .any(|request| request.path.ends_with("/heartbeat") && request.body["daemon"] == true)
    })
    .await;

    writer.stop().await;
    terminal.stop().await;
}

#[tokio::test]
async fn a_machine_the_platform_forgot_is_announced_again_with_its_sessions() {
    let (fake, client) = fake_platform().await;
    let writer = RegistryWriter::start_with(client, machine(), false, quick());
    writer.publish(record("a", SessionState::Idle));
    wait_until(|| fake.session("01H", "a").is_some()).await;

    // The next heartbeat is answered 404.
    let before = calls(&fake).len();
    fake.remove_machine("01H");
    wait_until(|| fake.session("01H", "a").is_some()).await;
    let after: Vec<String> = calls(&fake).split_off(before);
    let announced = after
        .iter()
        .position(|call| call == "PUT /api/machines/01H")
        .expect("announced again");
    assert_eq!(
        after[announced - 1..announced + 2],
        [
            "POST /api/machines/01H/heartbeat",
            "PUT /api/machines/01H",
            "PUT /api/machines/01H/sessions/a"
        ],
        "{after:?}"
    );
    assert_eq!(fake.machine("01H").unwrap().name, "mbp");

    // So is a session update.
    fake.remove_machine("01H");
    writer.publish(record("a", SessionState::Working));
    wait_until(|| state_of(&fake, "a").as_deref() == Some("working")).await;
    assert!(fake.machine("01H").is_some());
    writer.stop().await;
}

#[tokio::test]
async fn a_daemon_the_platform_forgot_sends_its_list_again() {
    let (fake, client) = fake_platform().await;
    let writer = RegistryWriter::start_with(client, machine(), true, quick());
    writer.publish_all(vec![
        daemon_record("a", SessionState::Ended),
        daemon_record("b", SessionState::Idle),
    ]);
    wait_until(|| fake.sessions("01H").len() == 2).await;
    writer.publish(daemon_record("b", SessionState::Working));
    wait_until(|| state_of(&fake, "b").as_deref() == Some("working")).await;

    fake.remove_machine("01H");
    wait_until(|| fake.sessions("01H").len() == 2).await;
    let machine = fake.machine("01H").expect("announced again");
    assert!(machine.daemon);
    assert_eq!(state_of(&fake, "a").as_deref(), Some("ended"));
    assert_eq!(state_of(&fake, "b").as_deref(), Some("working"));
    let lists = fake
        .requests()
        .iter()
        .filter(|request| request.path == "/api/machines/01H/sessions")
        .count();
    assert_eq!(lists, 2, "one request for all of them");
    writer.stop().await;
}

#[tokio::test]
async fn a_failed_send_is_retried_with_the_newest_state_and_blocks_nobody() {
    let (fake, client) = fake_platform().await;
    fake.fail_api(Some(503));
    let writer = RegistryWriter::start_with(client, machine(), false, quick());
    // `publish` returns at once, whatever the platform does.
    let before = std::time::Instant::now();
    writer.publish(record("a", SessionState::Working));
    assert!(before.elapsed() < Duration::from_millis(50));
    wait_until(|| calls(&fake).len() >= 3).await;
    assert!(
        calls(&fake)
            .iter()
            .all(|call| call == "PUT /api/machines/01H"),
        "nothing before the announcement: {:?}",
        calls(&fake)
    );
    assert!(!writer.unauthorized());
    writer.publish(record("a", SessionState::Idle));

    fake.fail_api(None);
    wait_until(|| fake.session("01H", "a").is_some()).await;
    assert_eq!(state_of(&fake, "a").as_deref(), Some("idle"));
    assert_eq!(session_puts(&fake, "a").len(), 1, "only the newest state");
    writer.stop().await;
}

#[tokio::test]
async fn a_rejected_token_is_reported() {
    let (fake, client) = fake_platform().await;
    let writer = RegistryWriter::start_with(client, machine(), true, quick());
    wait_until(|| fake.machine("01H").is_some()).await;
    assert!(!writer.unauthorized());
    fake.fail_api(Some(401));
    wait_until(|| writer.unauthorized()).await;
    writer.stop().await;
}

#[tokio::test]
async fn the_goodbye_marks_every_live_session_as_ended() {
    let (fake, client) = fake_platform().await;
    let writer = RegistryWriter::start(client, machine());
    writer.publish(SessionRecord {
        blocked_reason: Some("Workflow approval required".into()),
        ..record("a", SessionState::Blocked)
    });
    writer.publish(record("b", SessionState::Ended));
    writer.publish(record("c", SessionState::Working));
    wait_until(|| fake.sessions("01H").len() == 3).await;
    let before = fake.requests().len();

    writer.stop().await;
    let goodbye: Vec<ApiRequest> = fake.requests().into_iter().skip(before).collect();
    let paths: Vec<&str> = goodbye
        .iter()
        .map(|request| request.path.as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "/api/machines/01H/sessions/a",
            "/api/machines/01H/sessions/c"
        ],
        "`b` had ended already"
    );
    let a = fake.session("01H", "a").unwrap();
    assert_eq!(a["state"], "ended");
    assert_eq!(a["blockedReason"], Value::Null);
    assert_eq!(state_of(&fake, "c").as_deref(), Some("ended"));
    // There is no goodbye for the machine: it goes offline when the
    // heartbeats stop.
    assert!(fake.machine("01H").unwrap().online);
}

#[tokio::test]
async fn a_daemon_without_sessions_is_announced_and_has_no_goodbye() {
    let (fake, client) = fake_platform().await;
    let writer = RegistryWriter::start_daemon(client, machine());
    wait_until(|| fake.machine("01H").is_some()).await;
    assert!(fake.machine("01H").unwrap().daemon);
    writer.stop().await;
    assert_eq!(calls(&fake), ["PUT /api/machines/01H"]);
}

#[tokio::test]
async fn the_registry_is_read_with_one_request() {
    let (fake, client) = fake_platform().await;
    let empty = client.read().await.expect("read");
    assert!(empty.machines.is_empty() && empty.sessions.is_empty());

    let mbp = RegistryWriter::start_daemon(client.clone(), machine());
    mbp.publish_all(vec![SessionRecord {
        last_activity_at: Some("2026-11-12T16:00:12Z".into()),
        first_message: Some("Fix the login bug".into()),
        ..daemon_record("a", SessionState::Idle)
    }]);
    let linuxbox = RegistryWriter::start(
        client.clone(),
        MachineIdentity {
            machine_id: "02L".into(),
            name: "linuxbox".into(),
            platform: "linux".into(),
        },
    );
    linuxbox.publish(record("r", SessionState::Working));
    wait_until(|| fake.session("01H", "a").is_some() && fake.session("02L", "r").is_some()).await;
    // The platform, not the reader, decides who is online.
    fake.set_online("02L", false);

    let before = fake.requests().len();
    let registry = client.read().await.expect("read");
    let reads: Vec<ApiRequest> = fake.requests().into_iter().skip(before).collect();
    assert_eq!(reads.len(), 1);
    assert_eq!(
        (reads[0].method.as_str(), reads[0].path.as_str()),
        ("GET", "/api/registry")
    );

    let machine = registry.machine("01H").expect("mbp");
    assert_eq!(machine.name, "mbp");
    assert!(machine.online && machine.daemon);
    let other = registry.machine("02L").expect("linuxbox");
    assert_eq!(other.platform, "linux");
    assert!(!other.online && !other.daemon);

    let session = registry.sessions_on("01H").next().expect("a");
    assert_eq!(session.record.session_id, "a");
    assert_eq!(session.record.state, SessionState::Idle);
    assert_eq!(
        session.record.last_activity_at.as_deref(),
        Some("2026-11-12T16:00:12Z")
    );
    assert_eq!(
        session.record.first_message.as_deref(),
        Some("Fix the login bug")
    );
    assert_eq!(
        session.record.resume_argv,
        ["super", "--session", "/s/a.jsonl"]
    );
    let remote = registry.sessions_on("02L").next().expect("r");
    assert_eq!(remote.record.state, SessionState::Working);
    assert!(
        remote.record.last_activity_at.is_some(),
        "the platform's time"
    );

    mbp.stop().await;
    linuxbox.stop().await;
}
