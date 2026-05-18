# Clippy Warnings Cleanup — Design Spec

## Scope

Fix or suppress all 14 `cargo clippy --workspace` warnings present on `main` as of
2026-05-18 (commit `36608c7`). 11 warnings in `cli/`, 3 in `server/`.

## Strategy

- **Fix** where the change is mechanical and safe.
- **Suppress** only `too_many_arguments` (2 instances) — extracting a params struct
  for these two functions would be pure ceremony with no readability win.

---

## cli fixes (11 warnings)

### 1. `empty_line_after_doc_comments` — `tools/web_fetch_preapproved.rs:3`

**Fix:** Remove the blank line (line 4) between the doc comment and the function.

### 2. `ptr_arg` — `agents/registry.rs:20`

**Fix:** Change `cwd: &PathBuf` to `cwd: &Path`. The `cwd.join(...)` call in the
body works unchanged (`&Path::join` exists).

### 3. `type_complexity` — `commands/registry.rs:45`

**Fix:** Add a private type alias inside `register_builtins`:

```rust
type BuiltinDef<'a> = (&'a str, &'a [&'a str], &'a str, Option<&'static str>, CommandKind);
```

Use it in the vec declaration and the destructuring `for` loop.

### 4. `too_many_arguments` — `conversation/engine.rs:81` (`new_child`, 8 args)

**Fix:** Add `#[allow(clippy::too_many_arguments)]` on `new_child`.

### 5. `ptr_arg` — `conversation/system_prompt.rs:29`

**Fix:** Change both `load_claude_md(cwd: &PathBuf)` and `SystemPrompt::build(cwd:
&PathBuf)` to take `&Path`. The single call site in `main.rs` already passes
a `&PathBuf` which coerces to `&Path`.

### 6. `too_many_arguments` — `conversation/tool_loop.rs:15` (`run_tool_uses`, 9 args)

**Fix:** Add `#[allow(clippy::too_many_arguments)]` on `run_tool_uses`.

### 7. `option_map_unit_fn` — `mcp/client.rs:74`

**Fix:** Replace `.get_mut(name).map(|s| { ... })` with:

```rust
if let Some(s) = self.servers.write().await.get_mut(name) {
    s.connected = true;
    s.tools = tools;
}
```

### 8. `type_complexity` — `state/store.rs:87`

**Fix:** Add a module-level type alias:

```rust
type Subscriber = Box<dyn Fn(&AppState) + Send + Sync>;
```

Change the field to `subscribers: Arc<RwLock<Vec<Subscriber>>>`. The `push` call
in `subscribe` already returns `Box<dyn Fn(...)>` — no change needed there.

### 9. `result_large_err` — `tools/notebook_edit.rs:300` (`find_cell_index`)

**Fix:**
- Change return type to `Result<usize, Box<ToolResult>>`.
- Wrap the three `Err(ToolResult { ... })` sites with `Box::new(...)`.
- At the two call sites (lines 163, 216), the `Err(e)` match arm does `return
  e` — change to `return *e` to dereference the `Box<ToolResult>`.

### 10. `vec_init_then_push` — `tools/mod.rs:85`

**Fix:** Replace `Vec::new()` + push sequence with a `vec![...]` literal. The
`cron_jobs` `Arc` must be declared first so it can be cloned into the three cron
tool constructors inside the vec:

```rust
let cron_jobs = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
let mut tools: Vec<Arc<dyn Tool>> = vec![
    Arc::new(ReadTool),
    Arc::new(EditTool),
    // ... all tools that don't need cron_jobs ...
    Arc::new(CronCreateTool { jobs: cron_jobs.clone() }),
    Arc::new(CronDeleteTool { jobs: cron_jobs.clone() }),
    Arc::new(CronListTool { jobs: cron_jobs.clone() }),
    // ... remaining standalone tools ...
];
```

### 11. `collapsible_match` — `tui/app.rs:606`

**Fix:** Collapse the `if let` into the outer match arm:

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

Non-Interactive specs fall through to the existing `_ => {}` wildcard.

---

## server fixes (3 warnings)

### 12. `dead_code` — `openrouter_client.rs:26,28`

**Cause:** `limit` and `disabled` fields are deserialized but never read.

**Fix:** Prefix with `_` and add `#[serde(rename)]` so serde still matches the
original JSON keys:

```rust
#[serde(default, rename = "limit")]
_limit: Option<f64>,
#[serde(default, rename = "disabled")]
_disabled: bool,
```

### 13. `dead_code` — `auth/ports.rs:18,19`

**Cause:** `created_at` and `revoked_at` in `ApiKey` are never read.

**Fix:** Rename to `_created_at` and `_revoked_at`. Update the one construction
site in `server/src/adapters/sqlite_auth_repo.rs:141-154` to use the new names.

### 14. `dead_code` — `auth/ports.rs:33,34`

**Cause:** `label` and `limit_usd` in `OpenRouterKey` are never read.

**Fix:** Rename to `_label` and `_limit_usd`. Update the one construction site
in `server/src/adapters/openrouter_client.rs:75-80` to use the new names.

---

## Files touched

| File | Changes |
|------|---------|
| `cli/src/tools/web_fetch_preapproved.rs` | Remove blank line |
| `cli/src/agents/registry.rs` | `&PathBuf` → `&Path` |
| `cli/src/commands/registry.rs` | Type alias for builtin tuple |
| `cli/src/conversation/engine.rs` | `#[allow]` on `new_child` |
| `cli/src/conversation/system_prompt.rs` | Two `&PathBuf` → `&Path` |
| `cli/src/conversation/tool_loop.rs` | `#[allow]` on `run_tool_uses` |
| `cli/src/mcp/client.rs` | `if let` instead of `.map()` |
| `cli/src/state/store.rs` | Type alias for subscriber, rename field |
| `cli/src/tools/notebook_edit.rs` | `Box<ToolResult>` in `find_cell_index` |
| `cli/src/tools/mod.rs` | `vec![...]` instead of push sequence |
| `cli/src/tui/app.rs` | Collapse `if let` into match |
| `server/src/adapters/openrouter_client.rs` | Prefix dead fields, rename in construction |
| `server/src/domain/auth/ports.rs` | Prefix dead fields in `ApiKey` and `OpenRouterKey` |
| `server/src/adapters/sqlite_auth_repo.rs` | Update `ApiKey` construction with new field names |

## Verification

After all changes:
- `cargo clippy --workspace` produces zero warnings.
- `cargo build --workspace` succeeds.
- `cargo test --workspace` passes.
