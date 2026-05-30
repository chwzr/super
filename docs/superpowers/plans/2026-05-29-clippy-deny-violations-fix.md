# Clippy Deny Violations Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve all 83 clippy violations introduced by PR #40 so `cargo clippy --all-targets --all-features` reports zero errors, routing every fixed error to a user/LLM-visible surface per the spec.

**Architecture:** 46 Mutex/RwLock sites get statement-scoped `#[allow]`. 37 non-Mutex sites get real fixes routed via three surfaces: `ToolResult { is_error: true }` for tool-call failures, a new `SystemSubtype::Error` bus event for conversation infrastructure failures, and `tracing` + stderr for startup/background paths. Server DB parse failures propagate via the existing `AuthError::Internal` → HTTP 500 path.

**Tech Stack:** Rust, clippy, tracing, tokio broadcast, axum.

**Spec:** `docs/superpowers/specs/2026-05-29-clippy-deny-violations-fix-design.md`

---

## File Structure

**Modified (CLI):**
- `cli/src/sdk/protocol.rs` — add `SystemSubtype::Error` variant
- `cli/src/tui/render/mod.rs` — render `Error` variant with red `⚠` glyph
- `cli/src/config.rs` — `config_path()` returns `Option<PathBuf>`
- `cli/src/lsp/client.rs` — panic_any → `tracing::error!` + `io::Error`; `Builder::spawn` unwraps → `?`
- `cli/src/lsp/instance.rs` — `Uri::from_str` restructure + Mutex `#[allow]`
- `cli/src/lsp/mod.rs` — Mutex `#[allow]`
- `cli/src/conversation/sse.rs` — restructure `best.unwrap()` with binding pattern
- `cli/src/conversation/transcript.rs` — no-parent path emits `SystemSubtype::Error`; restructure writer init
- `cli/src/conversation/sidechain.rs` — entry API restructure
- `cli/src/conversation/message_queue.rs` — Mutex `#[allow]`
- `cli/src/conversation/cron_runtime.rs` — Mutex `#[allow]`
- `cli/src/state/store.rs` — Mutex/RwLock `#[allow]`
- `cli/src/utils/cwd.rs`, `cli/src/utils/worktree.rs` — RwLock `#[allow]`
- `cli/src/tools/mod.rs` — RwLock `#[allow]`
- `cli/src/tools/bash.rs` — `expect("MessageQueue...")` → `is_error: true` ToolResult
- `cli/src/tools/monitor.rs` — `expect("stdout not piped")` → `is_error: true` ToolResult
- `cli/src/tools/web_fetch.rs`, `cli/src/tools/web_search.rs` — `Client::builder().build()` → match + `is_error: true`
- `cli/src/tools/tool_search.rs` — restructure `strip_prefix` + RwLock `#[allow]`
- `cli/src/tools/cron_create.rs`, `cli/src/tools/cron_delete.rs`, `cli/src/tools/cron_list.rs` — Mutex `#[allow]`

**Modified (Server):**
- `server/src/main.rs` — `main()` returns `Result<(), Box<dyn Error>>`
- `server/src/adapters/sqlite_auth_repo.rs` — DB parse unwraps → `FromSqlConversionFailure`; Mutex `#[allow]`

---

## Task 1: Add `SystemSubtype::Error` variant

**Files:**
- Modify: `cli/src/sdk/protocol.rs:387-394`
- Modify: `cli/src/tui/render/mod.rs:199-213`

- [ ] **Step 1.1: Add `Error` variant to `SystemSubtype`**

In `cli/src/sdk/protocol.rs`, replace:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemSubtype {
    CompactBoundary,
    PostTurnSummary,
    ApiRetry,
    PermissionRequest,
    Notice,
    AsyncAgentDone,
}
```

with:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemSubtype {
    CompactBoundary,
    PostTurnSummary,
    ApiRetry,
    PermissionRequest,
    Notice,
    AsyncAgentDone,
    Error,
}
```

- [ ] **Step 1.2: Render `Error` variant in TUI**

In `cli/src/tui/render/mod.rs:199-213`, locate the existing match:

```rust
TranscriptItem::System { subtype, message } => {
    lines.push(Line::from(""));
    let recap_style = Style::default().fg(Color::DarkGray);
    let (prefix, style) = match subtype {
        SystemSubtype::PostTurnSummary => ("※ recap: ", recap_style),
        SystemSubtype::CompactBoundary => ("※ ", recap_style),
        _ => ("※ ", dim),
    };
```

Replace with:

```rust
TranscriptItem::System { subtype, message } => {
    lines.push(Line::from(""));
    let recap_style = Style::default().fg(Color::DarkGray);
    let error_style = Style::default().fg(Color::Red);
    let (prefix, style) = match subtype {
        SystemSubtype::PostTurnSummary => ("※ recap: ", recap_style),
        SystemSubtype::CompactBoundary => ("※ ", recap_style),
        SystemSubtype::Error => ("⚠ ", error_style),
        _ => ("※ ", dim),
    };
```

- [ ] **Step 1.3: Build to verify**

Run: `cargo build -p super-cli`
Expected: builds clean (clippy will still fail; we're not done).

- [ ] **Step 1.4: Commit**

```bash
git add cli/src/sdk/protocol.rs cli/src/tui/render/mod.rs
git commit -m "feat(protocol): add SystemSubtype::Error for in-conversation error display"
```

---

## Task 2: Mutex/RwLock allows — `cli/src/state/store.rs`

**Files:**
- Modify: `cli/src/state/store.rs:117, 122, 133, 137, 173, 192`

- [ ] **Step 2.1: Add per-statement `#[allow]` on RwLock acquisitions**

For each violation line (117, 122, 133, 137, 173, 192), find the `.read().unwrap()` or `.write().unwrap()` statement and prepend `#[allow(clippy::unwrap_used)]` to the enclosing statement OR add `#[allow(clippy::unwrap_used)]` to the enclosing function if all `.unwrap()` calls in it are RwLock acquisitions.

Example pattern at line 173:

```rust
// Read out the handle outside set_state so we can call its send()
#[allow(clippy::unwrap_used)] // RwLock poisoning is irrecoverable
let handle = self
    .state
    .read()
    .unwrap()
    .async_agents
    .get(agent_id)
    .cloned();
```

Apply equivalent annotations at each line. If two adjacent `.unwrap()` calls are both RwLock acquisitions in the same function, prefer one `#[allow]` on the function:

```rust
#[allow(clippy::unwrap_used)] // RwLock poisoning is irrecoverable
pub fn list_async_agents(&self) -> Vec<AsyncAgentHandle> {
    self.state
        .read()
        .unwrap()
        ...
}
```

- [ ] **Step 2.2: Verify clippy clean for this file**

Run: `cargo clippy -p super-cli --lib 2>&1 | grep "state/store.rs"`
Expected: no output (all 6 violations resolved).

- [ ] **Step 2.3: Commit**

```bash
git add cli/src/state/store.rs
git commit -m "fix(state): allow Mutex/RwLock unwraps in store"
```

---

## Task 3: Mutex/RwLock allows — `cli/src/tools/mod.rs`

**Files:**
- Modify: `cli/src/tools/mod.rs:183, 187, 196, 205, 222`

- [ ] **Step 3.1: Add `#[allow]` on each RwLock call**

For each line, add `#[allow(clippy::unwrap_used)]` per the pattern in Task 2.1.

Example at line 183:

```rust
#[allow(clippy::unwrap_used)] // RwLock poisoning is irrecoverable
pub fn register(&self, tool: Arc<dyn Tool>) {
    self.tools.write().unwrap().push(tool);
}
```

- [ ] **Step 3.2: Verify**

Run: `cargo clippy -p super-cli --lib 2>&1 | grep "tools/mod.rs"`
Expected: no output.

- [ ] **Step 3.3: Commit**

```bash
git add cli/src/tools/mod.rs
git commit -m "fix(tools): allow RwLock unwraps in registry"
```

---

## Task 4: Mutex allows — `cli/src/utils/` and `cli/src/tools/cron_*.rs` and `cli/src/tools/tool_search.rs:97`

**Files:**
- Modify: `cli/src/utils/cwd.rs:16, 23`
- Modify: `cli/src/utils/worktree.rs:25, 29`
- Modify: `cli/src/tools/cron_create.rs:150`
- Modify: `cli/src/tools/cron_delete.rs:58`
- Modify: `cli/src/tools/cron_list.rs:64`
- Modify: `cli/src/tools/tool_search.rs:97`

- [ ] **Step 4.1: Add `#[allow]` to each Mutex/RwLock site**

For each listed line, prepend `#[allow(clippy::unwrap_used)]` to the statement or the enclosing function per the pattern in Task 2.1. The lint message confirms each is `Mutex::lock()` or `RwLock::read/write()`.

- [ ] **Step 4.2: Verify**

Run:
```bash
cargo clippy -p super-cli --lib 2>&1 | grep -E "utils/cwd|utils/worktree|cron_create|cron_delete|cron_list|tool_search.rs:97"
```
Expected: no output.

- [ ] **Step 4.3: Commit**

```bash
git add cli/src/utils/cwd.rs cli/src/utils/worktree.rs cli/src/tools/cron_create.rs cli/src/tools/cron_delete.rs cli/src/tools/cron_list.rs cli/src/tools/tool_search.rs
git commit -m "fix(utils,tools): allow Mutex unwraps in cwd/worktree/cron/tool_search"
```

---

## Task 5: Mutex allows — `cli/src/conversation/message_queue.rs` and `cli/src/conversation/cron_runtime.rs`

**Files:**
- Modify: `cli/src/conversation/message_queue.rs:50, 81, 103, 118, 123, 133`
- Modify: `cli/src/conversation/cron_runtime.rs:159, 180`

- [ ] **Step 5.1: Apply function-scope `#[allow]` in `message_queue.rs`**

All six violations are `self.inner.lock().unwrap()` on the same `std::sync::Mutex`. Apply one `#[allow]` per containing function. Functions to annotate:

- `enqueue` (line 49)
- `drain` (line 76)
- `remove_by_filter` (line 102)
- `clear` (line 117)
- `has_pending` (line 122)
- `len` (line 132)

Example:

```rust
#[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
pub fn enqueue(&self, cmd: QueuedCommand) {
    self.inner.lock().unwrap().queue.push(cmd);
}
```

- [ ] **Step 5.2: Apply per-statement `#[allow]` in `cron_runtime.rs`**

For lines 159 and 180, prepend `#[allow(clippy::unwrap_used)]` to each statement.

- [ ] **Step 5.3: Verify**

Run: `cargo clippy -p super-cli --lib 2>&1 | grep -E "message_queue|cron_runtime"`
Expected: no output.

- [ ] **Step 5.4: Commit**

```bash
git add cli/src/conversation/message_queue.rs cli/src/conversation/cron_runtime.rs
git commit -m "fix(conversation): allow Mutex unwraps in message_queue/cron_runtime"
```

---

## Task 6: Mutex allows — `cli/src/lsp/instance.rs` and `cli/src/lsp/mod.rs`

**Files:**
- Modify: `cli/src/lsp/instance.rs:89, 131, 152, 179, 202, 252` (Mutex sites)
- Modify: `cli/src/lsp/mod.rs:76, 90, 103, 120, 140, 150, 159`

- [ ] **Step 6.1: Apply `#[allow]` to LSP Mutex sites**

Same pattern as previous tasks. In `lsp/instance.rs`, lines 89/131/152/179/202/252 are all `client.lock().unwrap()` or `cleanup_client.lock().unwrap()`. Line 274 is the non-Mutex `Uri::from_str` — that one is fixed in Task 9, NOT this task.

In `lsp/mod.rs`, lines 76/90/103/120/140/150/159 are all `MANAGER.lock().unwrap()` or `INIT_TASK.lock().unwrap()`.

- [ ] **Step 6.2: Verify**

Run: `cargo clippy -p super-cli --lib 2>&1 | grep -E "lsp/instance.rs|lsp/mod.rs"`
Expected: only `lsp/instance.rs:274` remains (handled in Task 9).

- [ ] **Step 6.3: Commit**

```bash
git add cli/src/lsp/instance.rs cli/src/lsp/mod.rs
git commit -m "fix(lsp): allow Mutex unwraps in manager and instance"
```

---

## Task 7: Mutex allows — `server/src/adapters/sqlite_auth_repo.rs` (10 Mutex sites only)

**Files:**
- Modify: `server/src/adapters/sqlite_auth_repo.rs:54, 75, 98, 127, 137, 165, 175, 188, 218, 235`

- [ ] **Step 7.1: Apply function-scope `#[allow]` to each method**

The 10 sites are all `let conn = self.conn.lock().unwrap();` at the top of each async method. Each method is otherwise a separate boundary, so one `#[allow]` per method is appropriate:

Methods to annotate (one per Mutex site):

- `create_user` (~line 51)
- `find_user_by_email` (~line 74)
- `find_user_by_id` (~line 97)
- `store_api_key` (~line 120)
- `get_active_api_key` (~line 136)
- `revoke_api_key` (~line 163)
- `store_authorization_code` (~line 174)
- `consume_authorization_code` (~line 184)
- `store_refresh_token` (~line 217)
- `consume_refresh_token` (~line 231)

⚠ **IMPORTANT:** Each method ALSO contains DB parse unwraps inside `query_map` closures (lines 82, 85, 105, 108, 146, 150, 198, 200, 245, 246). Those are NOT Mutex and are NOT covered by the function-scope `#[allow]`. They get fixed in Task 13.

Example annotation:

```rust
#[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable; DB parse unwraps fixed separately
async fn create_user(&self, email: &str, password_hash: &str) -> Result<User, AuthError> {
    let id = Uuid::new_v4();
    let now = Utc::now().to_rfc3339();
    let conn = self.conn.lock().unwrap();
    ...
}
```

Wait — function-scope `#[allow]` will ALSO suppress the lint on the DB parse unwraps inside the closures, which means Task 13 won't see them as violations and they'd remain `.unwrap()`. To avoid that: use **per-statement `#[allow]`** on the `let conn = ...` line only:

```rust
async fn create_user(&self, email: &str, password_hash: &str) -> Result<User, AuthError> {
    let id = Uuid::new_v4();
    let now = Utc::now().to_rfc3339();
    #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
    let conn = self.conn.lock().unwrap();
    ...
}
```

Apply the per-statement annotation at all 10 sites.

- [ ] **Step 7.2: Verify only Mutex sites resolved**

Run:
```bash
cargo clippy -p server 2>&1 | grep "sqlite_auth_repo.rs" | wc -l
```
Expected: 10 (the DB parse violations remain, to be fixed in Task 13).

- [ ] **Step 7.3: Commit**

```bash
git add server/src/adapters/sqlite_auth_repo.rs
git commit -m "fix(server): allow Mutex unwraps in SQLite auth repo"
```

---

## Task 8: Tool path fixes — `bash.rs`, `monitor.rs`, `web_fetch.rs`, `web_search.rs`

**Files:**
- Modify: `cli/src/tools/bash.rs:152-155`
- Modify: `cli/src/tools/monitor.rs:98`
- Modify: `cli/src/tools/web_fetch.rs:103-106`
- Modify: `cli/src/tools/web_search.rs:87-90`

- [ ] **Step 8.1: Fix `bash.rs` MessageQueue expect**

In `cli/src/tools/bash.rs`, replace lines 152-155:

```rust
            let queue = context
                .queue
                .clone()
                .expect("MessageQueue must be available in ToolCallContext");
```

with:

```rust
            let Some(queue) = context.queue.clone() else {
                return ToolResult {
                    content: "internal error: MessageQueue not provided to bash tool".into(),
                    is_error: true,
                    ..Default::default()
                };
            };
```

- [ ] **Step 8.2: Fix `monitor.rs` stdout expect**

In `cli/src/tools/monitor.rs`, replace line 98:

```rust
        let stdout = child.stdout.take().expect("stdout not piped");
```

with:

```rust
        let Some(stdout) = child.stdout.take() else {
            return ToolResult {
                content: "internal error: monitor child stdout not piped".into(),
                is_error: true,
                ..Default::default()
            };
        };
```

- [ ] **Step 8.3: Fix `web_fetch.rs` Client::build unwrap**

In `cli/src/tools/web_fetch.rs`, replace lines 103-106:

```rust
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();
```

with:

```rust
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("HTTP client init failed: {e}"),
                    is_error: true,
                    ..Default::default()
                };
            }
        };
```

- [ ] **Step 8.4: Fix `web_search.rs` Client::build unwrap**

In `cli/src/tools/web_search.rs`, replace lines 87-90 with the same `match` pattern as Step 8.3 (timeout is 15 seconds, not 30):

```rust
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("HTTP client init failed: {e}"),
                    is_error: true,
                    ..Default::default()
                };
            }
        };
```

- [ ] **Step 8.5: Verify**

Run: `cargo clippy -p super-cli --lib 2>&1 | grep -E "bash.rs|monitor.rs|web_fetch.rs|web_search.rs"`
Expected: no output.

- [ ] **Step 8.6: Commit**

```bash
git add cli/src/tools/bash.rs cli/src/tools/monitor.rs cli/src/tools/web_fetch.rs cli/src/tools/web_search.rs
git commit -m "fix(tools): surface MessageQueue/stdout/HTTP-init failures as is_error ToolResults"
```

---

## Task 9: Restructure `tool_search.rs` strip_prefix and `lsp/instance.rs` Uri::from_str

**Files:**
- Modify: `cli/src/tools/tool_search.rs:125-140`
- Modify: `cli/src/lsp/instance.rs:271-275`

- [ ] **Step 9.1: Restructure `tool_search.rs` to avoid double `strip_prefix`**

In `cli/src/tools/tool_search.rs`, replace lines 125-140:

```rust
        // --- +prefix: require term in name ---
        let require_in_name: Option<String> = query
            .strip_prefix('+')
            .and_then(|s| s.split_whitespace().next().map(|t| t.to_lowercase()));

        let search_terms: Vec<&str> = if require_in_name.is_some() {
            let after_plus = query.strip_prefix('+').unwrap();
            let parts: Vec<&str> = after_plus.splitn(2, ' ').collect();
            if parts.len() > 1 {
                parts[1].split_whitespace().collect()
            } else {
                vec![]
            }
        } else {
            query.split_whitespace().collect()
        };
```

with:

```rust
        // --- +prefix: require term in name ---
        let stripped = query.strip_prefix('+');
        let require_in_name: Option<String> = stripped
            .and_then(|s| s.split_whitespace().next().map(|t| t.to_lowercase()));

        let search_terms: Vec<&str> = match stripped {
            Some(after_plus) => {
                let parts: Vec<&str> = after_plus.splitn(2, ' ').collect();
                if parts.len() > 1 {
                    parts[1].split_whitespace().collect()
                } else {
                    vec![]
                }
            }
            None => query.split_whitespace().collect(),
        };
```

- [ ] **Step 9.2: Fix `lsp/instance.rs:274` Uri::from_str unwrap**

In `cli/src/lsp/instance.rs:271-275`, replace:

```rust
        root_uri: Some(
            workspace_uri
                .parse::<Uri>()
                .unwrap_or_else(|_| Uri::from_str("file:///").unwrap()),
        ),
```

with:

```rust
        root_uri: workspace_uri
            .parse::<Uri>()
            .ok()
            .or_else(|| "file:///".parse::<Uri>().ok()),
```

This changes `root_uri` from always-Some to Some-on-parse-success. `InitializeParams.root_uri` is `Option<Uri>`, so this is type-compatible. If both parses fail (impossible — `"file:///"` is hardcoded valid), the LSP gets `None` for root_uri, which is spec-compliant.

- [ ] **Step 9.3: Verify**

Run: `cargo clippy -p super-cli --lib 2>&1 | grep -E "tool_search.rs:131|instance.rs:274"`
Expected: no output.

- [ ] **Step 9.4: Commit**

```bash
git add cli/src/tools/tool_search.rs cli/src/lsp/instance.rs
git commit -m "refactor: restructure strip_prefix and Uri::from_str to remove unwraps"
```

---

## Task 10: LSP `panic_any` → `tracing::error!` + `io::Error`

**Files:**
- Modify: `cli/src/lsp/client.rs:50-68`

- [ ] **Step 10.1: Replace `panic_any` calls with tracing + Err return**

In `cli/src/lsp/client.rs`, replace lines 50-68:

```rust
impl ChildIoThreads {
    fn join(self) -> io::Result<()> {
        // Reader
        match self.reader.join() {
            Ok(r) => r?,
            Err(err) => std::panic::panic_any(err),
        }
        // Dropper
        match self.dropper.join() {
            Ok(_) => (),
            Err(err) => std::panic::panic_any(err),
        }
        // Writer
        match self.writer.join() {
            Ok(r) => r,
            Err(err) => std::panic::panic_any(err),
        }
    }
}
```

with:

```rust
impl ChildIoThreads {
    fn join(self) -> io::Result<()> {
        // Reader
        match self.reader.join() {
            Ok(r) => r?,
            Err(err) => {
                tracing::error!("LSP reader thread panicked: {err:?}");
                return Err(io::Error::other("LSP reader thread panicked"));
            }
        }
        // Dropper
        match self.dropper.join() {
            Ok(_) => (),
            Err(err) => {
                tracing::error!("LSP dropper thread panicked: {err:?}");
                return Err(io::Error::other("LSP dropper thread panicked"));
            }
        }
        // Writer
        match self.writer.join() {
            Ok(r) => r,
            Err(err) => {
                tracing::error!("LSP writer thread panicked: {err:?}");
                Err(io::Error::other("LSP writer thread panicked"))
            }
        }
    }
}
```

The single caller at line 428 is `let _ = io.join();` — the Err is dropped by design (stop is best-effort). `tracing::error!` ensures the panic is still visible in logs.

- [ ] **Step 10.2: Verify**

Run: `cargo clippy -p super-cli --lib 2>&1 | grep "client.rs:5[5,6,0,5]\|client.rs:6[0,5]\|panic_any"`
Expected: no output.

- [ ] **Step 10.3: Commit**

```bash
git add cli/src/lsp/client.rs
git commit -m "fix(lsp): replace panic_any in thread-join rethrow with tracing + io::Error"
```

---

## Task 11: LSP `Builder::spawn` unwraps → Result propagation

**Files:**
- Modify: `cli/src/lsp/client.rs:72-137` (`child_transport` signature + body)
- Modify: `cli/src/lsp/client.rs:234` (caller)

- [ ] **Step 11.1: Change `child_transport` signature**

In `cli/src/lsp/client.rs`, find the function declaration around line 72:

```rust
fn child_transport(
    child_stdout: ChildStdout,
    child_stdin: ChildStdin,
) -> (Connection, ChildIoThreads) {
```

Replace with:

```rust
fn child_transport(
    child_stdout: ChildStdout,
    child_stdin: ChildStdin,
) -> io::Result<(Connection, ChildIoThreads)> {
```

- [ ] **Step 11.2: Replace three `.spawn(...).unwrap()` calls with `?`**

In the body of `child_transport`, at lines 79-89, 91-96, 100-130 (approx), replace each `.unwrap()` at the end of a `thread::Builder::new()...spawn(...)` chain with `?`.

Example for the writer block:

```rust
    let writer = thread::Builder::new()
        .name("LspClientWriter".to_owned())
        .spawn(move || {
            ...
        })
        .unwrap();
```

becomes:

```rust
    let writer = thread::Builder::new()
        .name("LspClientWriter".to_owned())
        .spawn(move || {
            ...
        })?;
```

Apply the same change to the `dropper` and `reader` spawns.

- [ ] **Step 11.3: Update the return value**

At the bottom of `child_transport`, the current return is `(connection, io_threads)`. Wrap in `Ok(...)`:

```rust
    Ok((connection, io_threads))
```

- [ ] **Step 11.4: Update the caller at line 234**

In `cli/src/lsp/client.rs:234`, find:

```rust
        let (connection, io_threads) = child_transport(stdout, stdin);
```

Replace with:

```rust
        let (connection, io_threads) = child_transport(stdout, stdin).map_err(|e| LspError {
            message: format!("Spawn LSP I/O threads: {e}"),
            code: None,
        })?;
```

The enclosing `start` method already returns `Result<(), LspError>` so `?` propagates.

- [ ] **Step 11.5: Verify**

Run: `cargo clippy -p super-cli --lib 2>&1 | grep "client.rs:7[9]\|client.rs:91\|client.rs:100"`
Expected: no output. Run `cargo build -p super-cli` and confirm clean build.

- [ ] **Step 11.6: Commit**

```bash
git add cli/src/lsp/client.rs
git commit -m "fix(lsp): propagate child_transport thread spawn failures as LspError"
```

---

## Task 12: Conversation infra restructures — sse, transcript, sidechain

**Files:**
- Modify: `cli/src/conversation/sse.rs:55-65`
- Modify: `cli/src/conversation/transcript.rs:77-102`
- Modify: `cli/src/conversation/sidechain.rs:32-47`

- [ ] **Step 12.1: Fix `sse.rs:59` with binding pattern**

In `cli/src/conversation/sse.rs`, replace lines 55-65:

```rust
fn find_frame_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    // We accept LF-LF, CRLF-CRLF, and CR-CR as frame terminators per the SSE spec.
    // Search for whichever appears first.
    let candidates: [(&[u8], usize); 3] = [(b"\r\n\r\n", 4), (b"\n\n", 2), (b"\r\r", 2)];
    let mut best: Option<(usize, usize)> = None;
    for (pat, sep_len) in candidates {
        if let Some(idx) = find_subslice(buf, pat) {
            best = Some(match best {
                Some((b_idx, _)) if b_idx <= idx => best.unwrap(),
                _ => (idx, sep_len),
            });
        }
    }
    best
}
```

with:

```rust
fn find_frame_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    // We accept LF-LF, CRLF-CRLF, and CR-CR as frame terminators per the SSE spec.
    // Search for whichever appears first.
    let candidates: [(&[u8], usize); 3] = [(b"\r\n\r\n", 4), (b"\n\n", 2), (b"\r\r", 2)];
    let mut best: Option<(usize, usize)> = None;
    for (pat, sep_len) in candidates {
        if let Some(idx) = find_subslice(buf, pat) {
            best = Some(match best {
                Some(existing @ (b_idx, _)) if b_idx <= idx => existing,
                _ => (idx, sep_len),
            });
        }
    }
    best
}
```

The `existing @ (b_idx, _)` binding pattern captures the whole tuple while still destructuring `b_idx` for the guard — no second `.unwrap()` needed.

- [ ] **Step 12.2: Fix `transcript.rs:79` no-parent path with bus.emit_system(Error)**

In `cli/src/conversation/transcript.rs`, replace lines 77-82:

```rust
pub fn spawn_transcript_writer(bus: Arc<SessionBus>, session_id: String) {
    let path = transcript_path(&session_id);
    if let Err(e) = fs::create_dir_all(path.parent().unwrap()) {
        tracing::warn!("transcript: cannot create dir for {:?}: {e}", path);
        return;
    }
```

with:

```rust
pub fn spawn_transcript_writer(bus: Arc<SessionBus>, session_id: String) {
    let path = transcript_path(&session_id);
    let Some(parent) = path.parent() else {
        let msg = format!("transcript: path has no parent: {:?}", path);
        tracing::warn!("{msg}");
        bus.emit_system(crate::sdk::protocol::SystemSubtype::Error, msg);
        return;
    };
    if let Err(e) = fs::create_dir_all(parent) {
        tracing::warn!("transcript: cannot create dir for {:?}: {e}", path);
        return;
    }
```

- [ ] **Step 12.3: Fix `transcript.rs:101` writer.as_mut().unwrap()**

In `cli/src/conversation/transcript.rs`, find the block around lines 92-101:

```rust
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
```

Replace with:

```rust
                    let w = match writer.as_mut() {
                        Some(w) => w,
                        None => match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => writer.insert(BufWriter::new(f)),
                            Err(e) => {
                                tracing::warn!("transcript: cannot open {:?}: {e}", path);
                                continue;
                            }
                        },
                    };
```

`Option::insert` returns `&mut T` after setting the value — no unwrap needed.

- [ ] **Step 12.4: Fix `sidechain.rs:39` writers.get_mut().unwrap() with entry API**

In `cli/src/conversation/sidechain.rs`, find the block around lines 32-47:

```rust
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
```

Replace with:

```rust
                    use std::collections::hash_map::Entry;
                    let agent_id = msg.session_id().to_string();
                    let writer = match writers.entry(agent_id.clone()) {
                        Entry::Occupied(o) => o.into_mut(),
                        Entry::Vacant(v) => {
                            let path = base_dir.join(format!("{agent_id}.jsonl"));
                            match OpenOptions::new().create(true).append(true).open(&path) {
                                Ok(f) => v.insert(BufWriter::new(f)),
                                Err(e) => {
                                    tracing::warn!("sidechain: cannot open {:?}: {e}", path);
                                    continue;
                                }
                            }
                        }
                    };
```

If `HashMap` is imported under a different path in the file, adjust the `use` accordingly. Verify the import context with `cargo build -p super-cli`.

- [ ] **Step 12.5: Verify**

Run: `cargo clippy -p super-cli --lib 2>&1 | grep -E "sse.rs:59|transcript.rs:(79|101)|sidechain.rs:39"`
Expected: no output.

- [ ] **Step 12.6: Commit**

```bash
git add cli/src/conversation/sse.rs cli/src/conversation/transcript.rs cli/src/conversation/sidechain.rs
git commit -m "fix(conversation): restructure provably-Some unwraps; emit Error on no-parent transcript path"
```

---

## Task 13: Server DB parse fixes — `sqlite_auth_repo.rs`

**Files:**
- Modify: `server/src/adapters/sqlite_auth_repo.rs:82, 85, 105, 108, 146, 150, 198, 200, 245, 246`

- [ ] **Step 13.1: Replace `Uuid::parse_str(...).unwrap()` inside query_map closures**

For each occurrence (lines 82, 105, 198, 245), the pattern is:

```rust
                Ok(User {
                    id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                    ...
                })
```

Replace with:

```rust
                Ok(User {
                    id: Uuid::parse_str(&row.get::<_, String>(0)?).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    ...
                })
```

The `?` propagates to `query_map`, which propagates to the outer `.map_err(|e| AuthError::Internal(e.to_string()))?` already in place. The HTTP response body (via `routes/auth.rs`) carries the message.

Apply at:
- Line 82 (in `find_user_by_email`)
- Line 105 (in `find_user_by_id`)
- Line 198 (in `consume_authorization_code`)
- Line 245 (in `consume_refresh_token`)

- [ ] **Step 13.2: Replace `DateTime::parse_from_rfc3339(...).unwrap()` inside query_map closures**

For each occurrence (lines 85, 108, 146, 150, 200, 246), the pattern is:

```rust
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(3)?)
                    .unwrap()
                    .with_timezone(&Utc),
```

Replace with:

```rust
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(3)?)
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?
                    .with_timezone(&Utc),
```

Apply at all six lines. Line 150 is inside a `.map(|s| { ... })` closure that returns `DateTime<Utc>`; the closure return type must change to `Result<DateTime<Utc>, rusqlite::Error>` and the outer `.map(|s| ...)` becomes `.map(|s| -> rusqlite::Result<_> { Ok(...) }).transpose()?`. Specifically lines 149-153:

```rust
                    _revoked_at: row.get::<_, Option<String>>(3)?.map(|s| {
                        chrono::DateTime::parse_from_rfc3339(&s)
                            .unwrap()
                            .with_timezone(&Utc)
                    }),
```

becomes:

```rust
                    _revoked_at: row
                        .get::<_, Option<String>>(3)?
                        .map(|s| {
                            chrono::DateTime::parse_from_rfc3339(&s).map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    0,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })
                        })
                        .transpose()?
                        .map(|dt| dt.with_timezone(&Utc)),
```

- [ ] **Step 13.3: Verify**

Run: `cargo clippy -p server 2>&1 | grep "sqlite_auth_repo.rs"`
Expected: no output.

Run: `cargo build -p server`
Expected: clean build.

- [ ] **Step 13.4: Commit**

```bash
git add server/src/adapters/sqlite_auth_repo.rs
git commit -m "fix(server): propagate DB parse failures as FromSqlConversionFailure"
```

---

## Task 14: Config path fix — `cli/src/config.rs`

**Files:**
- Modify: `cli/src/config.rs:1-32`

- [ ] **Step 14.1: Change `config_path()` to return `Option<PathBuf>`**

Replace the entire file `cli/src/config.rs`:

```rust
use shared::CliConfig;
use std::path::PathBuf;

pub fn config_path() -> Option<PathBuf> {
    Some(
        dirs::home_dir()?
            .join(".super")
            .join("config.json"),
    )
}

pub fn load_config() -> CliConfig {
    let Some(path) = config_path() else {
        tracing::warn!("HOME not set; running with default config — changes will not persist");
        let mut config = CliConfig::default();
        config.model =
            crate::providers::resolve_slug(&config.provider, &config.model_class).to_string();
        return config;
    };
    let mut config = if path.exists() {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        CliConfig::default()
    };
    config.model =
        crate::providers::resolve_slug(&config.provider, &config.model_class).to_string();
    config
}

pub fn save_config(config: &CliConfig) {
    let Some(path) = config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let content = serde_json::to_string_pretty(config).unwrap_or_default();
    std::fs::write(&path, content).ok();
}
```

- [ ] **Step 14.2: Find and update callers of `config_path()`**

Run: `grep -rn "config_path()" cli/src --include="*.rs"`

For each call site, update to handle `Option<PathBuf>`. If the call is `config::config_path().join(...)`, change to `if let Some(p) = config::config_path() { p.join(...) } else { return / continue / default }`. Each site needs context-appropriate handling.

⚠ If grep finds zero external call sites beyond `config.rs` itself, skip this step.

- [ ] **Step 14.3: Verify**

Run: `cargo clippy -p super-cli --lib 2>&1 | grep "config.rs"`
Expected: no output.

Run: `cargo build -p super-cli`
Expected: clean build (if callers needed updating, this catches missed ones).

- [ ] **Step 14.4: Commit**

```bash
git add cli/src/config.rs
git commit -m "fix(config): return Option<PathBuf> from config_path; warn when HOME unset"
```

---

## Task 15: Server main.rs — return `Result<(), Box<dyn Error>>`

**Files:**
- Modify: `server/src/main.rs:25-48`

- [ ] **Step 15.1: Change `main` to return Result**

In `server/src/main.rs`, replace lines 25-48:

```rust
#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let repo = Arc::new(SqliteAuthRepo::new("super.db").expect("failed to open database"));
    let openrouter = Arc::new(OpenRouterClient::new(
        std::env::var("OPENROUTER_MANAGEMENT_KEY").expect("OPENROUTER_MANAGEMENT_KEY not set"),
    ));
    let service = Arc::new(AuthService::new(repo, openrouter));

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .nest("/auth", routes::auth::routes_with_state(service.clone()))
        .layer(cors);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    tracing::info!("server listening on :3000");
    axum::serve(listener, app).await.unwrap();
}
```

with:

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let repo = Arc::new(
        SqliteAuthRepo::new("super.db")
            .map_err(|e| format!("failed to open database: {e}"))?,
    );
    let openrouter = Arc::new(OpenRouterClient::new(
        std::env::var("OPENROUTER_MANAGEMENT_KEY")
            .map_err(|_| "OPENROUTER_MANAGEMENT_KEY not set")?,
    ));
    let service = Arc::new(AuthService::new(repo, openrouter));

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .nest("/auth", routes::auth::routes_with_state(service.clone()))
        .layer(cors);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .map_err(|e| format!("failed to bind :3000: {e}"))?;
    tracing::info!("server listening on :3000");
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("axum serve error: {e}"))?;
    Ok(())
}
```

- [ ] **Step 15.2: Verify**

Run: `cargo clippy -p server 2>&1 | grep "main.rs"`
Expected: no output.

Run: `cargo build -p server`
Expected: clean build.

- [ ] **Step 15.3: Commit**

```bash
git add server/src/main.rs
git commit -m "fix(server): main returns Result; startup errors print to stderr"
```

---

## Task 16: Final verification

- [ ] **Step 16.1: Run full clippy**

Run: `cargo clippy --all-targets --all-features 2>&1 | tail -5`
Expected: `Finished` line, no `error:` lines. If any violations remain, fix them before continuing.

- [ ] **Step 16.2: Run test suite**

Run: `cargo test --workspace`
Expected: all tests pass. PR #40 added `#![cfg_attr(test, allow(...))]` so test code is exempt; this is a behavioral regression check.

- [ ] **Step 16.3: Run release build**

Run: `cargo build --release`
Expected: clean build.

- [ ] **Step 16.4: Manual smoke — in-conversation error display**

Start the CLI. Invoke web_fetch on an unreachable URL (e.g., `http://does-not-resolve.invalid/`). Verify the tool result appears inline in the transcript styled as an error (red, `⏺` prefix per the tool-result render path), NOT as a panic.

Note: the new `SystemSubtype::Error` variant is wired but no clippy fix in this PR routes through it from steady-state — it's emitted only on the rare transcript-no-parent path in Task 12.2. The variant is infrastructure for future in-conversation error reporting per the spec.

- [ ] **Step 16.5: Push and verify CI clean**

```bash
git push
```

Wait for CI; expect green.

---

## Self-Review Notes

**Spec coverage:** Every spec section maps to a task:
- Spec "Mutex/RwLock 46 sites" → Tasks 2, 3, 4, 5, 6, 7 (Mutex portion).
- Spec "Tool-Path Fixes" → Task 8 + Task 9 (tool_search restructure).
- Spec "LSP Fixes" → Tasks 9 (Uri), 10 (panic_any), 11 (Builder::spawn).
- Spec "Conversation Infra Fixes" → Task 12. Note: my deeper inspection found `message_queue.rs` and `cron_runtime.rs` violations are ALL Mutex (covered in Task 5), and the remaining sites in sse/transcript/sidechain are restructurable rather than needing bus emits — except `transcript.rs:79` which does emit `SystemSubtype::Error` on the no-parent path.
- Spec "Startup Path Fixes" → Tasks 14 (config), 15 (server main).
- Spec "Server DB Layer" → Task 13.
- Spec "New Variant" → Task 1.
- Spec "Verification Per Site" → enforced inline via the explicit fix patterns in each task.

**Placeholder scan:** Each fix step shows the actual replacement code. Task 14.2 ("Find and update callers") includes a grep command and an explicit skip-if-empty branch, not a "TBD".

**Type consistency:** `SystemSubtype::Error` is added in Task 1 and used in Task 12.2 — names match. `child_transport` signature change (Task 11) is consistent between definition and caller. `config_path()` Option return (Task 14) is consistent between definition and the in-file callers updated in the same task.
