# Daemon presence (ENG-37) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use super:subagent-driven-development (recommended) or super:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A signed-in `super daemon` announces its host and every session to the platform registry, mirrors each hosted session to its S2 streams, and takes `create_session`/`resume_session`/`stop_session`/listing commands from `hosts/{hostId}/commands`, answering on `hosts/{hostId}/events`.

**Architecture:** `super-remote` grows a multi-session `RegistryWriter`, a shared `PlatformCredentials`, public command-cursor helpers and a `fake` feature with the in-memory S2 used by tests. `super-daemon` gets `presence.rs`: a supervisor that watches the sign-in and runs one `Link` (registry writer, one `SessionTransport` per hosted session, a host-command tailer, a host-events appender) driven by the `SessionHost`'s own daemon events. The socket server's host-only command handling is extracted so the socket and the platform run the same code.

**Tech Stack:** Rust 2024, tokio, `super-remote` (S2 client, transport), `super-sdk` mock provider, fake S2 over a TCP listener.

Spec: `.super/specs/2026-10-29-daemon-design.md`, section "Slice 2". Linear: ENG-37.

## Global Constraints

- Same as slice 1: workspace version `0.0.37`, edition 2024, `[lints] workspace = true`; CI runs fmt, clippy `-D warnings` (ubuntu, macos, windows) and the workspace tests. `super-daemon`'s unix-only modules stay `cfg(unix)`; `presence.rs` is `cfg(unix)` too (it lives behind `run`).
- Stream names: `registry`, `sessions/{hostId}/{sessionId}/{events,entries,commands}`, `hosts/{hostId}/commands`, `hosts/{hostId}/events`. Host command headers `t`, `from`, `name`, `exp`; host event header `t` is the body's `type` (`response` for responses).
- Host commands allowed over the platform: `status`, `list_sessions`, `list_workspaces`, `list_directories`, `create_session`, `resume_session`, `stop_session`. Others answer `not available over the platform`.
- Registry: `host` once per link start, `heartbeat` every 30 s (`HEARTBEAT_INTERVAL`), `session` per session; stored sessions as `ended`; `daemon: true`, `headless: false`, `resumeArgv: ["super", "--session", <file>]`; full re-publish every six hours (`republish_interval`, tests shorten it).
- Supervisor: checks the sign-in every 30 s (`check_interval`); starts a link when signed in and none runs; stops it when signed out or unauthorized.
- No AI attribution in commits. Branch `daemon-presence`, worktree `/Users/chwzr/flxkpe/harness/super-daemon-presence`. Use absolute paths.

---

## File structure

| Path | Responsibility |
| --- | --- |
| `crates/super-remote/Cargo.toml` | `fake` feature; `super-ai` (native) dependency; self dev-dependency with `fake` |
| `crates/super-remote/src/fake.rs` (new, feature `fake`) | `FakeS2`, `serve`, `TestCredentials` |
| `crates/super-remote/src/commands.rs` (new) | `CommandCursor`, `is_expired`, `start_position` (moved out of `transport.rs`) |
| `crates/super-remote/src/transport.rs` | uses `commands::*`; `SessionTransport::unauthorized()` |
| `crates/super-remote/src/registry.rs` | multi-session `RegistryWriter`, `refresh()`, `SessionRecord.daemon` |
| `crates/super-remote/src/credentials.rs` (new) | `PlatformCredentials`, `is_signed_in`, `resolve_platform_url`, `configure_platform_url` |
| `crates/super-remote/src/lib.rs` | modules and re-exports |
| `crates/super-remote/tests/transport.rs` | uses `super_remote::fake` |
| `crates/super-remote/tests/registry.rs` (new) | multi-session writer tests |
| `crates/super/src/remote.rs` | uses `super_remote::PlatformCredentials` |
| `crates/super-daemon/Cargo.toml` | `super-remote` dependency; dev `super-remote` with `fake`, `reqwest` |
| `crates/super-daemon/src/server.rs` | `run_host_command`, `ServerContext::status` public |
| `crates/super-daemon/src/presence.rs` (new, unix) | `PresenceConfig`, `LinkConfig`, `Link`, `Presence`, `next_action`, `record_from_summary` |
| `crates/super-daemon/src/lib.rs` | `DaemonConfig.presence`, wiring in `run` |
| `crates/super-daemon/tests/presence.rs` (new) | link tests with the fake S2 |
| `crates/super/src/daemon_cli.rs` | builds `PresenceConfig` from settings |
| `docs/daemon.md`, `docs/super-platform.md`, `CHANGELOG.md` | documentation |

---

### Task 1: The fake S2 becomes a feature of `super-remote`; cursor helpers become public

**Files:**
- Modify: `crates/super-remote/Cargo.toml`
- Create: `crates/super-remote/src/fake.rs` (the `FakeS2`, `serve`, `handle`, `percent_decode`, `write_response`, `now_ms`, `TestCredentials` currently in `tests/transport.rs` lines 19-287, made `pub` where tests call them)
- Create: `crates/super-remote/src/commands.rs`
- Modify: `crates/super-remote/src/transport.rs` (remove `CommandCursor`, `Tailer::start_position`, `Tailer::is_expired`; call the module)
- Modify: `crates/super-remote/src/lib.rs`
- Modify: `crates/super-remote/tests/transport.rs` (delete the moved code, `use super_remote::fake::{FakeS2, TestCredentials, serve}`)

**Interfaces:**
- Produces: `super_remote::fake::FakeS2` with `pub fn append(&self, stream: &str, records: Vec<(Vec<(String, String)>, String)>) -> (u64, u64)`, `pub fn records(&self, stream: &str) -> Vec<StoredRecord>` (`StoredRecord { pub seq: u64, pub headers: Vec<(String, String)>, pub body: String }`), `pub fn seen_tokens(&self) -> Vec<String>`; `pub async fn serve(fake: Arc<FakeS2>) -> String` (the `http://…/v1` base url); `pub struct TestCredentials` implementing `RemoteCredentials`. `super_remote::commands::{CommandCursor::new(dir: Option<PathBuf>, stream: &str), load(&self) -> Option<u64>, store(&self, u64), is_expired(record: &SequencedRecord, default_expiry: Duration) -> bool, start_position(client: &S2Client, stream: &str, cursor: &CommandCursor, lookback: u64) -> ReadStart}`.

- [ ] **Step 1: Cargo.** In `crates/super-remote/Cargo.toml` add after `[lints]`:

```toml
[features]
# An in-memory S2 server for tests of this crate and of crates that build on it.
fake = []
```

and to `[dependencies]` add `super-ai = { workspace = true, features = ["native"] }` (used by Task 3; added now so the manifest changes once). To `[dev-dependencies]` add `super-remote = { path = ".", features = ["fake"] }`.

- [ ] **Step 2: Move the fake.** Create `crates/super-remote/src/fake.rs` with the doc comment `//! An in-memory S2 server for tests: streams in memory, SSE tails that deliver whatever is appended after the reader connected. Enabled with the \`fake\` feature.` followed by the code from `tests/transport.rs` lines 19-287 unchanged except: `struct StoredRecord` and its fields become `pub`; `struct FakeS2` becomes `pub struct FakeS2` with `#[derive(Default)]` kept and its fields private; add `pub fn seen_tokens(&self) -> Vec<String> { self.seen_tokens.lock().unwrap().clone() }`; `fn append` and `fn records` become `pub fn`; `async fn serve` becomes `pub async fn serve`; `struct TestCredentials` becomes `pub struct TestCredentials;` with its `impl RemoteCredentials` unchanged; `use` lines at the top: `use crate::RemoteCredentials; use serde_json::{Value, json}; use std::collections::BTreeMap; use std::future::Future; use std::pin::Pin; use std::sync::{Arc, Mutex}; use std::time::Duration; use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader}; use tokio::net::TcpListener;`. In `lib.rs` add `#[cfg(feature = "fake")] pub mod fake;`. In `tests/transport.rs` delete the moved code and the now-unused imports, and add `use super_remote::fake::{FakeS2, TestCredentials, serve};`. Any test that reads `fake.seen_tokens.lock()` switches to `fake.seen_tokens()`.

- [ ] **Step 3: Run the transport tests to prove the move is neutral**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-presence && cargo test -p super-remote --test transport 2>&1 | grep -E "^test |test result"`
Expected: the same three tests as before; `shell_can_be_refused_by_policy` still fails on this Mac (pre-existing), the other two pass.

- [ ] **Step 4: Extract the cursor helpers.** Create `crates/super-remote/src/commands.rs`:

```rust
//! What a command stream reader needs: where to start, when a command is
//! too old to run, and the cursor file that survives a restart.

use crate::s2::{ReadStart, S2Client, SequencedRecord};
use std::path::PathBuf;
use std::time::Duration;

/// The last applied sequence number of one command stream, kept on disk so a
/// restarted reader continues after it instead of re-running recent commands.
pub struct CommandCursor {
    path: Option<PathBuf>,
}

impl CommandCursor {
    /// `dir` defaults to `~/.super/agent/remote/cursors`; the file is named
    /// after the stream with every unsafe character replaced.
    pub fn new(dir: Option<PathBuf>, stream: &str) -> Self {
        let dir = dir.or_else(|| dirs::home_dir().map(|home| home.join(".super/agent/remote/cursors")));
        let name: String = stream
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Self {
            path: dir.map(|dir| dir.join(name)),
        }
    }

    pub fn load(&self) -> Option<u64> {
        let path = self.path.as_ref()?;
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    }

    pub fn store(&self, seq_num: u64) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(error) = std::fs::write(path, seq_num.to_string()) {
            crate::log::line(format!("command cursor not saved: {error}"));
        }
    }
}

/// A command past its `exp` header, or older than `default_expiry` when it
/// has none, must not run.
pub fn is_expired(record: &SequencedRecord, default_expiry: Duration) -> bool {
    let now = crate::now_ms();
    match record
        .header("exp")
        .and_then(|value| value.parse::<i64>().ok())
    {
        Some(exp) => exp < now,
        None => {
            let issued = i64::try_from(record.timestamp).unwrap_or(now);
            issued + default_expiry.as_millis() as i64 <= now
        }
    }
}

/// Where to start reading: after the last applied command when that is still
/// inside the stream; otherwise the recent commands (first start) or only new
/// ones (a cursor past the tail means the stream was recreated).
pub async fn start_position(
    client: &S2Client,
    stream: &str,
    cursor: &CommandCursor,
    lookback: u64,
) -> ReadStart {
    let Some(last) = cursor.load() else {
        return ReadStart::TailOffset(lookback);
    };
    match client.tail(stream).await {
        Ok(tail) if last < tail => ReadStart::SeqNum(last + 1),
        Ok(_) => ReadStart::TailOffset(0),
        Err(error) => {
            crate::log::line(format!("commands tail unknown, using lookback: {error}"));
            ReadStart::TailOffset(lookback)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(timestamp: u64, exp: Option<&str>) -> SequencedRecord {
        SequencedRecord {
            seq_num: 1,
            timestamp,
            headers: exp.map(|exp| vec![("exp".to_string(), exp.to_string())]).unwrap_or_default(),
            body: String::new(),
        }
    }

    #[test]
    fn a_command_expires_by_header_or_by_age() {
        let now = crate::now_ms() as u64;
        assert!(is_expired(&record(now, Some("1")), Duration::from_secs(300)));
        assert!(!is_expired(&record(now, Some(&(now + 60_000).to_string())), Duration::from_secs(300)));
        assert!(!is_expired(&record(now, None), Duration::from_secs(300)));
        assert!(is_expired(&record(now - 400_000, None), Duration::from_secs(300)));
    }

    #[test]
    fn the_cursor_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let cursor = CommandCursor::new(Some(dir.path().to_path_buf()), "hosts/01H/commands");
        assert_eq!(cursor.load(), None);
        cursor.store(41);
        assert_eq!(cursor.load(), Some(41));
        assert!(dir.path().join("hosts_01H_commands").exists());
    }
}
```

In `transport.rs`: delete `struct CommandCursor` and its `impl` (lines 115-160), delete `Tailer::start_position` and `Tailer::is_expired`, and in `Tailer::run` use `let cursor = CommandCursor::new(self.options.cursor_dir.clone(), &self.stream); let start = start_position(&self.client, &self.stream, &cursor, self.options.command_lookback).await;` and `if is_expired(&record, self.options.default_command_expiry)`. Add `use crate::commands::{CommandCursor, is_expired, start_position};`. In `lib.rs` add `pub mod commands;`.

- [ ] **Step 5: Run the unit tests and the transport tests**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-presence && cargo test -p super-remote 2>&1 | grep -E "^test |test result"`
Expected: the two new `commands::tests` pass; everything else as before.

- [ ] **Step 6: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-presence && git add crates/super-remote && git commit -m "remote: fake S2 as a feature, command cursor helpers shared"
```

---

### Task 2: The registry writer holds a record per session

**Files:**
- Modify: `crates/super-remote/src/registry.rs`
- Create: `crates/super-remote/tests/registry.rs`

**Interfaces:**
- Produces: `RegistryWriter::start(client, host, device_id)` (unchanged), `publish(&self, SessionRecord)` (now keyed by `session_id`), `refresh(&self)` (re-sends every record), `stop(self)` (announces `ended` for every record not ended). `SessionRecord` gains `pub daemon: bool` (serde default, skipped when false).

- [ ] **Step 1: Write the failing test.** Create `crates/super-remote/tests/registry.rs`:

```rust
//! The registry writer against the fake S2: one host record, one record per
//! session, nothing re-sent unchanged, and a goodbye for every live session.

use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use super_remote::fake::{FakeS2, TestCredentials, serve};
use super_remote::s2::S2Client;
use super_remote::{HostIdentity, RegistryWriter, SessionRecord, SessionState};

fn record(id: &str, state: SessionState) -> SessionRecord {
    SessionRecord {
        session_id: id.into(),
        workspace: "/w".into(),
        cwd: "/w".into(),
        state,
        blocked_reason: None,
        model: "mock/mock-1".into(),
        thinking_level: "off".into(),
        name: None,
        first_message: None,
        resume_argv: vec!["super".into(), "--session".into(), format!("/s/{id}.jsonl")],
        headless: false,
        daemon: true,
    }
}

fn bodies(fake: &FakeS2) -> Vec<Value> {
    fake.records("registry")
        .iter()
        .map(|record| serde_json::from_str(&record.body).unwrap())
        .collect()
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
async fn one_host_record_then_one_record_per_session() {
    let fake = Arc::new(FakeS2::default());
    let base = serve(fake.clone()).await;
    let client = Arc::new(S2Client::new(reqwest::Client::new(), base, Arc::new(TestCredentials)));
    let host = HostIdentity {
        host_id: "01H".into(),
        name: "mbp".into(),
        platform: "macos".into(),
    };
    let writer = RegistryWriter::start(client, host, "device-1".into());
    writer.publish(record("a", SessionState::Idle));
    writer.publish(record("b", SessionState::Ended));
    writer.publish(record("a", SessionState::Idle)); // unchanged: not re-sent
    writer.publish(record("a", SessionState::Working));
    wait_until(|| bodies(&fake).len() == 4).await;
    let all = bodies(&fake);
    assert_eq!(all[0]["kind"], "host");
    assert_eq!(all[0]["hostId"], "01H");
    let sessions: Vec<(String, String)> = all[1..]
        .iter()
        .map(|b| (b["sessionId"].as_str().unwrap().to_string(), b["state"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(
        sessions,
        vec![("a".into(), "idle".into()), ("b".into(), "ended".into()), ("a".into(), "working".into())]
    );
    assert_eq!(all[1]["daemon"], true);

    writer.refresh();
    wait_until(|| bodies(&fake).len() == 6).await;

    writer.stop().await;
    let all = bodies(&fake);
    // The goodbye: `a` was working and is announced ended; `b` already was.
    let goodbye: Vec<&Value> = all[6..].iter().collect();
    assert_eq!(goodbye.len(), 1, "{all:?}");
    assert_eq!(goodbye[0]["sessionId"], "a");
    assert_eq!(goodbye[0]["state"], "ended");
}
```

Add `reqwest.workspace = true` to `[dev-dependencies]` of `super-remote` (it is already a normal dependency, so the dev entry is only for clarity; skip it if cargo complains about duplication).

- [ ] **Step 2: Run it to see it fail**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-presence && cargo test -p super-remote --test registry 2>&1 | grep -E "^error|^test |test result" | head`
Expected: compile errors: no field `daemon`, no method `refresh`.

- [ ] **Step 3: Rewrite the writer.** In `registry.rs`, add to `SessionRecord` after `headless`:

```rust
    /// Set when the session is hosted by the SUPER daemon.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub daemon: bool,
```

Fix the existing unit test's literal (`daemon: false`). Then replace `RegistryWriter` and its `impl` with:

```rust
enum Update {
    Publish(SessionRecord),
    Refresh,
}

/// Publishes the host's presence and a record per session. Drop or call
/// [`RegistryWriter::stop`] to announce the end.
pub struct RegistryWriter {
    updates: mpsc::UnboundedSender<Update>,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl RegistryWriter {
    /// Start announcing. The host record goes out first, then every change
    /// to any session record, and a heartbeat every 30 seconds.
    pub fn start(client: Arc<S2Client>, host: HostIdentity, device_id: String) -> Self {
        let (updates, mut updates_rx) = mpsc::unbounded_channel::<Update>();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let mut announced_host = false;
            let mut sent: BTreeMap<String, SessionRecord> = BTreeMap::new();
            let mut pending: BTreeMap<String, SessionRecord> = BTreeMap::new();
            loop {
                if !announced_host {
                    let body = HostBody {
                        kind: "host",
                        host_id: &host.host_id,
                        name: &host.name,
                        platform: &host.platform,
                        device_id: &device_id,
                        at: crate::now_ms(),
                    };
                    announced_host = append(&client, "host", &body).await;
                }
                let due: Vec<SessionRecord> = pending.values().cloned().collect();
                for record in due {
                    if sent.get(&record.session_id) == Some(&record) {
                        pending.remove(&record.session_id);
                        continue;
                    }
                    let body = SessionBody {
                        kind: "session",
                        host_id: &host.host_id,
                        session: &record,
                        at: crate::now_ms(),
                    };
                    if append(&client, "session", &body).await {
                        pending.remove(&record.session_id);
                        sent.insert(record.session_id.clone(), record);
                    } else {
                        break;
                    }
                }
                let retry = if pending.is_empty() {
                    HEARTBEAT_INTERVAL
                } else {
                    Duration::from_secs(5)
                };
                tokio::select! {
                    _ = worker_cancel.cancelled() => break,
                    update = updates_rx.recv() => match update {
                        Some(Update::Publish(record)) => {
                            pending.insert(record.session_id.clone(), record);
                        }
                        Some(Update::Refresh) => {
                            for (id, record) in sent.drain() {
                                pending.entry(id).or_insert(record);
                            }
                        }
                        None => break,
                    },
                    _ = tokio::time::sleep(retry) => {
                        if pending.is_empty() {
                            let body = HeartbeatBody { kind: "heartbeat", host_id: &host.host_id, at: crate::now_ms() };
                            append(&client, "heartbeat", &body).await;
                        }
                    }
                }
            }
            // Say goodbye so controllers do not show a stale "working".
            for (_, mut record) in sent {
                if record.state == SessionState::Ended {
                    continue;
                }
                record.state = SessionState::Ended;
                record.blocked_reason = None;
                let body = SessionBody {
                    kind: "session",
                    host_id: &host.host_id,
                    session: &record,
                    at: crate::now_ms(),
                };
                append(&client, "session", &body).await;
            }
        });
        Self {
            updates,
            cancel,
            task: Some(task),
        }
    }

    /// Publish a session's snapshot. An unchanged snapshot is not re-sent.
    pub fn publish(&self, record: SessionRecord) {
        let _ = self.updates.send(Update::Publish(record));
    }

    /// Send every known record again, for readers whose copy expired.
    pub fn refresh(&self) {
        let _ = self.updates.send(Update::Refresh);
    }

    /// Announce the end and wait for the last append.
    pub async fn stop(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
        }
    }
}
```

Replace `use tokio::sync::watch;` with `use tokio::sync::mpsc;` and add `use std::collections::BTreeMap;`. The `Update::Refresh` arm drains `sent` into `pending`, so the loop's next pass re-sends everything (the `sent.get == Some` shortcut no longer matches). A goodbye for an ended `pending` record is not needed.

- [ ] **Step 4: Run the registry test, the crate tests, and the binary's compile**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-presence && cargo test -p super-remote --test registry 2>&1 | grep -E "test result|panicked" && cargo test -p super-remote --lib 2>&1 | grep "test result" && cargo check -p super 2>&1 | grep -E "^error|Finished"`
Expected: the registry test passes; `remote.rs` in the binary compiles unchanged (it uses `publish` and `stop` only; its `session_record` builder needs `daemon: false` added).

- [ ] **Step 5: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-presence && git add crates/super-remote crates/super/src/remote.rs && git commit -m "remote: registry writer keeps a record per session and can refresh them"
```

---

### Task 3: Shared platform credentials in `super-remote`

**Files:**
- Create: `crates/super-remote/src/credentials.rs`
- Modify: `crates/super-remote/src/lib.rs` (`pub mod credentials; pub use credentials::PlatformCredentials;`)
- Modify: `crates/super/src/remote.rs` (delete the private `PlatformCredentials` and its impl; use `super_remote::PlatformCredentials::new(platform_url(settings), host.name.clone())`; `platform_url` stays)

**Interfaces:**
- Produces: `PlatformCredentials::new(url: String, device_name: String) -> Self` implementing `RemoteCredentials`; `pub fn is_signed_in() -> bool`; `pub fn resolve_platform_url(setting: Option<&str>) -> String` (setting, then `SUPER_PLATFORM_URL`, then the default); `pub fn configure_platform_url(setting: Option<&str>)`.

- [ ] **Step 1: Write the module**

```rust
//! The `RemoteCredentials` every signed-in `super` process uses: the stored
//! platform sign-in in `~/.super/agent/auth.json`, refreshed on demand.

use crate::RemoteCredentials;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

pub const PROVIDER_ID: &str = super_ai::auth::super_platform::PROVIDER;

/// Credentials backed by the stored `super` OAuth credential. Token refresh
/// goes through the same path every provider uses, under the auth-file lock.
pub struct PlatformCredentials {
    url: String,
    device_name: String,
}

impl PlatformCredentials {
    pub fn new(url: String, device_name: String) -> Self {
        Self { url, device_name }
    }
}

impl RemoteCredentials for PlatformCredentials {
    fn platform_url(&self) -> String {
        self.url.clone()
    }

    fn device_name(&self) -> String {
        self.device_name.clone()
    }

    fn access_token(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + '_>> {
        Box::pin(async {
            super_ai::auth::resolve_api_key_async(PROVIDER_ID, &BTreeMap::new())
                .await?
                .ok_or_else(|| anyhow::anyhow!("not signed in to SUPER"))
        })
    }
}

/// Is there a stored sign-in for the SUPER platform? No network access.
pub fn is_signed_in() -> bool {
    super_ai::auth::super_platform::is_signed_in()
}

/// Where the platform lives: `SUPER_PLATFORM_URL`, then the setting, then
/// the default.
pub fn resolve_platform_url(setting: Option<&str>) -> String {
    super_ai::auth::super_platform::resolve_base_url(
        setting,
        std::env::var("SUPER_PLATFORM_URL").ok().as_deref(),
    )
}

/// Hand the `platform.url` setting to the auth module, so token refreshes
/// reach the platform the user signed in to.
pub fn configure_platform_url(setting: Option<&str>) {
    super_ai::auth::super_platform::configure_url(setting);
}
```

In `crates/super/src/remote.rs`: delete `struct PlatformCredentials` and its `impl RemoteCredentials` (and the now-unused `BTreeMap`, `Future`, `Pin` imports), keep `pub const PROVIDER_ID` as `pub use super_remote::credentials::PROVIDER_ID;`, and at the construction site write `let credentials: Arc<dyn RemoteCredentials> = Arc::new(super_remote::PlatformCredentials::new(platform_url(settings), host.name.clone()));`.

- [ ] **Step 2: Build and run the binary's remote tests**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-presence && cargo test -p super remote:: 2>&1 | grep -E "test result|^error"`
Expected: the four `remote::tests` pass.

- [ ] **Step 3: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-presence && git add crates/super-remote crates/super/src/remote.rs && git commit -m "remote: platform credentials shared by the CLI and the daemon"
```

---

### Task 4: One place runs host-level commands, for the socket and the platform

**Files:**
- Modify: `crates/super-daemon/src/server.rs`

**Interfaces:**
- Produces: `pub async fn run_host_command(ctx: &Arc<ServerContext>, command: DaemonCommand) -> Result<Value, String>` handling `Status`, `ListSessions`, `ListWorkspaces`, `ListDirectories`, `CreateSession`, `ResumeSession`, `StopSession`; the connection-bound `Hello`, `Attach`, `Detach`, `Shutdown` return `Err("not available over the platform")`. `ServerContext::status` becomes `pub`. `daemon_command` keeps handling the connection-bound ones and delegates the rest.

- [ ] **Step 1: Refactor.** In `server.rs` make `async fn status` `pub`. Replace the body of `daemon_command` so the `match request.command` only has the four connection-bound arms (`Hello`, `Attach`, `Detach`, `Shutdown`, unchanged) and a final arm `other => run_host_command(ctx, other).await`. Add:

```rust
/// The commands that do not depend on a connection. The socket server and
/// the platform link both run them here, so a command means the same thing
/// wherever it came from.
pub async fn run_host_command(
    ctx: &Arc<ServerContext>,
    command: DaemonCommand,
) -> Result<Value, String> {
    match command {
        DaemonCommand::Status {} => {
            serde_json::to_value(ctx.status().await).map_err(|error| error.to_string())
        }
        DaemonCommand::ListSessions { cwd } => {
            let cwd = cwd.map(PathBuf::from);
            Ok(json!({"sessions": ctx.host.list(cwd.as_deref()).await}))
        }
        DaemonCommand::ListWorkspaces {} => {
            Ok(json!({"workspaces": ctx.host.workspaces().await}))
        }
        DaemonCommand::ListDirectories { path } => list_directories(&path)
            .map(|directories| json!({"path": path, "directories": directories})),
        DaemonCommand::CreateSession { cwd, model, provider, thinking_level, name } => ctx
            .host
            .create(CreateRequest { cwd: PathBuf::from(cwd), model, provider, thinking_level, name })
            .await
            .map(|hosted| json!({"session": ctx.host.summary(&hosted)}))
            .map_err(|error| error.to_string()),
        DaemonCommand::ResumeSession { session_id } => ctx
            .host
            .load(&session_id)
            .await
            .map(|hosted| json!({"session": ctx.host.summary(&hosted)}))
            .map_err(|error| error.to_string()),
        DaemonCommand::StopSession { session_id } => ctx
            .host
            .stop(&session_id, EndReason::Stopped)
            .await
            .map(|()| json!({"sessionId": session_id}))
            .map_err(|error| error.to_string()),
        DaemonCommand::Hello { .. }
        | DaemonCommand::Attach { .. }
        | DaemonCommand::Detach {}
        | DaemonCommand::Shutdown {} => Err("not available over the platform".into()),
    }
}
```

- [ ] **Step 2: The end-to-end socket test stays green**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-presence && cargo test -p super-daemon 2>&1 | grep -E "test result|FAILED"`
Expected: all green (14 unit, 7 end-to-end).

- [ ] **Step 3: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-presence && git add crates/super-daemon/src/server.rs && git commit -m "daemon: host-level commands run through one function"
```

---

### Task 5: The platform link

**Files:**
- Modify: `crates/super-daemon/Cargo.toml` (`super-remote.workspace = true`; dev: `super-remote = { workspace = true, features = ["fake"] }`, `reqwest.workspace = true`)
- Create: `crates/super-daemon/src/presence.rs`
- Modify: `crates/super-daemon/src/lib.rs` (`#[cfg(unix)] pub mod presence;`)
- Create: `crates/super-daemon/tests/presence.rs`

**Interfaces:**
- Consumes: `super_remote::{S2Client, RegistryWriter, SessionRecord, SessionTransport, HostIdentity, transport::{SessionStreams, TransportOptions, NoticeSink, event_record}, commands::{CommandCursor, is_expired, start_position}, s2::{AppendRecord, AppendOptions, ReadEvent}}`, `crate::server::{ServerContext, run_host_command}`, `crate::host::SessionHost`.
- Produces: `LinkConfig { allow_shell: bool, cursor_dir: Option<PathBuf>, republish_interval: Duration, command_lookback: u64, default_command_expiry: Duration }` (with `Default`); `Link::start(client: Arc<S2Client>, ctx: Arc<ServerContext>, identity: HostIdentity, device_id: String, config: LinkConfig) -> Link`; `Link::unauthorized(&self) -> bool`; `Link::stop(self)` (async); `record_from_summary(summary: &SessionSummary) -> SessionRecord`; `host_streams(host_id) -> (commands, events)`.

- [ ] **Step 1: Write the failing tests.** Create `crates/super-daemon/tests/presence.rs`:

```rust
//! The platform link against the fake S2 and the mock provider.

#![cfg(unix)]

use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use super_daemon::host::{HostConfig, SessionHost};
use super_daemon::presence::{Link, LinkConfig};
use super_daemon::server::{DaemonInfo, ServerContext};
use super_remote::HostIdentity;
use super_remote::fake::{FakeS2, TestCredentials, serve};
use super_remote::s2::S2Client;
use super_sdk::mock::{MockProvider, MockScript};
use tokio_util::sync::CancellationToken;

struct Fixture {
    _provider: MockProvider,
    dir: tempfile::TempDir,
    fake: Arc<FakeS2>,
    ctx: Arc<ServerContext>,
    link: Link,
}

async fn fixture() -> Fixture {
    let dir = tempfile::Builder::new().prefix("sp").tempdir().unwrap();
    let provider = MockProvider::start(dir.path().join("mock"), MockScript::text("Hello.")).await.unwrap();
    std::fs::create_dir_all(dir.path().join("work")).unwrap();
    let host = SessionHost::new(HostConfig {
        session_dir: dir.path().join("sessions"),
        idle_unload: Duration::ZERO,
        models_file: Some(provider.catalog_path()),
        no_context_files: true,
    });
    // A stored session from before the daemon started.
    let stored = host
        .create(super_daemon::host::CreateRequest {
            cwd: dir.path().join("work"),
            model: Some("mock/mock-1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    stored.session.prompt("old").await.unwrap();
    let stored_id = stored.session_id.clone();
    drop(stored);
    host.stop(&stored_id, super_sdk::daemon::EndReason::Stopped).await.unwrap();

    let ctx = ServerContext::new(
        host,
        DaemonInfo {
            version: "test".into(),
            host_id: "01H".into(),
            host_name: "mbp".into(),
            platform: "macos".into(),
            socket_path: dir.path().join("d.sock"),
        },
        CancellationToken::new(),
    );
    let fake = Arc::new(FakeS2::default());
    let base = serve(fake.clone()).await;
    let client = Arc::new(S2Client::new(reqwest::Client::new(), base, Arc::new(TestCredentials)));
    let identity = HostIdentity { host_id: "01H".into(), name: "mbp".into(), platform: "macos".into() };
    let link = Link::start(
        client,
        ctx.clone(),
        identity,
        "device-1".into(),
        LinkConfig {
            cursor_dir: Some(dir.path().join("cursors")),
            republish_interval: Duration::from_secs(3600),
            ..LinkConfig::default()
        },
    );
    Fixture { _provider: provider, dir, fake, ctx, link }
}

fn bodies(fake: &FakeS2, stream: &str) -> Vec<Value> {
    fake.records(stream).iter().map(|r| serde_json::from_str(&r.body).unwrap()).collect()
}

async fn wait_until(mut check: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met in time");
}

fn command(fake: &FakeS2, id: &str, body: Value) {
    let mut body = body;
    body["id"] = Value::String(id.into());
    fake.append(
        "hosts/01H/commands",
        vec![(
            vec![
                ("t".to_string(), body["type"].as_str().unwrap().to_string()),
                ("from".to_string(), "device-ios".to_string()),
                ("name".to_string(), "iPhone".to_string()),
            ],
            body.to_string(),
        )],
    );
}

#[tokio::test]
async fn the_host_and_every_stored_session_are_announced_at_start() {
    let fixture = fixture().await;
    wait_until(|| bodies(&fixture.fake, "registry").len() >= 2).await;
    let registry = bodies(&fixture.fake, "registry");
    assert_eq!(registry[0]["kind"], "host");
    assert_eq!(registry[0]["name"], "mbp");
    assert_eq!(registry[1]["kind"], "session");
    assert_eq!(registry[1]["state"], "ended");
    assert_eq!(registry[1]["daemon"], true);
    assert_eq!(registry[1]["resumeArgv"][1], "--session");
    fixture.link.stop().await;
}

#[tokio::test]
async fn a_create_session_over_the_platform_is_answered_mirrored_and_registered() {
    let fixture = fixture().await;
    wait_until(|| bodies(&fixture.fake, "registry").len() >= 2).await;
    let work = fixture.dir.path().join("work").display().to_string();
    command(&fixture.fake, "c1", json!({"type": "create_session", "cwd": work, "model": "mock/mock-1"}));

    wait_until(|| bodies(&fixture.fake, "hosts/01H/events").iter().any(|b| b["type"] == "response" && b["id"] == "c1")).await;
    let events = bodies(&fixture.fake, "hosts/01H/events");
    let response = events.iter().find(|b| b["type"] == "response").unwrap();
    assert_eq!(response["success"], true, "{response}");
    let id = response["data"]["session"]["sessionId"].as_str().unwrap().to_string();
    assert!(events.iter().any(|b| b["type"] == "session_created"));

    // The new session is hosted, mirrored, and registered as idle.
    wait_until(|| bodies(&fixture.fake, "registry").iter().any(|b| b["sessionId"] == id && b["state"] == "idle")).await;
    let hosted = fixture.ctx.host.get(&id).expect("hosted");
    hosted.session.prompt("hi").await.unwrap();
    let stream = format!("sessions/01H/{id}/events");
    wait_until(|| bodies(&fixture.fake, &stream).iter().any(|b| b["type"] == "agent_settled")).await;
    wait_until(|| bodies(&fixture.fake, "registry").iter().any(|b| b["sessionId"] == id && b["state"] == "working")).await;
    let entries = format!("sessions/01H/{id}/entries");
    wait_until(|| bodies(&fixture.fake, &entries).len() >= 2).await;

    // A session command over the per-session stream still works and names its origin.
    command(&fixture.fake, "c2", json!({"type": "stop_session", "sessionId": id}));
    wait_until(|| bodies(&fixture.fake, "hosts/01H/events").iter().any(|b| b["type"] == "session_ended" && b["sessionId"] == id)).await;
    wait_until(|| bodies(&fixture.fake, "registry").iter().rev().find(|b| b["sessionId"] == id).is_some_and(|b| b["state"] == "ended")).await;
    assert!(fixture.ctx.host.get(&id).is_none());
    fixture.link.stop().await;
}

#[tokio::test]
async fn connection_bound_and_malformed_commands_are_refused_and_expired_ones_dropped() {
    let fixture = fixture().await;
    wait_until(|| bodies(&fixture.fake, "registry").len() >= 2).await;
    command(&fixture.fake, "c1", json!({"type": "attach", "sessionId": "x"}));
    fixture.fake.append(
        "hosts/01H/commands",
        vec![(vec![("t".into(), "list_sessions".into()), ("exp".into(), "1".into())], json!({"id": "c2", "type": "list_sessions"}).to_string())],
    );
    command(&fixture.fake, "c3", json!({"type": "teleport"}));
    command(&fixture.fake, "c4", json!({"type": "list_workspaces"}));
    wait_until(|| bodies(&fixture.fake, "hosts/01H/events").iter().any(|b| b["id"] == "c4")).await;
    let events = bodies(&fixture.fake, "hosts/01H/events");
    let by_id = |id: &str| events.iter().find(|b| b["id"] == id).cloned();
    assert_eq!(by_id("c1").unwrap()["error"], "not available over the platform");
    assert!(by_id("c2").is_none(), "an expired command is dropped");
    assert_eq!(by_id("c3").unwrap()["success"], false);
    assert!(by_id("c3").unwrap()["error"].as_str().unwrap().contains("teleport"));
    assert_eq!(by_id("c4").unwrap()["data"]["workspaces"][0]["name"], "work");
    fixture.link.stop().await;
}
```

- [ ] **Step 2: Run to see it fail**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-presence && cargo test -p super-daemon --test presence 2>&1 | grep -E "^error" | head -3`
Expected: `super_daemon::presence` not found.

- [ ] **Step 3: Write `presence.rs`**

```rust
//! The daemon's presence on the SUPER platform: the registry, one mirror per
//! hosted session, and the host-level command and event streams.

use crate::host::Hosted;
use crate::log;
use crate::server::{ServerContext, run_host_command};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use super_remote::commands::{CommandCursor, is_expired, start_position};
use super_remote::s2::{AppendOptions, AppendRecord, ReadEvent, S2Client};
use super_remote::transport::{NoticeSink, SessionStreams, TransportOptions, event_record};
use super_remote::{HostIdentity, RegistryWriter, RemoteStatus, SessionRecord, SessionTransport};
use super_sdk::daemon::{ClientLine, DaemonEvent, DaemonRequest, SessionSummary, decode_client_line};
use super_sdk::protocol::Response;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The two host-level streams: `(commands, events)`.
pub fn host_streams(host_id: &str) -> (String, String) {
    (
        format!("hosts/{host_id}/commands"),
        format!("hosts/{host_id}/events"),
    )
}

#[derive(Debug, Clone)]
pub struct LinkConfig {
    pub allow_shell: bool,
    pub cursor_dir: Option<PathBuf>,
    /// Every record is sent again this often, for readers whose copy expired.
    pub republish_interval: Duration,
    pub command_lookback: u64,
    pub default_command_expiry: Duration,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            allow_shell: true,
            cursor_dir: None,
            republish_interval: Duration::from_secs(6 * 3600),
            command_lookback: 50,
            default_command_expiry: Duration::from_secs(5 * 60),
        }
    }
}

/// The registry record for a session as the daemon sees it.
pub fn record_from_summary(summary: &SessionSummary) -> SessionRecord {
    SessionRecord {
        session_id: summary.session_id.clone(),
        workspace: summary.workspace.clone(),
        cwd: summary.cwd.clone(),
        state: summary.state,
        blocked_reason: summary.blocked_reason.clone(),
        model: summary.model.clone().unwrap_or_default(),
        thinking_level: summary.thinking_level.clone().unwrap_or_default(),
        name: summary.name.clone(),
        first_message: summary.first_message.clone(),
        resume_argv: vec!["super".into(), "--session".into(), summary.session_file.clone()],
        headless: false,
        daemon: true,
    }
}

/// One connection of the daemon to the platform.
pub struct Link {
    cancel: CancellationToken,
    registry: Mutex<Option<RegistryWriter>>,
    transports: Arc<Mutex<BTreeMap<String, SessionTransport>>>,
    unauthorized: Arc<AtomicBool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Link {
    pub fn start(
        client: Arc<S2Client>,
        ctx: Arc<ServerContext>,
        identity: HostIdentity,
        device_id: String,
        config: LinkConfig,
    ) -> Self {
        let cancel = CancellationToken::new();
        let registry = RegistryWriter::start(client.clone(), identity.clone(), device_id);
        let (commands_stream, events_stream) = host_streams(&identity.host_id);
        let transports: Arc<Mutex<BTreeMap<String, SessionTransport>>> = Default::default();
        let unauthorized = Arc::new(AtomicBool::new(false));
        let (out, out_rx) = mpsc::unbounded_channel::<AppendRecord>();
        let notices: NoticeSink = Arc::new(|notice| log::debug("presence", format!("{notice:?}")));

        let appender = tokio::spawn(append_host_events(client.clone(), events_stream, out_rx, cancel.clone(), unauthorized.clone()));

        let shared = Arc::new(Shared {
            client: client.clone(),
            ctx: ctx.clone(),
            host_id: identity.host_id.clone(),
            config: config.clone(),
            transports: transports.clone(),
            notices,
            out: out.clone(),
        });
        let follower = tokio::spawn(follow_host(shared.clone(), cancel.clone()));
        let tailer = tokio::spawn(tail_host_commands(shared, commands_stream, cancel.clone(), unauthorized.clone()));

        let link = Self {
            cancel,
            registry: Mutex::new(Some(registry)),
            transports,
            unauthorized,
            tasks: vec![appender, follower, tailer],
        };
        link
    }
}
```

Stop here and restructure: the registry writer must be reachable from the follower task, so it lives in `Shared` too. Write the module as below instead (this is the version to commit):

```rust
struct Shared {
    client: Arc<S2Client>,
    ctx: Arc<ServerContext>,
    host_id: String,
    config: LinkConfig,
    registry: RegistryWriter,
    transports: Mutex<BTreeMap<String, SessionTransport>>,
    /// The newest summary per session, so an ended session can be announced
    /// without listing the directory again.
    summaries: Mutex<BTreeMap<String, SessionSummary>>,
    notices: NoticeSink,
    out: mpsc::UnboundedSender<AppendRecord>,
}

impl Shared {
    fn publish(&self, summary: &SessionSummary) {
        self.summaries
            .lock()
            .unwrap()
            .insert(summary.session_id.clone(), summary.clone());
        self.registry.publish(record_from_summary(summary));
    }

    fn mirror(&self, hosted: &Arc<Hosted>) {
        let mut transports = self.transports.lock().unwrap();
        if transports.contains_key(&hosted.session_id) {
            return;
        }
        let streams = SessionStreams::new(&self.host_id, &hosted.session_id);
        let options = TransportOptions {
            allow_shell: self.config.allow_shell,
            cursor_dir: self.config.cursor_dir.clone(),
            ..TransportOptions::default()
        };
        let transport = SessionTransport::start(
            self.client.clone(),
            hosted.session.clone(),
            streams,
            options,
            self.notices.clone(),
        );
        transports.insert(hosted.session_id.clone(), transport);
        log::info("presence", format!("mirroring session {}", hosted.session_id));
    }

    fn unmirror(&self, session_id: &str) {
        let transport = self.transports.lock().unwrap().remove(session_id);
        if let Some(transport) = transport {
            log::info("presence", format!("mirror of session {session_id} stopped"));
            tokio::spawn(transport.stop());
        }
    }

    fn unauthorized_transport(&self) -> bool {
        self.transports
            .lock()
            .unwrap()
            .values()
            .any(|transport| transport.status() == RemoteStatus::Unauthorized)
    }

    /// Announce every session the daemon knows and mirror the hosted ones.
    async fn sync_all(&self) {
        for summary in self.ctx.host.list(None).await {
            if summary.hosted
                && let Some(hosted) = self.ctx.host.get(&summary.session_id)
            {
                self.mirror(&hosted);
            }
            self.publish(&summary);
        }
    }
}

pub struct Link {
    cancel: CancellationToken,
    shared: Arc<Shared>,
    unauthorized: Arc<AtomicBool>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Link {
    pub fn start(
        client: Arc<S2Client>,
        ctx: Arc<ServerContext>,
        identity: HostIdentity,
        device_id: String,
        config: LinkConfig,
    ) -> Self {
        let cancel = CancellationToken::new();
        let (commands_stream, events_stream) = host_streams(&identity.host_id);
        let (out, out_rx) = mpsc::unbounded_channel::<AppendRecord>();
        let unauthorized = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            client: client.clone(),
            ctx,
            host_id: identity.host_id.clone(),
            config,
            registry: RegistryWriter::start(client.clone(), identity, device_id),
            transports: Mutex::new(BTreeMap::new()),
            summaries: Mutex::new(BTreeMap::new()),
            notices: Arc::new(|notice| log::debug("presence", format!("{notice:?}"))),
            out,
        });
        let tasks = vec![
            tokio::spawn(append_host_events(
                client,
                events_stream,
                out_rx,
                cancel.clone(),
                unauthorized.clone(),
            )),
            tokio::spawn(follow_host(shared.clone(), cancel.clone())),
            tokio::spawn(tail_host_commands(
                shared.clone(),
                commands_stream,
                cancel.clone(),
                unauthorized.clone(),
            )),
        ];
        Self {
            cancel,
            shared,
            unauthorized,
            tasks: Mutex::new(tasks),
        }
    }

    /// The platform rejected the token; a new sign-in is needed.
    pub fn unauthorized(&self) -> bool {
        self.unauthorized.load(Ordering::Relaxed) || self.shared.unauthorized_transport()
    }

    /// Stop tailing, flush every mirror and the registry, announce the end.
    pub async fn stop(self) {
        self.cancel.cancel();
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
        let transports: Vec<SessionTransport> = {
            let mut guard = self.shared.transports.lock().unwrap();
            std::mem::take(&mut *guard).into_values().collect()
        };
        for transport in transports {
            transport.stop().await;
        }
        if let Ok(shared) = Arc::try_unwrap(self.shared) {
            shared.registry.stop().await;
        }
    }
}

/// Mirror the host's daemon events to `hosts/{id}/events`, keep the registry
/// current, and start or stop per-session mirrors as sessions load and unload.
async fn follow_host(shared: Arc<Shared>, cancel: CancellationToken) {
    let mut events = shared.ctx.host.subscribe();
    shared.sync_all().await;
    let mut republish = tokio::time::interval(shared.config.republish_interval);
    republish.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    republish.tick().await; // the first tick fires at once
    loop {
        let event = tokio::select! {
            _ = cancel.cancelled() => break,
            _ = republish.tick() => {
                shared.sync_all().await;
                shared.registry.refresh();
                continue;
            }
            event = events.recv() => event,
        };
        let event = match event {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                shared.sync_all().await;
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };
        if let Ok(value) = serde_json::to_value(&event) {
            let _ = shared.out.send(event_record(value));
        }
        match &event {
            DaemonEvent::SessionCreated { session } | DaemonEvent::SessionUpdated { session } => {
                if session.hosted
                    && let Some(hosted) = shared.ctx.host.get(&session.session_id)
                {
                    shared.mirror(&hosted);
                }
                shared.publish(session);
            }
            DaemonEvent::SessionEnded { session_id, .. } => {
                shared.unmirror(session_id);
                let ended = shared.summaries.lock().unwrap().get(session_id).cloned();
                if let Some(mut summary) = ended {
                    summary.state = super_sdk::daemon::SessionState::Ended;
                    summary.blocked_reason = None;
                    summary.hosted = false;
                    summary.attached_clients = 0;
                    shared.publish(&summary);
                }
            }
            DaemonEvent::DaemonShutdown { .. } => {}
        }
    }
}

/// Append host events in order; retry with backoff while the platform is
/// unreachable; note a rejected token.
async fn append_host_events(
    client: Arc<S2Client>,
    stream: String,
    mut queue: mpsc::UnboundedReceiver<AppendRecord>,
    cancel: CancellationToken,
    unauthorized: Arc<AtomicBool>,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let record = tokio::select! {
            _ = cancel.cancelled() => break,
            record = queue.recv() => match record { Some(record) => record, None => break },
        };
        loop {
            match client
                .append(&stream, std::slice::from_ref(&record), &AppendOptions::default())
                .await
            {
                Ok(_) => {
                    backoff = Duration::from_secs(1);
                    break;
                }
                Err(error) => {
                    if matches!(error, super_remote::s2::S2Error::Unauthorized) {
                        unauthorized.store(true, Ordering::Relaxed);
                    }
                    log::warn("presence", format!("host event append failed: {error}"));
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }
}

/// Run the commands controllers append to `hosts/{id}/commands`.
async fn tail_host_commands(
    shared: Arc<Shared>,
    stream: String,
    cancel: CancellationToken,
    unauthorized: Arc<AtomicBool>,
) {
    let cursor = CommandCursor::new(shared.config.cursor_dir.clone(), &stream);
    let start = start_position(&shared.client, &stream, &cursor, shared.config.command_lookback).await;
    let mut reader = shared.client.tail_stream(stream.clone(), start, cancel.clone());
    while let Some(event) = reader.recv().await {
        match event {
            ReadEvent::Connected => log::info("presence", format!("tailing {stream}")),
            ReadEvent::Ping { .. } | ReadEvent::Disconnected(_) => {}
            ReadEvent::Failed(reason) => {
                log::warn("presence", format!("host commands tail failed: {reason}"));
                if reason.contains("401") {
                    unauthorized.store(true, Ordering::Relaxed);
                }
                break;
            }
            ReadEvent::Records(records) => {
                for record in records {
                    let seq_num = record.seq_num;
                    if !is_expired(&record, shared.config.default_command_expiry) {
                        run_host_record(&shared, &record).await;
                    }
                    cursor.store(seq_num);
                }
            }
        }
    }
}

async fn run_host_record(shared: &Arc<Shared>, record: &super_remote::s2::SequencedRecord) {
    let origin = record
        .header("name")
        .or_else(|| record.header("from"))
        .unwrap_or("remote")
        .to_string();
    let response = match decode_client_line(&record.body) {
        Err(error) => Response::err("parse", error.to_string()),
        Ok(ClientLine::Session(request)) => {
            Response::err(request.command.name(), "not available over the platform").with_id(request.id)
        }
        Ok(ClientLine::Daemon(DaemonRequest { id, command })) => {
            let name = command.name();
            log::info("presence", format!("{origin}: {name}"));
            match run_host_command(&shared.ctx, command).await {
                Ok(data) => Response::ok_data(name, data).with_id(id),
                Err(error) => Response::err(name, error).with_id(id),
            }
        }
    };
    let value = serde_json::to_value(response).unwrap_or(Value::Null);
    let _ = shared.out.send(event_record(value));
}
```

Add `#[cfg(unix)] pub mod presence;` to `lib.rs`. `RemoteStatus` must be importable (`super_remote::RemoteStatus` is re-exported). `SessionTransport::status()` exists.

- [ ] **Step 4: Run the presence tests until green, then the whole crate**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-presence && cargo test -p super-daemon --test presence 2>&1 | grep -E "^test |test result|panicked|^error" | head -20`
Expected: 3 tests pass. The per-session `stop_session` over `hosts/01H/commands` goes through `run_host_command`, so the session unloads and `session_ended` reaches the host events stream; the registry's last record for the id is `ended`.

- [ ] **Step 5: Clippy, then commit**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-presence && cargo clippy -p super-daemon --all-targets -- -D warnings 2>&1 | grep -E "^(error|warning)" -A 6 | head -30`

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-presence && git add crates/super-daemon Cargo.lock && git commit -m "daemon: platform link with registry, per-session mirrors and host-level streams"
```

---

### Task 6: The supervisor, `run` wiring, settings

**Files:**
- Modify: `crates/super-daemon/src/presence.rs` (`PresenceConfig`, `Presence`, `next_action`, `Action`)
- Modify: `crates/super-daemon/src/lib.rs` (`DaemonConfig.presence: Option<PresenceConfig>`, start and stop in `run`)
- Modify: `crates/super-daemon/tests/socket_end_to_end.rs` (`presence: None` in both configs)
- Modify: `crates/super/src/daemon_cli.rs` (build `PresenceConfig` from settings)

**Interfaces:**
- Produces: `PresenceConfig { platform_url: String, device_name: String, check_interval: Duration, link: LinkConfig }`; `Presence::supervise(ctx: Arc<ServerContext>, identity: HostIdentity, config: PresenceConfig) -> Presence`; `Presence::stop(self)` (async); `pub enum Action { Start, Stop, Keep }`; `pub fn next_action(signed_in: bool, running: bool, unauthorized: bool) -> Action`.

- [ ] **Step 1: Write the failing unit test** at the bottom of `presence.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_supervisor_starts_when_signed_in_and_stops_when_not() {
        assert_eq!(next_action(true, false, false), Action::Start);
        assert_eq!(next_action(true, true, false), Action::Keep);
        assert_eq!(next_action(true, true, true), Action::Stop);
        assert_eq!(next_action(false, true, false), Action::Stop);
        assert_eq!(next_action(false, false, false), Action::Keep);
    }

    #[test]
    fn a_record_names_the_session_file_the_apps_expect() {
        let summary = SessionSummary {
            session_id: "a1".into(),
            cwd: "/w".into(),
            workspace: "/w".into(),
            session_file: "/s/a1.jsonl".into(),
            name: None,
            first_message: Some("hi".into()),
            state: super_sdk::daemon::SessionState::Idle,
            blocked_reason: None,
            hosted: true,
            attached_clients: 1,
            model: Some("mock/mock-1".into()),
            thinking_level: Some("off".into()),
            modified_at: "2026-10-29T00:00:00Z".into(),
        };
        let record = record_from_summary(&summary);
        assert_eq!(record.resume_argv, ["super", "--session", "/s/a1.jsonl"]);
        assert!(record.daemon);
        assert!(!record.headless);
        assert_eq!(record.model, "mock/mock-1");
    }
}
```

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-presence && cargo test -p super-daemon presence:: 2>&1 | grep -E "^error" | head -3`
Expected: `next_action`, `Action` not found.

- [ ] **Step 2: Add the supervisor** to `presence.rs`:

```rust
#[derive(Debug, Clone)]
pub struct PresenceConfig {
    pub platform_url: String,
    pub device_name: String,
    /// How often the stored sign-in is checked.
    pub check_interval: Duration,
    pub link: LinkConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Keep,
}

/// What the supervisor does on one check.
pub fn next_action(signed_in: bool, running: bool, unauthorized: bool) -> Action {
    match (signed_in, running) {
        (true, false) => Action::Start,
        (true, true) if unauthorized => Action::Stop,
        (true, true) => Action::Keep,
        (false, true) => Action::Stop,
        (false, false) => Action::Keep,
    }
}

/// Keeps one link up while the user is signed in.
pub struct Presence {
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Presence {
    pub fn supervise(ctx: Arc<ServerContext>, identity: HostIdentity, config: PresenceConfig) -> Self {
        let cancel = CancellationToken::new();
        let task = tokio::spawn(supervise(ctx, identity, config, cancel.clone()));
        Self { cancel, task }
    }

    pub async fn stop(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(10), self.task).await;
    }
}

async fn supervise(
    ctx: Arc<ServerContext>,
    identity: HostIdentity,
    config: PresenceConfig,
    cancel: CancellationToken,
) {
    let mut link: Option<Link> = None;
    loop {
        let signed_in = super_remote::credentials::is_signed_in();
        let unauthorized = link.as_ref().is_some_and(Link::unauthorized);
        match next_action(signed_in, link.is_some(), unauthorized) {
            Action::Start => {
                let credentials: Arc<dyn super_remote::RemoteCredentials> =
                    Arc::new(super_remote::PlatformCredentials::new(
                        config.platform_url.clone(),
                        config.device_name.clone(),
                    ));
                match super_remote::Remote::start(credentials, identity.clone()).await {
                    Ok(remote) => {
                        log::info(
                            "presence",
                            format!("connected to {} as device {}", config.platform_url, remote.config.device_id),
                        );
                        link = Some(Link::start(
                            remote.client.clone(),
                            ctx.clone(),
                            identity.clone(),
                            remote.config.device_id.clone(),
                            config.link.clone(),
                        ));
                    }
                    Err(error) => log::warn("presence", format!("platform unavailable: {error:#}")),
                }
            }
            Action::Stop => {
                log::info(
                    "presence",
                    if unauthorized { "sign-in rejected, presence stopped" } else { "signed out, presence stopped" },
                );
                if let Some(link) = link.take() {
                    link.stop().await;
                }
            }
            Action::Keep => {}
        }
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(config.check_interval) => {}
        }
    }
    if let Some(link) = link.take() {
        link.stop().await;
    }
}
```

- [ ] **Step 3: Wire `run`.** In `lib.rs` add `pub presence: Option<presence::PresenceConfig>` to `DaemonConfig` under `#[cfg(unix)]`? No: keep the struct the same on every platform by declaring the type in a platform-neutral way. Move `PresenceConfig` and `LinkConfig` into a small `#[cfg(unix)]`-free position: define them in `presence.rs` but make the module `pub mod presence;` on every platform with the link and supervisor types under `#[cfg(unix)]` inside the file (`#[cfg(unix)] mod link;` is overkill; put `#[cfg(unix)]` on `Link`, `Shared`, `Presence`, the free async functions and the `use` lines they need). Simpler and what this plan does: keep `presence.rs` unix-only and in `lib.rs` declare

```rust
#[cfg(unix)]
pub mod presence;

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    ...
    /// The platform link; `None` keeps the daemon local (tests, `remote.enabled = false`).
    #[cfg(unix)]
    pub presence: Option<presence::PresenceConfig>,
}
```

(the binary only constructs `DaemonConfig` inside its `cfg(unix)` `run_daemon`). In `run`, after `install_signal_handlers`:

```rust
    let presence = config.presence.clone().map(|presence_config| {
        super_remote::credentials::configure_platform_url(Some(&presence_config.platform_url));
        presence::Presence::supervise(
            ctx.clone(),
            super_remote::HostIdentity {
                host_id: config.host_id.clone(),
                name: config.host_name.clone(),
                platform: std::env::consts::OS.to_string(),
            },
            presence_config,
        )
    });
```

and in the shutdown sequence, after `host.shutdown().await` and the 200 ms sleep: `if let Some(presence) = presence { presence.stop().await; }`. `configure_platform_url` takes the resolved url, which is what the binary passes.

Both `DaemonConfig` literals in `tests/socket_end_to_end.rs` get `presence: None,`.

- [ ] **Step 4: The binary.** In `daemon_cli.rs::run_daemon`, after `let settings = ...`:

```rust
    let presence = settings.remote.is_enabled().then(|| super_daemon::presence::PresenceConfig {
        platform_url: super_remote::credentials::resolve_platform_url(settings.platform.url.as_deref()),
        device_name: host.name.clone(),
        check_interval: Duration::from_secs(30),
        link: super_daemon::presence::LinkConfig {
            allow_shell: settings.remote.shell_allowed(),
            ..Default::default()
        },
    });
```

(placed after `host` is resolved; `host.name` is cloned before `host.host_id`/`host.name` move into `DaemonConfig`) and `presence,` in the `DaemonConfig` literal.

- [ ] **Step 5: Run everything in the crate and the binary tests**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-presence && cargo test -p super-daemon 2>&1 | grep -E "test result|FAILED" && cargo test -p super --test daemon_cli 2>&1 | grep -E "test result|FAILED"`
Expected: all green. The binary tests run with `HOME` in a temp dir, so `is_signed_in()` is false and no link starts.

- [ ] **Step 6: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-presence && git add crates/super-daemon crates/super/src/daemon_cli.rs && git commit -m "daemon: presence supervisor follows the platform sign-in"
```

---

### Task 7: Documentation

**Files:**
- Modify: `docs/daemon.md` (a "Through the platform" section), `docs/super-platform.md` (one paragraph under "For other code in the workspace" naming `super_remote::credentials`), `CHANGELOG.md`

- [ ] **Step 1: `docs/daemon.md`**, append:

```markdown
## Through the platform

While the user is signed in to the SUPER platform (and `remote.enabled` is
not false), the daemon keeps a link to it, checked every 30 seconds:

- `registry`: one `host` record when the link starts, a heartbeat every
  30 seconds, and one `session` record per session the daemon knows, hosted
  ones with their live state and stored ones as `ended`. Every record is
  sent again every six hours. Daemon sessions carry `"daemon": true` and
  `"resumeArgv": ["super", "--session", <file>]`.
- `sessions/{hostId}/{sessionId}/{events,entries,commands}`: every hosted
  session is mirrored exactly like a terminal session, from the moment it
  loads until it unloads.
- `hosts/{hostId}/commands`: controllers append daemon-level commands here
  with the same headers as session commands (`t`, `from`, `name`, `exp`).
  `status`, `list_sessions`, `list_workspaces`, `list_directories`,
  `create_session`, `resume_session` and `stop_session` run exactly as over
  the socket; `hello`, `attach`, `detach` and `shutdown` answer
  `not available over the platform`. Expired commands are dropped and the
  cursor in `~/.super/agent/remote/cursors/` keeps a restarted daemon from
  running recent commands again.
- `hosts/{hostId}/events`: the response to every host command (with its
  `id`) and every daemon-level event (`session_created`, `session_updated`,
  `session_ended`, `daemon_shutdown`).

Signing out, or a token the platform rejects, stops the link; signing in
again starts it within 30 seconds. The daemon never rewrites an event body,
so a platform error inside a model response (`budget_exceeded`,
`model_not_available`) reaches every client as it is.
```

- [ ] **Step 2: `docs/super-platform.md`**: in "For other code in the workspace" add the bullet `` `super_remote::credentials` — `PlatformCredentials` (the `RemoteCredentials` backed by the stored sign-in), `is_signed_in()`, `resolve_platform_url(setting)` and `configure_platform_url(setting)`, shared by the CLI and the daemon. `` **`CHANGELOG.md`** under Unreleased / Added: `` - The daemon is present on the platform while signed in: host and session records in the registry (stored sessions as `ended`, `daemon: true`), one mirror per hosted session, and the host-level streams `hosts/{hostId}/commands` and `hosts/{hostId}/events` through which an app starts, resumes, stops and lists sessions on a device (ENG-37). ``

- [ ] **Step 3: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-presence && git add docs CHANGELOG.md && git commit -m "docs: the daemon through the platform"
```

---

### Task 8: Verification and the pull request

- [ ] **Step 1:** `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast` (in the background; the only expected failure is the pre-existing `super-remote` `shell_can_be_refused_by_policy`).
- [ ] **Step 2: Manual run against the real platform**, signed in on this Mac: `cargo build -p super`, `target/debug/super daemon run --socket /tmp/sd-presence.sock` outside the sandbox, then `super daemon status`, check `~/.super/agent/logs/daemon.log` for `connected to https://super.flxkpe.io as device …` and `tailing hosts/…/commands`, and stop it. (The platform refuses host-command appends from the apps until the super-platform PR lands; the daemon's own appends are from a `cli` device and are allowed today.)
- [ ] **Step 3:** push `daemon-presence`, `gh pr create` with a body in the scratchpad (what, streams, tests, the platform PR dependency, out of scope), bind the PR, comment on ENG-37.
