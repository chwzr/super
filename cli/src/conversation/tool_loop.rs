use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::sdk::protocol::ContentBlockFinal;
use crate::state::store::PermissionMode;
use crate::tools::contract::{Tool, ToolCallContext, ToolResult};
use crate::tools::ToolRegistry;

/// Execute all tool_use blocks from one assistant turn, returning the
/// corresponding tool_result blocks in emission order. Concurrency-safe tools
/// run in parallel via `tokio::task::JoinSet`; others run sequentially.
pub async fn run_tool_uses(
    registry: &ToolRegistry,
    tool_uses: Vec<(String, String, serde_json::Value)>, // (id, name, input)
    cwd: std::path::PathBuf,
    permission_mode: PermissionMode,
    abort_signal: Option<watch::Receiver<bool>>,
) -> Vec<ContentBlockFinal> {
    // Partition into safe (read-only / pure) and unsafe (writes, shell, network with side effects).
    // Preserve original order index so we can recombine into emission order at the end.
    let mut safe: Vec<(usize, String, Arc<dyn Tool>, serde_json::Value)> = Vec::new();
    let mut unsafe_: Vec<(usize, String, Arc<dyn Tool>, serde_json::Value)> = Vec::new();
    for (i, (id, name, input)) in tool_uses.into_iter().enumerate() {
        match registry.get(&name) {
            Some(tool) if tool.is_concurrency_safe() => {
                safe.push((i, id, tool, input));
            }
            Some(tool) => {
                unsafe_.push((i, id, tool, input));
            }
            None => {
                // Unknown tool — emit error result so the model sees it.
                unsafe_.push((i, id, Arc::new(MissingTool { name }) as Arc<dyn Tool>, input));
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
            permission_mode: permission_mode.clone(),
            abort_signal: abort_signal.clone(),
            parent_tool_use_id: None,
            bus: None,
        };
        set.spawn(async move {
            // Catch panics from inside the tool so the parent loop can still
            // emit a ToolResult tagged with the original tool_use_id. We do
            // this by re-spawning the call as its own task: tokio's JoinSet
            // already catches panics on the outer task and turns them into a
            // JoinError, but a JoinError doesn't carry our (i, id) tuple.
            // Spawning *inside* the outer task lets us recover the panic
            // payload here and rebuild a ToolResult tagged with the original
            // tool_use_id.
            let inner = tokio::task::spawn(async move {
                tool.call(input, &ctx).await
            });
            let res = match inner.await {
                Ok(r) => r,
                Err(e) if e.is_panic() => ToolResult {
                    content: format!(
                        "Tool panicked: {}",
                        downcast_panic(&e.into_panic())
                    ),
                    is_error: true,
                    metadata: None,
                },
                Err(e) => ToolResult {
                    content: format!("Tool task error: {e}"),
                    is_error: true,
                    metadata: None,
                },
            };
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
                        metadata: None,
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
            permission_mode: permission_mode.clone(),
            abort_signal: abort_signal.clone(),
            parent_tool_use_id: None,
            bus: None,
        };
        let res = tool.call(input, &ctx).await;
        unsafe_results.push((i, id, res));
    }

    // Merge and sort by original index so the returned vec preserves model emission order.
    let mut combined: Vec<(usize, String, ToolResult)> =
        safe_results.into_iter().chain(unsafe_results.into_iter()).collect();
    combined.sort_by_key(|(i, _, _)| *i);

    combined
        .into_iter()
        .map(|(_i, id, res)| ContentBlockFinal::ToolResult {
            tool_use_id: id,
            content: res.content,
            is_error: res.is_error,
        })
        .collect()
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
    fn description(&self) -> &str {
        "missing"
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    async fn call(&self, _input: serde_json::Value, _ctx: &ToolCallContext) -> ToolResult {
        ToolResult {
            content: format!("Unknown tool: {}", self.name),
            is_error: true,
            metadata: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::Store;
    use shared::CliConfig;

    #[tokio::test]
    async fn unknown_tool_produces_error_result() {
        let store = Arc::new(Store::new());
        let registry = ToolRegistry::new(store, CliConfig::default());
        let results = run_tool_uses(
            &registry,
            vec![("tu_1".into(), "DoesNotExist".into(), serde_json::json!({}))],
            std::env::current_dir().unwrap(),
            PermissionMode::Default,
            None,
        )
        .await;
        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlockFinal::ToolResult { tool_use_id, content, is_error } => {
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
        let registry = ToolRegistry::new(store, CliConfig::default());
        let tmpfile = std::env::temp_dir().join(format!(
            "super_tool_loop_test_{}.txt",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&tmpfile, "hello\nworld\n").unwrap();

        let results = run_tool_uses(
            &registry,
            vec![("tu_2".into(), "Read".into(), serde_json::json!({"file_path": tmpfile.to_string_lossy()}))],
            std::env::current_dir().unwrap(),
            PermissionMode::Default,
            None,
        )
        .await;
        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlockFinal::ToolResult { tool_use_id, content, is_error } => {
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
        let registry = ToolRegistry::new(store, CliConfig::default());
        let tmp_read = std::env::temp_dir().join(format!(
            "super_tool_loop_mix_read_{}.txt",
            uuid::Uuid::new_v4()
        ));
        let tmp_write = std::env::temp_dir().join(format!(
            "super_tool_loop_mix_write_{}.txt",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&tmp_read, "read-me").unwrap();

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
        )
        .await;

        assert_eq!(results.len(), 3);
        // Order must match input order regardless of which subset ran in parallel.
        let ids: Vec<&str> = results.iter().filter_map(|b| match b {
            ContentBlockFinal::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        }).collect();
        assert_eq!(ids, vec!["tu_a", "tu_b", "tu_c"]);

        std::fs::remove_file(&tmp_read).ok();
        std::fs::remove_file(&tmp_write).ok();
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
            fn name(&self) -> &str { "Panicker" }
            fn description(&self) -> &str { "always panics" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn is_concurrency_safe(&self) -> bool { true }   // must go through the JoinSet path
            async fn call(&self, _input: serde_json::Value, _ctx: &ToolCallContext) -> ToolResult {
                panic!("boom");
            }
        }

        let store = Arc::new(Store::new());
        let registry = ToolRegistry::new(store, CliConfig::default());
        registry.register(Arc::new(PanickingTool));

        let results = run_tool_uses(
            &registry,
            vec![("tu_panic".into(), "Panicker".into(), serde_json::json!({}))],
            std::env::current_dir().unwrap(),
            PermissionMode::Default,
            None,
        )
        .await;

        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlockFinal::ToolResult { tool_use_id, content, is_error } => {
                assert_eq!(tool_use_id, "tu_panic", "panicked tool_use_id MUST survive panic");
                assert!(*is_error);
                assert!(content.contains("panic") || content.contains("boom"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
