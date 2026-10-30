//! `get_available_models` offers what the user can switch to: the models of
//! providers that are configured in `models.json` or have a credential.
//! Own test binary: it points HOME at a temporary directory so the logins on
//! the machine running the tests cannot change the answer. Unix only, because
//! Windows resolves the profile directory without HOME.

#![cfg(all(unix, feature = "mock", feature = "native"))]

use std::collections::BTreeSet;
use super_sdk::mock::{MockProvider, MockScript};
use super_sdk::protocol::Command;
use super_sdk::{Session, SessionOptions, ThinkingLevel};

#[tokio::test]
async fn only_configured_or_authenticated_providers_are_offered() {
    let home = tempfile::tempdir().expect("temp home");
    // SAFETY: the only test in this binary; set before any thread reads the
    // environment.
    unsafe {
        std::env::set_var("HOME", home.path());
        for variable in super_ai::auth::env_var_names("anthropic") {
            std::env::remove_var(variable);
        }
    }
    super_ai::auth::store_api_key("openai", "sk-test").expect("store a test key");

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
        trust_project_files: false,
        no_context_files: true,
        ..Default::default()
    })
    .await
    .expect("session builds");

    let response = session
        .execute(Command::GetAvailableModels { search: None })
        .await;
    assert!(response.success, "{response:?}");
    let data = response.data.expect("models data");
    let models = data["models"].as_array().expect("a models array");
    let providers: BTreeSet<&str> = models
        .iter()
        .filter_map(|model| model["provider"].as_str())
        .collect();
    // Configured in models.json, and authenticated by a stored key.
    assert!(providers.contains("mock"), "{providers:?}");
    assert!(providers.contains("openai"), "{providers:?}");
    // Known to the registry, but nobody is logged in to it.
    assert!(!providers.contains("anthropic"), "{providers:?}");

    // The wire shape of each entry is the registry's model.
    let mock = models
        .iter()
        .find(|model| model["provider"] == "mock")
        .expect("the mock model");
    assert_eq!(mock["id"], "mock-1");
    assert_eq!(mock["name"], "Mock 1");
    assert_eq!(mock["contextWindow"], 128000);
    assert_eq!(mock["maxTokens"], 4096);
    assert_eq!(mock["reasoning"], false);
    assert_eq!(mock["input"], serde_json::json!(["text"]));
    assert!(mock["cost"].is_object(), "{mock}");

    // A search for the unauthenticated provider finds nothing of it.
    let searched = session
        .execute(Command::GetAvailableModels {
            search: Some("anthropic/".into()),
        })
        .await;
    let searched = searched.data.expect("models data");
    assert_eq!(searched["models"], serde_json::json!([]), "{searched}");

    // The search still narrows within the available set.
    let narrowed = session
        .execute(Command::GetAvailableModels {
            search: Some("MOCK/".into()),
        })
        .await;
    let narrowed = narrowed.data.expect("models data");
    assert_eq!(
        narrowed["models"].as_array().unwrap().len(),
        1,
        "{narrowed}"
    );
}
