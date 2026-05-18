# Clippy Warnings Cleanup — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix or suppress all 14 `cargo clippy --workspace` warnings, achieving zero-warning output.

**Architecture:** Mechanical, safe fixes across 13 files. Two `too_many_arguments` warnings get `#[allow]`; all others get proper fixes. No new dependencies. No logic changes.

**Tech Stack:** Rust, cargo clippy

---

### Task 1: Fix `empty_line_after_doc_comments` (web_fetch_preapproved.rs)

**Files:**
- Modify: `cli/src/tools/web_fetch_preapproved.rs:3-5`

- [ ] **Step 1: Remove blank line after doc comment**

The file currently has a blank line between the doc comment and the function:

```rust
/// hosts can be added as needed.

pub fn is_preapproved(_url: &str) -> bool {
```

Remove line 4 (the blank line) so the doc comment sits directly above the function:

```rust
/// hosts can be added as needed.
pub fn is_preapproved(_url: &str) -> bool {
```

- [ ] **Step 2: Commit**

```bash
git add cli/src/tools/web_fetch_preapproved.rs
git commit -m "fix: remove empty line after doc comment in web_fetch_preapproved"
```

---

### Task 2: Fix `ptr_arg` (agents/registry.rs)

**Files:**
- Modify: `cli/src/agents/registry.rs:20`

- [ ] **Step 1: Change `&PathBuf` to `&Path`**

Change the signature of `load`:

```rust
pub fn load(cwd: &Path) -> Self {
```

The body already calls `cwd.join(...)` which works on `&Path`.

- [ ] **Step 2: Commit**

```bash
git add cli/src/agents/registry.rs
git commit -m "fix: change &PathBuf to &Path in AgentRegistry::load"
```

---

### Task 3: Fix `type_complexity` (commands/registry.rs)

**Files:**
- Modify: `cli/src/commands/registry.rs:45`

- [ ] **Step 1: Add type alias and use it**

Inside `register_builtins`, add a type alias before the vec and use it in the declaration and destructuring:

```rust
fn register_builtins(&mut self) {
    type BuiltinDef<'a> = (&'a str, &'a [&'a str], &'a str, Option<&'static str>, CommandKind);

    let builtins: Vec<BuiltinDef> = vec![
        // ... entries unchanged ...
    ];

    for (name, aliases, description, argument_hint, kind) in builtins {
        // ... body unchanged ...
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add cli/src/commands/registry.rs
git commit -m "fix: add type alias for builtin command tuple in CommandRegistry"
```

---

### Task 4: Allow `too_many_arguments` (engine.rs)

**Files:**
- Modify: `cli/src/conversation/engine.rs:81`

- [ ] **Step 1: Add allow attribute**

Add `#[allow(clippy::too_many_arguments)]` immediately before `pub fn new_child`:

```rust
    #[allow(clippy::too_many_arguments)]
    pub fn new_child(
        store: Arc<Store>,
        config: CliConfig,
        registry: Arc<ToolRegistry>,
        bus: Arc<SessionBus>,
        agent_id: String,
        abort: Option<watch::Receiver<bool>>,
        permission_mode_override: Option<PermissionMode>,
        auto_deny_prompts: bool,
    ) -> Self {
```

- [ ] **Step 2: Commit**

```bash
git add cli/src/conversation/engine.rs
git commit -m "fix: allow too_many_arguments for ConversationEngine::new_child"
```

---

### Task 5: Fix `ptr_arg` (system_prompt.rs)

**Files:**
- Modify: `cli/src/conversation/system_prompt.rs:9,29`

- [ ] **Step 1: Change both functions to take `&Path`**

Change `SystemPrompt::build`:

```rust
pub fn build(cwd: &Path) -> Self {
```

Change `load_claude_md`:

```rust
fn load_claude_md(cwd: &Path) -> Option<String> {
```

The body of `load_claude_md` already calls `cwd.as_path()` — this now becomes redundant but harmless (calling `.as_path()` on a `&Path` returns itself). Simplify to:

```rust
fn load_claude_md(cwd: &Path) -> Option<String> {
    let mut dir = Some(cwd);
```

The caller in `main.rs` passes `&PathBuf` which coerces to `&Path`, so no change needed there.

- [ ] **Step 2: Commit**

```bash
git add cli/src/conversation/system_prompt.rs
git commit -m "fix: change &PathBuf to &Path in SystemPrompt::build and load_claude_md"
```

---

### Task 6: Allow `too_many_arguments` (tool_loop.rs)

**Files:**
- Modify: `cli/src/conversation/tool_loop.rs:15`

- [ ] **Step 1: Add allow attribute**

Add `#[allow(clippy::too_many_arguments)]` immediately before `pub async fn run_tool_uses`:

```rust
#[allow(clippy::too_many_arguments)]
pub async fn run_tool_uses(
    registry: &ToolRegistry,
    tool_uses: Vec<(String, String, serde_json::Value)>,
    cwd: std::path::PathBuf,
    permission_mode: PermissionMode,
    abort_signal: Option<watch::Receiver<bool>>,
    bus: Arc<SessionBus>,
    parent_tool_use_id: Option<String>,
    session_id: String,
    auto_deny_prompts: bool,
) -> Vec<ContentBlockFinal> {
```

- [ ] **Step 2: Commit**

```bash
git add cli/src/conversation/tool_loop.rs
git commit -m "fix: allow too_many_arguments for run_tool_uses"
```

---

### Task 7: Fix `option_map_unit_fn` (mcp/client.rs)

**Files:**
- Modify: `cli/src/mcp/client.rs:74-81`

- [ ] **Step 1: Replace `.map()` with `if let`**

Replace:

```rust
        self.servers
            .write()
            .await
            .get_mut(name)
            .map(|s| {
                s.connected = true;
                s.tools = tools;
            });
```

With:

```rust
        if let Some(s) = self.servers.write().await.get_mut(name) {
            s.connected = true;
            s.tools = tools;
        }
```

- [ ] **Step 2: Commit**

```bash
git add cli/src/mcp/client.rs
git commit -m "fix: replace option_map_unit_fn with if let in McpManager::connect"
```

---

### Task 8: Fix `type_complexity` (state/store.rs)

**Files:**
- Modify: `cli/src/state/store.rs:87`

- [ ] **Step 1: Add type alias and update field**

Add a module-level type alias near the top, after the `use` statements or right before the `Store` struct:

```rust
type Subscriber = Box<dyn Fn(&AppState) + Send + Sync>;
```

Change the field declaration in `Store` from:

```rust
    subscribers: Arc<RwLock<Vec<Box<dyn Fn(&AppState) + Send + Sync>>>>,
```

To:

```rust
    subscribers: Arc<RwLock<Vec<Subscriber>>>,
```

The `subscribe` method already wraps the listener in `Box::new(listener)` — no change needed there. The `Clone` impl and `new` also need no changes since they use `Arc::new(RwLock::new(Vec::new()))` which infers the type.

- [ ] **Step 2: Verify it builds**

```bash
cargo build -p super-cli
```

- [ ] **Step 3: Commit**

```bash
git add cli/src/state/store.rs
git commit -m "fix: add Subscriber type alias to simplify complex type in Store"
```

---

### Task 9: Fix `result_large_err` (notebook_edit.rs)

**Files:**
- Modify: `cli/src/tools/notebook_edit.rs:300-329`

- [ ] **Step 1: Change return type and wrap Err values**

Change the function signature from:

```rust
) -> Result<usize, ToolResult> {
```

To:

```rust
) -> Result<usize, Box<ToolResult>> {
```

Wrap each `Err(ToolResult { ... })` with `Box::new(...)`. There are three sites:

1. Lines 308-315 — cell not found by id:

```rust
                None => Err(Box::new(ToolResult {
                    content: format!(
                        "Cell with id '{}' not found in notebook {}",
                        cid, notebook_path
                    ),
                    is_error: true,
                    ..Default::default()
                })),
```

2. Lines 320-324 — notebook has no cells:

```rust
                Err(Box::new(ToolResult {
                    content: format!("Notebook {} has no cells to edit", notebook_path),
                    is_error: true,
                    ..Default::default()
                })),
```

3. Two call sites at lines 163 and 216 currently do:

```rust
                let idx = match cell_idx {
                    Ok(i) => i,
                    Err(e) => return e,
                };
```

Change to:

```rust
                let idx = match cell_idx {
                    Ok(i) => i,
                    Err(e) => return *e,
                };
```

- [ ] **Step 2: Verify it builds**

```bash
cargo build -p super-cli
```

- [ ] **Step 3: Commit**

```bash
git add cli/src/tools/notebook_edit.rs
git commit -m "fix: box ToolResult in find_cell_index to reduce Err variant size"
```

---

### Task 10: Fix `vec_init_then_push` (tools/mod.rs)

**Files:**
- Modify: `cli/src/tools/mod.rs:85-100`

- [ ] **Step 1: Replace Vec::new() + push with vec![...]**

The current code creates the vec manually and then registers tools via `registry.register(...)` for back-reference tools. Keep the two-phase structure but replace the initial batch of pushes with a vec literal.

Replace:

```rust
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();

        // Standard tools
        tools.push(Arc::new(ReadTool));
        tools.push(Arc::new(EditTool));
        tools.push(Arc::new(WriteTool));
        tools.push(Arc::new(GlobTool));
        tools.push(Arc::new(GrepTool));
        tools.push(Arc::new(NotebookEditTool));
        tools.push(Arc::new(BashTool));
        tools.push(Arc::new(ConfigTool));
        tools.push(Arc::new(WebFetchTool));
        tools.push(Arc::new(WebSearchTool));

        // LSP
        tools.push(Arc::new(LspTool));

        // Cron tools (share the same job registry)
        let cron_jobs = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        tools.push(Arc::new(CronCreateTool { jobs: cron_jobs.clone() }));
        tools.push(Arc::new(CronDeleteTool { jobs: cron_jobs.clone() }));
        tools.push(Arc::new(CronListTool { jobs: cron_jobs.clone() }));

        // Sleep
        tools.push(Arc::new(SleepTool));

        // Monitor (stub)
        tools.push(Arc::new(MonitorTool));

        // StructuredOutput
        tools.push(Arc::new(StructuredOutputTool));

        // Worktree tools (stubs)
        tools.push(Arc::new(EnterWorktreeTool));
        tools.push(Arc::new(ExitWorktreeTool));
```

With:

```rust
        let cron_jobs = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let mut tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(ReadTool),
            Arc::new(EditTool),
            Arc::new(WriteTool),
            Arc::new(GlobTool),
            Arc::new(GrepTool),
            Arc::new(NotebookEditTool),
            Arc::new(BashTool),
            Arc::new(ConfigTool),
            Arc::new(WebFetchTool),
            Arc::new(WebSearchTool),
            Arc::new(LspTool),
            Arc::new(CronCreateTool { jobs: cron_jobs.clone() }),
            Arc::new(CronDeleteTool { jobs: cron_jobs.clone() }),
            Arc::new(CronListTool { jobs: cron_jobs.clone() }),
            Arc::new(SleepTool),
            Arc::new(MonitorTool),
            Arc::new(StructuredOutputTool),
            Arc::new(EnterWorktreeTool),
            Arc::new(ExitWorktreeTool),
        ];
```

Note: `cron_jobs` declaration must move to BEFORE the vec literal since it's cloned inside the vec entries. The rest of the function (registry creation, `register` calls) stays unchanged.

- [ ] **Step 2: Verify it builds**

```bash
cargo build -p super-cli
```

- [ ] **Step 3: Commit**

```bash
git add cli/src/tools/mod.rs
git commit -m "fix: use vec![] macro instead of Vec::new() + push in ToolRegistry::new"
```

---

### Task 11: Fix `collapsible_match` (tui/app.rs)

**Files:**
- Modify: `cli/src/tui/app.rs:605-617`

- [ ] **Step 1: Collapse if let into outer match**

Replace:

```rust
                        BusMessage::InteractionRequested { tool_use_id, spec, parent_tool_use_id, .. } => {
                            if let shared::RenderSpec::Interactive {
                                widget: shared::InteractiveWidget::MultiQuestion { questions },
                                ..
                            } = spec {
                                self.pending_interaction = Some(PendingInteraction {
                                    tool_use_id: tool_use_id.clone(),
                                    parent_tool_use_id: parent_tool_use_id.clone(),
                                });
                                self.modal = Some(Modal::Question(
                                    crate::tui::modals::question::QuestionModal::new(questions.clone()),
                                ));
                            }
                        }
```

With:

```rust
                        BusMessage::InteractionRequested {
                            tool_use_id,
                            spec: shared::RenderSpec::Interactive {
                                widget: shared::InteractiveWidget::MultiQuestion { questions },
                                ..
                            },
                            parent_tool_use_id,
                            ..
                        } => {
                            self.pending_interaction = Some(PendingInteraction {
                                tool_use_id: tool_use_id.clone(),
                                parent_tool_use_id: parent_tool_use_id.clone(),
                            });
                            self.modal = Some(Modal::Question(
                                crate::tui::modals::question::QuestionModal::new(questions.clone()),
                            ));
                        }
```

Non-Interactive RenderSpec variants fall through to the existing `_ => {}` wildcard at the end of the match.

- [ ] **Step 2: Verify it builds**

```bash
cargo build -p super-cli
```

- [ ] **Step 3: Commit**

```bash
git add cli/src/tui/app.rs
git commit -m "fix: collapse if let into match arm for InteractionRequested in TUI"
```

---

### Task 12: Fix server `dead_code` warnings

**Files:**
- Modify: `server/src/adapters/openrouter_client.rs:22-29`
- Modify: `server/src/domain/auth/ports.rs:15-35`
- Modify: `server/src/adapters/sqlite_auth_repo.rs:141-154`

- [ ] **Step 1: Prefix dead fields in OpenRouterKeyResponse**

In `openrouter_client.rs`, rename `limit` and `disabled`:

```rust
struct OpenRouterKeyResponse {
    key: String,
    name: Option<String>,
    label: Option<String>,
    #[serde(default, rename = "limit")]
    _limit: Option<f64>,
    #[serde(default, rename = "disabled")]
    _disabled: bool,
}
```

- [ ] **Step 2: Prefix dead fields in ApiKey**

In `ports.rs`, rename `created_at` and `revoked_at`:

```rust
pub struct ApiKey {
    pub openrouter_key_id: String,
    pub openrouter_key_value: String,
    pub _created_at: DateTime<Utc>,
    pub _revoked_at: Option<DateTime<Utc>>,
}
```

- [ ] **Step 3: Prefix dead fields in OpenRouterKey**

In `ports.rs`, rename `label` and `limit_usd`:

```rust
pub struct OpenRouterKey {
    pub id: String,
    pub key: String,
    pub _label: String,
    pub _limit_usd: u32,
}
```

- [ ] **Step 4: Update OpenRouterKey construction in openrouter_client.rs**

At lines 75-80, update the field names:

```rust
        Ok(OpenRouterKey {
            id: label.to_string(),
            key: key.key,
            _label: label.to_string(),
            _limit_usd: limit_usd,
        })
```

- [ ] **Step 5: Update ApiKey construction in sqlite_auth_repo.rs**

At lines 141-154, update the field names:

```rust
                Ok(ApiKey {
                    openrouter_key_id: row.get(0)?,
                    openrouter_key_value: row.get(1)?,
                    _created_at: chrono::DateTime::parse_from_rfc3339(
                        &row.get::<_, String>(2)?,
                    )
                    .unwrap()
                    .with_timezone(&Utc),
                    _revoked_at: row.get::<_, Option<String>>(3)?.map(|s| {
                        chrono::DateTime::parse_from_rfc3339(&s)
                            .unwrap()
                            .with_timezone(&Utc)
                    }),
                })
```

- [ ] **Step 6: Verify server builds**

```bash
cargo build -p server
```

- [ ] **Step 7: Commit**

```bash
git add server/src/adapters/openrouter_client.rs server/src/domain/auth/ports.rs server/src/adapters/sqlite_auth_repo.rs
git commit -m "fix: prefix unused fields with underscore in server dead_code warnings"
```

---

### Task 13: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Run clippy on entire workspace**

```bash
cargo clippy --workspace 2>&1
```

Expected: zero warnings.

- [ ] **Step 2: Run full build**

```bash
cargo build --workspace
```

Expected: success.

- [ ] **Step 3: Run tests**

```bash
cargo test --workspace
```

Expected: all tests pass.

- [ ] **Step 4: Commit if any final cleanup was needed**

```bash
git status
```

(If clean, no commit needed. If small fixes were required from verification, commit them.)
