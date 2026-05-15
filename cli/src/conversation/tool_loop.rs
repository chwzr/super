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
            let res = tool.call(input, &ctx).await;
            (i, id, res)
        });
    }
    let mut safe_results: Vec<(usize, String, ToolResult)> = Vec::with_capacity(set.len());
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(tuple) => safe_results.push(tuple),
            Err(e) => {
                // Panic or cancellation inside the spawned task. Emit a synthetic error.
                safe_results.push((
                    usize::MAX,
                    String::new(),
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
        .filter_map(|(i, id, res)| {
            // Drop the synthetic join-error placeholders that have i == usize::MAX
            // unless that's the only result — in which case still surface the error.
            if i == usize::MAX && id.is_empty() {
                // Emit it as an unattached tool_result with a sentinel id so the
                // model at least sees that something went wrong.
                return Some(ContentBlockFinal::ToolResult {
                    tool_use_id: "join_error".to_string(),
                    content: res.content,
                    is_error: true,
                });
            }
            Some(ContentBlockFinal::ToolResult {
                tool_use_id: id,
                content: res.content,
                is_error: res.is_error,
            })
        })
        .collect()
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
        let tmpfile = std::env::temp_dir().join("super_tool_loop_test.txt");
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
}
