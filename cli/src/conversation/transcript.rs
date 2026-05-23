use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::conversation::anthropic::{HistoryEntry, Role};
use crate::conversation::session_bus::SessionBus;
use crate::sdk::protocol::BusMessage;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Metadata entries embedded in the transcript JSONL.
/// Written alongside BusMessage entries for session listing and discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TranscriptMeta {
    #[serde(rename = "custom-title")]
    CustomTitle {
        #[serde(rename = "customTitle")]
        custom_title: String,
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    #[serde(rename = "last-prompt")]
    LastPrompt {
        #[serde(rename = "lastPrompt")]
        last_prompt: String,
        #[serde(rename = "sessionId")]
        session_id: String,
    },
}

/// Lightweight session metadata extracted from head/tail scanning.
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub session_id: String,
    pub custom_title: Option<String>,
    pub first_prompt: String,
    pub timestamp: SystemTime,
    pub git_branch: Option<String>,
    pub cwd: Option<String>,
}

const HEAD_TAIL_BYTES: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

pub fn sessions_root() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".super").join("sessions")
}

pub fn session_dir(session_id: &str) -> PathBuf {
    sessions_root().join(session_id)
}

pub fn transcript_path(session_id: &str) -> PathBuf {
    session_dir(session_id).join("transcript.jsonl")
}

// ---------------------------------------------------------------------------
// Write path
// ---------------------------------------------------------------------------

/// Spawn a background task that subscribes to the bus and persists every
/// message with `parent_tool_use_id.is_none()` (root session messages)
/// into `transcript.jsonl`. Also writes `last-prompt` metadata entries
/// when it sees a User message.
pub fn spawn_transcript_writer(bus: Arc<SessionBus>, session_id: String) {
    let path = transcript_path(&session_id);
    if let Err(e) = fs::create_dir_all(path.parent().unwrap()) {
        tracing::warn!("transcript: cannot create dir for {:?}: {e}", path);
        return;
    }
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        let mut writer: Option<BufWriter<File>> = None;
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if msg.parent_tool_use_id().is_some() {
                        continue;
                    }
                    if writer.is_none() {
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => writer = Some(BufWriter::new(f)),
                            Err(e) => {
                                tracing::warn!("transcript: cannot open {:?}: {e}", path);
                                continue;
                            }
                        }
                    }
                    let w = writer.as_mut().unwrap();

                    // If user message, also write a last-prompt metadata entry.
                    if let BusMessage::User {
                        ref message,
                        ref session_id,
                        ..
                    } = &msg
                    {
                        let first_text = message.content.iter().find_map(|b| match b {
                            crate::sdk::protocol::ContentBlockFinal::Text { text } => {
                                Some(text.as_str())
                            }
                            _ => None,
                        });
                        if let Some(text) = first_text {
                            write_last_prompt_inline(w, session_id, text);
                        }
                    }

                    match serde_json::to_string(&msg) {
                        Ok(line) => {
                            let _ = w.write_all(line.as_bytes());
                            let _ = w.write_all(b"\n");
                            let _ = w.flush();
                        }
                        Err(e) => tracing::warn!("transcript: serialize failed: {e}"),
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Append a metadata entry directly to the transcript file.
pub fn write_meta_entry(session_id: &str, meta: &TranscriptMeta) {
    let path = transcript_path(session_id);
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            tracing::warn!("transcript: cannot create dir for {:?}: {e}", path);
            return;
        }
    }
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            if let Ok(line) = serde_json::to_string(meta) {
                let _ = f.write_all(line.as_bytes());
                let _ = f.write_all(b"\n");
                let _ = f.flush();
            }
        }
        Err(e) => tracing::warn!("transcript: cannot write meta to {:?}: {e}", path),
    }
}

/// Re-append custom-title and last-prompt entries so they stay in the tail
/// window. Called on resume to prevent the most recent metadata from scrolling
/// out of the 64KB tail scan window.
pub fn re_append_metadata(session_id: &str, custom_title: Option<&str>, last_prompt: Option<&str>) {
    if let Some(title) = custom_title {
        write_meta_entry(
            session_id,
            &TranscriptMeta::CustomTitle {
                custom_title: title.to_string(),
                session_id: session_id.to_string(),
            },
        );
    }
    if let Some(prompt) = last_prompt {
        write_meta_entry(
            session_id,
            &TranscriptMeta::LastPrompt {
                last_prompt: prompt.to_string(),
                session_id: session_id.to_string(),
            },
        );
    }
}

/// Write a last-prompt entry inline (already holding a BufWriter).
fn write_last_prompt_inline(w: &mut BufWriter<File>, session_id: &str, text: &str) {
    let flat = text.replace('\n', " ").trim().to_string();
    let prompt = if flat.len() > 200 {
        format!("{}…", &flat[..200].trim())
    } else {
        flat
    };
    let meta = TranscriptMeta::LastPrompt {
        last_prompt: prompt,
        session_id: session_id.to_string(),
    };
    if let Ok(line) = serde_json::to_string(&meta) {
        let _ = w.write_all(line.as_bytes());
        let _ = w.write_all(b"\n");
    }
}

// ---------------------------------------------------------------------------
// Read path — head/tail scanning
// ---------------------------------------------------------------------------

/// Internal metadata extracted from head/tail buffers.
struct RawMeta {
    custom_title: Option<String>,
    first_prompt: String,
    git_branch: Option<String>,
    cwd: Option<String>,
}

/// Scan `~/.super/sessions/*/transcript.jsonl` and return metadata for the
/// resume picker. Uses head/tail scanning to avoid reading full transcripts.
pub fn list_sessions() -> Vec<SessionMeta> {
    let root = sessions_root();
    let mut sessions: Vec<SessionMeta> = Vec::new();

    let entries = match fs::read_dir(&root) {
        Ok(e) => e,
        Err(_) => return sessions,
    };

    for entry in entries.flatten() {
        let session_id = entry.file_name().to_string_lossy().to_string();
        let transcript = transcript_path(&session_id);
        if !transcript.exists() {
            continue;
        }

        let file_size = match fs::metadata(&transcript) {
            Ok(m) => m.len(),
            Err(_) => continue,
        };
        if file_size == 0 {
            continue;
        }

        let timestamp = fs::metadata(&transcript)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);

        match scan_transcript_meta(&transcript, file_size) {
            Some(m) => {
                sessions.push(SessionMeta {
                    session_id,
                    custom_title: m.custom_title,
                    first_prompt: m.first_prompt,
                    timestamp,
                    git_branch: m.git_branch,
                    cwd: m.cwd,
                });
            }
            None => {
                // Sidechain-only session — skip.
            }
        }
    }

    sessions.sort_by_key(|b| std::cmp::Reverse(b.timestamp));
    sessions
}

/// Read first HEAD_TAIL_BYTES from the file and the last HEAD_TAIL_BYTES from the
/// file, then merge metadata: tail takes priority for fields that get re-appended
/// to EOF (`customTitle`, `lastPrompt`, `gitBranch`), head is used for `cwd` and
/// as a fallback when the tail has no match.
/// Returns None if the session is a sidechain file (no root messages).
fn scan_transcript_meta(path: &PathBuf, file_size: u64) -> Option<RawMeta> {
    let head_size = HEAD_TAIL_BYTES.min(file_size as usize);
    let tail_size = HEAD_TAIL_BYTES.min(file_size as usize);

    // Read head.
    let mut f = File::open(path).ok()?;
    let mut head_buf = vec![0u8; head_size];
    f.read_exact(&mut head_buf).ok()?;
    let head = String::from_utf8_lossy(&head_buf);

    // Read tail.
    let tail = if file_size > HEAD_TAIL_BYTES as u64 {
        f.seek(SeekFrom::End(-(tail_size as i64))).ok()?;
        let mut tail_buf = vec![0u8; tail_size];
        f.read_exact(&mut tail_buf).ok()?;
        String::from_utf8_lossy(&tail_buf).to_string()
    } else {
        // File is smaller than the tail window — head already covers everything.
        // No need to read again.
        String::new()
    };

    // Check first line: if it has a non-null parent_tool_use_id, it's a sidechain.
    if let Some(first_line) = head.lines().next() {
        if let Some(idx) = first_line.find("\"parent_tool_use_id\":") {
            let after = &first_line[idx + "\"parent_tool_use_id\":".len()..];
            let after = after.trim();
            if !after.starts_with("null") {
                return None;
            }
        }
    }

    // custom_title — tail (re-append) first, fall back to head.
    let custom_title = if !tail.is_empty() {
        extract_last_json_string_field(&tail, "customTitle")
            .or_else(|| extract_last_json_string_field(&head, "customTitle"))
    } else {
        extract_last_json_string_field(&head, "customTitle")
    };

    // first_prompt — tail `lastPrompt` (most recent user message), fall back to
    // head's first content text.
    let first_prompt = if !tail.is_empty() {
        extract_last_json_string_field(&tail, "lastPrompt").unwrap_or_else(|| {
            extract_last_json_string_field(&head, "lastPrompt").unwrap_or_default()
        })
    } else {
        extract_last_json_string_field(&head, "lastPrompt").unwrap_or_default()
    };

    // git_branch — tail first (re-append), fall back to head.
    let git_branch = if !tail.is_empty() {
        extract_last_json_string_field(&tail, "gitBranch")
            .or_else(|| extract_first_json_string_field(&head, "gitBranch"))
    } else {
        extract_first_json_string_field(&head, "gitBranch")
    };

    // cwd — head only (doesn't change across the session).
    let cwd = extract_first_json_string_field(&head, "cwd");

    Some(RawMeta {
        custom_title,
        first_prompt,
        git_branch,
        cwd,
    })
}

/// Find the LAST occurrence of a `"key":"value"` string field in a buffer.
fn extract_last_json_string_field(buf: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\":", key);
    let idx = buf.rfind(&pattern)?;
    let after_colon = &buf[idx + pattern.len()..];
    let after_colon = after_colon.trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }
    let value_start = &after_colon[1..];
    let mut result = String::new();
    let mut chars = value_start.chars();
    loop {
        match chars.next()? {
            '\\' => {
                chars.next()?;
            }
            '"' => break,
            c => result.push(c),
        }
    }
    Some(result)
}

/// Find the FIRST occurrence of a `"key":"value"` string field in a buffer.
fn extract_first_json_string_field(buf: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\":", key);
    let idx = buf.find(&pattern)?;
    let after_colon = &buf[idx + pattern.len()..];
    let after_colon = after_colon.trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }
    let value_start = &after_colon[1..];
    let mut result = String::new();
    let mut chars = value_start.chars();
    loop {
        match chars.next()? {
            '\\' => {
                chars.next()?;
            }
            '"' => break,
            c => result.push(c),
        }
    }
    Some(result)
}

// ---------------------------------------------------------------------------
// Resume loading
// ---------------------------------------------------------------------------

/// Load a session transcript and reconstruct history entries for the engine.
pub fn load_session_for_resume(
    session_id: &str,
) -> Result<(SessionMeta, Vec<HistoryEntry>), String> {
    let path = transcript_path(session_id);
    if !path.exists() {
        return Err(format!("session {} not found", session_id));
    }

    let file = File::open(&path).map_err(|e| format!("cannot open transcript: {e}"))?;
    let reader = BufReader::new(file);

    let mut history: Vec<HistoryEntry> = Vec::new();

    for line in reader.lines() {
        let line = line.map_err(|e| format!("read error: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<BusMessage>(&line) {
            Ok(BusMessage::User { message, .. }) => {
                history.push(HistoryEntry {
                    role: Role::User,
                    content: message.content,
                });
            }
            Ok(BusMessage::Assistant { message, .. }) => {
                history.push(HistoryEntry {
                    role: Role::Assistant,
                    content: message.content,
                });
            }
            Ok(_) => {}  // StreamEvent, Result, SystemEvent — skip
            Err(_) => {} // TranscriptMeta entry — skip
        }
    }

    let file_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let meta = scan_transcript_meta(&path, file_size);
    let timestamp = fs::metadata(&path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let session_meta = if let Some(m) = meta {
        SessionMeta {
            session_id: session_id.to_string(),
            custom_title: m.custom_title,
            first_prompt: m.first_prompt,
            timestamp,
            git_branch: m.git_branch,
            cwd: m.cwd,
        }
    } else {
        SessionMeta {
            session_id: session_id.to_string(),
            custom_title: None,
            first_prompt: String::new(),
            timestamp,
            git_branch: None,
            cwd: None,
        }
    };

    Ok((session_meta, history))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::protocol::{
        AnthropicUsage, AssistantPayload, ContentBlockFinal, SystemSubtype, UserPayload,
    };

    fn make_user_msg(text: &str, session_id: &str) -> BusMessage {
        BusMessage::User {
            message: UserPayload {
                role: "user".into(),
                content: vec![ContentBlockFinal::Text { text: text.into() }],
            },
            parent_tool_use_id: None,
            uuid: uuid::Uuid::new_v4(),
            session_id: session_id.into(),
        }
    }

    fn make_assistant_msg(text: &str, session_id: &str) -> BusMessage {
        BusMessage::Assistant {
            message: AssistantPayload {
                id: "msg_1".into(),
                model: "claude".into(),
                role: "assistant".into(),
                content: vec![ContentBlockFinal::Text { text: text.into() }],
                stop_reason: Some("end_turn".into()),
                usage: AnthropicUsage::default(),
            },
            parent_tool_use_id: None,
            uuid: uuid::Uuid::new_v4(),
            session_id: session_id.into(),
        }
    }

    #[tokio::test]
    async fn transcript_writer_writes_root_messages() {
        let sid = "test-session-2";
        let bus = Arc::new(SessionBus::new(sid.into()));
        spawn_transcript_writer(bus.clone(), sid.into());

        bus.emit(make_user_msg("root message", sid));

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let contents = fs::read_to_string(transcript_path(sid)).unwrap();
        assert!(
            contents.contains("root message"),
            "root message must be in transcript"
        );

        // Cleanup
        let _ = fs::remove_dir_all(session_dir(sid));
    }

    #[tokio::test]
    async fn transcript_writer_skips_subagent_messages() {
        let sid = "test-session-2-subagent";
        let bus = Arc::new(SessionBus::new(sid.into()));
        spawn_transcript_writer(bus.clone(), sid.into());

        bus.emit(make_user_msg("root message", sid));
        bus.emit(BusMessage::SystemEvent {
            subtype: SystemSubtype::Notice,
            message: "subagent note".into(),
            parent_tool_use_id: Some("tu_1".into()),
            uuid: uuid::Uuid::new_v4(),
            session_id: "agent-1".into(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let contents = fs::read_to_string(transcript_path(sid)).unwrap();
        assert!(
            contents.contains("root message"),
            "root message must be in transcript"
        );
        assert!(
            !contents.contains("subagent note"),
            "subagent message must NOT be in transcript"
        );

        // Cleanup
        let _ = fs::remove_dir_all(session_dir(sid));
    }

    #[tokio::test]
    async fn transcript_writes_last_prompt_metadata() {
        let sid = "test-last-prompt";
        let bus = Arc::new(SessionBus::new(sid.into()));
        spawn_transcript_writer(bus.clone(), sid.into());

        bus.emit(make_user_msg("hello world, this is a test prompt", sid));

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let contents = fs::read_to_string(transcript_path(sid)).unwrap();
        assert!(
            contents.contains("\"type\":\"last-prompt\""),
            "should contain last-prompt entry"
        );
        assert!(
            contents.contains("hello world"),
            "should contain the user text"
        );

        // Cleanup
        let _ = fs::remove_dir_all(session_dir(sid));
    }

    #[test]
    fn test_write_meta_entry() {
        let sid = "test-meta-write";
        write_meta_entry(
            sid,
            &TranscriptMeta::CustomTitle {
                custom_title: "test title".into(),
                session_id: sid.into(),
            },
        );

        let tp = transcript_path(sid);
        assert!(tp.exists(), "transcript should exist at {:?}", tp);
        let contents = fs::read_to_string(&tp).unwrap();
        assert!(contents.contains("custom-title"));
        assert!(contents.contains("test title"));

        // Cleanup
        let _ = fs::remove_dir_all(session_dir(sid));
    }

    #[test]
    fn load_transcript_reconstructs_history() {
        let sid = "test-load";
        let path = transcript_path(sid);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();

        let msgs: Vec<BusMessage> = vec![
            make_user_msg("first question", sid),
            make_assistant_msg("first answer", sid),
            make_user_msg("second question", sid),
            make_assistant_msg("second answer", sid),
        ];
        for msg in &msgs {
            writeln!(f, "{}", serde_json::to_string(msg).unwrap()).unwrap();
        }
        drop(f);

        let (_meta, history) = load_session_for_resume(sid).unwrap();
        assert_eq!(history.len(), 4);
        match &history[0] {
            HistoryEntry {
                role: Role::User,
                content,
            } => {
                assert!(content.iter().any(
                    |b| matches!(b, ContentBlockFinal::Text { text } if text == "first question")
                ));
            }
            _ => panic!("expected user entry"),
        }
        match &history[1] {
            HistoryEntry {
                role: Role::Assistant,
                content,
            } => {
                assert!(content.iter().any(
                    |b| matches!(b, ContentBlockFinal::Text { text } if text == "first answer")
                ));
            }
            _ => panic!("expected assistant entry"),
        }

        // Cleanup
        let _ = fs::remove_dir_all(session_dir(sid));
    }

    #[test]
    fn list_sessions_returns_metadata() {
        let sid = "test-list";
        let path = transcript_path(sid);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(
            f,
            "{}",
            serde_json::to_string(&make_user_msg("list test prompt", sid)).unwrap()
        )
        .unwrap();
        writeln!(
            f,
            "{}",
            serde_json::to_string(&make_assistant_msg("list test answer", sid)).unwrap()
        )
        .unwrap();
        drop(f);

        write_meta_entry(
            sid,
            &TranscriptMeta::CustomTitle {
                custom_title: "my custom title".into(),
                session_id: sid.into(),
            },
        );

        let sessions = list_sessions();
        let found = sessions.iter().find(|s| s.session_id == sid);
        assert!(found.is_some(), "should find test-list session");
        let meta = found.unwrap();
        assert_eq!(meta.custom_title.as_deref(), Some("my custom title"));

        // Cleanup
        let _ = fs::remove_dir_all(session_dir(sid));
    }

    #[test]
    fn scan_filters_out_sidechain_sessions() {
        let sid = "test-sidechain-filter";
        let path = transcript_path(sid);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();

        let sc_msg = BusMessage::SystemEvent {
            subtype: SystemSubtype::Notice,
            message: "sidechain".into(),
            parent_tool_use_id: Some("tu_parent".into()),
            uuid: uuid::Uuid::new_v4(),
            session_id: sid.into(),
        };
        writeln!(f, "{}", serde_json::to_string(&sc_msg).unwrap()).unwrap();
        drop(f);

        let file_size = fs::metadata(&path).unwrap().len();
        let result = scan_transcript_meta(&path, file_size);
        assert!(result.is_none(), "sidechain file should be filtered out");

        // Cleanup
        let _ = fs::remove_dir_all(session_dir(sid));
    }

    #[test]
    fn extract_json_string_fields() {
        let json =
            r#"{"type":"user","cwd":"/home/alice","content":[{"type":"text","text":"hello"}]}"#;
        assert_eq!(
            extract_first_json_string_field(json, "cwd").as_deref(),
            Some("/home/alice")
        );
        assert_eq!(
            extract_first_json_string_field(json, "text").as_deref(),
            Some("hello")
        );
        let multi = r#"{"customTitle":"first"} {"customTitle":"second"}"#;
        assert_eq!(
            extract_last_json_string_field(multi, "customTitle").as_deref(),
            Some("second")
        );
    }
}
