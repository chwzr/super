//! The registry writer against the fake S2: one host record, one record per
//! session, nothing re-sent unchanged, everything again on a refresh, and a
//! goodbye for every live session.

use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use super_remote::fake::{FakeS2, TestCredentials, serve};
use super_remote::s2::S2Client;
use super_remote::{HostIdentity, RegistryWriter, SessionRecord, SessionState};

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
        daemon: true,
    }
}

fn bodies(fake: &FakeS2) -> Vec<Value> {
    fake.records("registry")
        .iter()
        .map(|record| serde_json::from_str(&record.body).unwrap())
        .collect()
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
async fn one_host_record_then_one_record_per_session() {
    let fake = Arc::new(FakeS2::default());
    let base = serve(fake.clone()).await;
    let client = Arc::new(S2Client::new(
        reqwest::Client::new(),
        base,
        Arc::new(TestCredentials),
    ));
    let host = HostIdentity {
        host_id: "01H".into(),
        name: "mbp".into(),
        platform: "macos".into(),
    };
    let writer = RegistryWriter::start(client, host, "device-1".into());
    writer.publish(record("a", SessionState::Idle));
    wait_until(|| bodies(&fake).len() == 2).await;
    writer.publish(record("b", SessionState::Ended));
    wait_until(|| bodies(&fake).len() == 3).await;
    writer.publish(record("a", SessionState::Idle)); // unchanged: not re-sent
    writer.publish(record("a", SessionState::Working));
    wait_until(|| bodies(&fake).len() == 4).await;
    let all = bodies(&fake);
    assert_eq!(all[0]["kind"], "host");
    assert_eq!(all[0]["hostId"], "01H");
    let sessions: Vec<(String, String)> = all[1..]
        .iter()
        .map(|b| {
            (
                b["sessionId"].as_str().unwrap().to_string(),
                b["state"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        sessions,
        vec![
            ("a".into(), "idle".into()),
            ("b".into(), "ended".into()),
            ("a".into(), "working".into())
        ]
    );
    assert_eq!(all[1]["daemon"], true);

    // A refresh sends the host record and every session record again, so a
    // host that runs for longer than the registry's retention stays listed.
    writer.refresh();
    wait_until(|| bodies(&fake).len() == 7).await;
    let all = bodies(&fake);
    assert_eq!(all[4]["kind"], "host");
    assert_eq!(all[4]["hostId"], "01H");
    assert_eq!(
        (all[5]["sessionId"].as_str(), all[5]["state"].as_str()),
        (Some("a"), Some("working"))
    );
    assert_eq!(
        (all[6]["sessionId"].as_str(), all[6]["state"].as_str()),
        (Some("b"), Some("ended"))
    );

    writer.stop().await;
    let all = bodies(&fake);
    // The goodbye: `a` was working and is announced ended; `b` already was.
    let goodbye: Vec<&Value> = all[7..].iter().collect();
    assert_eq!(goodbye.len(), 1, "{all:?}");
    assert_eq!(goodbye[0]["sessionId"], "a");
    assert_eq!(goodbye[0]["state"], "ended");
}
