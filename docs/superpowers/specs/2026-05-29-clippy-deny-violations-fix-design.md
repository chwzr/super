# Clippy Deny Violations — Fix Design

**Date:** 2026-05-29
**Status:** Draft → awaiting user approval

## Context

PR #40 (`chore: deny unwrap/expect/panic in production via workspace clippy lints`) enables
`unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `unreachable` as `deny` at the
workspace level. It does not fix the 83 violations the new lints surface — that work is this spec.

## Violation Inventory

`cargo clippy --all-targets --all-features` reports **83 errors** broken down as:

| Category | Count | Disposition |
|---|---|---|
| `std::sync::Mutex::lock().unwrap()` | 34 | `#[allow]` — idiomatic |
| `RwLock::read().unwrap()` | 7 | `#[allow]` — idiomatic |
| `RwLock::write().unwrap()` | 5 | `#[allow]` — idiomatic |
| `panic_any` (LSP thread-join rethrows) | 3 | Fix — convert to `io::Error` + `tracing::error!` |
| `Option::unwrap()` / `expect()` (non-Mutex) | 8 | Fix |
| `Result::unwrap()` / `expect()` (non-Mutex) | 26 | Fix |

Total fixes: 37 sites. Total `#[allow]`: 46 sites.

## Hard Constraint — User Stated

> Errors that occur semantically during a conversation MUST appear inline in the conversation,
> like Claude Code does. No silent swallowing. No log-only handling for conversation-path errors.

## Allow-Attribute Strategy

`#[allow(clippy::unwrap_used)]` is applied at the **statement level** so the deny lint still
catches new non-Mutex unwraps in the same file. For functions with multiple adjacent
Mutex/RwLock calls (e.g. `tools/mod.rs`, `state/store.rs`, `sqlite_auth_repo.rs`), one
`#[allow(clippy::unwrap_used)]` on the enclosing function is acceptable when every unwrap in
that function is a Mutex/RwLock lock acquisition. A short comment above the attribute
documents the reason ("Mutex poisoning is irrecoverable").

`#[allow]` is **never** applied at module or crate level for these lints.

## In-Conversation Error Routing

Three existing surfaces carry errors back to the user/LLM. A fourth (a new `SystemSubtype`
variant) is added here.

| Surface | Used for | Renders where |
|---|---|---|
| `ToolResult { is_error: true, content }` | Failures inside `Tool::call()` | Inline next to tool-use block; also fed to LLM |
| `bus.emit_system(SystemSubtype::Error, msg)` | Conversation infra failures (SSE, transcript, sidechain, message queue) | Inline `※`-prefixed line in TUI/web. **NEW VARIANT** |
| `bus.emit_system(SystemSubtype::Notice, msg)` | Neutral informational | Inline `※`-prefixed line |
| `tracing::error!` / `tracing::warn!` | Background failures with no conversation context (LSP thread crash, server startup, config load) | Log only |

### New Variant

```rust
// cli/src/sdk/protocol.rs
pub enum SystemSubtype {
    CompactBoundary,
    PostTurnSummary,
    ApiRetry,
    PermissionRequest,
    Notice,
    AsyncAgentDone,
    Error,  // NEW — in-conversation error, styled distinctly
}
```

The TUI renderer (`cli/src/tui/render/mod.rs:199-213`) gains a match arm:

```rust
SystemSubtype::Error => ("⚠ ", Style::default().fg(Color::Red)),
```

The web frontend receives the bus message via the existing relay — no protocol-shape change
beyond the added variant.

## Per-Site Disposition

### Mutex/RwLock (46 sites → `#[allow]`)

Files: `cli/src/state/store.rs` (6), `cli/src/tools/mod.rs` (5), `cli/src/tools/cron_create.rs`,
`cron_delete.rs`, `cron_list.rs`, `cli/src/tools/tool_search.rs:97`, `cli/src/utils/cwd.rs` (2),
`cli/src/utils/worktree.rs` (2), `cli/src/lsp/instance.rs` (7), `cli/src/lsp/mod.rs` (7),
`cli/src/conversation/message_queue.rs` (6 — verify each), `cli/src/conversation/cron_runtime.rs` (2),
`server/src/adapters/sqlite_auth_repo.rs` (10 `conn.lock().unwrap()`).

Each gets `#[allow(clippy::unwrap_used)]` at statement or function scope.

### Tool-Path Fixes (route via `ToolResult { is_error: true }`)

| Site | Fix |
|---|---|
| `cli/src/tools/bash.rs:152` `expect("MessageQueue must be available")` | Replace with `let Some(queue) = context.queue.clone() else { return ToolResult { content: "internal: MessageQueue not provided to bash tool".into(), is_error: true, ..Default::default() }; };` |
| `cli/src/tools/monitor.rs:98` `expect("stdout not piped")` | Same pattern — `let Some(stdout) = child.stdout.take() else { return ToolResult { content: "internal: monitor child stdout not piped".into(), is_error: true, ..Default::default() }; };` |
| `cli/src/tools/web_fetch.rs:103` `Client::builder()...build().unwrap()` | Use `match Client::builder()...build() { Ok(c) => c, Err(e) => return ToolResult { content: format!("HTTP client init failed: {e}"), is_error: true, ..Default::default() } }`. In practice only fails if TLS backend missing at link time. |
| `cli/src/tools/web_search.rs:87` | Same as `web_fetch`. |
| `cli/src/tools/tool_search.rs:131` `strip_prefix('+').unwrap()` | Restructure — value is already `Some` by construction (line 126 succeeded). Fold the check and extraction: bind the result of `strip_prefix` once and reuse, instead of calling twice. |

### LSP Fixes (route via `LspError` → `LspTool::call` → `ToolResult`)

| Site | Fix |
|---|---|
| `cli/src/lsp/client.rs:55, 60, 65` `panic_any(err)` in `ChildIoThreads::join()` | Replace each with `tracing::error!("LSP {} thread panicked: {err:?}", thread_name); return Err(io::Error::other(format!("LSP thread panicked: {err:?}")));`. Single caller at `client.rs:428` is `let _ = io.join();` — caller stays unchanged; tracing surfaces it. |
| `cli/src/lsp/client.rs:79, 91, 100` `Builder::spawn(...).unwrap()` | Replace with `?` — surrounding `child_transport` function changes signature to return `io::Result<(Connection, ChildIoThreads)>`. The single caller (`LspClient::start`) is in a Result context. |
| `cli/src/lsp/instance.rs` (7 sites) | All inside async `start`/`stop` returning `Result<_, String>`. Convert each `.unwrap()` to `.map_err(|e| e.to_string())?` or equivalent. The caller (`LspTool::call` via `LspManager`) already formats failures as `ToolResult { is_error: true }`. |
| `cli/src/lsp/mod.rs` (7 sites) | Same pattern. All in Result contexts. |

### Conversation Infra Fixes (route via `bus.emit_system(Error, ...)`)

| Site | Fix |
|---|---|
| `cli/src/conversation/sse.rs:59` SSE parse during stream | If the stream event fails to parse, the LLM response is corrupt mid-turn. Bus-emit `Error`: `bus.emit_system(SystemSubtype::Error, format!("Stream parse failed: {e}; aborting turn"))` and return early from the streaming loop. |
| `cli/src/conversation/transcript.rs:79, 101` | If file write fails: `bus.emit_system(SystemSubtype::Error, format!("Transcript write failed: {e}; session history may be incomplete"))`. Continue (best-effort). |
| `cli/src/conversation/sidechain.rs:39` | If sidechain LLM call fails: `bus.emit_system(SystemSubtype::Error, format!("Sidechain failed: {e}"))` and return early. |
| `cli/src/conversation/message_queue.rs:50, 81, 103, 118, 123, 133` | **Inspect each before fixing.** Most are likely Mutex (`#[allow]`). Any genuine unwrap inside a queue operation that is called from the conversation hot-path emits `Error` via the bus and continues. |
| `cli/src/conversation/cron_runtime.rs:159, 180` | Mutex per the lint diagnostic — `#[allow]`. |

### Startup Path Fixes (not in conversation — `tracing` + stderr)

| Site | Fix |
|---|---|
| `cli/src/config.rs:5` `expect("no home directory")` | Change `config_path()` to return `Option<PathBuf>`. `load_config()` falls back to `CliConfig::default()` and emits `tracing::warn!("HOME not set; running with default config — changes will not persist")`. `save_config()` becomes a silent no-op when path is None. |
| `server/src/main.rs:30, 32, 45, 47` | Change `main()` to return `Result<(), Box<dyn std::error::Error>>`. Replace `.expect("...")`/`.unwrap()` with `.map_err(|e| format!("...: {e}"))?` preserving existing messages. Errors print on stderr and exit non-zero. |

### Server DB Layer (route via `AuthError::Internal` → HTTP 500 body)

| Sites | Fix |
|---|---|
| `server/src/adapters/sqlite_auth_repo.rs:82, 85, 105, 108, 146, 150, 198, 200, 245, 246` (10 sites) — `Uuid::parse_str(...).unwrap()` and `DateTime::parse_from_rfc3339(...).unwrap()` inside `query_map` closures | The closures return `rusqlite::Result<_>`. Replace each `.unwrap()` with `.map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?`. The error propagates through `query_map` → outer `?` → existing `.map_err(|e| AuthError::Internal(e.to_string()))?` → HTTP 500 with body `"internal error: ..."`. `routes/auth.rs` already does this conversion. |

## Verification Per Site

Each fix MUST be accompanied (in the implementation PR diff or via inline comment) by a
brief note identifying the consumer that surfaces the error. Examples:

```rust
// Fed to LLM via ToolResult.is_error
return ToolResult { content: format!("..."), is_error: true, ..Default::default() };
```

```rust
// Inline in conversation via SystemSubtype::Error
bus.emit_system(SystemSubtype::Error, format!("..."));
```

```rust
// Server-side; surfaced as HTTP 500 body via routes/auth.rs
.map_err(|e| AuthError::Internal(format!("invalid uuid in users.id: {e}")))?;
```

No fix may simply convert `.unwrap()` to `.unwrap_or_default()` / `.ok()` without one of:
- Returning an `is_error` `ToolResult`
- Emitting a `SystemSubtype::Error` bus event
- Propagating a `Result` to a caller that does one of the above
- Logging via `tracing::error!` with the rationale documented in a one-line comment (only
  valid for non-conversation paths)

## Test Plan

- `cargo clippy --all-targets --all-features` returns clean (zero violations).
- `cargo test --workspace` passes — no behavioral regressions.
- `cargo build --release` succeeds.
- **Manual smoke** — induce one in-conversation error path (e.g., set `web_fetch` to an
  unreachable URL) and verify the error appears inline in the TUI transcript, styled
  distinctly from a neutral notice.
- **Manual smoke** — induce a transcript write failure (e.g., point session dir at a
  read-only path) and verify the `SystemSubtype::Error` line appears inline.

## Out of Scope

- Restructuring Mutex usage. `std::sync::Mutex` stays; `#[allow]` is added in place.
- Replacing `thiserror`/`AuthError` with a richer error type. Existing types are reused.
- New tests beyond the manual smokes above. The lint enforcement is the regression test.
- Modifying the web frontend's rendering of `SystemSubtype::Error` — out of scope here;
  it will receive the new variant via the existing bus relay and can style it later.

## Files Touched

CLI:
- `cli/src/config.rs`
- `cli/src/sdk/protocol.rs` (add `SystemSubtype::Error`)
- `cli/src/tui/render/mod.rs` (render `Error` variant)
- `cli/src/lsp/client.rs`, `instance.rs`, `mod.rs`
- `cli/src/conversation/sse.rs`, `transcript.rs`, `sidechain.rs`, `message_queue.rs`, `cron_runtime.rs`
- `cli/src/state/store.rs`
- `cli/src/utils/cwd.rs`, `worktree.rs`
- `cli/src/tools/mod.rs`, `bash.rs`, `monitor.rs`, `web_fetch.rs`, `web_search.rs`, `tool_search.rs`, `cron_create.rs`, `cron_delete.rs`, `cron_list.rs`

Server:
- `server/src/main.rs`
- `server/src/adapters/sqlite_auth_repo.rs`
