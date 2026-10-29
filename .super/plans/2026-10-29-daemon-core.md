# Daemon core (ENG-36) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use super:subagent-driven-development (recommended) or super:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `super daemon run` hosts any number of SUPER sessions in one long-lived process and serves them to local clients over a Unix socket, with `super daemon status|stop` as thin clients.

**Architecture:** A new library crate `super-daemon` holds the runtime (lock, log, socket server, session host); the wire types live in `super_sdk::daemon` beside the existing RPC protocol; the `super` binary gains the `daemon` subcommand. Each hosted session is an `Arc<super_sdk::Session>`; after `hello` and `attach` a connection carries the unchanged session RPC, daemon-level commands work on any connection, daemon-level events reach every connection.

**Tech Stack:** Rust 2024 edition, tokio (`net`, `signal`, `sync`), `fs2` file locks, serde/serde_json, `super-sdk` (with the `mock` feature in tests), `super-coding` session files.

Spec: `.super/specs/2026-10-29-daemon-design.md`. Linear: ENG-36 (sub-issue of ENG-5).

## Global Constraints

- Workspace version `0.0.37`; new crate inherits `version.workspace = true`, edition 2024, MIT, `[lints] workspace = true`.
- CI runs `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo nextest run --workspace` on ubuntu, macos and windows. Unix-socket code is `#[cfg(unix)]`; on Windows `super daemon run` prints "the SUPER daemon needs a Unix socket and is not available on Windows" and exits 1.
- Socket `~/.super/run/daemon.sock` (directory 0700, socket 0600, peer uid must equal the daemon's uid); lock `~/.super/run/daemon.lock` with the pid inside; log `~/.super/agent/logs/daemon.log` rotated at 5 MiB keeping `daemon.log.1`; never log tokens, prompts or record bodies.
- Exit 0 only on `shutdown`, SIGTERM or SIGINT; every failure exits 1.
- `PROTOCOL_VERSIONS = [1, 1]`; `hello` must be the first line; error text for no overlap: `this SUPER daemon speaks protocol <range>; this client speaks <range>` where a range is `1` or `2 to 3`.
- Idle unload: `daemon.idleUnloadMinutes` in `~/.super/agent/settings.json`, default 60, `0` disables; only `idle` sessions with no attached client are unloaded.
- Commit messages: plain `area: what changed` lines, no attribution trailers. Branch `daemon-core`.
- Use absolute paths in shell commands; the shell's working directory flips between checkouts. The worktree is `/Users/chwzr/flxkpe/harness/super-daemon-core`.

---

## File structure

| Path | Responsibility |
| --- | --- |
| `crates/super-sdk/src/daemon.rs` (new) | Wire types: `PROTOCOL_VERSIONS`, `SessionState`, `ClientKind`, `EndReason`, `SessionSummary`, `WorkspaceSummary`, `DirectoryEntry`, `HelloReply`, `DaemonStatus`, `DaemonCommand`, `DaemonRequest`, `DaemonEvent`, `ClientLine`, `decode_client_line`, `negotiate`, `describe_range` |
| `crates/super-sdk/src/lib.rs` | `pub mod daemon;` |
| `crates/super-sdk/src/session.rs` | `install_workflow_approval_gate` (moved from `modes/rpc.rs`) |
| `crates/super-remote/src/registry.rs` | `SessionState` becomes a re-export of the SDK type |
| `crates/super-daemon/Cargo.toml` (new) | the crate |
| `crates/super-daemon/src/lib.rs` (new) | `DaemonConfig`, `run` |
| `crates/super-daemon/src/paths.rs` (new) | `DaemonPaths` |
| `crates/super-daemon/src/lock.rs` (new) | `DaemonLock`, `LockError` |
| `crates/super-daemon/src/log.rs` (new) | `Logger`, `Level`, global `init/info/warn/error/debug` |
| `crates/super-daemon/src/env.rs` (new) | `login_shell_path`, `merge_paths`, `apply_login_shell_path` |
| `crates/super-daemon/src/host.rs` (new) | `HostConfig`, `CreateRequest`, `HostError`, `Hosted`, `SessionHost`, `next_state` |
| `crates/super-daemon/src/server.rs` (new, unix) | `DaemonInfo`, `ServerContext`, `serve` |
| `crates/super-daemon/src/client.rs` (new, unix) | `DaemonClient` |
| `crates/super-daemon/tests/socket_end_to_end.rs` (new) | the acceptance test |
| `crates/super-coding/src/settings.rs` | `DaemonSettings` |
| `crates/super/src/args.rs` | `Command::Daemon`, `DaemonSubcommand` |
| `crates/super/src/daemon_cli.rs` (new) | `run`, `status`, `stop` |
| `crates/super/src/main.rs` | dispatch, PATH resolution before the runtime |
| `crates/super/src/modes/rpc.rs` | use the SDK's approval gate |
| `crates/super/tests/daemon_cli.rs` (new) | binary tests |
| `docs/daemon.md` (new), `docs/rpc.md`, `README.md`, `CHANGELOG.md` | documentation |

---

### Task 1: Wire types in `super_sdk::daemon`

**Files:**
- Create: `crates/super-sdk/src/daemon.rs`
- Modify: `crates/super-sdk/src/lib.rs` (add `pub mod daemon;` after `pub mod protocol;`)
- Modify: `crates/super-remote/src/registry.rs:15-33` (replace the `SessionState` enum and its impl with a re-export)

**Interfaces:**
- Consumes: `crate::protocol::{ProtocolError, Request, decode_request}`.
- Produces: everything in the file structure row for `daemon.rs`. Later tasks use `DaemonCommand::{Hello, Status, ListSessions, ListWorkspaces, ListDirectories, CreateSession, ResumeSession, Attach, Detach, StopSession, Shutdown}`, `DaemonEvent::{SessionCreated, SessionUpdated, SessionEnded, DaemonShutdown}`, `SessionSummary` fields exactly as below, `decode_client_line(&str) -> Result<ClientLine, ProtocolError>`, `negotiate([u32; 2], [u32; 2]) -> Result<u32, String>`.

- [ ] **Step 1: Write the failing tests** at the bottom of the new file `crates/super-sdk/src/daemon.rs` (the module does not compile yet, so first create the file with only the tests and the `use` lines; the test run in step 2 fails to compile, which is the expected failure)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hello_round_trips_in_camel_case() {
        let line = r#"{"id":"1","type":"hello","client":"tui","name":"Terminal","version":"0.0.38","protocolVersions":[1,1]}"#;
        let ClientLine::Daemon(request) = decode_client_line(line).expect("decodes") else {
            panic!("hello is a daemon command");
        };
        assert_eq!(request.id.as_deref(), Some("1"));
        assert_eq!(
            request.command,
            DaemonCommand::Hello {
                client: ClientKind::Tui,
                name: Some("Terminal".into()),
                version: "0.0.38".into(),
                protocol_versions: [1, 1],
            }
        );
        let encoded = serde_json::to_value(&request.command).unwrap();
        assert_eq!(encoded["type"], "hello");
        assert_eq!(encoded["protocolVersions"], json!([1, 1]));
    }

    #[test]
    fn a_session_command_is_routed_to_the_session_decoder() {
        let line = r#"{"id":"7","type":"prompt","message":"hi"}"#;
        match decode_client_line(line).expect("decodes") {
            ClientLine::Session(request) => {
                assert_eq!(request.id.as_deref(), Some("7"));
                assert_eq!(request.command.name(), "prompt");
            }
            ClientLine::Daemon(_) => panic!("prompt is a session command"),
        }
    }

    #[test]
    fn an_unknown_type_names_itself() {
        let error = decode_client_line(r#"{"type":"teleport"}"#).expect_err("unknown");
        assert!(error.to_string().contains("teleport"), "{error}");
        assert!(matches!(
            decode_client_line(r#"{"id":"1"}"#),
            Err(ProtocolError::MissingType)
        ));
    }

    #[test]
    fn negotiation_picks_the_highest_shared_version() {
        assert_eq!(negotiate([1, 2], [1, 1]), Ok(1));
        assert_eq!(negotiate([1, 3], [2, 5]), Ok(3));
        assert_eq!(negotiate([1, 1], [1, 1]), Ok(1));
    }

    #[test]
    fn no_overlap_names_both_ranges() {
        let error = negotiate([1, 1], [2, 3]).expect_err("no overlap");
        assert_eq!(
            error,
            "this SUPER daemon speaks protocol 2 to 3; this client speaks 1"
        );
        assert_eq!(describe_range([4, 4]), "4");
    }

    #[test]
    fn events_carry_their_reason_and_summary() {
        let ended = DaemonEvent::SessionEnded {
            session_id: "a1b2".into(),
            reason: EndReason::Idle,
        };
        let value: Value = serde_json::from_str(&ended.to_line()).unwrap();
        assert_eq!(value["type"], "session_ended");
        assert_eq!(value["sessionId"], "a1b2");
        assert_eq!(value["reason"], "idle");
        assert_eq!(ended.event_type(), "session_ended");
        assert_eq!(SessionState::Blocked.as_str(), "blocked");
        assert_eq!(
            serde_json::to_value(SessionState::Ended).unwrap(),
            json!("ended")
        );
    }
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-agent-sdk daemon::`
Expected: compile error, `ClientLine`, `decode_client_line`, `negotiate` not found (after adding `pub mod daemon;` to `lib.rs`).

- [ ] **Step 3: Write the module** (replace the top of `crates/super-sdk/src/daemon.rs`, keeping the tests)

```rust
//! The daemon protocol: what a client of `super daemon` says over the local
//! socket besides the session commands in [`crate::protocol`].
//!
//! Like [`crate::protocol`] this module is pure serde, so every SDK surface
//! and the WebAssembly build see one definition. The user-facing description
//! is `docs/daemon.md`.

use crate::protocol::{ProtocolError, Request, decode_request};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol versions this build speaks, as `[min, max]`.
///
/// Additive changes (new commands, new optional fields, new events) do not
/// bump the version. Renaming or removing a command, or changing what one
/// means, bumps `max`; `min` follows one release later.
pub const PROTOCOL_VERSIONS: [u32; 2] = [1, 1];

/// What a client sees of a session's life. `Ended` means "not loaded in the
/// daemon right now", never "gone": every ended session can be resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Idle,
    Working,
    Blocked,
    Ended,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Ended => "ended",
        }
    }
}

/// Which kind of program a client is. Shown in `super daemon status` and in
/// the log; `hello.name` is what other clients see as the origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Tui,
    Cli,
    Macos,
    Ios,
    Other,
}

impl ClientKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tui => "tui",
            Self::Cli => "cli",
            Self::Macos => "macos",
            Self::Ios => "ios",
            Self::Other => "other",
        }
    }
}

/// Why a hosted session was unloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Stopped,
    Idle,
    Shutdown,
}

/// One session as the daemon lists it: hosted or stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_id: String,
    pub cwd: String,
    /// The same as `cwd` until workspaces mean more than a directory.
    pub workspace: String,
    pub session_file: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub first_message: Option<String>,
    pub state: SessionState,
    #[serde(default)]
    pub blocked_reason: Option<String>,
    pub hosted: bool,
    pub attached_clients: usize,
    /// `provider/model`; null for a stored session.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking_level: Option<String>,
    /// RFC 3339, seconds, UTC.
    pub modified_at: String,
}

/// A directory with sessions, derived from the sessions' working directories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSummary {
    pub path: String,
    pub name: String,
    pub session_count: usize,
    pub latest_state: SessionState,
    pub latest_modified_at: String,
}

/// One entry of `list_directories`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
}

/// The daemon's answer to `hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloReply {
    pub protocol_version: u32,
    pub daemon_version: String,
    pub host_id: String,
    pub host_name: String,
    pub platform: String,
    pub pid: u32,
    pub started_at: String,
}

/// The daemon's answer to `status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    pub daemon_version: String,
    pub protocol_versions: [u32; 2],
    pub host_id: String,
    pub host_name: String,
    pub platform: String,
    pub pid: u32,
    pub started_at: String,
    pub uptime_seconds: u64,
    pub hosted_sessions: usize,
    pub stored_sessions: usize,
    pub clients: usize,
    pub socket_path: String,
    pub session_dir: String,
}

/// Every daemon-level command. Session commands are [`crate::protocol::Command`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonCommand {
    #[serde(rename_all = "camelCase")]
    Hello {
        client: ClientKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        version: String,
        protocol_versions: [u32; 2],
    },
    Status {},
    #[serde(rename_all = "camelCase")]
    ListSessions {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    ListWorkspaces {},
    ListDirectories {
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    CreateSession {
        cwd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking_level: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ResumeSession {
        session_id: String,
    },
    #[serde(rename_all = "camelCase")]
    Attach {
        session_id: String,
    },
    Detach {},
    #[serde(rename_all = "camelCase")]
    StopSession {
        session_id: String,
    },
    Shutdown {},
}

impl DaemonCommand {
    /// The snake_case name echoed as `command` on the response.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::Status {} => "status",
            Self::ListSessions { .. } => "list_sessions",
            Self::ListWorkspaces {} => "list_workspaces",
            Self::ListDirectories { .. } => "list_directories",
            Self::CreateSession { .. } => "create_session",
            Self::ResumeSession { .. } => "resume_session",
            Self::Attach { .. } => "attach",
            Self::Detach {} => "detach",
            Self::StopSession { .. } => "stop_session",
            Self::Shutdown {} => "shutdown",
        }
    }
}

/// One decoded daemon-level line: the command plus the client's optional id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(flatten)]
    pub command: DaemonCommand,
}

/// Events the daemon sends to every connection, attached or not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    SessionCreated {
        session: SessionSummary,
    },
    SessionUpdated {
        session: SessionSummary,
    },
    #[serde(rename_all = "camelCase")]
    SessionEnded {
        session_id: String,
        reason: EndReason,
    },
    DaemonShutdown {
        reason: String,
    },
}

impl DaemonEvent {
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::SessionCreated { .. } => "session_created",
            Self::SessionUpdated { .. } => "session_updated",
            Self::SessionEnded { .. } => "session_ended",
            Self::DaemonShutdown { .. } => "daemon_shutdown",
        }
    }

    /// One JSON line without the trailing newline.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// A line a client sent, sorted into the two command sets.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientLine {
    Daemon(DaemonRequest),
    Session(Request),
}

/// Decode one client line. Daemon-level types win; anything else goes to the
/// session decoder, whose error names an unknown type.
pub fn decode_client_line(line: &str) -> Result<ClientLine, ProtocolError> {
    let value: Value = serde_json::from_str(line)?;
    if value.get("type").and_then(Value::as_str).is_none() {
        return Err(ProtocolError::MissingType);
    }
    match serde_json::from_value::<DaemonRequest>(value) {
        Ok(request) => Ok(ClientLine::Daemon(request)),
        Err(error) if error.to_string().contains("unknown variant") => {
            decode_request(line).map(ClientLine::Session)
        }
        Err(error) => Err(ProtocolError::Json(error)),
    }
}

/// Pick the highest version both sides speak, or explain why there is none.
pub fn negotiate(client: [u32; 2], daemon: [u32; 2]) -> Result<u32, String> {
    let low = client[0].max(daemon[0]);
    let high = client[1].min(daemon[1]);
    if low <= high {
        Ok(high)
    } else {
        Err(format!(
            "this SUPER daemon speaks protocol {}; this client speaks {}",
            describe_range(daemon),
            describe_range(client)
        ))
    }
}

/// `1` or `2 to 3`.
pub fn describe_range(range: [u32; 2]) -> String {
    if range[0] == range[1] {
        range[0].to_string()
    } else {
        format!("{} to {}", range[0], range[1])
    }
}
```

- [ ] **Step 4: Re-export the state type from `super-remote`**. In `crates/super-remote/src/registry.rs` delete the `SessionState` enum (lines 15-33, the `#[derive ...]`, the enum and its `impl SessionState { pub fn as_str ... }`) and put in its place:

```rust
pub use super_sdk::daemon::SessionState;
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-agent-sdk daemon:: && cargo test -p super-remote && cargo check -p super`
Expected: the six `daemon::tests` pass; `super-remote` and `super` still compile (the registry test `session_records_serialize_in_the_documented_shape` still passes).

- [ ] **Step 6: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && git add crates/super-sdk/src/daemon.rs crates/super-sdk/src/lib.rs crates/super-remote/src/registry.rs && git commit -m "sdk: daemon protocol types, handshake negotiation and shared session state"
```

---

### Task 2: Crate scaffold, paths and the single-instance lock

**Files:**
- Modify: `Cargo.toml` (workspace `members` and `[workspace.dependencies]`)
- Create: `crates/super-daemon/Cargo.toml`, `crates/super-daemon/src/lib.rs`, `crates/super-daemon/src/paths.rs`, `crates/super-daemon/src/lock.rs`

**Interfaces:**
- Produces: `DaemonPaths { run_dir, socket, lock, log }` with `default_paths() -> Option<DaemonPaths>`, `beside_socket(PathBuf) -> DaemonPaths`, `ensure_run_dir(&self) -> io::Result<()>`; `DaemonLock::acquire(&Path) -> Result<DaemonLock, LockError>`, `DaemonLock::holder_pid(&Path) -> Option<u32>`, `LockError::{Held { pid: Option<u32> }, Io(io::Error)}`.

- [ ] **Step 1: Register the crate in the workspace.** In `/Users/chwzr/flxkpe/harness/super-daemon-core/Cargo.toml` add `"crates/super-daemon",` after `"crates/super-remote",` in `members`, and add this line to `[workspace.dependencies]` after the `super-remote` entry:

```toml
super-daemon = { path = "crates/super-daemon", version = "0.0.37" }
```

- [ ] **Step 2: Create `crates/super-daemon/Cargo.toml`**

```toml
[package]
name = "super-daemon"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
description = "The SUPER daemon: hosts coding-agent sessions on a device and serves them over a local socket"

[lints]
workspace = true

[dependencies]
anyhow.workspace = true
chrono.workspace = true
dirs.workspace = true
fs2.workspace = true
serde.workspace = true
serde_json.workspace = true
super-coding.workspace = true
super-sdk.workspace = true
thiserror.workspace = true
tokio.workspace = true
tokio-util.workspace = true

[target.'cfg(unix)'.dependencies]
libc.workspace = true

[dev-dependencies]
super-sdk = { workspace = true, features = ["mock"] }
tempfile.workspace = true
```

- [ ] **Step 3: Create `crates/super-daemon/src/lib.rs`** (the `run` function arrives in Task 6; for now only the modules)

```rust
//! The SUPER daemon: one long-lived process per user that hosts coding-agent
//! sessions and serves them to local clients over a Unix socket.
//!
//! `super daemon run` calls [`run`]. The protocol a client speaks is defined in
//! `super_sdk::daemon` and described in `docs/daemon.md`; the design is in
//! `.super/specs/2026-10-29-daemon-design.md`.

pub mod lock;
pub mod paths;
```

- [ ] **Step 4: Write the failing tests for paths and lock.** Create `crates/super-daemon/src/paths.rs` with only this test module and `crates/super-daemon/src/lock.rs` with only its test module:

```rust
// paths.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_live_under_the_super_directory() {
        let paths = DaemonPaths::default_paths().expect("a home directory");
        assert!(paths.socket.ends_with(".super/run/daemon.sock"));
        assert!(paths.lock.ends_with(".super/run/daemon.lock"));
        assert!(paths.log.ends_with(".super/agent/logs/daemon.log"));
        assert_eq!(paths.run_dir, paths.socket.parent().unwrap());
    }

    #[test]
    fn a_custom_socket_keeps_its_lock_beside_it() {
        let paths = DaemonPaths::beside_socket(PathBuf::from("/tmp/x/d.sock"));
        assert_eq!(paths.lock, PathBuf::from("/tmp/x/d.lock"));
        assert_eq!(paths.run_dir, PathBuf::from("/tmp/x"));
    }
}
```

```rust
// lock.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_acquire_sees_the_holder_pid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("run/daemon.lock");
        let first = DaemonLock::acquire(&path).expect("first lock");
        assert_eq!(DaemonLock::holder_pid(&path), Some(std::process::id()));
        match DaemonLock::acquire(&path) {
            Err(LockError::Held { pid }) => assert_eq!(pid, Some(std::process::id())),
            other => panic!("expected Held, got {other:?}"),
        }
        drop(first);
        assert!(DaemonLock::acquire(&path).is_ok());
    }

    #[test]
    fn the_error_text_names_the_pid() {
        let held = LockError::Held { pid: Some(4242) };
        assert_eq!(
            held.to_string(),
            "another SUPER daemon is running (pid 4242)"
        );
        assert_eq!(
            LockError::Held { pid: None }.to_string(),
            "another SUPER daemon is running"
        );
    }
}
```

- [ ] **Step 5: Run them to see them fail**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-daemon`
Expected: compile errors, `DaemonPaths` and `DaemonLock` not found.

- [ ] **Step 6: Write `paths.rs`** (above the tests)

```rust
//! Where the daemon keeps its socket, lock and log.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonPaths {
    /// Holds the socket and the lock; created with mode 0700.
    pub run_dir: PathBuf,
    pub socket: PathBuf,
    pub lock: PathBuf,
    pub log: PathBuf,
}

impl DaemonPaths {
    /// `~/.super/run/daemon.sock`, `~/.super/run/daemon.lock` and
    /// `~/.super/agent/logs/daemon.log`.
    pub fn default_paths() -> Option<Self> {
        let home = dirs::home_dir()?;
        let run_dir = home.join(".super/run");
        Some(Self {
            socket: run_dir.join("daemon.sock"),
            lock: run_dir.join("daemon.lock"),
            log: home.join(".super/agent/logs/daemon.log"),
            run_dir,
        })
    }

    /// A socket somewhere else (`--socket`, tests). The lock sits beside the
    /// socket so two daemons on different sockets do not fight; the log stays
    /// at its default place when there is a home directory.
    pub fn beside_socket(socket: PathBuf) -> Self {
        let run_dir = socket
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let lock = socket.with_extension("lock");
        let log = Self::default_paths()
            .map(|paths| paths.log)
            .unwrap_or_else(|| run_dir.join("daemon.log"));
        Self {
            run_dir,
            socket,
            lock,
            log,
        }
    }

    /// Create the run directory, readable by this user only.
    pub fn ensure_run_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.run_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&self.run_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
}
```

- [ ] **Step 7: Write `lock.rs`** (above the tests)

```rust
//! One daemon per user: an exclusive lock file with the holder's pid inside.

use fs2::FileExt as _;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

/// Held for the daemon's lifetime; dropping it releases the lock.
pub struct DaemonLock {
    _file: File,
    path: PathBuf,
}

#[derive(Debug)]
pub enum LockError {
    /// Another process holds the lock. `pid` is what it wrote, when readable.
    Held { pid: Option<u32> },
    Io(std::io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held { pid: Some(pid) } => {
                write!(f, "another SUPER daemon is running (pid {pid})")
            }
            Self::Held { pid: None } => write!(f, "another SUPER daemon is running"),
            Self::Io(error) => write!(f, "daemon lock: {error}"),
        }
    }
}

impl std::error::Error for LockError {}

impl DaemonLock {
    /// Take the lock, writing this process's pid into the file.
    pub fn acquire(path: &Path) -> Result<Self, LockError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(LockError::Io)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(LockError::Io)?;
        if let Err(error) = file.try_lock_exclusive() {
            if is_contention(&error) {
                return Err(LockError::Held {
                    pid: Self::holder_pid(path),
                });
            }
            return Err(LockError::Io(error));
        }
        file.set_len(0).map_err(LockError::Io)?;
        file.seek(SeekFrom::Start(0)).map_err(LockError::Io)?;
        writeln!(file, "{}", std::process::id()).map_err(LockError::Io)?;
        file.flush().map_err(LockError::Io)?;
        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The pid written by whoever holds, or last held, the lock.
    pub fn holder_pid(path: &Path) -> Option<u32> {
        let mut text = String::new();
        File::open(path).ok()?.read_to_string(&mut text).ok()?;
        text.trim().parse().ok()
    }
}

/// fs2 reports contention through its own error, not always `WouldBlock`.
fn is_contention(error: &std::io::Error) -> bool {
    let contended = fs2::lock_contended_error();
    error.kind() == std::io::ErrorKind::WouldBlock
        || (contended.kind() == error.kind() && contended.raw_os_error() == error.raw_os_error())
}
```

- [ ] **Step 8: Run the tests to see them pass**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-daemon`
Expected: 4 tests pass.

- [ ] **Step 9: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && git add Cargo.toml Cargo.lock crates/super-daemon && git commit -m "daemon: new super-daemon crate with run paths and the single-instance lock"
```

---

### Task 3: The rotated log

**Files:**
- Create: `crates/super-daemon/src/log.rs`
- Modify: `crates/super-daemon/src/lib.rs` (add `pub mod log;`)

**Interfaces:**
- Produces: `Level::{Error, Warn, Info, Debug}` with `Level::from_env()`, `DEFAULT_MAX_BYTES`, `Logger::new(path: Option<PathBuf>, level: Level, stderr: bool, max_bytes: u64)`, `Logger::log(&self, level, target: &str, message: &str)`, global `init(Logger)`, `error/warn/info/debug(target: &str, message: impl AsRef<str>)`.

- [ ] **Step 1: Write the failing tests** (create `log.rs` with only this module)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_rotates_once_at_the_size_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("logs/daemon.log");
        let logger = Logger::new(Some(path.clone()), Level::Info, false, 120);
        for i in 0..6 {
            logger.log(Level::Info, "test", &format!("line number {i} with some padding text"));
        }
        let rotated = dir.path().join("logs/daemon.log.1");
        assert!(rotated.exists(), "the full file was renamed");
        let current = std::fs::read_to_string(&path).expect("current file");
        assert!(current.len() < 120, "the current file started over: {current}");
        assert!(current.contains("INFO test: line number 5"));
    }

    #[test]
    fn debug_lines_are_dropped_at_info() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("daemon.log");
        let logger = Logger::new(Some(path.clone()), Level::Info, false, DEFAULT_MAX_BYTES);
        logger.log(Level::Debug, "test", "hidden");
        logger.log(Level::Warn, "test", "shown");
        let text = std::fs::read_to_string(&path).expect("file");
        assert!(!text.contains("hidden"));
        assert!(text.contains("WARN test: shown"));
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-daemon log::`
Expected: compile error, `Logger` not found.

- [ ] **Step 3: Write the logger** (above the tests)

```rust
//! The daemon log: one line per message to a rotated file and to stderr.
//! Never logs tokens, prompts or record bodies; command types and session
//! ids only.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
        }
    }

    /// `SUPER_LOG=daemon=debug` (or `debug`, or `daemon`) turns debug on; the
    /// same variable `super-remote` reads for `remote`.
    pub fn from_env() -> Self {
        match std::env::var("SUPER_LOG") {
            Ok(value)
                if value
                    .split(',')
                    .any(|part| matches!(part.trim(), "debug" | "daemon" | "daemon=debug")) =>
            {
                Level::Debug
            }
            _ => Level::Info,
        }
    }
}

/// Rotate at 5 MiB, keeping one previous file.
pub const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;

pub struct Logger {
    path: Option<PathBuf>,
    file: Mutex<Option<File>>,
    level: Level,
    stderr: bool,
    max_bytes: u64,
}

impl Logger {
    pub fn new(path: Option<PathBuf>, level: Level, stderr: bool, max_bytes: u64) -> Self {
        let file = path.as_deref().and_then(|path| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok()?;
            }
            open_append(path)
        });
        Self {
            path,
            file: Mutex::new(file),
            level,
            stderr,
            max_bytes,
        }
    }

    pub fn log(&self, level: Level, target: &str, message: &str) {
        if level > self.level {
            return;
        }
        let line = format!(
            "{} {} {}: {}",
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ"),
            level.label(),
            target,
            message
        );
        if self.stderr {
            eprintln!("{line}");
        }
        let Some(path) = &self.path else { return };
        let mut guard = self.file.lock().unwrap();
        let full = guard
            .as_ref()
            .and_then(|file| file.metadata().ok())
            .is_some_and(|metadata| metadata.len() >= self.max_bytes);
        if full {
            let _ = std::fs::rename(path, rotated(path));
            *guard = None;
        }
        if guard.is_none() {
            *guard = open_append(path);
        }
        if let Some(file) = guard.as_mut() {
            let _ = writeln!(file, "{line}");
        }
    }
}

fn rotated(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".1");
    path.with_file_name(name)
}

/// Open for appending. Does not create directories: a log directory that
/// went away (a test's temp dir) must not be resurrected.
fn open_append(path: &Path) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(path).ok()
}

static GLOBAL: OnceLock<Logger> = OnceLock::new();

/// Install the process-wide logger. A second call is ignored.
pub fn init(logger: Logger) {
    let _ = GLOBAL.set(logger);
}

fn emit(level: Level, target: &str, message: &str) {
    match GLOBAL.get() {
        Some(logger) => logger.log(level, target, message),
        None if level <= Level::Warn => eprintln!("{} {target}: {message}", level.label()),
        None => {}
    }
}

pub fn error(target: &str, message: impl AsRef<str>) {
    emit(Level::Error, target, message.as_ref());
}

pub fn warn(target: &str, message: impl AsRef<str>) {
    emit(Level::Warn, target, message.as_ref());
}

pub fn info(target: &str, message: impl AsRef<str>) {
    emit(Level::Info, target, message.as_ref());
}

pub fn debug(target: &str, message: impl AsRef<str>) {
    emit(Level::Debug, target, message.as_ref());
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-daemon log::`
Expected: 2 tests pass.

- [ ] **Step 5: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && git add crates/super-daemon && git commit -m "daemon: rotated log file with stderr mirror"
```

---

### Task 4: The login-shell PATH

**Files:**
- Create: `crates/super-daemon/src/env.rs`
- Modify: `crates/super-daemon/src/lib.rs` (add `pub mod env;`)

**Interfaces:**
- Produces: `merge_paths(login: &str, current: &str) -> String`, `login_shell_path() -> Option<String>`, `apply_login_shell_path() -> Option<String>` (called by `main.rs` before the tokio runtime exists, Task 7).

- [ ] **Step 1: Write the failing test** (create `env.rs` with only this module)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_entries_come_first_and_nothing_repeats() {
        let merged = merge_paths(
            "/opt/homebrew/bin:/usr/bin",
            "/usr/bin:/bin:/opt/homebrew/bin:/usr/sbin",
        );
        assert_eq!(merged, "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin");
        assert_eq!(merge_paths("", "/bin"), "/bin");
    }
}
```

- [ ] **Step 2: Run it to see it fail**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-daemon env::`
Expected: compile error, `merge_paths` not found.

- [ ] **Step 3: Write the module** (above the test)

```rust
//! launchd and systemd start the daemon with a bare environment. The sessions
//! it hosts spawn the user's tools (`git`, `node`, ...), so the daemon reads
//! the login shell's PATH once at start and adopts it.

use std::process::{Command, Stdio};
use std::time::Duration;

/// The PATH the user's login shell exports, or `None` when `SHELL` is unset
/// or the shell does not answer within five seconds.
pub fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").ok().filter(|value| !value.is_empty())?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let output = Command::new(shell)
            .args(["-lc", "printf '%s' \"$PATH\""])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        let _ = sender.send(output);
    });
    let output = receiver.recv_timeout(Duration::from_secs(5)).ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// Login-shell entries first, then whatever the current PATH adds, each once.
pub fn merge_paths(login: &str, current: &str) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for entry in login.split(':').chain(current.split(':')) {
        if !entry.is_empty() && !seen.contains(&entry) {
            seen.push(entry);
        }
    }
    seen.join(":")
}

/// Resolve the login shell's PATH and set it on this process. Call this
/// before any other thread exists: setting an environment variable while
/// another thread may read the environment is unsound.
pub fn apply_login_shell_path() -> Option<String> {
    let login = login_shell_path()?;
    let current = std::env::var("PATH").unwrap_or_default();
    let merged = merge_paths(&login, &current);
    // SAFETY: the caller runs this before the tokio runtime is built, so no
    // other thread of this process reads the environment concurrently.
    unsafe { std::env::set_var("PATH", &merged) };
    Some(merged)
}
```

- [ ] **Step 4: Run the test to see it pass**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-daemon env::`
Expected: 1 test passes.

- [ ] **Step 5: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && git add crates/super-daemon && git commit -m "daemon: adopt the login shell's PATH at start"
```

---

### Task 5: The session host

**Files:**
- Modify: `crates/super-sdk/src/session.rs` (add `install_workflow_approval_gate` after `mirrored_sink`)
- Modify: `crates/super-sdk/src/lib.rs` (re-export it in the `pub use session::{...}` list)
- Modify: `crates/super/src/modes/rpc.rs` (replace the private `install_approval_gate` with the SDK function)
- Create: `crates/super-daemon/src/host.rs`
- Modify: `crates/super-daemon/src/lib.rs` (add `pub mod host;`)

**Interfaces:**
- Consumes: `super_sdk::{Session, SessionOptions, SessionSource, ThinkingLevel, AgentMessage, SessionEntry}`, `super_coding::SessionManager` and `super_coding::session::manager::SessionListing`, `super_sdk::daemon::*` from Task 1.
- Produces: `HostConfig { session_dir: PathBuf, idle_unload: Duration, models_file: Option<PathBuf>, no_context_files: bool }`; `CreateRequest { cwd, model, provider, thinking_level, name }`; `HostError::{Unknown(String), Locked(String), Invalid(String), Startup(anyhow::Error)}`; `Hosted { session: Arc<Session>, session_id: String, cwd: PathBuf, file: PathBuf, .. }` with `touch()`, `state() -> (SessionState, Option<String>)`, `attached() -> usize`; `SessionHost::new(HostConfig) -> Arc<SessionHost>`, `config()`, `subscribe() -> broadcast::Receiver<DaemonEvent>`, `announce(DaemonEvent)`, `get(&str) -> Option<Arc<Hosted>>`, `hosted_count()`, `create(CreateRequest) -> Result<Arc<Hosted>, HostError>` (async), `load(&str) -> Result<Arc<Hosted>, HostError>` (async), `attach(&Arc<Hosted>)`, `detach(&Arc<Hosted>)`, `stop(&str, EndReason) -> Result<(), HostError>` (async), `shutdown()` (async), `sweep_idle() -> usize` (async), `list(Option<&Path>) -> Vec<SessionSummary>` (async), `workspaces() -> Vec<WorkspaceSummary>` (async), `summary(&Arc<Hosted>) -> SessionSummary`; free function `next_state(&(SessionState, Option<String>), &Value, streaming: bool) -> Option<(SessionState, Option<String>)>`.

- [ ] **Step 1: Move the workflow approval gate into the SDK.** In `crates/super-sdk/src/session.rs`, after the `mirrored_sink` function, add:

```rust
/// Route workflow approvals through [`Session::open_approval`], so a headless
/// session asks its clients (`approval_request`) instead of auto-approving.
/// `--mode rpc` and the daemon both install this.
pub fn install_workflow_approval_gate(session: &Arc<Session>) {
    let sdk = session.clone();
    session
        .agent_session()
        .set_workflow_approver(Arc::new(move |plan| {
            let sdk = sdk.clone();
            Box::pin(async move {
                let (_, answer) = sdk.open_approval(ApprovalRequest {
                    kind: crate::approvals::ApprovalKind::Workflow,
                    title: format!("Run workflow {}?", plan.name),
                    detail: Some(plan.agent_estimate()),
                    choices: vec![
                        crate::approvals::ApprovalChoice::new("approve", "Run it"),
                        crate::approvals::ApprovalChoice::new("cancel", "Cancel"),
                    ],
                });
                match answer.await {
                    Ok(answer) if answer.choice == "approve" => {
                        super_coding::workflows::ApprovalDecision::Approve
                    }
                    _ => super_coding::workflows::ApprovalDecision::Cancel,
                }
            })
        }));
}
```

In `crates/super-sdk/src/lib.rs` add `install_workflow_approval_gate,` to the `pub use session::{ ... }` list. In `crates/super/src/modes/rpc.rs` delete the private `install_approval_gate` function (the block starting with `/// Workflows ask for approval; without a terminal ...`) and change its call site to `super_sdk::install_workflow_approval_gate(&session);`.

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo check -p super && cargo test -p super-agent-sdk --features "mock rpc" approval`
Expected: compiles; the rpc end-to-end approval test still passes.

- [ ] **Step 2: Write the failing host tests.** Create `crates/super-daemon/src/host.rs` with only this test module (`use super::*;` resolves once the module exists):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use super_sdk::mock::{MockProvider, MockScript};

    fn state(kind: SessionState) -> (SessionState, Option<String>) {
        (kind, None)
    }

    #[test]
    fn events_drive_the_state_machine() {
        let idle = state(SessionState::Idle);
        assert_eq!(
            next_state(&idle, &json!({"type": "agent_start"}), true),
            Some(state(SessionState::Working))
        );
        let working = state(SessionState::Working);
        assert_eq!(
            next_state(&working, &json!({"type": "agent_settled"}), false),
            Some(state(SessionState::Idle))
        );
        assert_eq!(
            next_state(&working, &json!({"type": "approval_request", "title": "Run workflow audit?"}), true),
            Some((SessionState::Blocked, Some("Run workflow audit?".into())))
        );
        let blocked = (SessionState::Blocked, Some("x".to_string()));
        assert_eq!(next_state(&blocked, &json!({"type": "agent_settled"}), false), None);
        assert_eq!(
            next_state(&blocked, &json!({"type": "approval_resolved"}), true),
            Some(state(SessionState::Working))
        );
        assert_eq!(
            next_state(&blocked, &json!({"type": "approval_resolved"}), false),
            Some(state(SessionState::Idle))
        );
        assert_eq!(next_state(&working, &json!({"type": "message_update"}), true), None);
    }

    struct Fixture {
        _provider: MockProvider,
        dir: tempfile::TempDir,
        host: Arc<SessionHost>,
    }

    async fn fixture(idle_unload: Duration) -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let provider = MockProvider::start(dir.path().join("mock"), MockScript::text("Hello."))
            .await
            .expect("mock provider");
        std::fs::create_dir_all(dir.path().join("work")).unwrap();
        let host = SessionHost::new(HostConfig {
            session_dir: dir.path().join("sessions"),
            idle_unload,
            models_file: Some(provider.catalog_path()),
            no_context_files: true,
        });
        Fixture {
            _provider: provider,
            dir,
            host,
        }
    }

    fn request(fixture: &Fixture) -> CreateRequest {
        CreateRequest {
            cwd: fixture.dir.path().join("work"),
            model: Some("mock/mock-1".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_created_session_is_listed_hosted_and_stored_after_stop() {
        let fixture = fixture(Duration::ZERO).await;
        let mut events = fixture.host.subscribe();
        let hosted = fixture.host.create(request(&fixture)).await.expect("create");
        assert_eq!(hosted.state().0, SessionState::Idle);
        assert!(matches!(events.recv().await, Ok(DaemonEvent::SessionCreated { .. })));

        hosted.session.prompt("say hi").await.expect("prompt");
        let listed = fixture.host.list(None).await;
        assert_eq!(listed.len(), 1);
        assert!(listed[0].hosted);
        assert_eq!(listed[0].first_message.as_deref(), Some("say hi"));
        assert_eq!(listed[0].model.as_deref(), Some("mock/mock-1"));

        let id = hosted.session_id.clone();
        drop(hosted);
        fixture.host.stop(&id, EndReason::Stopped).await.expect("stop");
        let listed = fixture.host.list(None).await;
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].hosted);
        assert_eq!(listed[0].state, SessionState::Ended);
        assert_eq!(fixture.host.hosted_count(), 0);

        let workspaces = fixture.host.workspaces().await;
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].name, "work");
        assert_eq!(workspaces[0].session_count, 1);
    }

    #[tokio::test]
    async fn a_stored_session_loads_again_unless_another_process_holds_it() {
        let fixture = fixture(Duration::ZERO).await;
        let hosted = fixture.host.create(request(&fixture)).await.expect("create");
        hosted.session.prompt("remember this").await.expect("prompt");
        let id = hosted.session_id.clone();
        let file = hosted.file.clone();
        drop(hosted);
        fixture.host.stop(&id, EndReason::Stopped).await.expect("stop");

        let lock_path = file.with_extension("jsonl.lock");
        let foreign = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&foreign).unwrap();
        assert!(matches!(fixture.host.load(&id).await, Err(HostError::Locked(_))));
        drop(foreign);

        let resumed = fixture.host.load(&id).await.expect("load");
        assert_eq!(resumed.session_id, id);
        assert_eq!(resumed.session.messages().len(), 2);
        assert!(matches!(fixture.host.load("nope").await, Err(HostError::Unknown(_))));
    }

    #[tokio::test]
    async fn the_sweep_unloads_idle_sessions_nobody_watches() {
        let fixture = fixture(Duration::from_millis(10)).await;
        let hosted = fixture.host.create(request(&fixture)).await.expect("create");
        hosted.session.prompt("hi").await.expect("prompt");
        fixture.host.attach(&hosted);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(fixture.host.sweep_idle().await, 0, "an attached session stays");
        fixture.host.detach(&hosted);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(fixture.host.sweep_idle().await, 1);
        assert_eq!(fixture.host.hosted_count(), 0);
    }

    #[tokio::test]
    async fn create_rejects_missing_or_relative_directories() {
        let fixture = fixture(Duration::ZERO).await;
        let mut missing = request(&fixture);
        missing.cwd = fixture.dir.path().join("absent");
        assert!(matches!(fixture.host.create(missing).await, Err(HostError::Invalid(_))));
        let mut relative = request(&fixture);
        relative.cwd = PathBuf::from("work");
        assert!(matches!(fixture.host.create(relative).await, Err(HostError::Invalid(_))));
    }
}
```

- [ ] **Step 3: Run them to see them fail**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-daemon host::`
Expected: compile errors, `SessionHost`, `HostConfig`, `next_state` not found.

- [ ] **Step 4: Write the host** (above the tests)

```rust
//! The sessions the daemon hosts: loading, listing, state, unloading.
//!
//! A session is its JSONL file; it is *hosted* while an `Arc<Session>` for it
//! lives here and *stored* otherwise. Clients see `idle`, `working` and
//! `blocked` for a hosted session and `ended` for a stored one.

use crate::log;
use chrono::{DateTime, SecondsFormat, Utc};
use fs2::FileExt as _;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};
use super_coding::SessionManager;
use super_coding::session::manager::SessionListing;
use super_sdk::daemon::{DaemonEvent, EndReason, SessionState, SessionSummary, WorkspaceSummary};
use super_sdk::{AgentMessage, Session, SessionEntry, SessionOptions, SessionSource, ThinkingLevel};
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub session_dir: PathBuf,
    /// Unload a hosted session with no client after this much quiet.
    /// `Duration::ZERO` disables the sweep.
    pub idle_unload: Duration,
    /// An alternative model catalog; tests point this at the mock provider.
    pub models_file: Option<PathBuf>,
    pub no_context_files: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CreateRequest {
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub thinking_level: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("no session {0}")]
    Unknown(String),
    #[error("session {0} is open in another process")]
    Locked(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0:#}")]
    Startup(anyhow::Error),
}

/// One hosted session and what the daemon knows about it.
pub struct Hosted {
    pub session: Arc<Session>,
    pub session_id: String,
    pub cwd: PathBuf,
    pub file: PathBuf,
    /// Released by `stop`, before the `Arc` necessarily goes away.
    lock: Mutex<Option<File>>,
    attached: AtomicUsize,
    last_activity: Mutex<Instant>,
    state: Mutex<(SessionState, Option<String>)>,
    tracker: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Hosted {
    /// Note activity, which postpones the idle unload.
    pub fn touch(&self) {
        *self.last_activity.lock().unwrap() = Instant::now();
    }

    pub fn state(&self) -> (SessionState, Option<String>) {
        self.state.lock().unwrap().clone()
    }

    pub fn attached(&self) -> usize {
        self.attached.load(Ordering::SeqCst)
    }

    fn idle_for(&self) -> Duration {
        self.last_activity.lock().unwrap().elapsed()
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        if let Some(tracker) = self.tracker.lock().unwrap().take() {
            tracker.abort();
        }
        self.session.close();
    }
}

pub struct SessionHost {
    config: HostConfig,
    hosted: Mutex<BTreeMap<String, Arc<Hosted>>>,
    events: broadcast::Sender<DaemonEvent>,
}

impl SessionHost {
    pub fn new(config: HostConfig) -> Arc<Self> {
        let (events, _) = broadcast::channel(256);
        Arc::new(Self {
            config,
            hosted: Mutex::new(BTreeMap::new()),
            events,
        })
    }

    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    /// Daemon-level events: every connection subscribes.
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.events.subscribe()
    }

    pub fn announce(&self, event: DaemonEvent) {
        let _ = self.events.send(event);
    }

    pub fn get(&self, session_id: &str) -> Option<Arc<Hosted>> {
        self.hosted.lock().unwrap().get(session_id).cloned()
    }

    pub fn hosted_count(&self) -> usize {
        self.hosted.lock().unwrap().len()
    }

    /// Build a new session in `cwd`, which must exist.
    pub async fn create(self: &Arc<Self>, request: CreateRequest) -> Result<Arc<Hosted>, HostError> {
        if !request.cwd.is_absolute() {
            return Err(HostError::Invalid(format!(
                "cwd must be an absolute path: {}",
                request.cwd.display()
            )));
        }
        if !request.cwd.is_dir() {
            return Err(HostError::Invalid(format!(
                "no such directory: {}",
                request.cwd.display()
            )));
        }
        let thinking_level = match request.thinking_level.as_deref() {
            None => None,
            Some(level) => Some(ThinkingLevel::parse(level).ok_or_else(|| {
                HostError::Invalid(format!("unknown thinking level '{level}'"))
            })?),
        };
        let options = SessionOptions {
            cwd: request.cwd.clone(),
            model: request.model,
            provider: request.provider,
            thinking_level,
            models_file: self.config.models_file.clone(),
            session: SessionSource::Create,
            session_dir: Some(self.config.session_dir.clone()),
            session_name: request.name,
            no_context_files: self.config.no_context_files,
            ..Default::default()
        };
        let session = Session::create(options)
            .await
            .map_err(|error| HostError::Startup(anyhow::Error::new(error)))?;
        let file = session
            .state()
            .session_file
            .ok_or_else(|| HostError::Invalid("the session has no file".into()))?;
        let session_id = session.state().session_id;
        let lock = lock_session_file(&file)
            .map_err(|error| HostError::Startup(error.into()))?
            .ok_or_else(|| HostError::Locked(session_id.clone()))?;
        let hosted = self.install(session, request.cwd, file, lock);
        log::info("host", format!("created session {session_id}"));
        self.announce(DaemonEvent::SessionCreated {
            session: self.summary(&hosted),
        });
        Ok(hosted)
    }

    /// Host a stored session; a no-op when it is hosted already.
    pub async fn load(self: &Arc<Self>, session_id: &str) -> Result<Arc<Hosted>, HostError> {
        if let Some(hosted) = self.get(session_id) {
            return Ok(hosted);
        }
        let dir = self.config.session_dir.clone();
        let wanted = session_id.to_string();
        let found = tokio::task::spawn_blocking(move || {
            let path = SessionManager::find_by_id(&dir, &wanted)?;
            path.map(|path| SessionManager::open(&path).map(|manager| (path, manager.cwd().to_path_buf())))
                .transpose()
        })
        .await
        .map_err(|error| HostError::Invalid(error.to_string()))?
        .map_err(HostError::Startup)?;
        let Some((path, cwd)) = found else {
            return Err(HostError::Unknown(session_id.to_string()));
        };
        let Some(lock) = lock_session_file(&path).map_err(|error| HostError::Startup(error.into()))?
        else {
            // Either another process, or a concurrent `load` of ours.
            return match self.get(session_id) {
                Some(hosted) => Ok(hosted),
                None => Err(HostError::Locked(session_id.to_string())),
            };
        };
        let options = SessionOptions {
            cwd: cwd.clone(),
            models_file: self.config.models_file.clone(),
            session: SessionSource::Open(path.clone()),
            session_dir: Some(self.config.session_dir.clone()),
            no_context_files: self.config.no_context_files,
            ..Default::default()
        };
        let session = Session::create(options)
            .await
            .map_err(|error| HostError::Startup(anyhow::Error::new(error)))?;
        let opened_id = session.state().session_id;
        if !opened_id.starts_with(session_id) {
            return Err(HostError::Invalid(format!(
                "session {session_id} resolved to {opened_id}"
            )));
        }
        let hosted = self.install(session, cwd, path, lock);
        log::info("host", format!("loaded session {opened_id}"));
        self.announce(DaemonEvent::SessionUpdated {
            session: self.summary(&hosted),
        });
        Ok(hosted)
    }

    fn install(self: &Arc<Self>, session: Arc<Session>, cwd: PathBuf, file: PathBuf, lock: File) -> Arc<Hosted> {
        super_sdk::install_workflow_approval_gate(&session);
        let session_id = session.state().session_id;
        let events = session.events();
        let hosted = Arc::new(Hosted {
            session,
            session_id: session_id.clone(),
            cwd,
            file,
            lock: Mutex::new(Some(lock)),
            attached: AtomicUsize::new(0),
            last_activity: Mutex::new(Instant::now()),
            state: Mutex::new((SessionState::Idle, None)),
            tracker: Mutex::new(None),
        });
        let tracker = tokio::spawn(track_state(
            Arc::downgrade(self),
            Arc::downgrade(&hosted),
            events,
        ));
        *hosted.tracker.lock().unwrap() = Some(tracker);
        self.hosted.lock().unwrap().insert(session_id, hosted.clone());
        hosted
    }

    /// A client attached; the session will not be swept while any is.
    pub fn attach(&self, hosted: &Arc<Hosted>) {
        hosted.attached.fetch_add(1, Ordering::SeqCst);
        hosted.touch();
        self.announce(DaemonEvent::SessionUpdated {
            session: self.summary(hosted),
        });
    }

    pub fn detach(&self, hosted: &Arc<Hosted>) {
        let _ = hosted
            .attached
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n.saturating_sub(1)));
        hosted.touch();
        self.announce(DaemonEvent::SessionUpdated {
            session: self.summary(hosted),
        });
    }

    /// Abort the running turn, unload the session, keep its file.
    pub async fn stop(&self, session_id: &str, reason: EndReason) -> Result<(), HostError> {
        let hosted = self
            .hosted
            .lock()
            .unwrap()
            .remove(session_id)
            .ok_or_else(|| HostError::Unknown(session_id.to_string()))?;
        hosted.session.abort();
        let _ = tokio::time::timeout(Duration::from_secs(5), hosted.session.wait_idle()).await;
        *hosted.lock.lock().unwrap() = None;
        drop(hosted);
        log::info("host", format!("unloaded session {session_id} ({})", reason_label(reason)));
        self.announce(DaemonEvent::SessionEnded {
            session_id: session_id.to_string(),
            reason,
        });
        Ok(())
    }

    pub async fn shutdown(&self) {
        let ids: Vec<String> = self.hosted.lock().unwrap().keys().cloned().collect();
        for id in ids {
            let _ = self.stop(&id, EndReason::Shutdown).await;
        }
    }

    /// Unload hosted sessions that are idle, unattached and quiet for longer
    /// than the configured timeout. Returns how many were unloaded.
    pub async fn sweep_idle(&self) -> usize {
        if self.config.idle_unload.is_zero() {
            return 0;
        }
        let candidates: Vec<String> = self
            .hosted
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, hosted)| {
                hosted.attached() == 0
                    && hosted.state().0 == SessionState::Idle
                    && hosted.idle_for() >= self.config.idle_unload
            })
            .map(|(id, _)| id.clone())
            .collect();
        let mut unloaded = 0;
        for id in candidates {
            if self.stop(&id, EndReason::Idle).await.is_ok() {
                unloaded += 1;
            }
        }
        unloaded
    }

    /// Hosted and stored sessions, newest first. `cwd` limits the listing.
    pub async fn list(&self, cwd: Option<&Path>) -> Vec<SessionSummary> {
        let dir = self.config.session_dir.clone();
        let filter = cwd.map(Path::to_path_buf);
        let scan_cwd = filter.clone();
        let stored: Vec<SessionListing> = tokio::task::spawn_blocking(move || match scan_cwd {
            Some(cwd) => SessionManager::list(&cwd, &dir),
            None => SessionManager::list_all(&dir),
        })
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
        let hosted: BTreeMap<String, Arc<Hosted>> = self.hosted.lock().unwrap().clone();
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<SessionSummary> = Vec::new();
        for listing in stored {
            seen.insert(listing.id.clone());
            match hosted.get(&listing.id) {
                Some(live) => out.push(self.summary(live)),
                None => out.push(stored_summary(&listing)),
            }
        }
        for (id, live) in &hosted {
            let in_scope = filter.as_deref().is_none_or(|cwd| cwd == live.cwd);
            if in_scope && !seen.contains(id) {
                out.push(self.summary(live));
            }
        }
        out.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
        out
    }

    /// The sessions grouped by working directory, newest first.
    pub async fn workspaces(&self) -> Vec<WorkspaceSummary> {
        let mut groups: BTreeMap<String, WorkspaceSummary> = BTreeMap::new();
        for session in self.list(None).await {
            let entry = groups
                .entry(session.cwd.clone())
                .or_insert_with(|| WorkspaceSummary {
                    name: Path::new(&session.cwd)
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| session.cwd.clone()),
                    path: session.cwd.clone(),
                    session_count: 0,
                    latest_state: session.state,
                    latest_modified_at: session.modified_at.clone(),
                });
            entry.session_count += 1;
        }
        let mut out: Vec<WorkspaceSummary> = groups.into_values().collect();
        out.sort_by(|a, b| b.latest_modified_at.cmp(&a.latest_modified_at));
        out
    }

    pub fn summary(&self, hosted: &Arc<Hosted>) -> SessionSummary {
        let agent = hosted.session.agent_session();
        let model = agent.model();
        let thinking = agent.thinking_level();
        let (name, first_message) = {
            let manager = agent.manager.lock().unwrap();
            (manager.session_name(), first_user_message(manager.entries()))
        };
        let (state, blocked_reason) = hosted.state();
        SessionSummary {
            session_id: hosted.session_id.clone(),
            cwd: hosted.cwd.display().to_string(),
            workspace: hosted.cwd.display().to_string(),
            session_file: hosted.file.display().to_string(),
            name,
            first_message,
            state,
            blocked_reason,
            hosted: true,
            attached_clients: hosted.attached(),
            model: Some(format!("{}/{}", model.provider, model.id)),
            thinking_level: Some(thinking.as_str().to_string()),
            modified_at: std::fs::metadata(&hosted.file)
                .and_then(|metadata| metadata.modified())
                .map(rfc3339)
                .unwrap_or_else(|_| rfc3339(SystemTime::now())),
        }
    }
}

fn stored_summary(listing: &SessionListing) -> SessionSummary {
    SessionSummary {
        session_id: listing.id.clone(),
        cwd: listing.cwd.clone(),
        workspace: listing.cwd.clone(),
        session_file: listing.path.display().to_string(),
        name: listing.name.clone(),
        first_message: listing.first_message.clone(),
        state: SessionState::Ended,
        blocked_reason: None,
        hosted: false,
        attached_clients: 0,
        model: None,
        thinking_level: None,
        modified_at: rfc3339(listing.modified),
    }
}

fn first_user_message(entries: &[SessionEntry]) -> Option<String> {
    entries.iter().find_map(|entry| match entry {
        SessionEntry::Message {
            message: AgentMessage::User(user),
            ..
        } => {
            let text = user.content.as_text();
            let text = text.trim();
            (!text.is_empty()).then(|| text.chars().take(200).collect::<String>())
        }
        _ => None,
    })
}

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn reason_label(reason: EndReason) -> &'static str {
    match reason {
        EndReason::Stopped => "stopped",
        EndReason::Idle => "idle",
        EndReason::Shutdown => "shutdown",
    }
}

/// Lock `<file>.jsonl.lock` beside a session file. `Ok(None)` means another
/// process holds it.
fn lock_session_file(file: &Path) -> std::io::Result<Option<File>> {
    let path = file.with_extension("jsonl.lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    match lock.try_lock_exclusive() {
        Ok(()) => Ok(Some(lock)),
        Err(error) => {
            let contended = fs2::lock_contended_error();
            if error.kind() == std::io::ErrorKind::WouldBlock
                || (contended.kind() == error.kind()
                    && contended.raw_os_error() == error.raw_os_error())
            {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }
}

/// What one session event does to the client-visible state. `None` leaves
/// it alone. `Blocked` is sticky until the approval is resolved.
pub fn next_state(
    current: &(SessionState, Option<String>),
    event: &Value,
    streaming: bool,
) -> Option<(SessionState, Option<String>)> {
    let kind = event.get("type").and_then(Value::as_str)?;
    match kind {
        "approval_request" => Some((
            SessionState::Blocked,
            event.get("title").and_then(Value::as_str).map(str::to_string),
        )),
        "approval_resolved" => Some((
            if streaming {
                SessionState::Working
            } else {
                SessionState::Idle
            },
            None,
        )),
        _ if current.0 == SessionState::Blocked => None,
        "agent_start" | "turn_start" => Some((SessionState::Working, None)),
        "agent_settled" => Some((SessionState::Idle, None)),
        _ => None,
    }
}

/// Follow one hosted session's events, keep its state, announce changes.
/// Holds only weak references, so a dropped session ends the task.
async fn track_state(
    host: Weak<SessionHost>,
    hosted: Weak<Hosted>,
    mut events: super_sdk::EventStream,
) {
    while let Some(event) = events.recv().await {
        let (Some(host), Some(hosted)) = (host.upgrade(), hosted.upgrade()) else {
            break;
        };
        hosted.touch();
        let streaming = hosted.session.agent_session().is_running();
        let current = hosted.state();
        if let Some(next) = next_state(&current, &event.0, streaming)
            && next != current
        {
            *hosted.state.lock().unwrap() = next;
            host.announce(DaemonEvent::SessionUpdated {
                session: host.summary(&hosted),
            });
        }
    }
}
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-daemon host::`
Expected: 5 tests pass. If `Option::is_none_or` is unavailable on the toolchain, replace it with `.map_or(true, |cwd| cwd == live.cwd)`.

- [ ] **Step 6: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && git add crates/super-sdk crates/super/src/modes/rpc.rs crates/super-daemon && git commit -m "daemon: session host with lazy load, per-file locks, state tracking and idle unload"
```

---

### Task 6: The socket server, `run`, and the end-to-end test

**Files:**
- Create: `crates/super-daemon/src/server.rs`
- Modify: `crates/super-daemon/src/lib.rs` (modules, `DaemonConfig`, `run`)
- Create: `crates/super-daemon/tests/socket_end_to_end.rs`

**Interfaces:**
- Consumes: Task 1 types, `SessionHost` from Task 5, `DaemonLock`, `DaemonPaths`, `Logger`.
- Produces: `DaemonConfig { paths: DaemonPaths, host: HostConfig, host_id: String, host_name: String, log_level: Level, log_to_stderr: bool, sweep_interval: Duration }`; `pub async fn run(config: DaemonConfig, shutdown: CancellationToken) -> anyhow::Result<i32>` (unix only); `server::{DaemonInfo, ServerContext, serve}`.

- [ ] **Step 1: Write the failing end-to-end test.** Create `crates/super-daemon/tests/socket_end_to_end.rs`:

```rust
//! The daemon as a client sees it: a real Unix socket, the mock provider.

#![cfg(unix)]

use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use super_daemon::host::HostConfig;
use super_daemon::log::Level;
use super_daemon::paths::DaemonPaths;
use super_daemon::DaemonConfig;
use super_sdk::mock::{MockProvider, MockScript};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio_util::sync::CancellationToken;

struct Daemon {
    _provider: MockProvider,
    dir: tempfile::TempDir,
    socket: PathBuf,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<anyhow::Result<i32>>,
}

impl Daemon {
    async fn start(idle_unload: Duration, sweep_interval: Duration) -> Daemon {
        // `sun_path` is short; keep the temp dir's own name and a short file name.
        let dir = tempfile::Builder::new().prefix("sd").tempdir().expect("tempdir");
        let provider = MockProvider::start(
            dir.path().join("mock"),
            MockScript::text("Hello from the mock provider."),
        )
        .await
        .expect("mock provider");
        std::fs::create_dir_all(dir.path().join("work")).unwrap();
        let socket = dir.path().join("d.sock");
        let config = DaemonConfig {
            paths: DaemonPaths {
                run_dir: dir.path().to_path_buf(),
                socket: socket.clone(),
                lock: dir.path().join("d.lock"),
                log: dir.path().join("d.log"),
            },
            host: HostConfig {
                session_dir: dir.path().join("sessions"),
                idle_unload,
                models_file: Some(provider.catalog_path()),
                no_context_files: true,
            },
            host_id: "01TEST".into(),
            host_name: "testbox".into(),
            log_level: Level::Debug,
            log_to_stderr: false,
            sweep_interval,
        };
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(super_daemon::run(config, shutdown.clone()));
        for _ in 0..200 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(socket.exists(), "the daemon created its socket");
        Daemon {
            _provider: provider,
            dir,
            socket,
            shutdown,
            task,
        }
    }

    fn work_dir(&self) -> String {
        self.dir.path().join("work").display().to_string()
    }

    async fn raw_client(&self) -> Client {
        let stream = UnixStream::connect(&self.socket).await.expect("connect");
        let (read, write) = stream.into_split();
        Client {
            read: BufReader::new(read),
            write,
            counter: 0,
        }
    }

    /// A connection that already said hello as `name`.
    async fn client(&self, name: &str) -> Client {
        let mut client = self.raw_client().await;
        let reply = client
            .call(json!({"type": "hello", "client": "other", "name": name, "version": "test", "protocolVersions": [1, 1]}))
            .await;
        assert_eq!(reply["success"], true, "{reply}");
        assert_eq!(reply["data"]["protocolVersion"], 1);
        assert_eq!(reply["data"]["hostName"], "testbox");
        client
    }

    async fn stop(self) -> i32 {
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), self.task)
            .await
            .expect("run returned in time")
            .expect("run did not panic")
            .expect("run succeeded")
    }
}

struct Client {
    read: BufReader<OwnedReadHalf>,
    write: OwnedWriteHalf,
    counter: u64,
}

impl Client {
    async fn send(&mut self, mut command: Value) -> String {
        self.counter += 1;
        let id = format!("c{}", self.counter);
        command["id"] = Value::String(id.clone());
        let line = format!("{command}\n");
        self.write.write_all(line.as_bytes()).await.expect("write");
        self.write.flush().await.expect("flush");
        id
    }

    /// The next line, or `None` when the daemon closed the connection.
    async fn next(&mut self) -> Option<Value> {
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(20), self.read.read_line(&mut line))
            .await
            .expect("the daemon answered in time")
            .expect("read a line");
        if read == 0 {
            return None;
        }
        Some(serde_json::from_str(line.trim_end()).expect("JSON line"))
    }

    async fn read_until(&mut self, mut predicate: impl FnMut(&Value) -> bool) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let value = self.next().await.expect("the connection stays open");
            let done = predicate(&value);
            seen.push(value);
            if done {
                return seen;
            }
        }
    }

    /// Send a command and return its response, skipping events on the way.
    async fn call(&mut self, command: Value) -> Value {
        let id = self.send(command).await;
        self.read_until(|value| value["type"] == "response" && value["id"] == id)
            .await
            .pop()
            .unwrap()
    }
}

fn text_of(lines: &[Value]) -> String {
    lines
        .iter()
        .filter(|value| value["type"] == "message_update")
        .filter_map(|value| {
            let inner = &value["assistantMessageEvent"];
            (inner["type"] == "text_delta")
                .then(|| inner["delta"].as_str().unwrap_or_default().to_string())
        })
        .collect()
}

#[tokio::test]
async fn hello_must_come_first() {
    let daemon = Daemon::start(Duration::ZERO, Duration::from_secs(60)).await;
    let mut client = daemon.raw_client().await;
    let reply = client.call(json!({"type": "ping"})).await;
    assert_eq!(reply["command"], "hello");
    assert_eq!(reply["success"], false);
    assert!(reply["error"].as_str().unwrap().contains("say hello first"), "{reply}");
    assert!(client.next().await.is_none(), "the daemon closed the connection");
    daemon.stop().await;
}

#[tokio::test]
async fn a_protocol_mismatch_names_both_ranges() {
    let daemon = Daemon::start(Duration::ZERO, Duration::from_secs(60)).await;
    let mut client = daemon.raw_client().await;
    let reply = client
        .call(json!({"type": "hello", "client": "cli", "version": "9.9.9", "protocolVersions": [9, 9]}))
        .await;
    assert_eq!(reply["success"], false);
    assert_eq!(
        reply["error"],
        "this SUPER daemon speaks protocol 1; this client speaks 9"
    );
    assert_eq!(reply["data"]["protocolVersions"], json!([1, 1]));
    assert!(client.next().await.is_none());
    daemon.stop().await;
}

#[tokio::test]
async fn two_clients_share_one_session_until_it_is_stopped_and_resumed() {
    let daemon = Daemon::start(Duration::ZERO, Duration::from_secs(60)).await;
    let mut a = daemon.client("A").await;
    let mut b = daemon.client("B").await;

    let created = a
        .call(json!({"type": "create_session", "cwd": daemon.work_dir(), "model": "mock/mock-1"}))
        .await;
    assert_eq!(created["success"], true, "{created}");
    let session = &created["data"]["session"];
    assert_eq!(session["hosted"], true);
    assert_eq!(session["state"], "idle");
    let id = session["sessionId"].as_str().unwrap().to_string();

    // B heard about the new session without being attached to anything.
    let seen = b.read_until(|v| v["type"] == "session_created").await;
    assert_eq!(seen.last().unwrap()["session"]["sessionId"], id);

    // A session command before attaching is refused.
    let refused = a.call(json!({"type": "get_state"})).await;
    assert_eq!(refused["success"], false);
    assert_eq!(refused["error"], "not attached to a session");

    let attached = a.call(json!({"type": "attach", "sessionId": id})).await;
    assert_eq!(attached["success"], true, "{attached}");
    assert_eq!(attached["data"]["session"]["attachedClients"], 1);
    let attached = b.call(json!({"type": "attach", "sessionId": id})).await;
    assert_eq!(attached["data"]["session"]["attachedClients"], 2);

    let accepted = a.call(json!({"type": "prompt", "message": "say hi"})).await;
    assert_eq!(accepted["data"]["disposition"], "started");
    let a_lines = a.read_until(|v| v["type"] == "agent_settled").await;
    let b_lines = b.read_until(|v| v["type"] == "agent_settled").await;
    assert_eq!(text_of(&a_lines), "Hello from the mock provider.");
    assert_eq!(text_of(&b_lines), "Hello from the mock provider.");
    // The daemon-level updates come from another task than the session
    // events, so collect until the idle update is in and then look.
    let state_update = |v: &Value, state: &str| {
        v["type"] == "session_updated" && v["session"]["state"] == state
    };
    let mut b_lines = b_lines;
    if !b_lines.iter().any(|v| state_update(v, "idle")) {
        b_lines.extend(b.read_until(|v| state_update(v, "idle")).await);
    }
    assert!(
        b_lines.iter().any(|v| state_update(v, "working")),
        "B saw the session go to work"
    );

    // The transcript is the same from both pipes.
    let entries = b.call(json!({"type": "get_entries"})).await;
    let entry_count = entries["data"]["entries"].as_array().unwrap().len();
    assert!(entry_count >= 2, "a user and an assistant entry: {entries}");

    let listed = a.call(json!({"type": "list_sessions"})).await;
    let sessions = listed["data"]["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["attachedClients"], 2);
    assert_eq!(sessions[0]["firstMessage"], "say hi");

    let detached = b.call(json!({"type": "detach"})).await;
    assert_eq!(detached["data"]["sessionId"], id);

    let stopped = a.call(json!({"type": "stop_session", "sessionId": id})).await;
    assert_eq!(stopped["success"], true, "{stopped}");
    let ended = a.read_until(|v| v["type"] == "session_ended").await;
    assert_eq!(ended.last().unwrap()["reason"], "stopped");
    let ended = b.read_until(|v| v["type"] == "session_ended").await;
    assert_eq!(ended.last().unwrap()["sessionId"], id);
    let refused = a.call(json!({"type": "get_state"})).await;
    assert_eq!(refused["error"], "not attached to a session");

    let listed = a.call(json!({"type": "list_sessions"})).await;
    let sessions = listed["data"]["sessions"].as_array().unwrap();
    assert_eq!(sessions[0]["hosted"], false);
    assert_eq!(sessions[0]["state"], "ended");

    // Attaching to a stored session loads it again with its history.
    let attached = a.call(json!({"type": "attach", "sessionId": id})).await;
    assert_eq!(attached["success"], true, "{attached}");
    assert_eq!(attached["data"]["session"]["hosted"], true);
    let entries = a.call(json!({"type": "get_entries"})).await;
    assert_eq!(entries["data"]["entries"].as_array().unwrap().len(), entry_count);

    let workspaces = a.call(json!({"type": "list_workspaces"})).await;
    assert_eq!(workspaces["data"]["workspaces"][0]["name"], "work");
    assert_eq!(workspaces["data"]["workspaces"][0]["sessionCount"], 1);

    let status = a.call(json!({"type": "status"})).await;
    assert_eq!(status["data"]["hostedSessions"], 1);
    assert_eq!(status["data"]["clients"], 2);
    assert_eq!(status["data"]["hostId"], "01TEST");

    daemon.stop().await;
}

#[tokio::test]
async fn the_sweep_unloads_a_session_nobody_watches() {
    let daemon = Daemon::start(Duration::from_millis(50), Duration::from_millis(20)).await;
    let mut a = daemon.client("A").await;
    let created = a
        .call(json!({"type": "create_session", "cwd": daemon.work_dir(), "model": "mock/mock-1"}))
        .await;
    let id = created["data"]["session"]["sessionId"].as_str().unwrap().to_string();
    a.call(json!({"type": "attach", "sessionId": id})).await;
    a.call(json!({"type": "prompt", "message": "hi"})).await;
    a.read_until(|v| v["type"] == "agent_settled").await;
    a.call(json!({"type": "detach"})).await;
    let ended = a.read_until(|v| v["type"] == "session_ended").await;
    assert_eq!(ended.last().unwrap()["reason"], "idle");
    daemon.stop().await;
}

#[tokio::test]
async fn list_directories_skips_hidden_entries_and_sorts() {
    let daemon = Daemon::start(Duration::ZERO, Duration::from_secs(60)).await;
    let root = daemon.dir.path().join("tree");
    for name in ["beta", "alpha", ".hidden"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    std::fs::write(root.join("file.txt"), "x").unwrap();
    let mut a = daemon.client("A").await;
    let listed = a
        .call(json!({"type": "list_directories", "path": root.display().to_string()}))
        .await;
    let names: Vec<&str> = listed["data"]["directories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["alpha", "beta"]);
    let missing = a
        .call(json!({"type": "list_directories", "path": root.join("absent").display().to_string()}))
        .await;
    assert_eq!(missing["success"], false);
    daemon.stop().await;
}

#[tokio::test]
async fn shutdown_over_the_socket_ends_the_daemon_with_zero() {
    let daemon = Daemon::start(Duration::ZERO, Duration::from_secs(60)).await;
    let mut a = daemon.client("A").await;
    let reply = a.call(json!({"type": "shutdown"})).await;
    assert_eq!(reply["success"], true);
    let seen = a.read_until(|v| v["type"] == "daemon_shutdown").await;
    assert!(!seen.is_empty());
    let socket = daemon.socket.clone();
    assert_eq!(daemon.stop().await, 0);
    assert!(!socket.exists(), "the socket file is removed on exit");
}

#[tokio::test]
async fn a_second_daemon_on_the_same_lock_exits_with_one() {
    let daemon = Daemon::start(Duration::ZERO, Duration::from_secs(60)).await;
    let config = DaemonConfig {
        paths: DaemonPaths {
            run_dir: daemon.dir.path().to_path_buf(),
            socket: daemon.dir.path().join("other.sock"),
            lock: daemon.dir.path().join("d.lock"),
            log: daemon.dir.path().join("other.log"),
        },
        host: HostConfig {
            session_dir: daemon.dir.path().join("sessions"),
            idle_unload: Duration::ZERO,
            models_file: None,
            no_context_files: true,
        },
        host_id: "01TEST".into(),
        host_name: "testbox".into(),
        log_level: Level::Info,
        log_to_stderr: false,
        sweep_interval: Duration::from_secs(60),
    };
    let code = super_daemon::run(config, CancellationToken::new()).await.expect("run");
    assert_eq!(code, 1);
    daemon.stop().await;
}
```

- [ ] **Step 2: Run it to see it fail**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-daemon --test socket_end_to_end`
Expected: compile error, `super_daemon::DaemonConfig` and `super_daemon::run` not found.

- [ ] **Step 3: Write `server.rs`**

```rust
//! The socket server: one task per connection, at most one attached session
//! per connection, daemon-level commands on any connection.

use crate::host::{CreateRequest, Hosted, SessionHost};
use crate::log;
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use super_sdk::daemon::{
    ClientLine, DaemonCommand, DaemonEvent, DaemonRequest, DaemonStatus, DirectoryEntry,
    EndReason, HelloReply, PROTOCOL_VERSIONS, decode_client_line, negotiate,
};
use super_sdk::protocol::Response;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::unix::OwnedReadHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

/// What `hello` and `status` report about this daemon.
#[derive(Debug, Clone)]
pub struct DaemonInfo {
    pub version: String,
    pub host_id: String,
    pub host_name: String,
    pub platform: String,
    pub socket_path: PathBuf,
}

pub struct ServerContext {
    pub host: Arc<SessionHost>,
    pub info: DaemonInfo,
    pub shutdown: CancellationToken,
    started: Instant,
    started_at: String,
    clients: AtomicUsize,
    next_client: AtomicU64,
}

impl ServerContext {
    pub fn new(host: Arc<SessionHost>, info: DaemonInfo, shutdown: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            host,
            info,
            shutdown,
            started: Instant::now(),
            started_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            clients: AtomicUsize::new(0),
            next_client: AtomicU64::new(1),
        })
    }

    pub fn clients(&self) -> usize {
        self.clients.load(Ordering::SeqCst)
    }

    fn hello_reply(&self, protocol_version: u32) -> HelloReply {
        HelloReply {
            protocol_version,
            daemon_version: self.info.version.clone(),
            host_id: self.info.host_id.clone(),
            host_name: self.info.host_name.clone(),
            platform: self.info.platform.clone(),
            pid: std::process::id(),
            started_at: self.started_at.clone(),
        }
    }

    async fn status(&self) -> DaemonStatus {
        let sessions = self.host.list(None).await;
        let hosted = sessions.iter().filter(|session| session.hosted).count();
        DaemonStatus {
            daemon_version: self.info.version.clone(),
            protocol_versions: PROTOCOL_VERSIONS,
            host_id: self.info.host_id.clone(),
            host_name: self.info.host_name.clone(),
            platform: self.info.platform.clone(),
            pid: std::process::id(),
            started_at: self.started_at.clone(),
            uptime_seconds: self.started.elapsed().as_secs(),
            hosted_sessions: hosted,
            stored_sessions: sessions.len() - hosted,
            clients: self.clients(),
            socket_path: self.info.socket_path.display().to_string(),
            session_dir: self.host.config().session_dir.display().to_string(),
        }
    }
}

/// Accept connections until `shutdown` fires.
pub async fn serve(listener: UnixListener, ctx: Arc<ServerContext>) {
    loop {
        let accepted = tokio::select! {
            _ = ctx.shutdown.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, _)) => {
                let ctx = ctx.clone();
                tokio::spawn(async move { connection(stream, ctx).await });
            }
            Err(error) => {
                log::warn("server", format!("accept failed: {error}"));
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

fn peer_is_this_user(stream: &UnixStream) -> bool {
    match stream.peer_cred() {
        // SAFETY: getuid has no preconditions and cannot fail.
        Ok(credentials) => credentials.uid() == unsafe { libc::getuid() },
        Err(error) => {
            log::warn("server", format!("peer credentials unavailable: {error}"));
            false
        }
    }
}

struct Attachment {
    hosted: Arc<Hosted>,
    pump: tokio::task::JoinHandle<()>,
}

/// One connection after a successful hello.
struct Client {
    id: u64,
    origin: String,
    attachment: Mutex<Option<Attachment>>,
    out: mpsc::UnboundedSender<String>,
}

impl Client {
    /// Attach to `hosted`, detaching from the previous session first. The
    /// event subscription starts before the caller responds, so no event
    /// between the response and the first read is lost.
    fn attach(&self, ctx: &ServerContext, hosted: Arc<Hosted>) -> Option<String> {
        let previous = self.detach(ctx);
        let mut events = hosted.session.events();
        let out = self.out.clone();
        let pump = tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                if out.send(event.to_line()).is_err() {
                    break;
                }
            }
        });
        ctx.host.attach(&hosted);
        log::info("server", format!("client {} attached {}", self.id, hosted.session_id));
        *self.attachment.lock().unwrap() = Some(Attachment { hosted, pump });
        previous
    }

    fn detach(&self, ctx: &ServerContext) -> Option<String> {
        let taken = self.attachment.lock().unwrap().take()?;
        taken.pump.abort();
        ctx.host.detach(&taken.hosted);
        log::info("server", format!("client {} detached {}", self.id, taken.hosted.session_id));
        Some(taken.hosted.session_id.clone())
    }

    /// The attached session was unloaded: forget it without touching the host.
    fn forget_if(&self, session_id: &str) {
        let mut guard = self.attachment.lock().unwrap();
        if guard.as_ref().is_some_and(|attachment| attachment.hosted.session_id == session_id)
            && let Some(attachment) = guard.take()
        {
            attachment.pump.abort();
        }
    }

    fn attached(&self) -> Option<Arc<Hosted>> {
        self.attachment
            .lock()
            .unwrap()
            .as_ref()
            .map(|attachment| attachment.hosted.clone())
    }
}

async fn connection(stream: UnixStream, ctx: Arc<ServerContext>) {
    if !peer_is_this_user(&stream) {
        log::warn("server", "refused a connection from another user");
        return;
    }
    let client_id = ctx.next_client.fetch_add(1, Ordering::SeqCst);
    ctx.clients.fetch_add(1, Ordering::SeqCst);
    let (read, mut write) = stream.into_split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
    // One writer owns the socket so lines never interleave.
    let writer = tokio::spawn(async move {
        while let Some(line) = out_rx.recv().await {
            if write.write_all(line.as_bytes()).await.is_err()
                || write.write_all(b"\n").await.is_err()
                || write.flush().await.is_err()
            {
                break;
            }
        }
        let _ = write.shutdown().await;
    });
    let mut reader = BufReader::new(read);
    let mut buffer = String::new();

    let origin = match read_line(&mut reader, &mut buffer).await {
        Some(first) => handshake(&ctx, client_id, &first, &out),
        None => None,
    };
    let Some(origin) = origin else {
        drop(out);
        let _ = writer.await;
        ctx.clients.fetch_sub(1, Ordering::SeqCst);
        return;
    };
    let client = Arc::new(Client {
        id: client_id,
        origin,
        attachment: Mutex::new(None),
        out: out.clone(),
    });

    // Daemon-level events go to every connection.
    let events_pump = {
        let client = client.clone();
        let mut events = ctx.host.subscribe();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        if let DaemonEvent::SessionEnded { session_id, .. } = &event {
                            client.forget_if(session_id);
                        }
                        if client.out.send(event.to_line()).is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    };

    while let Some(line) = read_line(&mut reader, &mut buffer).await {
        dispatch(&ctx, &client, &line);
    }

    events_pump.abort();
    client.detach(&ctx);
    drop(client);
    drop(out);
    let _ = writer.await;
    ctx.clients.fetch_sub(1, Ordering::SeqCst);
    log::debug("server", format!("client {client_id} disconnected"));
}

/// The first line must be `hello`. Returns the client's origin name.
fn handshake(
    ctx: &ServerContext,
    client_id: u64,
    line: &str,
    out: &mpsc::UnboundedSender<String>,
) -> Option<String> {
    let refuse = |id: Option<String>, message: String, data: Option<Value>| {
        let response = Response {
            kind: "response".into(),
            id,
            command: "hello".into(),
            success: false,
            data,
            error: Some(message),
        };
        let _ = out.send(encode(&response));
        None
    };
    match decode_client_line(line) {
        Ok(ClientLine::Daemon(DaemonRequest {
            id,
            command:
                DaemonCommand::Hello {
                    client,
                    name,
                    version,
                    protocol_versions,
                },
        })) => match negotiate(protocol_versions, PROTOCOL_VERSIONS) {
            Ok(protocol_version) => {
                let reply = ctx.hello_reply(protocol_version);
                let response = Response::ok_data("hello", serde_json::to_value(reply).unwrap_or(Value::Null))
                    .with_id(id);
                let _ = out.send(encode(&response));
                let origin = name.filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| client.as_str().to_string());
                log::info(
                    "server",
                    format!("client {client_id} hello: {} {origin} {version} protocol {protocol_version}", client.as_str()),
                );
                Some(origin)
            }
            Err(message) => {
                log::warn("server", format!("client {client_id} refused: {message}"));
                refuse(
                    id,
                    message,
                    Some(json!({
                        "daemonVersion": ctx.info.version,
                        "protocolVersions": PROTOCOL_VERSIONS,
                    })),
                )
            }
        },
        Ok(ClientLine::Daemon(DaemonRequest { id, .. })) => {
            refuse(id, "say hello first".into(), None)
        }
        Ok(ClientLine::Session(request)) => refuse(request.id, "say hello first".into(), None),
        Err(error) => refuse(None, format!("say hello first: {error}"), None),
    }
}

fn dispatch(ctx: &Arc<ServerContext>, client: &Arc<Client>, line: &str) {
    match decode_client_line(line) {
        Err(error) => {
            let _ = client.out.send(error_line("parse", error));
        }
        Ok(ClientLine::Daemon(request)) => {
            let ctx = ctx.clone();
            let client = client.clone();
            tokio::spawn(async move {
                let is_shutdown = matches!(request.command, DaemonCommand::Shutdown {});
                let response = daemon_command(&ctx, &client, request).await;
                if !is_shutdown {
                    let _ = client.out.send(encode(&response));
                }
            });
        }
        Ok(ClientLine::Session(request)) => match client.attached() {
            None => {
                let response = Response::err(request.command.name(), "not attached to a session")
                    .with_id(request.id);
                let _ = client.out.send(encode(&response));
            }
            Some(hosted) => {
                hosted.touch();
                let origin = client.origin.clone();
                let out = client.out.clone();
                tokio::spawn(async move {
                    let response = hosted
                        .session
                        .execute_from(request.command, request.id, Some(origin))
                        .await;
                    let _ = out.send(encode(&response));
                });
            }
        },
    }
}

async fn daemon_command(ctx: &Arc<ServerContext>, client: &Arc<Client>, request: DaemonRequest) -> Response {
    let name = request.command.name();
    let id = request.id;
    log::debug("server", format!("client {} {name}", client.id));
    let result: Result<Value, String> = match request.command {
        DaemonCommand::Hello { .. } => Err("already said hello".into()),
        DaemonCommand::Status {} => serde_json::to_value(ctx.status().await).map_err(|e| e.to_string()),
        DaemonCommand::ListSessions { cwd } => {
            let cwd = cwd.map(PathBuf::from);
            Ok(json!({"sessions": ctx.host.list(cwd.as_deref()).await}))
        }
        DaemonCommand::ListWorkspaces {} => Ok(json!({"workspaces": ctx.host.workspaces().await})),
        DaemonCommand::ListDirectories { path } => {
            list_directories(&path).map(|directories| json!({"path": path, "directories": directories}))
        }
        DaemonCommand::CreateSession {
            cwd,
            model,
            provider,
            thinking_level,
            name,
        } => ctx
            .host
            .create(CreateRequest {
                cwd: PathBuf::from(cwd),
                model,
                provider,
                thinking_level,
                name,
            })
            .await
            .map(|hosted| json!({"session": ctx.host.summary(&hosted)}))
            .map_err(|error| error.to_string()),
        DaemonCommand::ResumeSession { session_id } => ctx
            .host
            .load(&session_id)
            .await
            .map(|hosted| json!({"session": ctx.host.summary(&hosted)}))
            .map_err(|error| error.to_string()),
        DaemonCommand::Attach { session_id } => match ctx.host.load(&session_id).await {
            Ok(hosted) => {
                let previous = client.attach(ctx, hosted.clone());
                Ok(json!({"session": ctx.host.summary(&hosted), "previousSessionId": previous}))
            }
            Err(error) => Err(error.to_string()),
        },
        DaemonCommand::Detach {} => Ok(json!({"sessionId": client.detach(ctx)})),
        DaemonCommand::StopSession { session_id } => ctx
            .host
            .stop(&session_id, EndReason::Stopped)
            .await
            .map(|()| json!({"sessionId": session_id}))
            .map_err(|error| error.to_string()),
        DaemonCommand::Shutdown {} => {
            log::info("server", format!("client {} asked for shutdown", client.id));
            let response = Response::ok("shutdown").with_id(id);
            // Queue the response before cancelling so it still goes out; the
            // caller does not send a second one for `shutdown`.
            let _ = client.out.send(encode(&response));
            ctx.shutdown.cancel();
            return response;
        }
    };
    match result {
        Ok(data) => Response::ok_data(name, data).with_id(id),
        Err(error) => Response::err(name, error).with_id(id),
    }
}

fn list_directories(path: &str) -> Result<Vec<DirectoryEntry>, String> {
    let dir = Path::new(path);
    if !dir.is_absolute() {
        return Err(format!("path must be absolute: {path}"));
    }
    let entries = std::fs::read_dir(dir).map_err(|error| format!("cannot read {path}: {error}"))?;
    let mut out: Vec<DirectoryEntry> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            (!name.starts_with('.')).then(|| DirectoryEntry {
                path: entry.path().display().to_string(),
                name,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The next non-empty line without its terminator, or `None` at EOF.
async fn read_line(reader: &mut BufReader<OwnedReadHalf>, buffer: &mut String) -> Option<String> {
    loop {
        buffer.clear();
        match reader.read_line(buffer).await {
            Ok(0) | Err(_) => return None,
            Ok(_) => {
                let line = buffer.trim_end_matches('\n').trim_end_matches('\r');
                if line.trim().is_empty() {
                    continue;
                }
                return Some(line.to_string());
            }
        }
    }
}

fn encode(response: &Response) -> String {
    serde_json::to_string(response).unwrap_or_else(|error| error_line("parse", error))
}

fn error_line(command: &str, error: impl std::fmt::Display) -> String {
    let message = serde_json::to_string(&error.to_string())
        .unwrap_or_else(|_| "\"internal error\"".to_string());
    format!(r#"{{"type":"response","command":"{command}","success":false,"error":{message}}}"#)
}
```

- [ ] **Step 4: Write `lib.rs` with `DaemonConfig` and `run`**

```rust
//! The SUPER daemon: one long-lived process per user that hosts coding-agent
//! sessions and serves them to local clients over a Unix socket.
//!
//! `super daemon run` calls [`run`]. The protocol a client speaks is defined in
//! `super_sdk::daemon` and described in `docs/daemon.md`; the design is in
//! `.super/specs/2026-10-29-daemon-design.md`.

pub mod env;
pub mod host;
pub mod lock;
pub mod log;
pub mod paths;

#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod server;

use crate::host::HostConfig;
use crate::paths::DaemonPaths;
use std::time::Duration;

/// Everything `run` needs. The binary fills it from settings and flags; tests
/// point every path at a temporary directory.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub paths: DaemonPaths,
    pub host: HostConfig,
    pub host_id: String,
    pub host_name: String,
    pub log_level: log::Level,
    pub log_to_stderr: bool,
    /// How often the idle sweep runs.
    pub sweep_interval: Duration,
}

/// Run the daemon until `shutdown` fires, SIGTERM or SIGINT arrives, or a
/// client sends `shutdown`. Returns the process exit code: 0 for a deliberate
/// stop, 1 when another daemon holds the lock.
#[cfg(unix)]
pub async fn run(
    config: DaemonConfig,
    shutdown: tokio_util::sync::CancellationToken,
) -> anyhow::Result<i32> {
    use crate::lock::{DaemonLock, LockError};
    use crate::server::{DaemonInfo, ServerContext};
    use anyhow::Context as _;
    use std::os::unix::fs::PermissionsExt as _;
    use super_sdk::daemon::DaemonEvent;

    config.paths.ensure_run_dir().with_context(|| {
        format!("create {}", config.paths.run_dir.display())
    })?;
    let _lock = match DaemonLock::acquire(&config.paths.lock) {
        Ok(lock) => lock,
        Err(error @ LockError::Held { .. }) => {
            eprintln!("{error}");
            return Ok(1);
        }
        Err(LockError::Io(error)) => {
            return Err(anyhow::Error::new(error)
                .context(format!("lock {}", config.paths.lock.display())));
        }
    };
    log::init(log::Logger::new(
        Some(config.paths.log.clone()),
        config.log_level,
        config.log_to_stderr,
        log::DEFAULT_MAX_BYTES,
    ));

    if config.paths.socket.exists() {
        std::fs::remove_file(&config.paths.socket)
            .with_context(|| format!("remove stale {}", config.paths.socket.display()))?;
    }
    let listener = tokio::net::UnixListener::bind(&config.paths.socket)
        .with_context(|| format!("listen on {}", config.paths.socket.display()))?;
    std::fs::set_permissions(&config.paths.socket, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod {}", config.paths.socket.display()))?;

    let host = host::SessionHost::new(config.host.clone());
    let info = DaemonInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        host_id: config.host_id.clone(),
        host_name: config.host_name.clone(),
        platform: std::env::consts::OS.to_string(),
        socket_path: config.paths.socket.clone(),
    };
    let ctx = ServerContext::new(host.clone(), info, shutdown.clone());
    log::info(
        "daemon",
        format!(
            "SUPER daemon {} listening on {} (pid {})",
            env!("CARGO_PKG_VERSION"),
            config.paths.socket.display(),
            std::process::id()
        ),
    );
    install_signal_handlers(shutdown.clone());

    let sweeper = {
        let host = host.clone();
        let cancel = shutdown.clone();
        let every = config.sweep_interval.max(Duration::from_millis(10));
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tick.tick() => {
                        let unloaded = host.sweep_idle().await;
                        if unloaded > 0 {
                            log::info("daemon", format!("idle sweep unloaded {unloaded} session(s)"));
                        }
                    }
                }
            }
        })
    };

    server::serve(listener, ctx.clone()).await;

    log::info("daemon", "shutting down");
    sweeper.abort();
    host.announce(DaemonEvent::DaemonShutdown {
        reason: "stopped".into(),
    });
    host.shutdown().await;
    // Let the connection writers flush the last lines before the sockets go.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = std::fs::remove_file(&config.paths.socket);
    log::info("daemon", "stopped");
    Ok(0)
}

#[cfg(unix)]
fn install_signal_handlers(shutdown: tokio_util::sync::CancellationToken) {
    use tokio::signal::unix::{SignalKind, signal};
    for (kind, name) in [
        (SignalKind::terminate(), "SIGTERM"),
        (SignalKind::interrupt(), "SIGINT"),
    ] {
        match signal(kind) {
            Ok(mut stream) => {
                let shutdown = shutdown.clone();
                tokio::spawn(async move {
                    if stream.recv().await.is_some() {
                        log::info("daemon", format!("{name} received"));
                        shutdown.cancel();
                    }
                });
            }
            Err(error) => log::warn("daemon", format!("cannot listen for {name}: {error}")),
        }
    }
}
```

Create an empty `crates/super-daemon/src/client.rs` containing only `//! A client of the daemon socket. Filled in Task 7.` so the module compiles.

- [ ] **Step 5: Run the end-to-end test until it passes**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-daemon --test socket_end_to_end`
Expected: 7 tests pass. Then `cargo test -p super-daemon` for the whole crate.

- [ ] **Step 6: Clippy on the crate**

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo clippy -p super-daemon --all-targets -- -D warnings`
Expected: clean. Fix anything it flags before committing.

- [ ] **Step 7: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && git add crates/super-daemon && git commit -m "daemon: socket server with hello handshake, attached sessions and graceful shutdown"
```

---

### Task 7: `DaemonClient`, the `super daemon` subcommand, settings, binary tests

**Files:**
- Create: `crates/super-daemon/src/client.rs` (replace the placeholder)
- Modify: `crates/super-coding/src/settings.rs` (add `DaemonSettings`, field `daemon` on `Settings` after `platform`)
- Modify: `crates/super/Cargo.toml` (`super-daemon.workspace = true` under `[dependencies]`)
- Modify: `crates/super/src/args.rs` (`DaemonSubcommand`, `Command::Daemon`)
- Create: `crates/super/src/daemon_cli.rs`
- Modify: `crates/super/src/main.rs` (`mod daemon_cli;`, the PATH step, the dispatch arm)
- Create: `crates/super/tests/daemon_cli.rs`

**Interfaces:**
- Consumes: `super_daemon::{DaemonConfig, run, paths::DaemonPaths, host::HostConfig, lock::DaemonLock, log::Level, env::apply_login_shell_path}`, `super_sdk::daemon::{ClientKind, DaemonCommand, DaemonStatus, HelloReply}`, `super_remote::HostIdentity`, `crate::setup::session_dir`.
- Produces: `DaemonClient::connect(&Path) -> io::Result<DaemonClient>`, `hello(&mut self, ClientKind, name: &str, version: &str) -> anyhow::Result<HelloReply>`, `call(&mut self, DaemonCommand) -> anyhow::Result<Response>`, `next_line(&mut self) -> io::Result<Option<String>>`; `DaemonSettings::idle_unload_minutes(&self) -> u64`; `super daemon run [--socket PATH]`, `super daemon status [--socket PATH]`, `super daemon stop [--socket PATH]`.

- [ ] **Step 1: Write the failing settings test.** In `crates/super-coding/src/settings.rs`, inside the existing `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn daemon_idle_unload_defaults_to_an_hour() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.daemon.idle_unload_minutes(), 60);
        let settings: Settings =
            serde_json::from_str(r#"{"daemon":{"idleUnloadMinutes":5}}"#).unwrap();
        assert_eq!(settings.daemon.idle_unload_minutes(), 5);
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["daemon"]["idleUnloadMinutes"], 5);
    }
```

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-coding daemon_idle`
Expected: compile error, no field `daemon`.

- [ ] **Step 2: Add the settings.** Before `pub struct Settings` in `settings.rs` add:

```rust
/// The SUPER daemon (`super daemon run`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DaemonSettings {
    /// Minutes a hosted session may sit idle with no client attached before
    /// the daemon unloads it. Unset means 60; `0` disables unloading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_unload_minutes: Option<u64>,
}

impl DaemonSettings {
    pub fn idle_unload_minutes(&self) -> u64 {
        self.idle_unload_minutes.unwrap_or(60)
    }
}
```

and the field `pub daemon: DaemonSettings,` right after `pub platform: PlatformSettings,`.

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super-coding daemon_idle`
Expected: passes.

- [ ] **Step 3: Write `client.rs`**

```rust
//! A client of the daemon socket: connect, say hello, call commands. `super
//! daemon status|stop` use it; the terminal client of slice 3 grows from it.

use anyhow::{Context as _, bail};
use std::path::Path;
use super_sdk::daemon::{ClientKind, DaemonCommand, DaemonRequest, HelloReply, PROTOCOL_VERSIONS};
use super_sdk::protocol::{Incoming, Response, decode_line};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

pub struct DaemonClient {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next_id: u64,
}

impl DaemonClient {
    pub async fn connect(socket: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(socket).await?;
        let (read, write) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(read),
            writer: write,
            next_id: 0,
        })
    }

    /// The handshake. Fails with the daemon's own words on a protocol
    /// mismatch.
    pub async fn hello(&mut self, client: ClientKind, name: &str, version: &str) -> anyhow::Result<HelloReply> {
        let response = self
            .call(DaemonCommand::Hello {
                client,
                name: Some(name.to_string()),
                version: version.to_string(),
                protocol_versions: PROTOCOL_VERSIONS,
            })
            .await?;
        if !response.success {
            bail!("{}", response.error.unwrap_or_else(|| "hello refused".into()));
        }
        let data = response.data.context("hello without data")?;
        Ok(serde_json::from_value(data)?)
    }

    /// Send one daemon-level command and wait for its response, discarding
    /// events on the way.
    pub async fn call(&mut self, command: DaemonCommand) -> anyhow::Result<Response> {
        self.next_id += 1;
        let id = format!("c{}", self.next_id);
        let request = DaemonRequest {
            id: Some(id.clone()),
            command,
        };
        self.send_line(&serde_json::to_string(&request)?).await?;
        loop {
            let Some(line) = self.next_line().await? else {
                bail!("the daemon closed the connection");
            };
            if let Ok(Incoming::Response(response)) = decode_line(&line)
                && response.id.as_deref() == Some(&id)
            {
                return Ok(response);
            }
        }
    }

    pub async fn send_line(&mut self, line: &str) -> std::io::Result<()> {
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await
    }

    /// The next line without its terminator, or `None` when the daemon
    /// closed the connection.
    pub async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        let mut line = String::new();
        loop {
            line.clear();
            if self.reader.read_line(&mut line).await? == 0 {
                return Ok(None);
            }
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            if !trimmed.trim().is_empty() {
                return Ok(Some(trimmed.to_string()));
            }
        }
    }
}
```

- [ ] **Step 4: The subcommand.** In `crates/super/src/args.rs`, before `pub enum Command`, add:

```rust
#[derive(Debug, Clone, Subcommand)]
pub enum DaemonSubcommand {
    /// Run the daemon in the foreground. launchd and systemd start it this way.
    Run {
        /// Listen on this socket instead of ~/.super/run/daemon.sock.
        #[arg(long, value_name = "PATH")]
        socket: Option<std::path::PathBuf>,
    },

    /// Show whether a daemon is running and what it hosts.
    Status {
        /// The socket of the daemon to ask.
        #[arg(long, value_name = "PATH")]
        socket: Option<std::path::PathBuf>,
    },

    /// Stop the running daemon.
    Stop {
        /// The socket of the daemon to stop.
        #[arg(long, value_name = "PATH")]
        socket: Option<std::path::PathBuf>,
    },
}
```

and in `pub enum Command`, after the `Acp` variant:

```rust
    /// Run or control the background daemon that hosts sessions.
    Daemon {
        #[command(subcommand)]
        command: DaemonSubcommand,
    },
```

- [ ] **Step 5: Write `crates/super/src/daemon_cli.rs`**

```rust
//! `super daemon run|status|stop`.

use crate::args::{Args, DaemonSubcommand};
use anyhow::Result;
use std::path::PathBuf;
use super_daemon::paths::DaemonPaths;

pub async fn run(args: &Args, command: &DaemonSubcommand) -> Result<i32> {
    match command {
        DaemonSubcommand::Run { socket } => run_daemon(args, socket.clone()).await,
        DaemonSubcommand::Status { socket } => status(socket.clone()).await,
        DaemonSubcommand::Stop { socket } => stop(socket.clone()).await,
    }
}

fn paths(socket: Option<PathBuf>) -> Result<DaemonPaths> {
    match socket {
        Some(socket) => Ok(DaemonPaths::beside_socket(socket)),
        None => DaemonPaths::default_paths().ok_or_else(|| anyhow::anyhow!("no home directory")),
    }
}

#[cfg(not(unix))]
async fn run_daemon(_args: &Args, _socket: Option<PathBuf>) -> Result<i32> {
    eprintln!("the SUPER daemon needs a Unix socket and is not available on Windows");
    Ok(1)
}

#[cfg(unix)]
async fn run_daemon(args: &Args, socket: Option<PathBuf>) -> Result<i32> {
    use anyhow::Context as _;
    use std::io::IsTerminal as _;
    use std::time::Duration;
    use super_daemon::host::HostConfig;
    use super_remote::HostIdentity;

    let paths = paths(socket)?;
    let home = dirs::home_dir().context("no home directory")?;
    let settings = super_coding::Settings::load(&home, false);
    let session_dir = crate::setup::session_dir(args, &settings);
    let host = match HostIdentity::default_path() {
        Some(path) => HostIdentity::load_or_create(&path)?,
        None => HostIdentity {
            host_id: "local".into(),
            name: "local".into(),
            platform: super_remote::host::platform_name().into(),
        },
    };
    let config = super_daemon::DaemonConfig {
        paths,
        host: HostConfig {
            session_dir,
            idle_unload: Duration::from_secs(60 * settings.daemon.idle_unload_minutes()),
            models_file: std::env::var_os("SUPER_MODELS_FILE").map(PathBuf::from),
            no_context_files: args.no_context_files,
        },
        host_id: host.host_id,
        host_name: host.name,
        log_level: super_daemon::log::Level::from_env(),
        log_to_stderr: std::io::stderr().is_terminal(),
        sweep_interval: Duration::from_secs(60),
    };
    super_daemon::run(config, tokio_util::sync::CancellationToken::new()).await
}

#[cfg(not(unix))]
async fn status(_socket: Option<PathBuf>) -> Result<i32> {
    println!("no SUPER daemon is running");
    Ok(1)
}

#[cfg(unix)]
async fn status(socket: Option<PathBuf>) -> Result<i32> {
    use anyhow::Context as _;
    use super_daemon::client::DaemonClient;
    use super_sdk::daemon::{ClientKind, DaemonCommand, DaemonStatus};

    let paths = paths(socket)?;
    let Ok(mut client) = DaemonClient::connect(&paths.socket).await else {
        println!("no SUPER daemon is running");
        return Ok(1);
    };
    if let Err(error) = client
        .hello(ClientKind::Cli, "super", env!("CARGO_PKG_VERSION"))
        .await
    {
        eprintln!("error: {error}");
        return Ok(2);
    }
    let response = client.call(DaemonCommand::Status {}).await?;
    if !response.success {
        anyhow::bail!("{}", response.error.unwrap_or_else(|| "status failed".into()));
    }
    let status: DaemonStatus =
        serde_json::from_value(response.data.context("status without data")?)?;
    println!(
        "SUPER daemon {} running (pid {}, up {})",
        status.daemon_version,
        status.pid,
        uptime(status.uptime_seconds)
    );
    println!("socket   {}", status.socket_path);
    println!(
        "sessions {} hosted, {} on disk",
        status.hosted_sessions, status.stored_sessions
    );
    println!("clients  {}", status.clients);
    Ok(0)
}

#[cfg(not(unix))]
async fn stop(_socket: Option<PathBuf>) -> Result<i32> {
    println!("no SUPER daemon is running");
    Ok(0)
}

#[cfg(unix)]
async fn stop(socket: Option<PathBuf>) -> Result<i32> {
    use std::time::Duration;
    use super_daemon::client::DaemonClient;
    use super_daemon::lock::DaemonLock;
    use super_sdk::daemon::{ClientKind, DaemonCommand};

    let paths = paths(socket)?;
    match DaemonClient::connect(&paths.socket).await {
        Ok(mut client) => {
            client
                .hello(ClientKind::Cli, "super", env!("CARGO_PKG_VERSION"))
                .await?;
            let response = client.call(DaemonCommand::Shutdown {}).await?;
            if !response.success {
                anyhow::bail!("{}", response.error.unwrap_or_else(|| "shutdown refused".into()));
            }
            // The daemon closes every connection on its way out.
            let closed = tokio::time::timeout(Duration::from_secs(10), async {
                while let Ok(Some(_)) = client.next_line().await {}
            })
            .await
            .is_ok();
            if closed {
                println!("stopped");
            } else {
                println!("stop requested; the daemon is still shutting down");
            }
            Ok(0)
        }
        Err(_) => match DaemonLock::holder_pid(&paths.lock) {
            Some(pid) if process_alive(pid) => {
                // SAFETY: kill with SIGTERM has no memory-safety preconditions.
                let sent = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } == 0;
                if sent {
                    println!("stopped (pid {pid})");
                } else {
                    println!("no SUPER daemon is running");
                }
                Ok(0)
            }
            _ => {
                println!("no SUPER daemon is running");
                Ok(0)
            }
        },
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn uptime(seconds: u64) -> String {
    let (hours, minutes) = (seconds / 3600, (seconds % 3600) / 60);
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_reads_like_a_person_wrote_it() {
        assert_eq!(uptime(5), "5s");
        assert_eq!(uptime(125), "2m");
        assert_eq!(uptime(3 * 3600 + 12 * 60), "3h 12m");
    }
}
```

- [ ] **Step 6: Wire `main.rs`.** Add `mod daemon_cli;` to the module list (after `mod cache;`). In `main()`, right after `let args = match Args::try_parse() { ... };` and before the runtime is built, add:

```rust
    // The daemon is started by launchd or systemd with a bare environment.
    // Adopt the login shell's PATH now, while this process is single-threaded.
    if matches!(
        &args.command,
        Some(Command::Daemon {
            command: args::DaemonSubcommand::Run { .. }
        })
    ) {
        super_daemon::env::apply_login_shell_path();
    }
```

In `run_command`, after the `Command::Acp` arm, add:

```rust
        Command::Daemon { command } => daemon_cli::run(args, command).await,
```

Add `super-daemon.workspace = true` to `[dependencies]` in `crates/super/Cargo.toml` (after `super-remote.workspace = true`).

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo build -p super && target/debug/super daemon status; echo "exit $?"`
Expected: builds; prints `no SUPER daemon is running` and `exit 1`.

- [ ] **Step 7: Write the binary tests.** Create `crates/super/tests/daemon_cli.rs`:

```rust
//! `super daemon run|status|stop` as processes. HOME points at a temporary
//! directory so nothing under the real ~/.super is touched.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn super_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_super"))
}

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: tempfile::Builder::new().prefix("sb").tempdir().expect("tempdir"),
        }
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("d.sock")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = super_bin();
        command
            .args(args)
            .env("HOME", self.dir.path())
            .env("SUPER_SESSION_DIR", self.dir.path().join("sessions"))
            .env_remove("SHELL")
            .env_remove("SUPER_LOG");
        command
    }

    fn spawn_daemon(&self) -> Child {
        let socket = self.socket();
        let child = self
            .command(&["daemon", "run", "--socket", socket.to_str().unwrap()])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn super daemon run");
        wait_for(&socket, Duration::from_secs(20));
        child
    }
}

fn wait_for(socket: &Path, limit: Duration) {
    let start = Instant::now();
    while !socket.exists() {
        assert!(start.elapsed() < limit, "the daemon did not create {}", socket.display());
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_exit(mut child: Child) -> i32 {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status.code().unwrap_or(-1);
        }
        assert!(start.elapsed() < Duration::from_secs(20), "the daemon did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn status_without_a_daemon_exits_one() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command(&["daemon", "status", "--socket", sandbox.socket().to_str().unwrap()])
        .output()
        .expect("run status");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("no SUPER daemon is running"));
}

#[test]
fn run_answers_status_and_exits_zero_on_sigterm() {
    let sandbox = Sandbox::new();
    let child = sandbox.spawn_daemon();
    let output = sandbox
        .command(&["daemon", "status", "--socket", sandbox.socket().to_str().unwrap()])
        .output()
        .expect("run status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("running (pid"), "{stdout}");
    assert!(stdout.contains("sessions 0 hosted"), "{stdout}");
    // SAFETY: SIGTERM to a child we own.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(wait_exit(child), 0);
    assert!(!sandbox.socket().exists());
}

#[test]
fn stop_stops_a_running_daemon() {
    let sandbox = Sandbox::new();
    let child = sandbox.spawn_daemon();
    let output = sandbox
        .command(&["daemon", "stop", "--socket", sandbox.socket().to_str().unwrap()])
        .output()
        .expect("run stop");
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("stopped"));
    assert_eq!(wait_exit(child), 0);
    let again = sandbox
        .command(&["daemon", "stop", "--socket", sandbox.socket().to_str().unwrap()])
        .output()
        .expect("run stop again");
    assert_eq!(again.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&again.stdout).contains("no SUPER daemon is running"));
}
```

`libc` is already a dependency of the binary crate, so the test can use it. Add `tempfile` is already a dev-dependency.

Run: `cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test -p super --test daemon_cli`
Expected: 3 tests pass.

- [ ] **Step 8: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && git add crates/super-daemon crates/super-coding/src/settings.rs crates/super/Cargo.toml crates/super/src/args.rs crates/super/src/daemon_cli.rs crates/super/src/main.rs crates/super/tests/daemon_cli.rs Cargo.lock && git commit -m "cli: super daemon run, status and stop"
```

---

### Task 8: Documentation

**Files:**
- Create: `docs/daemon.md`
- Modify: `docs/rpc.md` (one paragraph pointing at the daemon), `README.md` (one line in the feature or CLI list), `CHANGELOG.md` (an Unreleased entry in the file's existing style)

- [ ] **Step 1: Write `docs/daemon.md`**

```markdown
# The SUPER daemon

`super daemon run` keeps every SUPER session on a device alive in one
long-running process. Clients (the terminal, the macOS app, `super daemon
status`) connect to it over a local Unix socket, attach to a session, and
speak the RPC protocol from [rpc.md](rpc.md) to it. A session outlives the
client that started it; any client can attach to any session; a session the
daemon unloaded can always be resumed.

On macOS the SUPER app bundles the daemon and registers it with launchd. On
Linux `super install` (coming) registers a systemd user unit. Running `super`
in a terminal without a daemon works as it always did.

## Commands

| Command | What it does |
| --- | --- |
| `super daemon run [--socket PATH]` | Run in the foreground until `shutdown`, SIGTERM or SIGINT. Exit 0 on a deliberate stop, 1 on any error. Refuses to start while another daemon holds `~/.super/run/daemon.lock`. |
| `super daemon status [--socket PATH]` | Print version, pid, uptime, sessions and clients. Exit 1 when no daemon answers, 2 when it refuses the protocol. |
| `super daemon stop [--socket PATH]` | Ask the daemon to shut down, or send SIGTERM to the pid in the lock file when the socket is dead. |

## Files

| Path | Purpose |
| --- | --- |
| `~/.super/run/daemon.sock` | The socket. Directory mode 0700, socket 0600; a peer with another uid is refused. |
| `~/.super/run/daemon.lock` | Exclusive lock with the daemon's pid inside. |
| `~/.super/agent/logs/daemon.log` | Log, rotated at 5 MiB keeping `daemon.log.1`. `SUPER_LOG=daemon=debug` adds detail. Never contains tokens, prompts or record bodies. |
| `~/.super/agent/sessions/` | The session files, shared with the terminal. |
| `~/.super/agent/settings.json` | `{"daemon": {"idleUnloadMinutes": 60}}`: minutes an idle, unattached session stays loaded; `0` keeps sessions loaded until stopped. |

The daemon is started by launchd or systemd with a bare environment, so it
reads the login shell's PATH once at start and uses it for every session's
tools.

## The connection

One JSON object per line, `\n` separated, exactly as in [rpc.md](rpc.md).
The first line must be `hello`; anything else is answered with an error and
the connection closes.

```json
{"id":"1","type":"hello","client":"tui","name":"Terminal","version":"0.0.38","protocolVersions":[1,1]}
{"type":"response","id":"1","command":"hello","success":true,"data":{"protocolVersion":1,"daemonVersion":"0.0.41","hostId":"01J…","hostName":"mbp","platform":"macos","pid":4242,"startedAt":"2026-10-29T09:00:00Z"}}
```

`client` is `tui`, `cli`, `macos`, `ios` or `other`. `name` is what other
clients see as the origin of your commands and approvals (`by: "Terminal"`).
`protocolVersions` is `[min, max]`; the daemon answers with the highest
version both sides speak. With no overlap the response fails with
`"this SUPER daemon speaks protocol 2 to 3; this client speaks 1"` and
`data.protocolVersions` naming the daemon's range, then the connection closes.
Additive changes (new commands, optional fields, events) never bump the
version; a breaking change bumps it and the daemon keeps the previous version
for one release.

A connection is attached to at most one session. After `attach`, session
commands (`prompt`, `get_state`, `get_entries`, `answer_approval`, ...) go to
that session unchanged and its events come back unchanged. A session command
on an unattached connection fails with `not attached to a session`.
Daemon-level commands work at any time; daemon-level events reach every
connection.

## Daemon-level commands

| Command | Payload | Response `data` |
| --- | --- | --- |
| `status` | | `daemonVersion`, `protocolVersions`, `hostId`, `hostName`, `platform`, `pid`, `startedAt`, `uptimeSeconds`, `hostedSessions`, `storedSessions`, `clients`, `socketPath`, `sessionDir` |
| `list_sessions` | `cwd?` | `sessions`: hosted and stored sessions, newest first |
| `list_workspaces` | | `workspaces`: `{path, name, sessionCount, latestState, latestModifiedAt}` derived from the sessions' directories |
| `list_directories` | `path` | `path`, `directories`: `{name, path}` for the subdirectories, hidden ones skipped |
| `create_session` | `cwd`, `model?`, `provider?`, `thinkingLevel?`, `name?` | `session` |
| `resume_session` | `sessionId` | `session`; loads a stored session |
| `attach` | `sessionId` | `session`, `previousSessionId`; loads the session when stored, switches when already attached |
| `detach` | | `sessionId` |
| `stop_session` | `sessionId` | `sessionId`; aborts the turn and unloads, the file stays |
| `shutdown` | | the daemon exits 0 |

A session:

```json
{"sessionId":"a1b2c3d4","cwd":"/Users/felix/dev/super","workspace":"/Users/felix/dev/super",
 "sessionFile":"/Users/felix/.super/agent/sessions/--Users-felix-dev-super--/2026…_a1b2c3d4.jsonl",
 "name":null,"firstMessage":"Fix the login bug","state":"working","blockedReason":null,
 "hosted":true,"attachedClients":2,"model":"super/gpt-5","thinkingLevel":"medium",
 "modifiedAt":"2026-10-29T11:58:03Z"}
```

`state` is `idle`, `working` or `blocked` (an approval is waiting;
`blockedReason` is its title) for a hosted session and `ended` for a stored
one. Ended means "not loaded", never "gone". `model` and `thinkingLevel` are
null for a stored session.

## Daemon-level events

| Event | Payload | When |
| --- | --- | --- |
| `session_created` | `session` | after `create_session` |
| `session_updated` | `session` | state, name, model, thinking level or attached-client count changed |
| `session_ended` | `sessionId`, `reason` (`stopped`, `idle`, `shutdown`) | the session was unloaded; attached connections become unattached |
| `daemon_shutdown` | `reason` | the daemon is about to exit |

## Lifecycle

- A hosted session with no attached client, in state `idle`, with no command
  or event for `idleUnloadMinutes` is unloaded and reported as `ended`.
  `attach` or `resume_session` loads it again with its full history. A
  `working` or `blocked` session is never unloaded.
- While hosting a session the daemon holds `<file>.jsonl.lock` beside it and
  refuses a file another process holds (`session a1b2c3d4 is open in another
  process`).
- `shutdown`, SIGTERM and SIGINT abort in-flight turns, unload every session
  and exit 0. After a restart nothing is hosted; sessions load lazily.
- Workflow approvals are asked as `approval_request` events and answered with
  `answer_approval` from any attached client; `approval_resolved.by` names the
  client's `hello.name`.
```

- [ ] **Step 2: Cross-link and record.** In `docs/rpc.md`, after the first paragraph, add:

```markdown
The same protocol is what a client speaks to a session hosted by the SUPER
daemon after attaching; see [daemon.md](daemon.md).
```

In `README.md`, find the list that mentions `--mode rpc` or `super acp` and add one bullet: `` `super daemon run` hosts every session on the device so the terminal and the apps can attach to them; see `docs/daemon.md`. `` In `CHANGELOG.md`, under the top unreleased heading (create `## Unreleased` above the newest version if there is none), add: `` - `super daemon run|status|stop`: a per-user daemon that hosts sessions and serves them over `~/.super/run/daemon.sock` (ENG-36). ``

- [ ] **Step 3: Commit**

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && git add docs/daemon.md docs/rpc.md README.md CHANGELOG.md && git commit -m "docs: the SUPER daemon and its socket protocol"
```

---

### Task 9: Verification and the pull request

- [ ] **Step 1: Format, lint, test the whole workspace** (each must be clean; fix and amend into the right commit, or add a `fix:` commit)

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo fmt --all --check
```

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo nextest run --workspace 2>&1 | tail -20
```

(`cargo test --workspace` if nextest is not installed; it is used by CI.)

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo test --workspace --doc 2>&1 | tail -5
```

- [ ] **Step 2: Manual smoke test** with the real binary on this Mac, against the real session directory but a private socket:

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && cargo build -p super && (target/debug/super daemon run --socket /tmp/sd-smoke.sock &) && sleep 2 && target/debug/super daemon status --socket /tmp/sd-smoke.sock && target/debug/super daemon stop --socket /tmp/sd-smoke.sock
```

Expected: the status block, then `stopped`. Check `~/.super/agent/logs/daemon.log` has the listening and stopped lines.

- [ ] **Step 3: Push and open the PR** (no attribution lines)

```bash
cd /Users/chwzr/flxkpe/harness/super-daemon-core && git push -u origin daemon-core && gh pr create --repo chwzr/super --base main --head daemon-core --title "super daemon: host sessions over a local socket (ENG-36)" --body-file /Users/chwzr/flxkpe/harness/super-daemon-core/.super/plans/pr-body.md
```

The PR body (write it to `.super/plans/pr-body.md` first, do not commit it): what the daemon is, the socket and handshake, the command set, how it was tested (the end-to-end socket test, the binary tests, the manual smoke test), what is out of scope (slices 2 to 4), and links to the spec file and to ENG-36.

- [ ] **Step 4: Linear.** Comment on ENG-36 with the PR link and the one-paragraph summary; leave ENG-36 and ENG-5 In Progress until the PR merges.
