# Subagent Execution Follow-ups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the seven follow-up gaps identified in the final code review of PR #11 so the subagent execution work merges with full parity to Claude Code.

**Architecture:** Each item is small and isolated. Most are protocol-additive or one-tool changes. The TUI drill-in is the largest single piece — an app-level boolean toggled by Ctrl+O drives whether Task tool-call items render the child's full transcript or a collapsed `Done (...)` summary.

**Tech Stack:** Rust (`super-cli`), `ratatui`, `tokio`, `serde_json`. Crate is binary-only — run tests with `cargo test -p super-cli` (no `--lib`).

**Reference:** `docs/superpowers/specs/2026-05-15-subagent-followups-design.md` (spec) and the tmux-harness observation that Claude Code's Ctrl+O is an app-wide toggle, status footer changes to `Showing detailed transcript · ctrl+o to toggle`.

---

## File Structure

**Modified files (no new files):**

- `cli/src/sdk/protocol.rs` — add `parent_tool_use_id: Option<String>` to `BusMessage::Result` (Task 2).
- `cli/src/conversation/engine.rs` — stamp the new field; add `auto_deny_prompts` field + `new_child` param (Tasks 2, 4).
- `cli/src/tui/transcript.rs` — drop `Result`'s `matches_filter` special case (Task 2).
- `cli/src/state/store.rs` — `AsyncAgentHandle.started_at`; `shutdown_async_agents` (Tasks 3, 6).
- `cli/src/tools/agent.rs` — drop dead lines; populate `started_at`; thread `is_async` into `auto_deny_prompts`; dynamic `subagent_type` schema (Tasks 1, 3, 4, 5).
- `cli/src/tools/ask_user_question.rs` — read `ctx.auto_deny_prompts` (Task 4).
- `cli/src/tui/app.rs` — Ctrl+O handler, shutdown call, footer change (Tasks 6, 7).
- `cli/src/tui/scroll_area.rs` — collapsed Done summary, expanded child-transcript rendering, ctrl+o hint line (Task 7).

---

## Task 1: Drop dead lines (m5)

**Why first:** Tiniest change; sets up a clean diff baseline for subsequent commits.

**Files:**
- Modify: `cli/src/tools/agent.rs` (around lines 155-157 in current code)

- [ ] **Step 1: Find the dead lines**

```bash
grep -n "agent_type_for_task" cli/src/tools/agent.rs
```

Expected: two lines — one declaration, one `let _ = ...` drop.

- [ ] **Step 2: Delete both lines**

Open `cli/src/tools/agent.rs`. Find the block in `call()` that looks like:

```rust
        let store_for_task = self.store.clone();
        let bus_for_task = bus.clone();
        let agent_id_for_task = agent_id.clone();
        let parent_tu_for_task = parent_tool_use_id.clone();
        let agent_type_for_task = agent_def.agent_type.clone();
        let _ = agent_type_for_task;
```

Delete the two `agent_type_for_task` lines. The block becomes:

```rust
        let store_for_task = self.store.clone();
        let bus_for_task = bus.clone();
        let agent_id_for_task = agent_id.clone();
        let parent_tu_for_task = parent_tool_use_id.clone();
```

- [ ] **Step 3: Verify build and tests still pass**

```bash
cargo test -p super-cli 2>&1 | tail -3
```

Expected: 70 tests pass.

- [ ] **Step 4: Commit**

```bash
git add cli/src/tools/agent.rs
git commit -m "chore(agent-tool): drop dead agent_type_for_task lines"
```

---

## Task 2: `BusMessage::Result` carries `parent_tool_use_id` (m3)

**Files:**
- Modify: `cli/src/sdk/protocol.rs` (Result variant; accessor; round-trip test)
- Modify: `cli/src/conversation/engine.rs` (Result emit at line ~267)
- Modify: `cli/src/tui/transcript.rs` (matches_filter)
- Modify: `cli/src/conversation/sidechain.rs` (add a test)

- [ ] **Step 1: Write the failing protocol round-trip test**

Append to `#[cfg(test)] mod tests` in `cli/src/sdk/protocol.rs`:

```rust
    #[test]
    fn result_message_carries_parent_tool_use_id() {
        let msg = BusMessage::Result {
            stop_reason: Some("end_turn".into()),
            usage: AnthropicUsage::default(),
            total_cost_usd: 0.01,
            duration_ms: 1234,
            num_turns: 3,
            parent_tool_use_id: Some("tu_parent".into()),
            uuid: Uuid::new_v4(),
            session_id: "agent-1".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"parent_tool_use_id\":\"tu_parent\""));
        let back: BusMessage = serde_json::from_str(&json).unwrap();
        match back {
            BusMessage::Result { parent_tool_use_id, num_turns, .. } => {
                assert_eq!(parent_tool_use_id.as_deref(), Some("tu_parent"));
                assert_eq!(num_turns, 3);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn result_message_accessor_returns_parent_tool_use_id() {
        let msg = BusMessage::Result {
            stop_reason: None,
            usage: AnthropicUsage::default(),
            total_cost_usd: 0.0,
            duration_ms: 0,
            num_turns: 1,
            parent_tool_use_id: Some("tu_x".into()),
            uuid: Uuid::new_v4(),
            session_id: "child".into(),
        };
        assert_eq!(msg.parent_tool_use_id(), Some("tu_x"));
        assert_eq!(msg.session_id(), "child");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p super-cli sdk::protocol::tests::result_message 2>&1 | head -20
```

Expected: compile error — `parent_tool_use_id` not a field of `Result`.

- [ ] **Step 3: Add the field to `BusMessage::Result`**

In `cli/src/sdk/protocol.rs`, find the `Result` variant (search for `#[serde(rename = "result")]`). Currently:

```rust
    #[serde(rename = "result")]
    Result {
        stop_reason: Option<String>,
        usage: AnthropicUsage,
        total_cost_usd: f64,
        duration_ms: u64,
        num_turns: u32,
        uuid: Uuid,
        session_id: String,
    },
```

Change to:

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

- [ ] **Step 4: Update `BusMessage::parent_tool_use_id` accessor**

Find the accessor in `cli/src/sdk/protocol.rs` (currently around line 268). It special-cases `Result` to return `None`. Change to include `Result` in the OR-chain:

```rust
    pub fn parent_tool_use_id(&self) -> Option<&str> {
        match self {
            BusMessage::User { parent_tool_use_id, .. }
            | BusMessage::Assistant { parent_tool_use_id, .. }
            | BusMessage::StreamEvent { parent_tool_use_id, .. }
            | BusMessage::ToolProgress { parent_tool_use_id, .. }
            | BusMessage::SystemEvent { parent_tool_use_id, .. }
            | BusMessage::Result { parent_tool_use_id, .. } => parent_tool_use_id.as_deref(),
        }
    }
```

- [ ] **Step 5: Update `matches_filter` in transcript.rs**

In `cli/src/tui/transcript.rs` (around line 173):

```rust
fn matches_filter(ev: &BusMessage, filter: Option<&str>) -> bool {
    let parent = match ev {
        BusMessage::User { parent_tool_use_id, .. }
        | BusMessage::Assistant { parent_tool_use_id, .. }
        | BusMessage::StreamEvent { parent_tool_use_id, .. }
        | BusMessage::ToolProgress { parent_tool_use_id, .. }
        | BusMessage::SystemEvent { parent_tool_use_id, .. }
        | BusMessage::Result { parent_tool_use_id, .. } => parent_tool_use_id.as_deref(),
    };
    match filter {
        None => parent.is_none(),
        Some(want) => parent == Some(want),
    }
}
```

(The `Result` variant is no longer a separate arm.)

- [ ] **Step 6: Update engine emit site to stamp the field**

In `cli/src/conversation/engine.rs` around line 267, change:

```rust
                self.bus.emit(BusMessage::Result {
                    stop_reason: stop_reason.clone(),
                    usage,
                    total_cost_usd: 0.0,
                    duration_ms: started.elapsed().as_millis() as u64,
                    num_turns,
                    uuid: Uuid::new_v4(),
                    session_id: session_id.clone(),
                });
```

to:

```rust
                self.bus.emit(BusMessage::Result {
                    stop_reason: stop_reason.clone(),
                    usage,
                    total_cost_usd: 0.0,
                    duration_ms: started.elapsed().as_millis() as u64,
                    num_turns,
                    parent_tool_use_id: parent_tool_use_id.clone(),
                    uuid: Uuid::new_v4(),
                    session_id: session_id.clone(),
                });
```

- [ ] **Step 7: Update existing test sites that construct `BusMessage::Result`**

```bash
grep -rn "BusMessage::Result {" cli/src/ | grep -v "pub struct"
```

Expected sites:
- `cli/src/sdk/protocol.rs::tests` — `result_message_*` tests (already updated in Step 1).
- `cli/src/conversation/session_bus.rs::tests` — `subscribe_receives_emitted_messages`.
- `cli/src/tui/transcript.rs::tests` — `result_message_does_not_add_transcript_item`.
- Any other site.

For each, add `parent_tool_use_id: None,` after `num_turns: <n>,`. Example:

```rust
let msg = BusMessage::Result {
    stop_reason: Some("end_turn".into()),
    usage: AnthropicUsage::default(),
    total_cost_usd: 0.0,
    duration_ms: 0,
    num_turns: 1,
    parent_tool_use_id: None,
    uuid: Uuid::new_v4(),
    session_id: "s1".into(),
};
```

- [ ] **Step 8: Add sidechain capture test**

In `cli/src/conversation/sidechain.rs::tests`, append a new test:

```rust
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
            uuid: Uuid::new_v4(),
            session_id: "agent-1".into(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let path = tmp.join("agent-1.jsonl");
        assert!(path.exists(), "expected sidechain at {:?}", path);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("\"type\":\"result\""), "got: {contents}");
        assert!(contents.contains("\"num_turns\":3"), "got: {contents}");
    }
```

The `tempfile_dir()` helper already exists from the prior test.

- [ ] **Step 9: Run all tests**

```bash
cargo test -p super-cli 2>&1 | tail -5
```

Expected: 73 tests pass (70 + 3 new: `result_message_carries_parent_tool_use_id`, `result_message_accessor_returns_parent_tool_use_id`, `sidechain_captures_child_result_event`).

- [ ] **Step 10: Commit**

```bash
git add cli/src/sdk/protocol.rs cli/src/conversation/engine.rs cli/src/conversation/session_bus.rs cli/src/conversation/sidechain.rs cli/src/tui/transcript.rs
git commit -m "feat(protocol): BusMessage::Result carries parent_tool_use_id; sidechain captures child Result events"
```

---

## Task 3: `AsyncAgentHandle.started_at` field

**Files:**
- Modify: `cli/src/state/store.rs` (struct + test sites)
- Modify: `cli/src/tools/agent.rs` (populate at registration)

- [ ] **Step 1: Add the field to the struct**

In `cli/src/state/store.rs`, find:

```rust
#[derive(Clone)]
pub struct AsyncAgentHandle {
    pub agent_id: String,
    pub parent_tool_use_id: String,
    pub abort: tokio::sync::watch::Sender<bool>,
    pub description: String,
}
```

Change to:

```rust
#[derive(Clone)]
pub struct AsyncAgentHandle {
    pub agent_id: String,
    pub parent_tool_use_id: String,
    pub abort: tokio::sync::watch::Sender<bool>,
    pub description: String,
    pub started_at: std::time::Instant,
}
```

- [ ] **Step 2: Update test sites in store.rs**

In `cli/src/state/store.rs::tests`, find both `AsyncAgentHandle { ... }` constructions in `register_and_complete_async_agent` and `abort_async_agent_signals_watch`. Append `started_at: std::time::Instant::now(),` to each literal.

- [ ] **Step 3: Update AgentTool::call to populate the field**

In `cli/src/tools/agent.rs`, find the async-path block that constructs `AsyncAgentHandle`. Add `started_at: std::time::Instant::now(),` to the literal:

```rust
        let handle = AsyncAgentHandle {
            agent_id: agent_id.clone(),
            parent_tool_use_id: parent_tool_use_id.clone(),
            abort: abort_tx,
            description: description.to_string(),
            started_at: std::time::Instant::now(),
        };
        self.store.register_async_agent(handle);
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p super-cli state::store::tests
```

Expected: existing 2 tests still pass. Full suite still at 73.

- [ ] **Step 5: Commit**

```bash
git add cli/src/state/store.rs cli/src/tools/agent.rs
git commit -m "feat(store): AsyncAgentHandle.started_at field"
```

---

## Task 4: `auto_deny_prompts` enforcement (I3)

This task has three parts: (a) add the field to `ConversationEngine`, (b) `AgentTool` passes the right value for async children, (c) `AskUserQuestionTool` reads the flag and short-circuits.

**Files:**
- Modify: `cli/src/conversation/engine.rs` (add field, update `new`, update `new_child` signature)
- Modify: `cli/src/tools/agent.rs` (both `new_child` call sites)
- Modify: `cli/src/tools/ask_user_question.rs` (read the flag)

- [ ] **Step 1: Write the failing tests**

In `cli/src/conversation/engine.rs::tests`, append:

```rust
    #[test]
    fn new_child_propagates_auto_deny_prompts_flag() {
        use crate::conversation::session_bus::SessionBus;
        let bus = std::sync::Arc::new(SessionBus::new("s-root".into()));
        let store_arc = std::sync::Arc::new(crate::state::store::Store::new());
        let agent_reg = std::sync::Arc::new(crate::agents::AgentRegistry::built_in_only());
        let registry = crate::tools::ToolRegistry::new(
            store_arc.clone(),
            shared::CliConfig::default(),
            agent_reg,
        );
        let sync_child = ConversationEngine::new_child(
            store_arc.clone(),
            shared::CliConfig::default(),
            registry.clone(),
            bus.clone(),
            "agent-sync".into(),
            None,
            None,
            false,
        );
        let async_child = ConversationEngine::new_child(
            store_arc,
            shared::CliConfig::default(),
            registry,
            bus,
            "agent-async".into(),
            None,
            None,
            true,
        );
        assert!(!sync_child.auto_deny_prompts);
        assert!(async_child.auto_deny_prompts);
    }
```

In `cli/src/tools/ask_user_question.rs`, add a test module at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::PermissionMode;

    #[tokio::test]
    async fn auto_denies_when_flag_set() {
        let tool = AskUserQuestionTool;
        let ctx = ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: true,
            tool_use_id: String::new(),
        };
        let result = tool.call(serde_json::json!({"question": "ok?"}), &ctx).await;
        assert!(result.is_error);
        assert!(result.content.contains("async") || result.content.contains("Permission denied"));
    }

    #[tokio::test]
    async fn allows_when_flag_unset() {
        let tool = AskUserQuestionTool;
        let ctx = ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: false,
            tool_use_id: String::new(),
        };
        let result = tool.call(serde_json::json!({"question": "ok?"}), &ctx).await;
        assert!(!result.is_error);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p super-cli conversation::engine::tests::new_child_propagates_auto_deny_prompts_flag tools::ask_user_question::tests 2>&1 | head -30
```

Expected: compile errors — `new_child` doesn't take 8 args; `auto_deny_prompts` field missing on `ConversationEngine`.

- [ ] **Step 3: Add the field to `ConversationEngine`**

In `cli/src/conversation/engine.rs`, find `pub struct ConversationEngine` and append:

```rust
    /// Forwarded to `ToolCallContext.auto_deny_prompts` for every tool call
    /// driven by this engine. `true` for async subagent engines, `false`
    /// everywhere else (root, sync subagents).
    pub auto_deny_prompts: bool,
```

In `ConversationEngine::new`, set `auto_deny_prompts: false` alongside the other defaults.

In `ConversationEngine::new_child`, change the signature to accept the flag. Current signature (post-Task-12 + the C1/I1 fix):

```rust
    pub fn new_child(
        store: Arc<Store>,
        config: CliConfig,
        registry: Arc<ToolRegistry>,
        bus: Arc<SessionBus>,
        agent_id: String,
        abort: Option<watch::Receiver<bool>>,
        permission_mode_override: Option<PermissionMode>,
    ) -> Self {
```

Change to:

```rust
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
        Self {
            store,
            config,
            registry,
            bus,
            abort,
            session_id_override: Some(agent_id),
            history_override: Some(Vec::new()),
            permission_mode_override,
            auto_deny_prompts,
        }
    }
```

Also update the body of `process_prompt`: change the hardcoded `false` in the `run_tool_uses` call (the 9th positional argument, `auto_deny_prompts`) to `self.auto_deny_prompts`. Find it:

```bash
grep -n "false, *// auto_deny" cli/src/conversation/engine.rs
```

Replace that arg with `self.auto_deny_prompts,`.

- [ ] **Step 4: Update the two `new_child` call sites in `agent.rs`**

In `cli/src/tools/agent.rs::call`:

**Sync path** (around line 107):

```rust
            let child = ConversationEngine::new_child(
                self.store.clone(),
                child_config,
                child_registry,
                bus.clone(),
                agent_id.clone(),
                ctx.abort_signal.clone(),
                Some(child_perm),
                false,
            );
```

**Async path** (around line 143):

```rust
        let child = ConversationEngine::new_child(
            self.store.clone(),
            child_config,
            child_registry,
            bus.clone(),
            agent_id.clone(),
            Some(abort_rx),
            Some(child_perm.clone()),
            true,
        );
```

(`child_perm` needs to be `clone()`d in the async case because it was previously moved into the sync branch's call. Adjust the binding order if needed: compute `child_perm` once before the branch, then pass `Some(child_perm.clone())` in the sync path too OR move the construction into each branch.)

The simplest pattern: compute once and clone both times:

```rust
        let child_perm = resolve_permission_mode(
            &ctx.permission_mode,
            agent_def.permission_mode.as_ref(),
            is_async,
        );
        // Sync uses Some(child_perm.clone()), async uses Some(child_perm.clone()) too.
```

- [ ] **Step 5: Update existing `new_child` test sites**

```bash
grep -n "ConversationEngine::new_child" cli/src/conversation/engine.rs
```

The existing tests `new_child_constructs_with_overrides` and `new_child_seeds_empty_history_and_overrides_permission_mode` call `new_child` with 7 args. Add `false` as the 8th argument to each.

- [ ] **Step 6: Implement `AskUserQuestionTool` auto-deny**

In `cli/src/tools/ask_user_question.rs`, modify `call`:

```rust
    async fn call(&self, input: serde_json::Value, context: &ToolCallContext) -> ToolResult {
        if context.auto_deny_prompts {
            return ToolResult {
                content: "Permission denied: async subagents cannot prompt the user.".into(),
                is_error: true,
                metadata: None,
            };
        }
        let question = input["question"].as_str().unwrap_or("");
        ToolResult {
            content: format!("Question displayed: {question}"),
            is_error: false,
            metadata: Some([
                ("question".into(), question.into()),
                ("needs_response".into(), "true".into()),
            ].into()),
        }
    }
```

- [ ] **Step 7: Run all tests**

```bash
cargo test -p super-cli 2>&1 | tail -5
```

Expected: 76 tests pass (73 + 3 new).

- [ ] **Step 8: Commit**

```bash
git add cli/src/conversation/engine.rs cli/src/tools/agent.rs cli/src/tools/ask_user_question.rs
git commit -m "feat(engine): thread auto_deny_prompts through child engines; AskUserQuestion auto-denies for async subagents"
```

---

## Task 5: Dynamic `subagent_type` schema (I4)

**Files:**
- Modify: `cli/src/tools/agent.rs`

- [ ] **Step 1: Write the failing test**

Append to `cli/src/tools/agent.rs::tests`:

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
        assert!(enum_vals.contains(&"general-purpose"), "missing general-purpose: {enum_vals:?}");
        assert!(enum_vals.contains(&"Explore"), "missing Explore: {enum_vals:?}");
        assert!(enum_vals.contains(&"Plan"), "missing Plan: {enum_vals:?}");
        let desc = st["description"].as_str().unwrap();
        assert!(desc.contains("Explore"), "description missing Explore: {desc}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-cli tools::agent::tests::input_schema_lists_registered_agents 2>&1 | head -20
```

Expected: FAIL because `enum` is missing on `subagent_type`.

- [ ] **Step 3: Replace `input_schema`**

In `cli/src/tools/agent.rs`, find `fn input_schema(&self) -> serde_json::Value` and replace its body:

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
                "subagent_type": {
                    "type": "string",
                    "enum": agent_types,
                    "description": subagent_desc,
                },
                "model":         { "type": "string", "enum": ["sonnet", "opus", "haiku", "inherit"] },
                "run_in_background": { "type": "boolean" }
            },
            "required": ["description", "prompt", "subagent_type"]
        })
    }
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p super-cli tools::agent::tests
```

Expected: all 3 agent-tool tests pass (`agent_tool_errors_on_unknown_subagent_type`, `agent_tool_name_is_task`, `input_schema_lists_registered_agents`).

- [ ] **Step 5: Commit**

```bash
git add cli/src/tools/agent.rs
git commit -m "feat(agent-tool): dynamic subagent_type schema from AgentRegistry"
```

---

## Task 6: Async shutdown cleanup (I2)

**Files:**
- Modify: `cli/src/state/store.rs` (add `shutdown_async_agents` helper)
- Modify: `cli/src/tui/app.rs` (call it before exit)

- [ ] **Step 1: Write the failing test**

In `cli/src/state/store.rs::tests`, append:

```rust
    #[test]
    fn shutdown_aborts_all_registered_agents() {
        let store = Store::new();
        let (tx1, mut rx1) = tokio::sync::watch::channel(false);
        let (tx2, mut rx2) = tokio::sync::watch::channel(false);
        store.register_async_agent(AsyncAgentHandle {
            agent_id: "a1".into(),
            parent_tool_use_id: "tu_1".into(),
            abort: tx1,
            description: "x".into(),
            started_at: std::time::Instant::now(),
        });
        store.register_async_agent(AsyncAgentHandle {
            agent_id: "a2".into(),
            parent_tool_use_id: "tu_2".into(),
            abort: tx2,
            description: "y".into(),
            started_at: std::time::Instant::now(),
        });
        assert_eq!(store.shutdown_async_agents(), 2);
        assert!(*rx1.borrow_and_update());
        assert!(*rx2.borrow_and_update());
        assert!(store.list_async_agents().is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-cli state::store::tests::shutdown_aborts_all_registered_agents 2>&1 | head -20
```

Expected: compile error — `shutdown_async_agents` doesn't exist.

- [ ] **Step 3: Implement the helper**

In `cli/src/state/store.rs`, inside `impl Store` (near `abort_async_agent` and `list_async_agents`), add:

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
            if self.abort_async_agent(id) {
                count += 1;
            }
        }
        count
    }
```

- [ ] **Step 4: Wire into the App exit path**

In `cli/src/tui/app.rs`, find `pub fn run` (around line 547):

```rust
    pub fn run(&mut self, mut terminal: DefaultTerminal) -> std::io::Result<()> {
        while !self.should_quit {
            self.activity.tick();
            terminal.draw(|f| self.render(f))?;
            self.handle_event()?;
            self.process_pending();
        }
        Ok(())
    }
```

Change to:

```rust
    pub fn run(&mut self, mut terminal: DefaultTerminal) -> std::io::Result<()> {
        while !self.should_quit {
            self.activity.tick();
            terminal.draw(|f| self.render(f))?;
            self.handle_event()?;
            self.process_pending();
        }
        // Best-effort cleanup of any async subagents still running.
        let _ = self.store.shutdown_async_agents();
        Ok(())
    }
```

This assumes `self.store: Arc<Store>` is accessible on the App. Verify by reading around line 67 (the field list) — if the field is named differently, adapt. If there's no `store` field at all, find where the App is constructed and check what's passed in.

If `self.store` doesn't exist on App, fall back to calling shutdown from `run_with_engine` (around line 663) after the App's `run` returns. Example shape:

```rust
pub async fn run_with_engine(...) {
    // ... existing app construction ...
    let store_for_cleanup = store.clone();
    // ... app.run ...
    let _ = store_for_cleanup.shutdown_async_agents();
}
```

Pick whichever site has access to `store` and is reached on every shutdown path (including Ctrl+C via `should_quit = true`).

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-cli 2>&1 | tail -5
```

Expected: 78 tests pass (76 + 2: `shutdown_aborts_all_registered_agents`, plus whichever Task 4/5 added that I'm undercounting).

Actually: 76 + 1 = 77.

- [ ] **Step 6: Commit**

```bash
git add cli/src/state/store.rs cli/src/tui/app.rs
git commit -m "feat(store): Store::shutdown_async_agents; called from App exit path"
```

---

## Task 7: TUI drill-in — `Ctrl+O` toggle + collapsed/expanded rendering

This task is the largest. Split into three commits internally:

**Files:**
- Modify: `cli/src/tui/app.rs` (state, key handler, footer)
- Modify: `cli/src/tui/scroll_area.rs` (Done summary helper, collapsed/expanded render)

- [ ] **Step 1: Add the `show_detailed_transcript` field to `App`**

In `cli/src/tui/app.rs`, find the App struct (around line 67) and add:

```rust
    /// When true, Task tool-call items render their child transcript inline.
    /// Default false. Toggled by Ctrl+O.
    show_detailed_transcript: bool,
```

In the `App::new` (or wherever `should_quit: false` is initialized — likely around line 124), add `show_detailed_transcript: false,` to the literal.

- [ ] **Step 2: Plumb the flag into the scroll-area renderer**

`ScrollArea` already owns the events vector. Pass the flag in at render time. In `cli/src/tui/app.rs`, find where `self.scroll_area.render(...)` is called (search for `scroll_area.render`). If the signature is e.g. `render(area, frame)`, change it to `render(area, frame, self.show_detailed_transcript)`.

Then in `cli/src/tui/scroll_area.rs`, find the `render` method (around line 154 where `fold(&self.events, None)` is called) and add the parameter to its signature:

```rust
pub fn render(&self, area: Rect, frame: &mut Frame, show_detailed_transcript: bool) {
    // ... existing code ...
}
```

The simplest path is to make it a field on `ScrollArea` instead — set it from the App on every key press. Pick whichever pattern matches the existing render flow (which can be inferred from how other render flags like `slash_menu_open` are passed today). If `slash_menu_open` is read by the renderer via a function parameter, follow the same pattern; if it's a field on the rendered struct, set a field.

For consistency, the recommended approach is:
- Add `pub show_detailed_transcript: bool` field on `ScrollArea` (default `false`).
- Provide a `set_show_detailed_transcript(&mut self, v: bool)` setter.
- Have `App::handle_key` call `self.scroll_area.set_show_detailed_transcript(self.show_detailed_transcript)` whenever the flag changes, OR have `App::run`'s draw loop call the setter once per frame before invoking render.

Simpler: just keep one source of truth on the `ScrollArea` directly and have the Ctrl+O handler call `self.scroll_area.toggle_detailed_transcript()`. The App doesn't need its own field. Update the spec interpretation accordingly:

```rust
// In cli/src/tui/scroll_area.rs:
impl ScrollArea {
    pub fn toggle_detailed_transcript(&mut self) {
        self.show_detailed_transcript = !self.show_detailed_transcript;
    }

    pub fn is_detailed_transcript(&self) -> bool {
        self.show_detailed_transcript
    }
}
```

Add the field on the `ScrollArea` struct (default `false`). Skip plumbing through `App` entirely.

- [ ] **Step 3: Add the `Ctrl+O` key handler**

In `cli/src/tui/app.rs`, inside the `if key.modifiers.contains(KeyModifiers::CONTROL)` block (around line 308), add an arm:

```rust
                KeyCode::Char('o') => {
                    self.scroll_area.toggle_detailed_transcript();
                    return Ok(());
                }
```

Place it alongside the existing `Char('c')`, `Char('l')`, `Char('a')`, `Char('e')` arms.

- [ ] **Step 4: Write the `task_done_summary` helper + tests**

Append to `cli/src/tui/scroll_area.rs` (above the `impl ScrollArea` block, as free functions):

```rust
/// Compute the "Done (N tool uses · K tokens · Xs)" summary for a Task
/// tool-call. Returns None if the child has not yet emitted Result.
pub fn task_done_summary(events: &[crate::sdk::protocol::BusMessage], tool_use_id: &str) -> Option<String> {
    use crate::sdk::protocol::BusMessage;
    let result = events.iter().rev().find(|e| {
        matches!(e, BusMessage::Result { .. }) && e.parent_tool_use_id() == Some(tool_use_id)
    })?;
    let (tokens, duration_ms) = match result {
        BusMessage::Result { usage, duration_ms, .. } => (
            usage.output_tokens + usage.input_tokens,
            *duration_ms,
        ),
        _ => unreachable!(),
    };
    let tool_count = fold(events, Some(tool_use_id))
        .iter()
        .filter(|i| matches!(i, TranscriptItem::ToolCall { .. }))
        .count();
    let secs = duration_ms / 1000;
    Some(format!(
        "Done ({tool_count} tool uses · {} · {secs}s)",
        format_tokens(tokens)
    ))
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

Then add tests at the bottom of the test module:

```rust
    #[test]
    fn task_done_summary_computes_from_child_events() {
        let tu = "tu_task".to_string();
        let events = vec![
            // Child Result for the Task
            BusMessage::Result {
                stop_reason: Some("end_turn".into()),
                usage: crate::sdk::protocol::AnthropicUsage {
                    input_tokens: 1000,
                    output_tokens: 500,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
                total_cost_usd: 0.0,
                duration_ms: 11_000,
                num_turns: 1,
                parent_tool_use_id: Some(tu.clone()),
                uuid: Uuid::new_v4(),
                session_id: "agent-1".into(),
            },
        ];
        let summary = task_done_summary(&events, &tu).expect("returns summary");
        assert!(summary.contains("0 tool uses"), "got: {summary}");
        assert!(summary.contains("1.5k tokens"), "got: {summary}");
        assert!(summary.contains("11s"), "got: {summary}");
    }

    #[test]
    fn task_done_summary_returns_none_when_no_result() {
        let events: Vec<BusMessage> = vec![];
        assert!(task_done_summary(&events, "tu_x").is_none());
    }
```

The test imports may need `BusMessage`, `Uuid` already in scope; if not, add them at the top of the test module:

```rust
    use crate::sdk::protocol::BusMessage;
    use uuid::Uuid;
```

- [ ] **Step 5: Run the summary tests**

```bash
cargo test -p super-cli tui::scroll_area::tests::task_done_summary
```

Expected: both PASS.

- [ ] **Step 6: Integrate the summary into the Task render**

Find the Task render arm in `cli/src/tui/scroll_area.rs::render` (around line 190 where it does `TranscriptItem::ToolCall { name, input, result, .. }`). Currently the match arm renders the tool name and input summary, then attaches the result via `render_tool_result(...)`.

Add a special-case for `name == "Task"` that:
- In **collapsed mode** (`!self.show_detailed_transcript`): render the header line, then `⎿ Done (...)` summary if available (via `task_done_summary`), and a single dim hint `(ctrl+o to expand)`.
- In **expanded mode**: render the header, then for each child `TranscriptItem` from `fold(&self.events, Some(tool_use_id))`, render an indented `⎿ <type>(<inputs>)` line. End with the Done line if available.

Get the `tool_use_id` from the existing destructure. The structure is:

```rust
                TranscriptItem::ToolCall { tool_use_id, name, input, result, .. } => {
                    lines.push(Line::from(""));
                    let summary = summarize_tool_call(&name, &input);
                    lines.push(Line::from(vec![
                        Span::styled("⏺ ", assistant_prefix_style),
                        Span::styled(name.clone(), tool_style),
                        Span::raw(" "),
                        Span::styled(summary, dim),
                    ]));
                    if name == "Task" {
                        let child_items = fold(&self.events, Some(&tool_use_id));
                        if self.show_detailed_transcript {
                            // Render each child item indented
                            for child in &child_items {
                                render_child_indented(&mut lines, child, &dim);
                            }
                        }
                        // Done summary if Result has arrived
                        if let Some(summary) = task_done_summary(&self.events, &tool_use_id) {
                            lines.push(Line::from(vec![
                                Span::styled("  ⎿ ", dim),
                                Span::styled(summary, dim),
                            ]));
                        }
                        if !self.show_detailed_transcript {
                            lines.push(Line::from(vec![
                                Span::styled("  (ctrl+o to expand)", dim),
                            ]));
                        }
                    } else if let Some(r) = result {
                        render_tool_result(&mut lines, &r, &dim);
                    }
                }
```

Implement `render_child_indented`:

```rust
fn render_child_indented(
    lines: &mut Vec<Line<'_>>,
    item: &TranscriptItem,
    dim: &Style,
) {
    match item {
        TranscriptItem::User { text } => {
            lines.push(Line::from(vec![
                Span::styled("  ⎿ ", *dim),
                Span::styled("Prompt:", *dim),
            ]));
            for body in text.lines() {
                lines.push(Line::from(vec![
                    Span::styled("       ", *dim),
                    Span::styled(body.to_string(), *dim),
                ]));
            }
        }
        TranscriptItem::AssistantText { text, .. } => {
            lines.push(Line::from(vec![
                Span::styled("  ⎿ ", *dim),
                Span::styled("Response:", *dim),
            ]));
            for body in text.lines() {
                lines.push(Line::from(vec![
                    Span::styled("       ", *dim),
                    Span::styled(body.to_string(), *dim),
                ]));
            }
        }
        TranscriptItem::ToolCall { name, input, .. } => {
            let inner_summary = summarize_tool_call(name, input);
            lines.push(Line::from(vec![
                Span::styled("  ⎿ ", *dim),
                Span::styled(format!("{name}({inner_summary})"), *dim),
            ]));
        }
        TranscriptItem::Thinking { text, .. } => {
            lines.push(Line::from(vec![
                Span::styled("  ⎿ ", *dim),
                Span::styled("Thinking:", *dim),
            ]));
            for body in text.lines() {
                lines.push(Line::from(vec![
                    Span::styled("       ", *dim),
                    Span::styled(body.to_string(), *dim),
                ]));
            }
        }
        TranscriptItem::System { .. } => {
            // Skip — system events inside subagents are noise in the drill-in.
        }
    }
}
```

(Note: `Style` doesn't implement `Copy` cleanly in all ratatui versions. If the compiler complains, use `.clone()` instead of `*dim` or pass `&Style` by reference and dereference inline.)

- [ ] **Step 7: Change the footer when expanded**

In `cli/src/tui/app.rs::render_hint` (around line 557), modify the logic. Today:

```rust
        let line = if self.slash_menu_open {
            Line::from(vec![
                Span::styled("  ↑↓ select · enter to run · esc to dismiss", dim),
            ])
        } else if matches!(self.activity, ActivityState::Active { .. }) {
            Line::from(vec![Span::styled("  esc to interrupt · ? for shortcuts", dim)])
        } else {
            Line::from(vec![Span::styled("  ? for shortcuts", dim)])
        };
```

Change to:

```rust
        let line = if self.slash_menu_open {
            Line::from(vec![
                Span::styled("  ↑↓ select · enter to run · esc to dismiss", dim),
            ])
        } else if self.scroll_area.is_detailed_transcript() {
            Line::from(vec![Span::styled("  Showing detailed transcript · ctrl+o to toggle", dim)])
        } else if matches!(self.activity, ActivityState::Active { .. }) {
            Line::from(vec![Span::styled("  esc to interrupt · ? for shortcuts", dim)])
        } else {
            Line::from(vec![Span::styled("  ? for shortcuts", dim)])
        };
```

- [ ] **Step 8: Run all tests**

```bash
cargo test -p super-cli 2>&1 | tail -10
```

Expected: 79 tests pass (77 + 2: `task_done_summary_computes_from_child_events`, `task_done_summary_returns_none_when_no_result`).

- [ ] **Step 9: Build the release binary as a smoke check**

```bash
cargo build --release -p super-cli 2>&1 | tail -3
```

Expected: success.

- [ ] **Step 10: Commit**

```bash
git add cli/src/tui/app.rs cli/src/tui/scroll_area.rs
git commit -m "feat(tui): Ctrl+O toggle for detailed subagent transcript view"
```

---

## Task 8: Acceptance — run the tmux harness against super

This is a verification step, not a code change.

- [ ] **Step 1: Confirm everything compiles**

```bash
cargo build --release -p super-cli 2>&1 | tail -3
cargo test -p super-cli 2>&1 | tail -3
```

Expected: build succeeds, 79 tests pass.

- [ ] **Step 2: Tmux smoke test (optional but recommended)**

If you have an OpenRouter key configured (`~/.super/config.json`):

```bash
SOCK=followup-smoke
tmux -L "$SOCK" -f /dev/null kill-server 2>/dev/null
tmux -L "$SOCK" -f /dev/null new-session -d -s super -x 160 -y 50 './target/release/super'
sleep 1
tmux -L "$SOCK" send-keys -t super -l "use the Explore agent to find where SessionBus is defined"
tmux -L "$SOCK" send-keys -t super Enter
sleep 30
tmux -L "$SOCK" capture-pane -t super -p | tee /tmp/super-followup.txt
# Press Ctrl+O to expand
tmux -L "$SOCK" send-keys -t super C-o
sleep 1
tmux -L "$SOCK" capture-pane -t super -p
tmux -L "$SOCK" kill-server
```

Expected: collapsed view shows `⏺ Task (Explore) ...` then `⎿ Done (...)` then `(ctrl+o to expand)`. Expanded view shows the subagent's full internal activity indented under the Task line. Footer says `Showing detailed transcript · ctrl+o to toggle`.

- [ ] **Step 3: Verify sidechain JSONL contains Result lines**

```bash
ls ~/.super/sessions/*/sidechains/ 2>/dev/null | head
cat ~/.super/sessions/*/sidechains/*.jsonl 2>/dev/null | grep '"type":"result"' | head
```

Expected: at least one `agent-<uuid>.jsonl` containing a `"type":"result"` line.

- [ ] **Step 4: If anything fails — diagnose and commit any fixes**

If issues surface, fix them as additional small commits. Then push:

```bash
git push
```

Update the PR description if behavior changed.

---

## Self-review

**Spec coverage:**

| Spec section | Task |
|---|---|
| 1. I2 — async shutdown cleanup | Task 6 |
| 2. I3 — auto_deny_prompts enforcement | Task 4 |
| 3. I4 — dynamic subagent_type schema | Task 5 |
| 4. m3 — sidechain captures Result | Task 2 |
| 5. m5 — dead-code cleanup | Task 1 |
| 6. AsyncAgentHandle.started_at | Task 3 |
| 7. TUI drill-in (Ctrl+O) | Task 7 |
| Acceptance | Task 8 |

Every spec section has a task.

**Placeholder scan:** None. All code blocks are concrete. The Task 7 step on `render_child_indented`'s `Style` copy semantics flags a known ratatui quirk and provides a fallback in the same step.

**Type consistency:**
- `new_child` signature: 7 args after Task-12 + C1/I1 fix; +1 (`auto_deny_prompts`) in Task 4 → 8 args. Existing callers updated in Task 4 step 5.
- `AsyncAgentHandle` fields: spec adds `started_at` in Task 3. Test sites in Tasks 3 and 6 include the new field.
- `task_done_summary` returns `Option<String>`. `render_child_indented` is called per `TranscriptItem` and matches all 5 variants.

**Order rationale:**
- Task 1 (5-line removal) cleans agent.rs first so later diffs are tighter.
- Task 2 (Result protocol) precedes Task 7 because Task 7's `task_done_summary` reads the new `parent_tool_use_id` on Result.
- Task 3 (started_at) precedes Task 6 because Task 6's shutdown test sites construct `AsyncAgentHandle` and need the new field.
- Task 4 (auto_deny_prompts) and Task 5 (dynamic schema) are independent — either order works. Plan picks Task 4 first because it touches more of the engine and benefits from being verified before the more cosmetic Task 5.
- Task 6 (shutdown) is grouped with the App changes for Task 7 since both touch app.rs, but doesn't strictly need to wait.
- Task 7 (TUI drill-in) last because it depends on Tasks 2 (Result has parent_tool_use_id) and is the largest change.

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-15-subagent-followups.md`. Two execution options:

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
