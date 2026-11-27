//! A session follows the model catalog while it runs: a model added after
//! the session started can be chosen and is offered, whether another process
//! saved it to the catalog file or only the platform knows it yet. The daemon
//! hosts a session for days, so it cannot answer from the catalog of the day
//! the session started.
//!
//! Own test binary: it points HOME and the platform URL at a temporary
//! directory and a local server. Unix only, because Windows resolves the
//! profile directory without HOME.

#![cfg(all(unix, feature = "mock", feature = "native"))]

use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};
use super_sdk::mock::{MockProvider, MockScript};
use super_sdk::protocol::{Command, Response};
use super_sdk::{Session, SessionOptions, ThinkingLevel};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

/// Both tests change the process environment, so they take turns.
static ENVIRONMENT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn set_model(provider: &str, model_id: &str) -> Command {
    Command::SetModel {
        provider: provider.into(),
        model_id: model_id.into(),
    }
}

/// The `provider/id` of every model `get_available_models` offers.
async fn offered(session: &Arc<Session>) -> Vec<String> {
    let response = session
        .execute(Command::GetAvailableModels { search: None })
        .await;
    assert!(response.success, "{response:?}");
    response.data.expect("models data")["models"]
        .as_array()
        .expect("a models array")
        .iter()
        .map(|model| {
            format!(
                "{}/{}",
                model["provider"].as_str().unwrap(),
                model["id"].as_str().unwrap()
            )
        })
        .collect()
}

fn chosen(response: &Response) -> &str {
    assert!(response.success, "{response:?}");
    response.data.as_ref().expect("the model")["id"]
        .as_str()
        .expect("a model id")
}

/// Add a model to the `mock` provider of the catalog at `path`, as another
/// `super` process does when it saves a provider or the account's models.
fn add_to_catalog(path: &Path, id: &str) {
    let mut catalog: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("read the catalog"))
            .expect("the catalog is JSON");
    catalog["providers"]["mock"]["models"]
        .as_array_mut()
        .expect("the mock models")
        .push(json!({"id": id, "name": id, "contextWindow": 128000, "maxTokens": 4096}));
    std::fs::write(path, catalog.to_string()).expect("write the catalog");
}

#[tokio::test]
async fn a_model_saved_to_the_catalog_file_can_be_chosen() {
    let _turn = ENVIRONMENT.lock().await;
    let home = tempfile::tempdir().expect("temp home");
    // SAFETY: the tests of this binary take turns, and nothing else runs.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::remove_var("SUPER_MODELS_FILE");
    }

    let directory = tempfile::tempdir().expect("temp dir");
    let provider = MockProvider::start(directory.path(), MockScript::text("hi"))
        .await
        .expect("mock provider starts");
    let session = Session::create(SessionOptions {
        cwd: directory.path().to_path_buf(),
        model: Some("mock/mock-1".into()),
        models_file: Some(provider.catalog_path()),
        thinking_level: Some(ThinkingLevel::Off),
        settings: Some(super_coding::Settings::default()),
        no_context_files: true,
        ..Default::default()
    })
    .await
    .expect("session builds");

    let refused = session.execute(set_model("mock", "mock-2")).await;
    assert_eq!(
        refused.error.as_deref(),
        Some("model not found: mock/mock-2")
    );

    add_to_catalog(&provider.catalog_path(), "mock-2");
    assert_eq!(
        chosen(&session.execute(set_model("mock", "mock-2")).await),
        "mock-2"
    );
    assert_eq!(session.state().model.expect("a model").id, "mock-2");
    assert_eq!(offered(&session).await, ["mock/mock-1", "mock/mock-2"]);

    // A new model whose name is part of an older one's: the older one must
    // not be taken for it.
    add_to_catalog(&provider.catalog_path(), "mock");
    assert_eq!(
        chosen(&session.execute(set_model("mock", "mock")).await),
        "mock"
    );

    // The list is read from the file too, without a model being asked for.
    add_to_catalog(&provider.catalog_path(), "mock-3");
    assert!(offered(&session).await.contains(&"mock/mock-3".to_string()));
}

/// A platform that answers `GET /api/models` with the models it has now.
struct Platform {
    url: String,
    models: Arc<Mutex<Vec<String>>>,
    asked: Arc<Mutex<usize>>,
}

impl Platform {
    async fn start(models: &[&str]) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let models = Arc::new(Mutex::new(
            models.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        ));
        let asked = Arc::new(Mutex::new(0));
        let (served, count) = (models.clone(), asked.clone());
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut chunk = [0_u8; 4096];
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let read = socket.read(&mut chunk).await.unwrap();
                    assert!(read > 0, "the request ended early");
                    request.extend_from_slice(&chunk[..read]);
                }
                let request = String::from_utf8_lossy(&request).to_string();
                assert!(request.starts_with("GET /api/models "), "{request}");
                *count.lock().unwrap() += 1;
                let models: Vec<Value> = served
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|id| {
                        json!({
                            "id": id,
                            "name": id,
                            "api": "openai-completions",
                            "provider": "super",
                            "baseUrl": "https://gateway.example/v1",
                            "reasoning": false,
                            "input": ["text"],
                            "cost": {"input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0},
                            "contextWindow": 128000,
                            "maxTokens": 4096
                        })
                    })
                    .collect();
                let body = json!({"models": models}).to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        Self { url, models, asked }
    }

    fn add(&self, id: &str) {
        self.models.lock().unwrap().push(id.into());
    }

    fn asked(&self) -> usize {
        *self.asked.lock().unwrap()
    }
}

#[tokio::test]
async fn a_model_the_platform_added_can_be_chosen() {
    let _turn = ENVIRONMENT.lock().await;
    let home = tempfile::tempdir().expect("temp home");
    let platform = Platform::start(&["alpha"]).await;
    // SAFETY: the tests of this binary take turns, and nothing else runs.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("SUPER_PLATFORM_URL", &platform.url);
        std::env::remove_var("SUPER_MODELS_FILE");
    }
    super_ai::auth::store_oauth(
        "super",
        super_ai::auth::OAuthCredential {
            kind: "oauth".into(),
            access: "access".into(),
            refresh: "refresh".into(),
            expires: i64::MAX,
            account_id: String::new(),
            client_id: None,
            available_model_ids: None,
            device_id: None,
        },
    )
    .expect("store the sign-in");

    // Nothing is saved on this machine yet: the session starts on a model
    // only the platform knows.
    let directory = tempfile::tempdir().expect("temp dir");
    let session = Session::create(SessionOptions {
        cwd: directory.path().to_path_buf(),
        model: Some("super/alpha".into()),
        thinking_level: Some(ThinkingLevel::Off),
        settings: Some(super_coding::Settings::default()),
        no_context_files: true,
        ..Default::default()
    })
    .await
    .expect("session builds");
    assert_eq!(session.state().model.expect("a model").id, "alpha");
    assert_eq!(platform.asked(), 1);

    // The platform gains a model while the session runs.
    platform.add("beta");
    assert_eq!(
        chosen(&session.execute(set_model("super", "beta")).await),
        "beta"
    );
    assert_eq!(platform.asked(), 2);
    // A model the session knows is chosen without asking anyone.
    assert_eq!(
        chosen(&session.execute(set_model("super", "alpha")).await),
        "alpha"
    );
    assert_eq!(platform.asked(), 2);

    // The list comes from the platform's answer of a moment ago; a model it
    // does not have yet is still found when a client asks for it by name.
    platform.add("gamma");
    assert_eq!(offered(&session).await, ["super/alpha", "super/beta"]);
    assert_eq!(platform.asked(), 2);
    assert_eq!(
        chosen(&session.execute(set_model("super", "gamma")).await),
        "gamma"
    );
    assert_eq!(platform.asked(), 3);
    assert_eq!(
        offered(&session).await,
        ["super/alpha", "super/beta", "super/gamma"]
    );

    // A model nobody has is refused, after the platform was asked.
    let refused = session.execute(set_model("super", "delta")).await;
    assert_eq!(
        refused.error.as_deref(),
        Some("model not found: super/delta")
    );
    assert_eq!(platform.asked(), 4);
}
