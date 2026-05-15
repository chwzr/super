# Subagent execution follow-ups — design

**Status:** approved (pending user sign-off on this file)
**Parent PR:** #11 (worktree-scroll-area-tool-parity)
**Depends on:** `2026-05-15-subagent-execution-design.md` (the subagent implementation already merged on this branch through commit `cf6dfcb`)

---

## Goal

Address the seven follow-up items called out in the final code review of PR #11 so the subagent execution work can merge with full parity rather than punting them to a follow-on PR:

1. **I2** — async-agent shutdown cleanup
2. **I3** — `auto_deny_prompts` is enforced end-to-end
3. **I4** — `subagent_type` schema is generated dynamically from the registry
4. **m3** — sidechain JSONL captures the child engine's `Result` event
5. **m5** — dead-code cleanup in `AgentTool::call`
6. `AsyncAgentHandle.started_at` field
7. TUI drill-in for subagent transcripts (Ctrl+O toggle, matching Claude Code's "detailed transcript" mode)

All seven land as additional commits on branch `worktree-scroll-area-tool-parity` before #11 merges.

---

## Reference behavior (Claude Code, observed via tmux harness)

`use the Explore agent to find where SessionBus is defined` in Claude Code renders:

**Collapsed (default):**

```
⏺ Explore(Locate SessionBus definition)
  ⎿  Done (3 tool uses · 28.8k tokens · 11s)
  (ctrl+o to expand)
```

**Expanded (after Ctrl+O):**

```
⏺ Explore(Locate SessionBus definition)
  ⎿  Prompt:
       Find where SessionBus is defined...
  ⎿  Bash(find ... | head -20)
  ⎿  Bash(grep -r "SessionBus" ...)
  ⎿  Read(/path/to/session_bus.rs)
  ⎿  Response:
       SessionBus is a #[derive(Clone)] struct...
  ⎿  Done (3 tool uses · 28.8k tokens · 11s)
```

The status footer changes to `Showing detailed transcript · ctrl+o to toggle` when expanded. The toggle is a single app-wide boolean, not per-Task.

---

## 1. I2 — Async shutdown cleanup

### Where
- `cli/src/state/store.rs` — add `shutdown_async_agents()` helper.
- `cli/src/tui/app.rs` — the App's exit path. Call the helper before the run loop returns.

### Design

Today `Store::list_async_agents()` and `Store::abort_async_agent()` exist but are called only from tests. On quit, async tokio tasks (registered via `AgentTool::call`'s async branch) keep running until the tokio runtime is dropped — leaking API requests and JSONL writes.

Add to `Store`:

```rust
/// Abort every currently-registered async agent. Called by the TUI's exit
/// path. Returns the number of agents that were signalled.
pub fn shutdown_async_agents(&self) -> usize {
    let ids: Vec<String> = self.list_async_agents()
        .into_iter()
        .map(|h| h.agent_id)
        .collect();
    let mut count = 0;
    for id in &ids {
        if self.abort_async_agent(id) { count += 1; }
    }
    count
}
```

In `cli/src/tui/app.rs`, find the spot where the App's main loop exits (either at the bottom of `run`, or in a `Drop` impl). Call:

```rust
let _ = self.store.shutdown_async_agents();
```

before the function returns. We don't wait for the tasks to finish — abort is fire-and-forget. The child engines see the watch fire and bail out on their next iteration.

### Test

In `cli/src/state/store.rs::tests`, add:

```rust
#[test]
fn shutdown_aborts_all_registered_agents() {
    let store = Store::new();
    let (tx1, mut rx1) = tokio::sync::watch::channel(false);
    let (tx2, mut rx2) = tokio::sync::watch::channel(false);
    store.register_async_agent(AsyncAgentHandle {
        agent_id: "a1".into(), parent_tool_use_id: "tu_1".into(),
        abort: tx1, description: "x".into(), started_at: Instant::now(),
    });
    store.register_async_agent(AsyncAgentHandle {
        agent_id: "a2".into(), parent_tool_use_id: "tu_2".into(),
        abort: tx2, description: "y".into(), started_at: Instant::now(),
    });
    assert_eq!(store.shutdown_async_agents(), 2);
    assert!(*rx1.borrow_and_update());
    assert!(*rx2.borrow_and_update());
    assert!(store.list_async_agents().is_empty());
}
```

(This test also exercises the `started_at` field added in section 6.)

---

## 2. I3 — `auto_deny_prompts` enforcement

The field exists on `ToolCallContext` (added in Task 10 of the original plan) but nothing reads it, and the engine wires `false` even for async children. Two parts.

### 2a. Production reader: `AskUserQuestionTool`

`cli/src/tools/ask_user_question.rs::call` reads `ctx.auto_deny_prompts` at the top:

```rust
async fn call(&self, input: serde_json::Value, ctx: &ToolCallContext) -> ToolResult {
    if ctx.auto_deny_prompts {
        return ToolResult {
            content: "Permission denied: async subagents cannot prompt the user.".into(),
            is_error: true,
            metadata: None,
        };
    }
    // ... existing logic
}
```

(Other prompting tools — none in current `super`, but `ExitPlanMode` is a prompt-like operation — should also short-circuit. Scope to `AskUserQuestionTool` for v1; document the pattern via comment so future tools follow it.)

### 2b. Engine threads the flag correctly

Today `engine.rs::process_prompt` hardcodes `false` for the `auto_deny_prompts` argument to `run_tool_uses` (around the call site introduced in Task 11). For root engines this is always correct. For child engines spawned by `AgentTool` in async mode, this should be `true`.

Add to `ConversationEngine`:

```rust
pub auto_deny_prompts: bool,
```

- `new()` sets it to `false`.
- `new_child()` takes an additional parameter `auto_deny_prompts: bool` and stores it.
- `process_prompt` passes `self.auto_deny_prompts` to `run_tool_uses` (replacing the hardcoded `false`).

In `cli/src/tools/agent.rs::call`, both `new_child` call sites:
- Sync path: passes `false` (sync subagents can still surface prompts to the shared terminal).
- Async path: passes `true`.

### Tests

In `cli/src/tools/ask_user_question.rs` add a test:

```rust
#[tokio::test]
async fn ask_user_question_auto_denies_when_flag_set() {
    let tool = AskUserQuestionTool;
    let ctx = ToolCallContext {
        cwd: std::env::current_dir().unwrap(),
        permission_mode: crate::state::store::PermissionMode::Default,
        abort_signal: None,
        parent_tool_use_id: None,
        bus: None,
        auto_deny_prompts: true,
        tool_use_id: String::new(),
    };
    let result = tool.call(serde_json::json!({"questions": []}), &ctx).await;
    assert!(result.is_error);
    assert!(result.content.contains("async") || result.content.contains("Permission denied"));
}
```

In `cli/src/conversation/engine.rs::tests` extend `new_child_seeds_empty_history_and_overrides_permission_mode` (or add a sibling) to also assert `auto_deny_prompts` is plumbed correctly.

---

## 3. I4 — Dynamic `subagent_type` schema

### Where
- `cli/src/tools/agent.rs::input_schema`

### Design

Today `input_schema()` returns a fixed schema with no enum on `subagent_type`. The schema should advertise every known agent so the model can invoke user `.md` agents (not just built-ins).

Change `input_schema` to read `self.registry.list()`:

```rust
fn input_schema(&self) -> serde_json::Value {
    let agents = self.registry.list();
    let agent_types: Vec<String> = agents.iter()
        .map(|d| d.agent_type.clone())
        .collect();
    let agent_descriptions: String = agents.iter()
        .map(|d| format!("- {}: {}", d.agent_type, d.description))
        .collect::<Vec<_>>()
        .join("\n");
    let subagent_desc = format!(
        "The agent type to use. Available agents:\n{agent_descriptions}"
    );

    json!({
        "type": "object",
        "properties": {
            "description":   { "type": "string", "description": "A short (3-5 word) description of the task" },
            "prompt":        { "type": "string", "description": "The task for the agent to perform" },
            "subagent_type": { "type": "string", "enum": agent_types, "description": subagent_desc },
            "model":         { "type": "string", "enum": ["sonnet", "opus", "haiku", "inherit"] },
            "run_in_background": { "type": "boolean" }
        },
        "required": ["description", "prompt", "subagent_type"]
    })
}
```

The registry is loaded once at boot and not mutated, so re-running this on every schema read is cheap. No caching needed.

### Test

In `cli/src/tools/agent.rs::tests`:

```rust
#[test]
fn input_schema_lists_registered_agents() {
    let store = Arc::new(Store::new());
    let cfg = shared::CliConfig::default();
    let agent_reg = Arc::new(AgentRegistry::built_in_only());
    let tool_reg = ToolRegistry::new(store.clone(), cfg.clone(), agent_reg.clone());
    let tool = AgentTool {
        store, config: cfg, registry: agent_reg, tool_registry: tool_reg,
    };
    let schema = tool.input_schema();
    let st = &schema["properties"]["subagent_type"];
    let enum_vals: Vec<&str> = st["enum"].as_array().unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(enum_vals.contains(&"general-purpose"));
    assert!(enum_vals.contains(&"Explore"));
    assert!(enum_vals.contains(&"Plan"));
    let desc = st["description"].as_str().unwrap();
    assert!(desc.contains("Explore"));
}
```

---

## 4. m3 — Sidechain captures child `Result`

### Protocol change

`BusMessage::Result` currently has no `parent_tool_use_id` field. Add it:

```rust
#[serde(rename = "result")]
Result {
    stop_reason: Option<String>,
    usage: AnthropicUsage,
    total_cost_usd: f64,
    duration_ms: u64,
    num_turns: u32,
    #[serde(default)]
    parent_tool_use_id: Option<String>,
    uuid: Uuid,
    session_id: String,
},
```

### Engine change

`engine.rs::process_prompt` emits `Result` once per `process_prompt` call. The function already has `parent_tool_use_id` as a parameter — stamp the new field with `parent_tool_use_id.clone()`.

### Accessor update

`BusMessage::parent_tool_use_id()` currently returns `None` for the `Result` variant (special case). Drop the special case; include `Result` in the OR-chain.

### Filter update

`cli/src/tui/transcript.rs::matches_filter` already routes `Result` to `parent: None` because the field didn't exist. Drop that — include `Result` in the OR-chain alongside the other variants.

### Test

In `cli/src/conversation/sidechain.rs::tests`, extend the existing test or add a new one:

```rust
#[tokio::test]
async fn sidechain_captures_child_result_event() {
    let tmp = tempfile_dir();
    let bus = Arc::new(SessionBus::new("s-root".into()));
    spawn_sidechain_writer(bus.clone(), tmp.clone());

    // Child engine emits a Result with parent_tool_use_id.
    bus.emit(BusMessage::Result {
        stop_reason: Some("end_turn".into()),
        usage: AnthropicUsage::default(),
        total_cost_usd: 0.01,
        duration_ms: 1234,
        num_turns: 3,
        parent_tool_use_id: Some("tu_parent".into()),
        uuid: Uuid::new_v4(),
        session_id: "agent-1".into(),
    });

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let path = tmp.join("agent-1.jsonl");
    assert!(path.exists());
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("\"type\":\"result\""));
    assert!(contents.contains("\"num_turns\":3"));
}
```

---

## 5. m5 — Dead-code cleanup

Delete two lines in `cli/src/tools/agent.rs::call`:

```rust
let agent_type_for_task = agent_def.agent_type.clone();
let _ = agent_type_for_task;
```

The `agent_type` is already available via `agent_def.agent_type` later in the function (or via the cloned metadata). No test needed.

---

## 6. `AsyncAgentHandle.started_at`

### Change

In `cli/src/state/store.rs`:

```rust
#[derive(Clone)]
pub struct AsyncAgentHandle {
    pub agent_id: String,
    pub parent_tool_use_id: String,
    pub abort: tokio::sync::watch::Sender<bool>,
    pub description: String,
    pub started_at: std::time::Instant,  // NEW
}
```

In `cli/src/tools/agent.rs::call` (async path), populate `started_at: std::time::Instant::now()` when constructing the handle.

In the existing tests for `register_and_complete_async_agent` and `abort_async_agent_signals_watch`, add `started_at: std::time::Instant::now()` to the handle literal.

### Why

No production consumer yet. The field is added so future code (TUI status line, debugging) can read runtime without rippling more changes through the type. Matches the spec from the original design doc.

---

## 7. TUI drill-in (Ctrl+O toggle)

### Where
- `cli/src/tui/app.rs` — handle the Ctrl+O keypress.
- `cli/src/tui/scroll_area.rs` — render the indented child transcript when expanded.
- `cli/src/sdk/protocol.rs` — already has the data (events with `parent_tool_use_id`); no protocol change here.

### App state

Add to whatever struct holds app state in `tui::app`:

```rust
pub show_detailed_transcript: bool,  // default false
```

### Input handler

In the existing key handler (search for `KeyCode::` matches), add an arm for Ctrl+O:

```rust
KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
    self.show_detailed_transcript = !self.show_detailed_transcript;
}
```

### Rendering changes in `scroll_area.rs`

`ScrollArea` already has access to the full event slice. Today it calls `fold(events, None)` and renders root items. The Task tool-call render needs two new paths.

**Collapsed (default, `show_detailed_transcript == false`):**

For a `TranscriptItem::ToolCall { tool_use_id, name: "Task", .. }`:

```
⏺ Task (Explore) find auth code
  ⎿ Done (N tool uses · ~K tokens · Ys)
```

The summary is derived from a new helper, `task_done_summary(events, tool_use_id) -> String`:

- Counts child `TranscriptItem::ToolCall` entries via `fold(events, Some(tool_use_id))`.
- Sums output_tokens from any child `BusMessage::Result` event whose `session_id` matches the child's agent_id. The agent_id isn't directly known from the parent's view — but every event with `parent_tool_use_id == Some(tool_use_id)` shares the same child `session_id`. Walk the events once to find that `session_id`, then locate its `Result`.
- `duration_ms` comes from that `Result` too.

Pseudocode:

```rust
fn task_done_summary(events: &[BusMessage], tool_use_id: &str) -> Option<String> {
    // After section 4 (m3), child Result events carry parent_tool_use_id.
    // Find the most recent matching Result.
    let result = events.iter().rev().find(|e| {
        matches!(e, BusMessage::Result { .. })
            && e.parent_tool_use_id() == Some(tool_use_id)
    })?;
    let (tokens, duration_ms) = match result {
        BusMessage::Result { usage, duration_ms, .. } =>
            (usage.output_tokens + usage.input_tokens, *duration_ms),
        _ => unreachable!(),
    };
    let tool_count = fold(events, Some(tool_use_id))
        .iter()
        .filter(|i| matches!(i, TranscriptItem::ToolCall { .. }))
        .count();
    let secs = duration_ms / 1000;
    Some(format!("Done ({tool_count} tool uses · {} · {secs}s)", format_tokens(tokens)))
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M tokens", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k tokens", n as f64 / 1_000.0)
    } else {
        format!("{n} tokens")
    }
}
```

If the child has not yet emitted `Result` (still running), the summary line is omitted and the renderer shows a 1-second ticker (`⎿ <agent_type> running…`) just like other in-flight tool calls.

**Expanded (`show_detailed_transcript == true`):**

After the `⏺ Task (...)` header line, recursively render `fold(events, Some(tool_use_id))` indented by 2 spaces, with each child item rendered as a `⎿ <type>(<inputs>)` block per Claude Code style:

- Child `User` (the synthetic prompt) → `⎿ Prompt:\n     <text>`
- Child `ToolCall` → `⎿ <ToolName>(<summarized inputs>)`
- Child `AssistantText` → `⎿ Response:\n     <text>`
- The final `⎿ Done (...)` line stays at the bottom in both modes.

Subagent-spawned subagents (nested Task calls) render at the same indent in v1; recursion is technically allowed but Claude Code's expansion stops at one level visually. Match v1 to single-level expansion.

### Footer

In `tui::app`'s status footer rendering, when `show_detailed_transcript == true`, replace the default footer text with `Showing detailed transcript · ctrl+o to toggle`. Otherwise unchanged.

### Hint line

In collapsed mode, after each Task tool-call render, append a single dim line `(ctrl+o to expand)`. (Claude Code shows it under every Task; matching exactly is fine even if it's a little redundant.)

### Tests

Three tests in `cli/src/tui/scroll_area.rs::tests`:

```rust
#[test]
fn task_done_summary_computes_from_child_events() {
    // construct a BusMessage event vec with a Task tool_use plus a child
    // session that emits a Result; assert the summary string contains
    // the right tool_use count, token count, and duration.
}

#[test]
fn task_done_summary_returns_none_when_child_still_running() {
    // events contain the Task tool_use_id but no child Result yet → None
}

#[test]
fn expanded_render_includes_child_prompt_and_response() {
    // Build a ScrollArea, set show_detailed_transcript=true, render to a Buffer,
    // assert the rendered text contains the child's prompt text and response text
    // both indented under the Task line.
}
```

The third test uses ratatui's `Terminal::with_backend(TestBackend::new(120, 40))` to render to a buffer; assertions probe `buffer.content()` for substring matches. If TestBackend isn't already a dev-dep, add it via `ratatui = { features = ["...", "test"] }` or `dev-dependencies = { ratatui = { features = ["test"] } }`.

---

## Migration order

Each step is one commit; tests stay green at every step.

1. **m5** — drop dead lines. (1 commit, ~5 LOC.)
2. **m3** — protocol additive: `BusMessage::Result.parent_tool_use_id`; engine stamps it; accessor + matches_filter pick it up; sidechain test covers it. (1 commit, ~30 LOC + 1 test.)
3. **AsyncAgentHandle.started_at** — field add + 2 test-site updates. (1 commit, ~10 LOC.)
4. **I3** — `ConversationEngine.auto_deny_prompts` field + `new_child` param + AgentTool wires `is_async`; `AskUserQuestionTool` reads the flag. (1 commit, ~30 LOC + 2 tests.)
5. **I4** — dynamic `subagent_type` schema. (1 commit, ~25 LOC + 1 test.)
6. **I2** — `Store::shutdown_async_agents` + App exit-path call. (1 commit, ~15 LOC + 1 test.)
7. **TUI drill-in** — Ctrl+O toggle, expanded rendering, Done summary, footer. (1 commit, ~100–150 LOC + 3 tests.) Bulk of the work but isolated to two files.

Total: ~7 commits, ~220 LOC, ~8–9 new tests, no protocol breaks.

---

## Out of scope / explicitly deferred

- **Multi-level drill-in nesting.** Subagent-of-subagent expansion stops at one level visually. Tracked for a v2 follow-up; the data layer supports it via recursive `fold(events, filter)` calls.
- **Per-Task expand state.** Claude Code's Ctrl+O is global. We match that. Per-Task expand (mouse-clickable `[+]`) is deferred.
- **SendMessage delivery of `AsyncAgentDone`.** Surfacing async-completion events into the parent's next message instead of the scroll-area system row. Listed in the original spec's "out of scope" — still deferred.
- **Other prompting tools beyond `AskUserQuestionTool`.** `ExitPlanMode` is somewhat prompt-shaped but reachable only when the parent is in `plan` mode; subagents in plan mode are an unusual case. Defer until a concrete failure surfaces.

---

## Acceptance checklist

- [ ] `cargo test -p super-cli` passes with ≥ 78 tests (current 70 + 8 new).
- [ ] `Store::shutdown_async_agents` is called from the App's exit path.
- [ ] `AskUserQuestionTool` returns is_error when `ctx.auto_deny_prompts`.
- [ ] `ConversationEngine.auto_deny_prompts` is `true` for async children, `false` everywhere else.
- [ ] `AgentTool::input_schema()['properties']['subagent_type']['enum']` contains every registered agent's `agent_type`.
- [ ] `BusMessage::Result` has `parent_tool_use_id`; sidechain JSONL for a child agent contains its Result line.
- [ ] `agent_type_for_task` is gone from `agent.rs`.
- [ ] `AsyncAgentHandle.started_at` is populated by `AgentTool::call`.
- [ ] Pressing Ctrl+O in the TUI toggles `show_detailed_transcript`.
- [ ] In collapsed mode, a finished Task renders `⎿ Done (N tool uses · K tokens · Xs)` followed by `(ctrl+o to expand)`.
- [ ] In expanded mode, the subagent's prompt, internal tool calls, and final response render indented under the Task line.
- [ ] The status footer says `Showing detailed transcript · ctrl+o to toggle` when expanded.
- [ ] tmux harness diff between super and Claude Code for a Task tool-call is limited to model identity, super's mandated glyphs (per CLAUDE.md), and version strings.
