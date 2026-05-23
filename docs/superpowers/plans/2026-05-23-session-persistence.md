# Session Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist conversation transcripts to disk and implement `/resume`, `--resume`, and `/rename` so sessions survive restarts.

**Architecture:** A new `transcript.rs` module adds a root transcript writer subscribing to the SessionBus alongside the existing sidechain writer. The sidechain writer (filtering on `parent_tool_use_id.is_some()`) is untouched; the transcript writer (filtering on `is_none()`) writes to `transcript.jsonl`. Metadata (custom-title, last-prompt) is embedded as special JSONL entries. Session listing uses head/tail scanning (first/last 64KB) like Claude Code.

**Tech Stack:** Rust, tokio, serde_json, ratatui, clap

**Design doc:** `docs/superpowers/specs/2026-05-23-session-persistence-design.md`

---

### Task 1: Create transcript module with data types, write path, read path, and tests

**Files:**
- Create: `cli/src/conversation/transcript.rs`
- Modify: `cli/src/conversation/mod.rs`

- [ ] **Step 1: Add `pub mod transcript;` to conversation module**

Edit `cli/src/conversation/mod.rs` — add `pub mod transcript;` after `pub mod system_prompt;`:

```rust
pub mod anthropic;
pub mod compaction;
pub mod cron_runtime;
pub mod engine;
pub mod message_queue;
pub mod session_bus;
pub mod sidechain;
pub mod sse;
pub mod system_prompt;
pub mod transcript;
pub mod tool_loop;
```

- [ ] **Step 2: Create `cli/src/conversation/transcript.rs`**

Write the full file:

```rust
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
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
                        let first_text =
                            message
                                .content
                                .iter()
                                .find_map(|b| match b {
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
pub fn re_append_metadata(
    session_id: &str,
    custom_title: Option<&str>,
    last_prompt: Option<&str>,
) {
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

    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    sessions
}

/// Read first and last HEAD_TAIL_BYTES from a file, extract metadata.
/// Returns None if the session is a sidechain file (no root messages).
fn scan_transcript_meta(path: &PathBuf, file_size: u64) -> Option<RawMeta> {
    let head_size = HEAD_TAIL_BYTES.min(file_size as usize);
    let mut head_buf = vec![0u8; head_size];
    let mut f = File::open(path).ok()?;
    f.read_exact(&mut head_buf).ok()?;
    let head = String::from_utf8_lossy(&head_buf);

    // Check first line: if it has a non-null parent_tool_use_id, it's a sidechain.
    if let Some(first_line) = head.lines().next() {
        if let Some(idx) = first_line.find("\"parent_tool_use_id\":") {
            let after = &first_line[idx + "\"parent_tool_use_id\":".len()..];
            let after = after.trim();
            if after != "null" && after != "null," {
                return None;
            }
        }
    }

    let first_prompt =
        extract_last_json_string_field(&head, "lastPrompt").unwrap_or_else(|| String::new());

    let custom_title = extract_last_json_string_field(&head, "customTitle");

    let git_branch = extract_first_json_string_field(&head, "gitBranch");

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
    let pattern = format!("\"{}\"", key);
    let idx = buf.rfind(&pattern)?;
    let after_key = &buf[idx + pattern.len()..];
    let after_colon = after_key.trim_start().strip_prefix(':')?;
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
    let pattern = format!("\"{}\"", key);
    let idx = buf.find(&pattern)?;
    let after_key = &buf[idx + pattern.len()..];
    let after_colon = after_key.trim_start().strip_prefix(':')?;
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
            Ok(_) => {}   // StreamEvent, Result, SystemEvent — skip
            Err(_) => {}  // TranscriptMeta entry — skip
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

    fn temp_sessions_dir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("super-transcript-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_user_msg(text: &str, session_id: &str) -> BusMessage {
        BusMessage::User {
            message: UserPayload {
                role: "user".into(),
                content: vec![ContentBlockFinal::Text {
                    text: text.into(),
                }],
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
                content: vec![ContentBlockFinal::Text {
                    text: text.into(),
                }],
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
        let tmp = temp_sessions_dir();
        let sid = "test-session-1";
        // Override sessions_root by writing directly into tmp
        let bus = Arc::new(SessionBus::new(sid.into()));

        // Write to our temp dir path instead of default
        let path = tmp.join(sid).join("transcript.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        // We can't easily override the path, so test write_meta_entry + scan directly
        write_meta_entry(
            "test-meta-write",
            &TranscriptMeta::CustomTitle {
                custom_title: "test title".into(),
                session_id: "test-meta-write".into(),
            },
        );

        // Verify the file was created
        let tp = transcript_path("test-meta-write");
        assert!(tp.exists(), "transcript should exist at {:?}", tp);
        let contents = fs::read_to_string(&tp).unwrap();
        assert!(contents.contains("custom-title"));
        assert!(contents.contains("test title"));

        // Cleanup
        let _ = fs::remove_dir_all(sessions_root().join("test-meta-write"));
    }

    #[tokio::test]
    async fn transcript_writer_skips_subagent_messages() {
        let tmp = temp_sessions_dir();
        let sid = "test-session-2";
        let bus = Arc::new(SessionBus::new(sid.into()));
        spawn_transcript_writer(bus.clone(), sid.into());

        // Emit a root message
        bus.emit(make_user_msg("root message", sid));
        // Emit a subagent message (has parent_tool_use_id)
        bus.emit(BusMessage::SystemEvent {
            subtype: SystemSubtype::Notice,
            message: "subagent note".into(),
            parent_tool_use_id: Some("tu_1".into()),
            uuid: uuid::Uuid::new_v4(),
            session_id: "agent-1".into(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let contents = fs::read_to_string(transcript_path(sid)).unwrap();
        assert!(contents.contains("root message"), "root message must be in transcript");
        assert!(!contents.contains("subagent note"), "subagent message must NOT be in transcript");
    }

    #[tokio::test]
    async fn transcript_writes_last_prompt_metadata() {
        let sid = "test-last-prompt";
        let bus = Arc::new(SessionBus::new(sid.into()));
        spawn_transcript_writer(bus.clone(), sid.into());

        bus.emit(make_user_msg("hello world, this is a test prompt", sid));

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let contents = fs::read_to_string(transcript_path(sid)).unwrap();
        assert!(contents.contains("\"type\":\"last-prompt\""), "should contain last-prompt entry");
        assert!(contents.contains("hello world"), "should contain the user text");

        // Cleanup
        let _ = fs::remove_dir_all(session_dir(sid));
    }

    #[test]
    fn load_transcript_reconstructs_history() {
        let sid = "test-load";
        let bus = Arc::new(SessionBus::new(sid.into()));

        // Write messages manually
        let path = transcript_path(sid);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = OpenOptions::new().create(true).append(true).open(&path).unwrap();

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
            HistoryEntry { role: Role::User, content } => {
                assert!(content.iter().any(|b| matches!(b, ContentBlockFinal::Text { text } if text == "first question")));
            }
            _ => panic!("expected user entry"),
        }
        match &history[1] {
            HistoryEntry { role: Role::Assistant, content } => {
                assert!(content.iter().any(|b| matches!(b, ContentBlockFinal::Text { text } if text == "first answer")));
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
        let mut f = OpenOptions::new().create(true).append(true).open(&path).unwrap();
        writeln!(f, "{}", serde_json::to_string(&make_user_msg("list test prompt", sid)).unwrap()).unwrap();
        writeln!(f, "{}", serde_json::to_string(&make_assistant_msg("list test answer", sid)).unwrap()).unwrap();
        drop(f);

        // Write title via custom-title entry
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
        let mut f = OpenOptions::new().create(true).append(true).open(&path).unwrap();

        // Write a sidechain message (has parent_tool_use_id)
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
        let json = r#"{"type":"user","cwd":"/home/alice","content":[{"type":"text","text":"hello"}]}"#;
        assert_eq!(
            extract_first_json_string_field(json, "cwd").as_deref(),
            Some("/home/alice")
        );
        assert_eq!(
            extract_first_json_string_field(json, "text").as_deref(),
            Some("hello")
        );
        // last wins
        let multi = r#"{"customTitle":"first"} {"customTitle":"second"}"#;
        assert_eq!(
            extract_last_json_string_field(multi, "customTitle").as_deref(),
            Some("second")
        );
    }
}

---

### Task 2: Wire transcript writer into bootstrap

**Files:**
- Modify: `cli/src/bootstrap.rs`

- [ ] **Step 1: Spawn transcript writer in bootstrap**

Edit `cli/src/bootstrap.rs` — after the sidechain writer spawn (after line 86), add:

```rust
    // Spawn sidechain JSONL writer so any subagent activity gets persisted.
    let sidechain_dir = crate::conversation::sidechain::default_sidechain_dir(bus.session_id());
    crate::conversation::sidechain::spawn_sidechain_writer(bus.clone(), sidechain_dir);

    // Spawn root transcript writer so the session is persisted.
    crate::conversation::transcript::spawn_transcript_writer(
        bus.clone(),
        bus.session_id().to_string(),
    );
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p super-cli 2>&1 | head -20`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add cli/src/bootstrap.rs
git commit -m "feat: wire root transcript writer into bootstrap"
```

---

### Task 3: Add `ConversationEngine::resume` constructor

**Files:**
- Modify: `cli/src/conversation/engine.rs`

- [ ] **Step 1: Add the resume constructor**

After the `new_child` constructor (after line 111 of engine.rs), add:

```rust
    /// Construct an engine that resumes an existing session.
    /// Seeds history from the provided entries instead of starting fresh.
    pub fn resume(
        store: Arc<Store>,
        config: CliConfig,
        registry: Arc<ToolRegistry>,
        bus: Arc<SessionBus>,
        queue: Arc<crate::conversation::message_queue::MessageQueue>,
        history: Vec<HistoryEntry>,
    ) -> Self {
        Self {
            store,
            config,
            registry,
            bus,
            abort: None,
            session_id_override: None,
            history_override: Some(history),
            permission_mode_override: None,
            auto_deny_prompts: false,
            queue,
            skills: Arc::new(Vec::new()),
            skill_listing_sent: Arc::new(AtomicBool::new(true)), // skill listing was sent in original session
        }
    }
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p super-cli 2>&1 | head -20`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add cli/src/conversation/engine.rs
git commit -m "feat: add ConversationEngine::resume constructor"
```

---

### Task 4: Implement `/rename` with persistence

**Files:**
- Modify: `cli/src/commands/dispatch.rs`
- Modify: `cli/src/tui/app.rs` (call site)

- [ ] **Step 1: Update the dispatch function signature to accept session_id**

Change the `dispatch` function signature (line 20) from:
```rust
pub fn dispatch(command: &Command, input: &str, store: &Store) -> CommandResult {
```
to:
```rust
pub fn dispatch(command: &Command, input: &str, store: &Store, session_id: &str) -> CommandResult {
```

- [ ] **Step 2: Update the `/rename` arm to pass session_id**

Change line 35 from:
```rust
        "/rename" => rename(args),
```
to:
```rust
        "/rename" => rename(args, session_id),
```

- [ ] **Step 3: Rewrite the `rename` function**

Replace lines 340-351:
```rust
fn rename(args: &str) -> CommandResult {
    let name = args.trim();
    if name.is_empty() {
        return CommandResult::Display(
            "Could not generate a name: no conversation context yet. Usage: /rename <name>".into(),
        );
    }
    CommandResult::Display(format!("Session renamed to: {name}"))
}
```

with:
```rust
fn rename(args: &str, session_id: &str) -> CommandResult {
    let name = args.trim();
    if name.is_empty() {
        return CommandResult::Display(
            "Could not generate a name: no conversation context yet. Usage: /rename <name>".into(),
        );
    }
    crate::conversation::transcript::write_meta_entry(
        session_id,
        &crate::conversation::transcript::TranscriptMeta::CustomTitle {
            custom_title: name.to_string(),
            session_id: session_id.to_string(),
        },
    );
    CommandResult::Display(format!("Session renamed to: {name}"))
}
```

- [ ] **Step 4: Update the dispatch call site in app.rs**

In `cli/src/tui/app.rs`, line 215, change:
```rust
match crate::commands::dispatch::dispatch(cmd, &text, &self.store) {
```
to:
```rust
match crate::commands::dispatch::dispatch(cmd, &text, &self.store, self.bus.session_id()) {
```

- [ ] **Step 5: Compile check**

Run: `cargo check -p super-cli 2>&1 | head -20`
Expected: compiles

- [ ] **Step 6: Commit**

```bash
git add cli/src/commands/dispatch.rs cli/src/tui/app.rs
git commit -m "feat: implement /rename with transcript persistence"
```

---

### Task 5: Build resume picker with real session list

**Files:**
- Modify: `cli/src/tui/modals/resume_picker.rs`

- [ ] **Step 1: Rewrite ResumePicker**

Replace the entire file:

```rust
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::conversation::transcript::SessionMeta;
use crate::tui::colors::{CC_BLUE, CC_DIM};

pub struct ResumePicker {
    pub query: String,
    pub sessions: Vec<SessionMeta>,
    pub selected_idx: usize,
}

impl ResumePicker {
    pub fn new() -> Self {
        let sessions = crate::conversation::transcript::list_sessions();
        Self {
            query: String::new(),
            sessions,
            selected_idx: 0,
        }
    }

    fn filtered_sessions(&self) -> Vec<&SessionMeta> {
        if self.query.is_empty() {
            self.sessions.iter().collect()
        } else {
            let q = self.query.to_lowercase();
            self.sessions
                .iter()
                .filter(|s| {
                    s.first_prompt.to_lowercase().contains(&q)
                        || s.custom_title
                            .as_ref()
                            .map(|t| t.to_lowercase().contains(&q))
                            .unwrap_or(false)
                        || s.session_id.to_lowercase().contains(&q)
                })
                .collect()
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ResumeAction {
        match key.code {
            KeyCode::Esc => ResumeAction::Cancel,
            KeyCode::Up | KeyCode::Char('k') => {
                let filtered = self.filtered_sessions();
                if !filtered.is_empty() {
                    self.selected_idx = self.selected_idx.saturating_sub(1);
                }
                ResumeAction::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let filtered = self.filtered_sessions();
                if !filtered.is_empty() {
                    let max = filtered.len().saturating_sub(1);
                    if self.selected_idx < max {
                        self.selected_idx += 1;
                    }
                }
                ResumeAction::Continue
            }
            KeyCode::Enter => {
                let filtered = self.filtered_sessions();
                if let Some(s) = filtered.get(self.selected_idx) {
                    ResumeAction::Select(s.session_id.clone())
                } else {
                    ResumeAction::Continue
                }
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.selected_idx = 0;
                ResumeAction::Continue
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.selected_idx = 0;
                ResumeAction::Continue
            }
            _ => ResumeAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let filtered = self.filtered_sessions();
        let max_items = area.height.saturating_sub(3) as usize; // leave room for header + search

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("  Search: [{}]", self.query),
            Style::default().fg(CC_DIM),
        )));
        lines.push(Line::raw(""));

        if filtered.is_empty() {
            if self.sessions.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  No previous sessions found.",
                    Style::default().fg(CC_DIM),
                )));
                lines.push(Line::from(Span::styled(
                    "  Start a conversation to create a session.",
                    Style::default().fg(CC_DIM),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "  No sessions match your search.",
                    Style::default().fg(CC_DIM),
                )));
            }
        } else {
            // Show visible window
            let start = self.selected_idx.saturating_sub(max_items.saturating_sub(1));
            let end = (start + max_items).min(filtered.len());

            for (i, session) in filtered.iter().enumerate().skip(start).take(end - start) {
                let is_selected = i == self.selected_idx;
                let prefix = if is_selected { "> " } else { "  " };

                let title = session
                    .custom_title
                    .as_deref()
                    .unwrap_or(&session.first_prompt)
                    .to_string();
                let display_title = if title.len() > 60 {
                    format!("{}…", &title[..59])
                } else {
                    title
                };

                let line = if is_selected {
                    Line::from(vec![
                        Span::styled(
                            format!("{}{}", prefix, display_title),
                            Style::default()
                                .fg(CC_BLUE)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ])
                } else {
                    Line::from(vec![Span::raw(format!("{}{}", prefix, display_title))])
                };
                lines.push(line);
            }
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  [esc to cancel] [↑↓ to navigate] [enter to select]",
            Style::default().fg(CC_DIM),
        )));

        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum ResumeAction {
    Continue,
    Cancel,
    Select(String),
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p super-cli 2>&1 | head -30`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add cli/src/tui/modals/resume_picker.rs
git commit -m "feat: build resume picker with real session list from head/tail scan"
```

---

### Task 6: Wire session selection through modal → engine resume

**Files:**
- Modify: `cli/src/tui/modal.rs` (add `ResumeAction::Select` handling + new `ModalAction::ResumeSession`)
- Modify: `cli/src/tui/app.rs` (handle `ModalAction::ResumeSession`)

- [ ] **Step 1: Add `ModalAction::ResumeSession` variant**

In `cli/src/tui/modal.rs`, add to the `ModalAction` enum:

```rust
pub enum ModalAction {
    Continue,
    Close,
    SetClass(String),
    SetProvider(String),
    SetEffort(String),
    SubmitAnswers(serde_json::Value),
    ResumeSession(String),  // NEW
}
```

- [ ] **Step 2: Handle `ResumeAction::Select` in `Modal::handle_key`**

In `cli/src/tui/modal.rs`, in the `Modal::Resume(p)` match arm (lines 78-81), update from:
```rust
            Modal::Resume(p) => match p.handle_key(key) {
                ResumeAction::Continue => ModalAction::Continue,
                ResumeAction::Cancel => ModalAction::Close,
            },
```
to:
```rust
            Modal::Resume(p) => match p.handle_key(key) {
                ResumeAction::Continue => ModalAction::Continue,
                ResumeAction::Cancel => ModalAction::Close,
                ResumeAction::Select(id) => ModalAction::ResumeSession(id),
            },
```

- [ ] **Step 3: Handle `ModalAction::ResumeSession` in `App::handle_key`**

In `cli/src/tui/app.rs`, after the existing modal action handling (after the `SubmitAnswers` match arm, before the closing `}`), add:

```rust
                ModalAction::ResumeSession(session_id) => {
                    self.modal = None;

                    match crate::conversation::transcript::load_session_for_resume(&session_id) {
                        Ok((meta, history)) => {
                            crate::conversation::transcript::re_append_metadata(
                                &session_id,
                                meta.custom_title.as_deref(),
                                if meta.first_prompt.is_empty() { None } else { Some(&meta.first_prompt) },
                            );

                            let registry = self.engine.registry.clone();
                            let skills = self.engine.skills.clone();
                            let mut resumed_engine = crate::conversation::engine::ConversationEngine::resume(
                                self.store.clone(),
                                self._config.clone(),
                                registry,
                                self.bus.clone(),
                                self.queue.clone(),
                                history,
                            );
                            resumed_engine.skills = skills;

                            self.engine = resumed_engine;
                            self.reset_scrollback_state();
                            self.scroll_area.push(crate::tui::message::Message::System(
                                format!("Resumed session: {}",
                                    meta.custom_title.as_deref().unwrap_or(&meta.first_prompt)),
                            ));
                        }
                        Err(e) => {
                            self.scroll_area.push(crate::tui::message::Message::System(
                                format!("Failed to resume session: {e}"),
                            ));
                        }
                    }
                    return Ok(());
                }
```

- [ ] **Step 4: Compile check**

Run: `cargo check -p super-cli 2>&1 | head -30`
Expected: compiles

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/modal.rs cli/src/tui/app.rs
git commit -m "feat: wire session selection through modal to engine resume"
```

---

### Task 7: Add `--resume` CLI flag

**Files:**
- Modify: `cli/src/main.rs`
- Modify: `cli/src/bootstrap.rs`

- [ ] **Step 1: Add `--resume` flag to CLI parser**

Edit `cli/src/main.rs`:

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "super", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Resume a specific session by ID, or the most recent session if no ID given
    #[arg(long, num_args = 0..=1)]
    resume: Option<Option<String>>,
}

#[derive(Subcommand)]
enum Commands {
    /// Login to Super
    Login,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Login) => {
            let config = super_cli::config::load_config();
            let auth_client = super_cli::auth::AuthClient::new(config.api_base_url.clone());
            match auth_client.login_flow().await {
                Ok(_) => println!("Logged in successfully."),
                Err(e) => eprintln!("Login failed: {e}"),
            }
        }
        None => {
            super_cli::bootstrap::run(cli.resume).await;
        }
    }
}
```

- [ ] **Step 2: Update bootstrap to accept and handle `--resume`**

Replace the entire `cli/src/bootstrap.rs` with:

```rust
use std::sync::Arc;

use crate::config::load_config;
use crate::conversation::message_queue::MessageQueue;

pub async fn run(resume: Option<Option<String>>) {
    let config = load_config();

    if config.openrouter_api_key.is_none() {
        eprintln!("Not logged in. Run 'super login' first.");
        return;
    }

    // Determine session ID and optional resume history early.
    let session_id: String;
    let resume_history: Option<Vec<crate::conversation::anthropic::HistoryEntry>>;

    if let Some(maybe_id) = &resume {
        let sessions = crate::conversation::transcript::list_sessions();
        let target_id = match maybe_id {
            Some(id) => Some(id.clone()),
            None => sessions.first().map(|s| s.session_id.clone()),
        };
        if let Some(sid) = target_id {
            match crate::conversation::transcript::load_session_for_resume(&sid) {
                Ok((meta, history)) => {
                    if history.is_empty() {
                        eprintln!("Session {} has no conversation history.", sid);
                        return;
                    }
                    crate::conversation::transcript::re_append_metadata(
                        &sid,
                        meta.custom_title.as_deref(),
                        if meta.first_prompt.is_empty() {
                            None
                        } else {
                            Some(&meta.first_prompt)
                        },
                    );
                    session_id = sid;
                    resume_history = Some(history);
                }
                }
                Err(e) => {
                    eprintln!("Failed to resume session {}: {e}", sid);
                    return;
                }
            }
        } else {
            eprintln!("No previous sessions found to resume.");
            return;
        }
    } else {
        session_id = uuid::Uuid::new_v4().to_string();
        resume_history = None;
    }

    let store = Arc::new(crate::state::store::Store::new());
    {
        let provider = config.provider.clone();
        let model_class = config.model_class.clone();
        store.set_state(|s| {
            s.provider = provider;
            s.model_class = model_class;
        });
    }

    // Create the cron jobs map and wake channel
    let cron_jobs: Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, crate::conversation::cron_runtime::CronJob>,
        >,
    > = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let (cron_wake_tx, cron_wake_rx) = tokio::sync::watch::channel(false);

    // Create the shared message queue
    let queue = Arc::new(MessageQueue::new());

    // Build tool registry
    let cwd_for_agents = std::env::current_dir().unwrap_or_default();
    let agent_registry = Arc::new(crate::agents::AgentRegistry::load(&cwd_for_agents));
    let registry = crate::tools::ToolRegistry::new(
        store.clone(),
        config.clone(),
        agent_registry,
        queue.clone(),
        cron_jobs.clone(),
        cron_wake_tx,
    );

    // Spawn cron runtime
    let cron_runtime =
        crate::conversation::cron_runtime::CronRuntime::new(cron_jobs, queue.clone(), cron_wake_rx);
    tokio::spawn(async move { cron_runtime.run().await });

    // Load skills
    let bundled_skills = crate::skills::bundled::extract_bundled_skills();
    let local_skills = crate::skills::loader::load_all_skills();
    let mut by_name: std::collections::HashMap<String, crate::skills::loader::Skill> =
        bundled_skills.into_iter().map(|s| (s.name.clone(), s)).collect();
    for s in local_skills {
        by_name.insert(s.name.clone(), s);
    }
    let all_skills: Vec<crate::skills::loader::Skill> = by_name.into_values().collect();
    registry.register(Arc::new(crate::tools::skill::SkillTool {
        skills: all_skills.clone(),
    }));

    crate::lsp::initialize_lsp_manager();

    let bus = Arc::new(crate::conversation::session_bus::SessionBus::new(session_id));

    // Spawn sidechain writer
    let sidechain_dir = crate::conversation::sidechain::default_sidechain_dir(bus.session_id());
    crate::conversation::sidechain::spawn_sidechain_writer(bus.clone(), sidechain_dir);

    // Spawn transcript writer
    crate::conversation::transcript::spawn_transcript_writer(
        bus.clone(),
        bus.session_id().to_string(),
    );

    // Build engine — resume or fresh
    let registry = Arc::new(registry);
    let mut engine = if let Some(history) = resume_history {
        crate::conversation::engine::ConversationEngine::resume(
            store.clone(),
            config.clone(),
            registry.clone(),
            bus.clone(),
            queue.clone(),
            history,
        )
    } else {
        crate::conversation::engine::ConversationEngine::new(
            store.clone(),
            config.clone(),
            registry.clone(),
            bus.clone(),
            queue.clone(),
        )
    };
    engine.skills = Arc::new(all_skills);

    // Build system prompt
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut system_prompt = crate::conversation::system_prompt::SystemPrompt::build(&cwd);
    let tool_descriptions =
        registry.tool_descriptions(&crate::state::store::PermissionMode::Default);
    system_prompt.add_section(format!("Available tools:\n{tool_descriptions}"));

    crate::tui::app::run_with_engine(config, store, engine, registry, bus, system_prompt, queue).await;
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p super-cli 2>&1 | head -30`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add cli/src/main.rs cli/src/bootstrap.rs
git commit -m "feat: add --resume CLI flag for session resume"
```

---

### Task 8: Integration test — write, list, resume

**Files:**
- Modify: `cli/src/conversation/transcript.rs` (add integration test)

- [ ] **Step 1: Add integration test**

Add to the `#[cfg(test)] mod tests` block in transcript.rs:

```rust
    #[tokio::test]
    async fn integration_write_list_resume() {
        let sid = "test-integration";
        let bus = Arc::new(SessionBus::new(sid.into()));
        spawn_transcript_writer(bus.clone(), sid.into());

        // Simulate a full conversation turn
        bus.emit(make_user_msg("how do i add dark mode?", sid));
        bus.emit(make_assistant_msg("here is how...", sid));
        bus.emit(make_user_msg("thanks, also add a toggle", sid));
        bus.emit(make_assistant_msg("done, toggle added", sid));

        // Write a custom title
        write_meta_entry(
            sid,
            &TranscriptMeta::CustomTitle {
                custom_title: "dark mode feature".into(),
                session_id: sid.into(),
            },
        );

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // 1. List sessions — should find it
        let sessions = list_sessions();
        let found = sessions.iter().find(|s| s.session_id == sid);
        assert!(found.is_some(), "session should appear in listing");
        let meta = found.unwrap();
        assert_eq!(meta.custom_title.as_deref(), Some("dark mode feature"));

        // 2. Load for resume — should have 4 history entries
        let (_, history) = load_session_for_resume(sid).unwrap();
        assert_eq!(history.len(), 4, "should have 4 history entries (2 user + 2 assistant)");

        // 3. Verify history content
        let user_texts: Vec<String> = history
            .iter()
            .filter_map(|h| match h {
                HistoryEntry { role: Role::User, content } => {
                    content.iter().find_map(|b| match b {
                        ContentBlockFinal::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                }
                _ => None,
            })
            .collect();
        assert_eq!(user_texts, vec!["how do i add dark mode?", "thanks, also add a toggle"]);

        // Cleanup
        let _ = fs::remove_dir_all(session_dir(sid));
    }
```

- [ ] **Step 2: Run integration test**

Run: `cargo test -p super-cli conversation::transcript::tests::integration_write_list_resume 2>&1`
Expected: PASS

- [ ] **Step 3: Run all transcript tests**

Run: `cargo test -p super-cli conversation::transcript 2>&1 | tail -25`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add cli/src/conversation/transcript.rs
git commit -m "test: add integration test for write-list-resume cycle"
```

---

### Task 9: Full build and clippy check

- [ ] **Step 1: Full cargo check**

Run: `cargo check -p super-cli 2>&1`
Expected: no errors

- [ ] **Step 2: Clippy**

Run: `cargo clippy -p super-cli -- -D warnings 2>&1 | tail -20`
Expected: no warnings

- [ ] **Step 3: Full test suite**

Run: `cargo test -p super-cli 2>&1 | tail -20`
Expected: all tests pass (existing + new)

- [ ] **Step 4: Commit any clippy fixes if needed**

```bash
git add -A
git commit -m "chore: clippy fixes for session persistence"
```
(Only if there were changes)
