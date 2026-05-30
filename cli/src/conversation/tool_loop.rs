use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::conversation::session_bus::SessionBus;
use crate::executor::interactive::{self, InteractionOutcome};
use crate::sdk::protocol::{BusMessage, ContentBlockFinal};
use crate::state::store::PermissionMode;
use crate::tools::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, RenderOpts, Tool, ToolCallContext, ToolResult,
};
use crate::tools::ToolRegistry;

/// Emit a `BusMessage::RenderEvent` if the given spec is `Some` and not
/// `RenderSpec::Nothing`. Tools whose hook returns `Nothing` mean "no opinion";
/// we don't push empty events onto the bus.
fn emit_render_event(
    bus: &SessionBus,
    tool_use_id: &str,
    parent_tool_use_id: Option<&str>,
    session_id: &str,
    slot: shared::RenderSlot,
    spec: shared::RenderSpec,
) {
    if matches!(spec, shared::RenderSpec::Nothing) {
        return;
    }
    bus.emit(BusMessage::RenderEvent {
        tool_use_id: tool_use_id.into(),
        slot,
        spec,
        parent_tool_use_id: parent_tool_use_id.map(String::from),
        uuid: uuid::Uuid::new_v4(),
        session_id: session_id.into(),
    });
}

/// Execute all tool_use blocks from one assistant turn, returning the
/// corresponding tool_result blocks in emission order. Concurrency-safe tools
/// run in parallel via `tokio::task::JoinSet`; others run sequentially.
#[allow(clippy::too_many_arguments)]
pub async fn run_tool_uses(
    registry: &ToolRegistry,
    tool_uses: Vec<(String, String, serde_json::Value)>, // (id, name, input)
    cwd: std::path::PathBuf,
    permission_mode: PermissionMode,
    abort_signal: Option<watch::Receiver<bool>>,
    bus: Arc<SessionBus>,
    parent_tool_use_id: Option<String>,
    session_id: String,
    auto_deny_prompts: bool,
    queue: Arc<crate::conversation::message_queue::MessageQueue>,
) -> Vec<ContentBlockFinal> {
    // Partition into safe (read-only / pure) and unsafe (writes, shell, network with side effects).
    // Preserve original order index so we can recombine into emission order at the end.
    // Interactive tools always go into the serial (unsafe) path because they must
    // suspend the turn and wait for user input before proceeding.
    let mut safe: Vec<(usize, String, Arc<dyn Tool>, serde_json::Value)> = Vec::new();
    let mut unsafe_: Vec<(usize, String, Arc<dyn Tool>, serde_json::Value)> = Vec::new();
    for (i, (id, name, input)) in tool_uses.into_iter().enumerate() {
        match registry.get(&name) {
            Some(tool) if tool.is_concurrency_safe(&input) && !tool.requires_user_interaction() => {
                safe.push((i, id, tool, input));
            }
            Some(tool) => {
                unsafe_.push((i, id, tool, input));
            }
            None => {
                // Unknown tool — emit error result so the model sees it.
                unsafe_.push((
                    i,
                    id,
                    Arc::new(MissingTool { name }) as Arc<dyn Tool>,
                    input,
                ));
            }
        }
    }

    // Drive the safe set in parallel.
    //
    // Each spawned task captures its own (i, id) so that a panic inside a tool's
    // .call() still produces a ToolResult with the correct tool_use_id — Anthropic
    // rejects a turn if any prior tool_use_id lacks a matching tool_result, so we
    // CANNOT lose the id on the panic path.
    let mut set: JoinSet<(usize, String, ToolResult)> = JoinSet::new();
    for (i, id, tool, input) in safe {
        let ctx = ToolCallContext {
            cwd: cwd.clone(),
            permission_mode,
            abort_signal: abort_signal.clone(),
            parent_tool_use_id: parent_tool_use_id.clone(),
            bus: Some(bus.clone()),
            auto_deny_prompts,
            tool_use_id: id.clone(),
            progress_sink: None,
            queue: Some(queue.clone()),
        };
        let bus_for_task = bus.clone();
        let tool_name = tool.name().to_string();
        let parent_for_tick = parent_tool_use_id.clone();
        let session_for_tick = session_id.clone();
        set.spawn(async move {
            // Emit Message + Tag before the tool runs.
            let opts = RenderOpts { verbose: false, is_transcript_mode: false };
            let message_spec = tool.render_tool_use_message(&input, &opts);
            let tag_spec = tool.render_tool_use_tag(&input);
            emit_render_event(
                &bus_for_task,
                &id,
                parent_for_tick.as_deref(),
                &session_for_tick,
                shared::RenderSlot::Message,
                message_spec,
            );
            if let Some(tag) = tag_spec {
                emit_render_event(
                    &bus_for_task,
                    &id,
                    parent_for_tick.as_deref(),
                    &session_for_tick,
                    shared::RenderSlot::Tag,
                    tag,
                );
            }

            // 1Hz ticker emits BusMessage::ToolProgress while the tool runs.
            // Aborted as soon as the inner call returns so the activity row
            // can flip back to idle (or to the next tool) immediately.
            let bus_for_tick = bus_for_task.clone();
            let id_for_tick = id.clone();
            let name_for_tick = tool_name.clone();
            let parent_for_ticker = parent_for_tick.clone();
            let session_for_ticker = session_for_tick.clone();
            let ticker = tokio::spawn(async move {
                let start = std::time::Instant::now();
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
                interval.tick().await; // first tick fires immediately; skip it
                loop {
                    interval.tick().await;
                    bus_for_tick.emit(BusMessage::ToolProgress {
                        tool_use_id: id_for_tick.clone(),
                        tool_name: name_for_tick.clone(),
                        elapsed_seconds: start.elapsed().as_secs_f32(),
                        parent_tool_use_id: parent_for_ticker.clone(),
                        uuid: uuid::Uuid::new_v4(),
                        session_id: session_for_ticker.clone(),
                    });
                }
            });

            // Catch panics from inside the tool so the parent loop can still
            // emit a ToolResult tagged with the original tool_use_id. We do
            // this by re-spawning the call as its own task: tokio's JoinSet
            // already catches panics on the outer task and turns them into a
            // JoinError, but a JoinError doesn't carry our (i, id) tuple.
            // Spawning *inside* the outer task lets us recover the panic
            // payload here and rebuild a ToolResult tagged with the original
            // tool_use_id.
            let tool_for_call = tool.clone();
            let inner = tokio::task::spawn(async move { tool_for_call.call(input, &ctx, None).await });
            let res = match inner.await {
                Ok(r) => r,
                Err(e) if e.is_panic() => ToolResult {
                    content: format!("Tool panicked: {}", downcast_panic(&e.into_panic())),
                    is_error: true,
                    ..Default::default()
                },
                Err(e) => ToolResult {
                    content: format!("Tool task error: {e}"),
                    is_error: true,
                    ..Default::default()
                },
            };

            // Emit Result or Error after the tool completes.
            let output_json = serde_json::json!({
                "content": res.content,
                "is_error": res.is_error,
            });
            let slot = if res.is_error {
                shared::RenderSlot::Error
            } else {
                shared::RenderSlot::Result
            };
            let result_spec = if res.is_error {
                tool.render_tool_use_error_message(&output_json, &opts)
                    .unwrap_or(shared::RenderSpec::Nothing)
            } else {
                tool.render_tool_result_message(&output_json, &[], &opts)
                    .unwrap_or(shared::RenderSpec::Nothing)
            };
            emit_render_event(
                &bus_for_task,
                &id,
                parent_for_tick.as_deref(),
                &session_for_tick,
                slot,
                result_spec,
            );

            ticker.abort();
            (i, id, res)
        });
    }
    let mut safe_results: Vec<(usize, String, ToolResult)> = Vec::with_capacity(set.len());
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(tuple) => safe_results.push(tuple),
            Err(e) => {
                // The task was either cancelled or hit an unexpected join error
                // *after* our inner catch_unwind. We cannot recover the (i, id)
                // here, but in practice this path only fires on cancellation /
                // runtime shutdown — at which point the surrounding engine loop
                // is also tearing down. Surface a synthetic placeholder so the
                // engine sees something rather than silently dropping work.
                safe_results.push((
                    usize::MAX,
                    "__join_error__".to_string(),
                    ToolResult {
                        content: format!("Tool task join error: {e}"),
                        is_error: true,
                        ..Default::default()
                    },
                ));
            }
        }
    }

    // Drive the unsafe set serially.
    let mut unsafe_results: Vec<(usize, String, ToolResult)> = Vec::with_capacity(unsafe_.len());
    for (i, id, tool, input) in unsafe_ {
        let ctx = ToolCallContext {
            cwd: cwd.clone(),
            permission_mode,
            abort_signal: abort_signal.clone(),
            parent_tool_use_id: parent_tool_use_id.clone(),
            bus: Some(bus.clone()),
            auto_deny_prompts,
            tool_use_id: id.clone(),
            progress_sink: None,
            queue: Some(queue.clone()),
        };

        let unsafe_opts = RenderOpts { verbose: false, is_transcript_mode: false };

        // Interactive tools: render the spec, suspend the turn, await the
        // user response, then merge the answers into the tool input.
        let effective_input = if tool.requires_user_interaction() {
            let spec = tool.render_tool_use_message(&input, &unsafe_opts);

            if matches!(&spec, shared::RenderSpec::Interactive { .. }) {
                // Emit the spec via RenderEvent so the transcript can
                // optionally render it in scrollback (the modal is the
                // primary surface, but scrollback parity is useful).
                emit_render_event(
                    &bus,
                    &id,
                    parent_tool_use_id.as_deref(),
                    &session_id,
                    shared::RenderSlot::Message,
                    spec.clone(),
                );

                match interactive::await_interaction(
                    id.clone(),
                    &spec,
                    &bus,
                    parent_tool_use_id.clone(),
                )
                .await
                {
                    InteractionOutcome::Resolved { updated_input } => {
                        // Merge answers into the original input so call()
                        // sees the full picture.
                        updated_input
                    }
                    InteractionOutcome::Denied => {
                        if let Some(rejection) = tool.render_tool_use_rejected_message(
                            &input,
                            &unsafe_opts,
                        ) {
                            // Emit the rejection so the transcript shows it.
                            emit_render_event(
                                &bus,
                                &id,
                                parent_tool_use_id.as_deref(),
                                &session_id,
                                shared::RenderSlot::Rejected,
                                rejection,
                            );
                        }
                        unsafe_results.push((
                            i,
                            id,
                            ToolResult {
                                content: "User declined to answer questions".into(),
                                is_error: true,
                                ..Default::default()
                            },
                        ));
                        continue;
                    }
                    InteractionOutcome::Aborted => {
                        unsafe_results.push((
                            i,
                            id,
                            ToolResult {
                                content: "Interaction aborted".into(),
                                is_error: true,
                                ..Default::default()
                            },
                        ));
                        continue;
                    }
                }
            } else {
                input
            }
        } else {
            input
        };

        // Emit Message + Tag before the tool runs.
        let message_spec = tool.render_tool_use_message(&effective_input, &unsafe_opts);
        let tag_spec = tool.render_tool_use_tag(&effective_input);
        emit_render_event(
            &bus,
            &id,
            parent_tool_use_id.as_deref(),
            &session_id,
            shared::RenderSlot::Message,
            message_spec,
        );
        if let Some(tag) = tag_spec {
            emit_render_event(
                &bus,
                &id,
                parent_tool_use_id.as_deref(),
                &session_id,
                shared::RenderSlot::Tag,
                tag,
            );
        }

        // 1Hz ticker emits BusMessage::ToolProgress while the tool runs.
        let bus_for_tick = bus.clone();
        let id_for_tick = id.clone();
        let name_for_tick = tool.name().to_string();
        let parent_for_tick = parent_tool_use_id.clone();
        let session_for_tick = session_id.clone();
        let ticker = tokio::spawn(async move {
            let start = std::time::Instant::now();
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            interval.tick().await; // first tick fires immediately; skip it
            loop {
                interval.tick().await;
                bus_for_tick.emit(BusMessage::ToolProgress {
                    tool_use_id: id_for_tick.clone(),
                    tool_name: name_for_tick.clone(),
                    elapsed_seconds: start.elapsed().as_secs_f32(),
                    parent_tool_use_id: parent_for_tick.clone(),
                    uuid: uuid::Uuid::new_v4(),
                    session_id: session_for_tick.clone(),
                });
            }
        });

        let res = tool.call(effective_input, &ctx, None).await;
        ticker.abort();

        // Emit Result or Error after the tool completes.
        let output_json = serde_json::json!({
            "content": res.content,
            "is_error": res.is_error,
        });
        let slot = if res.is_error {
            shared::RenderSlot::Error
        } else {
            shared::RenderSlot::Result
        };
        let result_spec = if res.is_error {
            tool.render_tool_use_error_message(&output_json, &unsafe_opts)
                .unwrap_or(shared::RenderSpec::Nothing)
        } else {
            tool.render_tool_result_message(&output_json, &[], &unsafe_opts)
                .unwrap_or(shared::RenderSpec::Nothing)
        };
        emit_render_event(
            &bus,
            &id,
            parent_tool_use_id.as_deref(),
            &session_id,
            slot,
            result_spec,
        );

        unsafe_results.push((i, id, res));
    }

    // Merge and sort by original index so the returned vec preserves model emission order.
    let mut combined: Vec<(usize, String, ToolResult)> =
        safe_results.into_iter().chain(unsafe_results).collect();
    combined.sort_by_key(|(i, _, _)| *i);

    let mut blocks: Vec<ContentBlockFinal> = Vec::with_capacity(combined.len());
    for (_i, id, res) in combined {
        blocks.push(ContentBlockFinal::ToolResult {
            tool_use_id: id,
            content: res.content,
            is_error: res.is_error,
        });
        for msg in res.inject_messages {
            blocks.push(ContentBlockFinal::Text { text: msg });
        }
    }
    blocks
}

fn downcast_panic(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "(non-string panic payload)".to_string()
}

/// Sentinel for unknown tool names — produces an error tool_result so the
/// model sees the failure on the next round-trip and can recover.
struct MissingTool {
    name: String,
}

#[async_trait::async_trait]
impl Tool for MissingTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "missing".into()
    }
    fn prompt(&self, _ctx: &PromptCtx) -> String {
        String::new()
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    async fn call(
        &self,
        _input: serde_json::Value,
        _ctx: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        ToolResult {
            content: format!("Unknown tool: {}", self.name),
            is_error: true,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod render_emission_tests {
    use super::*;
    use crate::conversation::session_bus::SessionBus;
    use crate::state::store::Store;
    use shared::CliConfig;

    fn make_test_registry(store: Arc<Store>) -> Arc<ToolRegistry> {
        let queue = Arc::new(crate::conversation::message_queue::MessageQueue::new());
        let jobs = Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            String,
            crate::conversation::cron_runtime::CronJob,
        >::new()));
        let (tx, _rx) = tokio::sync::watch::channel(false);
        ToolRegistry::new(
            store,
            CliConfig::default(),
            Arc::new(crate::agents::AgentRegistry::built_in_only()),
            queue,
            jobs,
            tx,
        )
    }

    #[tokio::test]
    async fn safe_loop_emits_message_and_result_for_simple_tool() {
        let store = Arc::new(Store::new());
        let registry = make_test_registry(store);
        let bus = Arc::new(SessionBus::new("s".into()));
        let queue = Arc::new(crate::conversation::message_queue::MessageQueue::new());

        // Subscribe before the run so we capture all events.
        let mut rx = bus.subscribe();

        let _ = run_tool_uses(
            &registry,
            vec![(
                "tu_test".into(),
                "Bash".into(),
                serde_json::json!({"command": "echo hi"}),
            )],
            std::env::temp_dir(),
            PermissionMode::BypassPermissions,
            None,
            bus.clone(),
            None,
            "s".into(),
            true,
            queue,
        )
        .await;

        // Drain all messages that were broadcast.
        let mut messages = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            messages.push(msg);
        }

        let slots: Vec<shared::RenderSlot> = messages
            .iter()
            .filter_map(|m| match m {
                BusMessage::RenderEvent { slot, .. } => Some(*slot),
                _ => None,
            })
            .collect();
        assert!(slots.contains(&shared::RenderSlot::Message), "expected Message slot, got slots: {slots:?}");
        assert!(slots.contains(&shared::RenderSlot::Result), "expected Result slot, got slots: {slots:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::Store;
    use shared::CliConfig;

    fn make_test_registry(store: Arc<Store>) -> Arc<ToolRegistry> {
        let queue = Arc::new(crate::conversation::message_queue::MessageQueue::new());
        let jobs = Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            String,
            crate::conversation::cron_runtime::CronJob,
        >::new()));
        let (tx, _rx) = tokio::sync::watch::channel(false);
        ToolRegistry::new(
            store,
            CliConfig::default(),
            Arc::new(crate::agents::AgentRegistry::built_in_only()),
            queue,
            jobs,
            tx,
        )
    }

    #[tokio::test]
    async fn unknown_tool_produces_error_result() {
        let store = Arc::new(Store::new());
        let registry = make_test_registry(store);
        let bus = Arc::new(SessionBus::new("test".into()));
        let queue = Arc::new(crate::conversation::message_queue::MessageQueue::new());
        let results = run_tool_uses(
            &registry,
            vec![("tu_1".into(), "DoesNotExist".into(), serde_json::json!({}))],
            std::env::current_dir().unwrap(),
            PermissionMode::Default,
            None,
            bus,
            None,                  // parent_tool_use_id
            "test-session".into(), // session_id
            false,                 // auto_deny_prompts
            queue,
        )
        .await;
        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlockFinal::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tu_1");
                assert!(*is_error);
                assert!(content.contains("Unknown tool"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_tool_executes_against_real_file() {
        let store = Arc::new(Store::new());
        let registry = make_test_registry(store);
        let tmpfile =
            std::env::temp_dir().join(format!("super_tool_loop_test_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&tmpfile, "hello\nworld\n").unwrap();

        let bus = Arc::new(SessionBus::new("test".into()));
        let queue = Arc::new(crate::conversation::message_queue::MessageQueue::new());
        let results = run_tool_uses(
            &registry,
            vec![(
                "tu_2".into(),
                "Read".into(),
                serde_json::json!({"file_path": tmpfile.to_string_lossy()}),
            )],
            std::env::current_dir().unwrap(),
            PermissionMode::Default,
            None,
            bus,
            None,                  // parent_tool_use_id
            "test-session".into(), // session_id
            false,                 // auto_deny_prompts
            queue,
        )
        .await;
        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlockFinal::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tu_2");
                assert!(!*is_error, "got error: {content}");
                assert!(content.contains("hello"));
                assert!(content.contains("world"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
        std::fs::remove_file(&tmpfile).ok();
    }

    #[tokio::test]
    async fn mixed_safe_and_sequential_preserve_emission_order() {
        // Read is concurrency-safe; Write is not. Verify both produce results
        // tagged with the right tool_use_id in original emission order.
        let store = Arc::new(Store::new());
        let registry = make_test_registry(store);
        let tmp_read = std::env::temp_dir().join(format!(
            "super_tool_loop_mix_read_{}.txt",
            uuid::Uuid::new_v4()
        ));
        let tmp_write = std::env::temp_dir().join(format!(
            "super_tool_loop_mix_write_{}.txt",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&tmp_read, "read-me").unwrap();

        let bus = Arc::new(SessionBus::new("test".into()));
        let queue = Arc::new(crate::conversation::message_queue::MessageQueue::new());
        let results = run_tool_uses(
            &registry,
            vec![
                ("tu_a".into(), "Read".into(), serde_json::json!({"file_path": tmp_read.to_string_lossy()})),
                ("tu_b".into(), "Write".into(), serde_json::json!({"file_path": tmp_write.to_string_lossy(), "content": "wrote-me"})),
                ("tu_c".into(), "Read".into(), serde_json::json!({"file_path": tmp_read.to_string_lossy()})),
            ],
            std::env::current_dir().unwrap(),
            PermissionMode::Default,
            None,
            bus,
            None,                  // parent_tool_use_id
            "test-session".into(), // session_id
            false,                 // auto_deny_prompts
            queue,
        )
        .await;

        assert_eq!(results.len(), 3);
        // Order must match input order regardless of which subset ran in parallel.
        let ids: Vec<&str> = results
            .iter()
            .filter_map(|b| match b {
                ContentBlockFinal::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["tu_a", "tu_b", "tu_c"]);

        std::fs::remove_file(&tmp_read).ok();
        std::fs::remove_file(&tmp_write).ok();
    }

    #[tokio::test]
    async fn run_tool_uses_passes_parent_tool_use_id_to_context() {
        use crate::tools::contract::{Tool, ToolCallContext, ToolResult};
        use std::sync::{Arc, Mutex};

        struct CaptureTool {
            seen_parent: Arc<Mutex<Option<String>>>,
        }
        #[async_trait::async_trait]
        impl Tool for CaptureTool {
            fn name(&self) -> &str {
                "Capture"
            }
            fn description(
                &self,
                _input: Option<&serde_json::Value>,
                _ctx: &DescriptionCtx,
            ) -> String {
                "capture".into()
            }
            fn prompt(&self, _ctx: &PromptCtx) -> String {
                String::new()
            }
            fn input_schema(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            async fn call(
                &self,
                _input: serde_json::Value,
                ctx: &ToolCallContext,
                _on_progress: Option<ProgressSink>,
            ) -> ToolResult {
                *self.seen_parent.lock().unwrap() = ctx.parent_tool_use_id.clone();
                ToolResult {
                    content: "ok".into(),
                    is_error: false,
                    ..Default::default()
                }
            }
        }

        let seen = Arc::new(Mutex::new(None));
        let store = Arc::new(Store::new());
        let registry = make_test_registry(store);
        registry.register(Arc::new(CaptureTool {
            seen_parent: seen.clone(),
        }));

        let bus = Arc::new(SessionBus::new("s-root".into()));
        let queue = Arc::new(crate::conversation::message_queue::MessageQueue::new());
        let _ = run_tool_uses(
            &registry,
            vec![("tu_x".into(), "Capture".into(), serde_json::json!({}))],
            std::env::current_dir().unwrap(),
            PermissionMode::Default,
            None,
            bus,
            Some("tu_parent".into()),
            "agent-1".into(),
            false,
            queue,
        )
        .await;

        assert_eq!(seen.lock().unwrap().clone().as_deref(), Some("tu_parent"));
    }

    #[tokio::test]
    async fn run_tool_uses_passes_current_tool_use_id_to_context() {
        use crate::tools::contract::{Tool, ToolCallContext, ToolResult};
        use std::sync::{Arc, Mutex};

        struct CaptureTool {
            seen: Arc<Mutex<Option<String>>>,
        }
        #[async_trait::async_trait]
        impl Tool for CaptureTool {
            fn name(&self) -> &str {
                "Capture2"
            }
            fn description(
                &self,
                _input: Option<&serde_json::Value>,
                _ctx: &DescriptionCtx,
            ) -> String {
                "capture".into()
            }
            fn prompt(&self, _ctx: &PromptCtx) -> String {
                String::new()
            }
            fn input_schema(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            async fn call(
                &self,
                _input: serde_json::Value,
                ctx: &ToolCallContext,
                _on_progress: Option<ProgressSink>,
            ) -> ToolResult {
                *self.seen.lock().unwrap() = Some(ctx.tool_use_id.clone());
                ToolResult {
                    content: "ok".into(),
                    is_error: false,
                    ..Default::default()
                }
            }
        }

        let seen = Arc::new(Mutex::new(None));
        let store = Arc::new(Store::new());
        let registry = make_test_registry(store);
        registry.register(Arc::new(CaptureTool { seen: seen.clone() }));

        let bus = Arc::new(SessionBus::new("s-root".into()));
        let queue = Arc::new(crate::conversation::message_queue::MessageQueue::new());
        let _ = run_tool_uses(
            &registry,
            vec![("tu_actual".into(), "Capture2".into(), serde_json::json!({}))],
            std::env::current_dir().unwrap(),
            PermissionMode::Default,
            None,
            bus,
            None,
            "test-session".into(),
            false,
            queue,
        )
        .await;

        assert_eq!(seen.lock().unwrap().clone().as_deref(), Some("tu_actual"));
    }

    /// A panicking tool MUST still produce a ToolResult with the correct
    /// tool_use_id. Anthropic rejects a turn if any prior tool_use_id lacks
    /// a matching tool_result on the next user turn, so dropping the id on
    /// panic is a Blocker.
    #[tokio::test]
    async fn panicking_tool_produces_error_result_with_correct_id() {
        use crate::tools::contract::{Tool, ToolCallContext, ToolResult};
        struct PanickingTool;
        #[async_trait::async_trait]
        impl Tool for PanickingTool {
            fn name(&self) -> &str {
                "Panicker"
            }
            fn description(
                &self,
                _input: Option<&serde_json::Value>,
                _ctx: &DescriptionCtx,
            ) -> String {
                "always panics".into()
            }
            fn prompt(&self, _ctx: &PromptCtx) -> String {
                String::new()
            }
            fn input_schema(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
                true
            } // must go through the JoinSet path
            async fn call(
                &self,
                _input: serde_json::Value,
                _ctx: &ToolCallContext,
                _on_progress: Option<ProgressSink>,
            ) -> ToolResult {
                panic!("boom");
            }
        }

        let store = Arc::new(Store::new());
        let registry = make_test_registry(store);
        registry.register(Arc::new(PanickingTool));

        let bus = Arc::new(SessionBus::new("test".into()));
        let queue = Arc::new(crate::conversation::message_queue::MessageQueue::new());
        let results = run_tool_uses(
            &registry,
            vec![("tu_panic".into(), "Panicker".into(), serde_json::json!({}))],
            std::env::current_dir().unwrap(),
            PermissionMode::Default,
            None,
            bus,
            None,                  // parent_tool_use_id
            "test-session".into(), // session_id
            false,                 // auto_deny_prompts
            queue,
        )
        .await;

        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlockFinal::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(
                    tool_use_id, "tu_panic",
                    "panicked tool_use_id MUST survive panic"
                );
                assert!(*is_error);
                assert!(content.contains("panic") || content.contains("boom"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn inject_messages_become_text_blocks() {
        use crate::sdk::protocol::ContentBlockFinal;
        use crate::tools::contract::ToolResult;

        // Simulate what run_tool_uses produces for one ToolResult with inject_messages.
        let results: Vec<(usize, String, ToolResult)> = vec![(
            0,
            "tu_1".to_string(),
            ToolResult {
                content: "Launching skill: foo".into(),
                is_error: false,
                inject_messages: vec!["# Foo Skill\n\nDo the thing.".into()],
                ..Default::default()
            },
        )];

        let mut blocks: Vec<ContentBlockFinal> = Vec::new();
        for (_i, id, res) in results {
            blocks.push(ContentBlockFinal::ToolResult {
                tool_use_id: id,
                content: res.content,
                is_error: res.is_error,
            });
            for msg in res.inject_messages {
                blocks.push(ContentBlockFinal::Text { text: msg });
            }
        }

        assert_eq!(blocks.len(), 2);
        assert!(
            matches!(&blocks[0], ContentBlockFinal::ToolResult { content, .. } if content == "Launching skill: foo")
        );
        assert!(
            matches!(&blocks[1], ContentBlockFinal::Text { text } if text.contains("# Foo Skill"))
        );
    }
}
