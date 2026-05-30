#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable
)]

//! End-to-end smoke test for the Batch 1 trait/permission/renderspec migration.
//!
//! Verifies that:
//!   1. A registered tool can be looked up and called through the new contract.
//!   2. The permission system returns PermissionResult::Allow for a default-mode read.
//!   3. The session bus accepts a RenderEvent carrying RenderSpec::Nothing.

use std::sync::Arc;

use shared::RenderSpec;
use super_cli::conversation::cron_runtime::CronJob;
use super_cli::conversation::message_queue::MessageQueue;
use super_cli::conversation::session_bus::SessionBus;
use super_cli::sdk::protocol::BusMessage;
use super_cli::state::store::{PermissionMode, Store};
use super_cli::tools::contract::{DescriptionCtx, PromptCtx, RenderOpts, ToolCallContext};
use super_cli::tools::permission::{PermissionResult, PermissionSystem};
use super_cli::tools::ToolRegistry;

fn make_test_registry(store: Arc<Store>) -> Arc<ToolRegistry> {
    let queue = Arc::new(MessageQueue::new());
    let jobs = Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
        String,
        CronJob,
    >::new()));
    let (tx, _rx) = tokio::sync::watch::channel(false);
    ToolRegistry::new(
        store,
        shared::CliConfig::default(),
        Arc::new(super_cli::agents::AgentRegistry::built_in_only()),
        queue,
        jobs,
        tx,
    )
}

#[tokio::test]
async fn read_tool_round_trips_through_new_contract() {
    let store = Arc::new(Store::new());
    let registry = make_test_registry(store.clone());

    let read = registry.get("Read").expect("Read tool registered");
    let desc = read.description(None, &DescriptionCtx::default());
    assert!(desc.contains("Reads"));

    let prompt = read.prompt(&PromptCtx::default());
    // Placeholder marker until Batch 3 ports the real prompt text.
    assert!(prompt.contains("Reads a file from the local filesystem"));

    assert!(read.is_read_only(&serde_json::Value::Null));
    assert!(read.is_concurrency_safe(&serde_json::Value::Null));
    assert!(matches!(
        read.render_tool_use_message(&serde_json::Value::Null, &RenderOpts::default()),
        RenderSpec::Nothing
    ));
}

#[tokio::test]
async fn default_mode_permission_check_returns_allow() {
    let store = Arc::new(Store::new());
    let registry = make_test_registry(store.clone());
    let read = registry.get("Read").unwrap();
    let sys = PermissionSystem::new(PermissionMode::Default);
    let ctx = ToolCallContext {
        cwd: std::env::current_dir().unwrap(),
        permission_mode: PermissionMode::Default,
        abort_signal: None,
        parent_tool_use_id: None,
        bus: None,
        auto_deny_prompts: false,
        tool_use_id: "tu_smoke".into(),
        progress_sink: None,
        queue: None,
    };
    let result = sys
        .evaluate(
            read.as_ref(),
            &serde_json::json!({"file_path":"/tmp/x"}),
            &ctx,
        )
        .await;
    assert!(matches!(result, PermissionResult::Allow { .. }));
}

#[tokio::test]
async fn bus_round_trips_render_event() {
    let bus = SessionBus::new("smoke".into());
    let mut rx = bus.subscribe();
    bus.emit(BusMessage::RenderEvent {
        tool_use_id: "tu_1".into(),
        slot: shared::RenderSlot::Message,
        spec: RenderSpec::Nothing,
        parent_tool_use_id: None,
        uuid: uuid::Uuid::new_v4(),
        session_id: "smoke".into(),
    });
    let received = rx.recv().await.unwrap();
    assert!(matches!(received, BusMessage::RenderEvent { .. }));
}

#[tokio::test]
async fn bypass_mode_short_circuits_to_allow() {
    let store = Arc::new(Store::new());
    let registry = make_test_registry(store.clone());
    let bash = registry.get("Bash").unwrap();
    let sys = PermissionSystem::new(PermissionMode::BypassPermissions);
    let ctx = ToolCallContext {
        cwd: std::env::current_dir().unwrap(),
        permission_mode: PermissionMode::BypassPermissions,
        abort_signal: None,
        parent_tool_use_id: None,
        bus: None,
        auto_deny_prompts: false,
        tool_use_id: "tu_smoke".into(),
        progress_sink: None,
        queue: None,
    };
    let result = sys
        .evaluate(
            bash.as_ref(),
            &serde_json::json!({"command":"echo hi"}),
            &ctx,
        )
        .await;
    match result {
        PermissionResult::Allow {
            decision_reason, ..
        } => {
            assert!(decision_reason.is_some());
        }
        other => panic!("expected Allow, got {:?}", other),
    }
}
