use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::conversation::session_bus::SessionBus;

/// Spawn a background task that subscribes to the bus and persists every
/// message with `parent_tool_use_id.is_some()` into a per-agent JSONL file
/// under `base_dir`. Best-effort: I/O failures are logged and the writer
/// continues running.
///
/// `base_dir` should be `~/.super/sessions/<root_session>/sidechains/`.
pub fn spawn_sidechain_writer(bus: Arc<SessionBus>, base_dir: PathBuf) {
    let mut rx = bus.subscribe();
    if let Err(e) = std::fs::create_dir_all(&base_dir) {
        tracing::warn!("sidechain: cannot create {base_dir:?}: {e}");
        return;
    }
    tokio::spawn(async move {
        let mut writers: HashMap<String, BufWriter<File>> = HashMap::new();
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if msg.parent_tool_use_id().is_none() {
                        continue;
                    }
                    let agent_id = msg.session_id().to_string();
                    let writer = match writers.get_mut(&agent_id) {
                        Some(w) => w,
                        None => {
                            let path = base_dir.join(format!("{agent_id}.jsonl"));
                            match OpenOptions::new().create(true).append(true).open(&path) {
                                Ok(f) => {
                                    writers.insert(agent_id.clone(), BufWriter::new(f));
                                    writers.get_mut(&agent_id).unwrap()
                                }
                                Err(e) => {
                                    tracing::warn!("sidechain: cannot open {:?}: {e}", path);
                                    continue;
                                }
                            }
                        }
                    };
                    match serde_json::to_string(&msg) {
                        Ok(line) => {
                            let _ = writer.write_all(line.as_bytes());
                            let _ = writer.write_all(b"\n");
                            let _ = writer.flush();
                        }
                        Err(e) => tracing::warn!("sidechain: serialize failed: {e}"),
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Conventional base path for sidechains.
pub fn default_sidechain_dir(root_session_id: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".super")
        .join("sessions")
        .join(root_session_id)
        .join("sidechains")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::protocol::{
        AnthropicUsage, AssistantPayload, BusMessage, ContentBlockFinal, SystemSubtype,
    };
    use uuid::Uuid;

    #[tokio::test]
    async fn sidechain_writes_one_file_per_agent_id() {
        let tmp = tempfile_dir();
        let bus = Arc::new(SessionBus::new("s-root".into()));
        spawn_sidechain_writer(bus.clone(), tmp.clone());

        // Emit parent event (no parent_tool_use_id) — should NOT be written
        bus.emit(BusMessage::Assistant {
            message: AssistantPayload {
                id: "msg_root".into(),
                model: "x".into(),
                role: "assistant".into(),
                content: vec![ContentBlockFinal::Text {
                    text: "root".into(),
                }],
                stop_reason: None,
                usage: AnthropicUsage::default(),
            },
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s-root".into(),
        });

        // Emit child event — SHOULD be written under agent-1.jsonl
        bus.emit(BusMessage::SystemEvent {
            subtype: SystemSubtype::Notice,
            message: "child note".into(),
            parent_tool_use_id: Some("tu_1".into()),
            uuid: Uuid::new_v4(),
            session_id: "agent-1".into(),
        });

        // Let the writer drain
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let path = tmp.join("agent-1.jsonl");
        assert!(path.exists(), "expected sidechain at {:?}", path);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("child note"));
        assert!(
            !contents.contains("\"msg_root\""),
            "root event must not appear"
        );
    }

    #[tokio::test]
    async fn sidechain_captures_child_result_event() {
        let tmp = tempfile_dir();
        let bus = Arc::new(SessionBus::new("s-root".into()));
        spawn_sidechain_writer(bus.clone(), tmp.clone());

        bus.emit(BusMessage::Result {
            stop_reason: Some("end_turn".into()),
            usage: AnthropicUsage::default(),
            total_cost_usd: 0.01,
            duration_ms: 1234,
            num_turns: 3,
            parent_tool_use_id: Some("tu_parent".into()),
            uuid: uuid::Uuid::new_v4(),
            session_id: "agent-1".into(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let path = tmp.join("agent-1.jsonl");
        assert!(path.exists(), "expected sidechain at {:?}", path);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("\"type\":\"result\""), "got: {contents}");
        assert!(contents.contains("\"num_turns\":3"), "got: {contents}");
    }

    fn tempfile_dir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("super-sidechain-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
