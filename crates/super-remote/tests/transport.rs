//! The transport against a fake S2 server: events are mirrored, entries are
//! appended durably, commands are tailed, executed and answered, and expired
//! commands are dropped.

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_remote::RemoteStatus;
use super_remote::fake::{FakeS2, TestCredentials, now_ms, serve};
use super_remote::s2::S2Client;
use super_remote::transport::{RemoteNotice, SessionStreams, SessionTransport, TransportOptions};
use super_sdk::mock::{MockProvider, MockScript};
use super_sdk::{Session, SessionOptions, ThinkingLevel};

async fn session_with(
    directory: &std::path::Path,
    script: MockScript,
) -> (MockProvider, Arc<Session>) {
    let provider = MockProvider::start(directory, script).await.unwrap();
    let options = SessionOptions {
        cwd: directory.to_path_buf(),
        model: Some("mock/mock-1".into()),
        models_file: Some(provider.catalog_path()),
        thinking_level: Some(ThinkingLevel::Off),
        settings: Some(super_coding::Settings::default()),
        tools: Some(vec!["read".into()]),
        trust_project_files: false,
        no_context_files: true,
        ..Default::default()
    };
    let session = Session::create(options).await.unwrap();
    (provider, session)
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
async fn a_restarted_transport_does_not_execute_applied_commands_again() {
    let directory = tempfile::tempdir().unwrap();
    let cursors = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeS2::default());
    let base_url = serve(fake.clone()).await;
    let (_provider, session) = session_with(directory.path(), MockScript::text("Hello")).await;
    let client = Arc::new(S2Client::new(
        reqwest::Client::new(),
        base_url,
        Arc::new(TestCredentials),
    ));
    let streams = SessionStreams::new("host1", "sess1");
    let options = TransportOptions {
        linger: Duration::from_millis(5),
        cursor_dir: Some(cursors.path().to_path_buf()),
        ..TransportOptions::default()
    };
    let responses = |fake: &FakeS2| -> Vec<String> {
        fake.records(&streams.events)
            .iter()
            .filter(|record| {
                record
                    .headers
                    .iter()
                    .any(|(k, v)| k == "t" && v == "response")
            })
            .filter_map(|record| serde_json::from_str::<Value>(&record.body).ok())
            .filter_map(|value| value["id"].as_str().map(str::to_string))
            .collect()
    };
    let command = |id: &str, message: &str| {
        (
            vec![
                ("t".into(), "prompt".into()),
                ("exp".into(), (now_ms() + 60_000).to_string()),
            ],
            json!({"id": id, "type": "prompt", "message": message}).to_string(),
        )
    };

    // First run: one command arrives and is executed.
    let first = SessionTransport::start(
        client.clone(),
        session.clone(),
        streams.clone(),
        options.clone(),
        Arc::new(|_| {}),
    );
    fake.append(&streams.commands, vec![command("c1", "Say hello")]);
    wait_until(|| responses(&fake) == ["c1"]).await;
    wait_until(|| {
        fake.records(&streams.events).iter().any(|record| {
            record
                .headers
                .iter()
                .any(|(k, v)| k == "t" && v == "agent_settled")
        })
    })
    .await;
    first.stop().await;

    // Second run on the same streams, as after `super --resume`: the command
    // is still within the lookback and not expired, but it was applied, so
    // it must not run again. A new command still does.
    let notices: Arc<Mutex<Vec<RemoteNotice>>> = Default::default();
    let sink = notices.clone();
    let second = SessionTransport::start(
        client,
        session,
        streams.clone(),
        options,
        Arc::new(move |notice| sink.lock().unwrap().push(notice)),
    );
    wait_until(|| {
        notices
            .lock()
            .unwrap()
            .iter()
            .any(|notice| matches!(notice, RemoteNotice::Status(RemoteStatus::Reconnecting)))
    })
    .await;
    fake.append(&streams.commands, vec![command("c2", "Say hello again")]);
    wait_until(|| responses(&fake).contains(&"c2".to_string())).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(responses(&fake), ["c1", "c2"]);
    second.stop().await;
}

#[tokio::test]
async fn events_entries_and_commands_flow_through_the_streams() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeS2::default());
    let base_url = serve(fake.clone()).await;
    let (_provider, session) =
        session_with(directory.path(), MockScript::text("Hello from the mock")).await;
    let client = Arc::new(S2Client::new(
        reqwest::Client::new(),
        base_url,
        Arc::new(TestCredentials),
    ));
    let streams = SessionStreams::new("host1", "sess1");
    let notices: Arc<Mutex<Vec<RemoteNotice>>> = Default::default();
    let sink = notices.clone();
    let transport = SessionTransport::start(
        client,
        session.clone(),
        streams.clone(),
        TransportOptions {
            cursor_dir: Some(directory.path().join("cursors")),
            linger: Duration::from_millis(5),
            ..TransportOptions::default()
        },
        Arc::new(move |notice| sink.lock().unwrap().push(notice)),
    );

    // The status leaves `Starting` once the commands tail is open; it only
    // becomes `Connected` after the first append, which happens on demand.
    wait_until(|| {
        notices
            .lock()
            .unwrap()
            .iter()
            .any(|notice| matches!(notice, RemoteNotice::Status(RemoteStatus::Reconnecting)))
    })
    .await;

    // A controller sends a prompt through the commands stream.
    fake.append(
        &streams.commands,
        vec![(
            vec![
                ("t".into(), "prompt".into()),
                ("name".into(), "iPhone".into()),
                ("exp".into(), (now_ms() + 60_000).to_string()),
            ],
            json!({"id": "c1", "type": "prompt", "message": "Say hello"}).to_string(),
        )],
    );

    // The response and the whole run show up in the events stream.
    wait_until(|| {
        fake.records(&streams.events).iter().any(|record| {
            record
                .headers
                .iter()
                .any(|(k, v)| k == "t" && v == "agent_settled")
        })
    })
    .await;
    let events = fake.records(&streams.events);
    let response = events
        .iter()
        .find(|record| {
            record
                .headers
                .iter()
                .any(|(k, v)| k == "t" && v == "response")
        })
        .expect("response record");
    let response: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(response["id"], "c1");
    assert_eq!(response["success"], true);
    assert_eq!(response["data"]["disposition"], "started");
    assert!(events.iter().any(|record| {
        record
            .headers
            .iter()
            .any(|(k, v)| k == "t" && v == "message_end")
    }));

    // Durable entries carry the conversation with entry ids.
    wait_until(|| fake.records(&streams.entries).len() >= 2).await;
    let entries = fake.records(&streams.entries);
    assert!(
        entries
            .iter()
            .all(|record| record.headers.iter().any(|(k, _)| k == "eid"))
    );
    let bodies: Vec<Value> = entries
        .iter()
        .map(|record| serde_json::from_str(&record.body).unwrap())
        .collect();
    assert!(
        bodies
            .iter()
            .any(|entry| entry["message"]["role"] == "user")
    );
    assert!(
        bodies
            .iter()
            .any(|entry| entry["message"]["role"] == "assistant")
    );

    // The platform token went on the wire; nothing else.
    assert!(
        fake.seen_tokens()
            .iter()
            .all(|token| token == "Bearer secret-token")
    );

    // An expired command is dropped and reported.
    fake.append(
        &streams.commands,
        vec![(
            vec![
                ("t".into(), "ping".into()),
                ("exp".into(), (now_ms() - 1).to_string()),
            ],
            json!({"id": "old", "type": "ping"}).to_string(),
        )],
    );
    wait_until(|| {
        notices
            .lock()
            .unwrap()
            .iter()
            .any(|notice| matches!(notice, RemoteNotice::ExpiredCommands(1)))
    })
    .await;
    assert!(
        !fake
            .records(&streams.events)
            .iter()
            .any(|record| record.body.contains("\"id\":\"old\""))
    );

    // Status settled on connected.
    assert_eq!(transport.status(), RemoteStatus::Connected);
    transport.stop().await;
}

#[tokio::test]
async fn shell_can_be_refused_by_policy() {
    let directory = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeS2::default());
    let base_url = serve(fake.clone()).await;
    let (_provider, session) = session_with(directory.path(), MockScript::text("unused")).await;
    let client = Arc::new(S2Client::new(
        reqwest::Client::new(),
        base_url,
        Arc::new(TestCredentials),
    ));
    let streams = SessionStreams::new("host1", "sess2");
    let notices: Arc<Mutex<Vec<RemoteNotice>>> = Default::default();
    let sink = notices.clone();
    let transport = SessionTransport::start(
        client,
        session,
        streams.clone(),
        TransportOptions {
            cursor_dir: Some(directory.path().join("cursors")),
            allow_shell: false,
            linger: Duration::from_millis(5),
            ..TransportOptions::default()
        },
        Arc::new(move |notice| sink.lock().unwrap().push(notice)),
    );
    fake.append(
        &streams.commands,
        vec![(
            vec![
                ("name".into(), "iPhone".into()),
                ("exp".into(), (now_ms() + 60_000).to_string()),
            ],
            json!({"id": "sh", "type": "bash", "command": "echo hi"}).to_string(),
        )],
    );
    wait_until(|| {
        fake.records(&streams.events)
            .iter()
            .any(|record| record.body.contains("\"id\":\"sh\""))
    })
    .await;
    let record = fake
        .records(&streams.events)
        .into_iter()
        .find(|record| record.body.contains("\"id\":\"sh\""))
        .unwrap();
    let response: Value = serde_json::from_str(&record.body).unwrap();
    assert_eq!(response["success"], false);
    assert!(response["error"].as_str().unwrap().contains("disabled"));
    assert!(notices
        .lock()
        .unwrap()
        .iter()
        .any(|notice| matches!(notice, RemoteNotice::Refused { origin, command } if origin == "iPhone" && command == "bash")));
    transport.stop().await;
}
