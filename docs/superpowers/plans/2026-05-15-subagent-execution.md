# Subagent Execution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `AgentTool::call` stub with a working subagent execution path that matches Claude Code's `AgentTool` 1:1 — sync + async, built-in + `.claude/agents/*.md` loader, sidechain JSONL persistence, permission overlay, abort isolation.

**Architecture:** Construct a child `ConversationEngine` that shares the parent's `Arc<SessionBus>` but stamps every emitted event with a fresh `session_id` (the agent's id) and the parent's invoking `tool_use_id` as `parent_tool_use_id`. The `tui::transcript::fold` function already demuxes on these fields. Same loop function reused for parent and child — mirrors Claude Code's `query()` reuse via `runAgent`.

**Tech Stack:** Rust, `rig-core`, `ratatui`, `tokio` (async, broadcast, watch), `serde_yaml` (frontmatter), `uuid`.

**Reference:** `docs/superpowers/specs/2026-05-15-subagent-execution-design.md` (spec) and `claude-code-src/tools/AgentTool/` (1:1 source of truth).

---

## File Structure

**New files:**
- `cli/src/agents/mod.rs` — module root
- `cli/src/agents/definition.rs` — `AgentDefinition`, `AgentSource`
- `cli/src/agents/built_in.rs` — hardcoded built-ins (5 agents)
- `cli/src/agents/loader.rs` — frontmatter parser
- `cli/src/agents/registry.rs` — `AgentRegistry`
- `cli/src/agents/permission.rs` — `resolve_permission_mode`
- `cli/src/agents/model.rs` — model alias table
- `cli/src/conversation/sidechain.rs` — JSONL persistence task

**Modified files:**
- `cli/src/sdk/protocol.rs` — add `parent_tool_use_id` to `SystemEvent`; add `SystemSubtype::AsyncAgentDone`
- `cli/src/conversation/session_bus.rs` — update `emit_system` signature
- `cli/src/tui/transcript.rs` — `matches_filter` reads `SystemEvent::parent_tool_use_id`
- `cli/src/conversation/engine.rs` — `new_child`, `session_id_override`, `parent_tool_use_id` parameter
- `cli/src/conversation/tool_loop.rs` — populate `ctx.bus`, `ctx.parent_tool_use_id`; ToolProgress carries parent's parent_tool_use_id
- `cli/src/tools/contract.rs` — `auto_deny_prompts` field
- `cli/src/tools/mod.rs` — `filter_for_agent`; pass `Arc<AgentRegistry>` into `AgentTool`
- `cli/src/tools/agent.rs` — full rewrite ("Task")
- `cli/src/state/store.rs` — `async_agents: HashMap`, async-agent methods
- `cli/src/tui/scroll_area.rs` — Task summary line
- `cli/src/bootstrap.rs` — load `AgentRegistry`; spawn sidechain writer
- `cli/src/lib.rs` (or `cli/src/main.rs`) — `pub mod agents`

---

## Task 1: Add `parent_tool_use_id` to `SystemEvent` and new `AsyncAgentDone` subtype

**Why first:** Protocol changes are additive and unblock every later task that emits SystemEvents from a subagent context.

**Files:**
- Modify: `cli/src/sdk/protocol.rs:236-241` (`SystemEvent` variant), `cli/src/sdk/protocol.rs:272-278` (`SystemSubtype` enum)

- [ ] **Step 1: Write the failing test**

Append to `cli/src/sdk/protocol.rs` at the bottom of the `mod tests` block:

```rust
    #[test]
    fn system_event_carries_parent_tool_use_id() {
        let msg = BusMessage::SystemEvent {
            subtype: SystemSubtype::AsyncAgentDone,
            message: "agent-1 finished: ok".into(),
            parent_tool_use_id: Some("tu_parent".into()),
            uuid: Uuid::new_v4(),
            session_id: "agent-1".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"parent_tool_use_id\":\"tu_parent\""));
        assert!(json.contains("\"subtype\":\"async_agent_done\""));
        let back: BusMessage = serde_json::from_str(&json).unwrap();
        match back {
            BusMessage::SystemEvent { subtype, parent_tool_use_id, .. } => {
                assert!(matches!(subtype, SystemSubtype::AsyncAgentDone));
                assert_eq!(parent_tool_use_id.as_deref(), Some("tu_parent"));
            }
            _ => panic!("wrong variant"),
        }
    }
```

If `Uuid` isn't already in scope at the bottom of `mod tests`, add `use uuid::Uuid;` at the top of the module.

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-cli --lib sdk::protocol::tests::system_event_carries_parent_tool_use_id 2>&1 | head -40
```

Expected: compile error — `AsyncAgentDone` not in `SystemSubtype`, `parent_tool_use_id` not a field of `SystemEvent`.

- [ ] **Step 3: Add the field and the variant**

Modify `cli/src/sdk/protocol.rs:236-241` from:

```rust
    #[serde(rename = "system")]
    SystemEvent {
        subtype: SystemSubtype,
        message: String,
        uuid: Uuid,
        session_id: String,
    },
```

to:

```rust
    #[serde(rename = "system")]
    SystemEvent {
        subtype: SystemSubtype,
        message: String,
        #[serde(default)]
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
```

Modify `cli/src/sdk/protocol.rs:272-278` from:

```rust
pub enum SystemSubtype {
    CompactBoundary,
    PostTurnSummary,
    ApiRetry,
    PermissionRequest,
    Notice,
}
```

to:

```rust
pub enum SystemSubtype {
    CompactBoundary,
    PostTurnSummary,
    ApiRetry,
    PermissionRequest,
    Notice,
    AsyncAgentDone,
}
```

- [ ] **Step 4: Fix existing callers that construct `SystemEvent`**

Find them:

```bash
grep -rn "BusMessage::SystemEvent {" cli/src/ | grep -v test
```

Expected hits: at minimum `cli/src/conversation/session_bus.rs::emit_system`. Update `emit_system` from (file `cli/src/conversation/session_bus.rs:49-56`):

```rust
    pub fn emit_system(&self, subtype: SystemSubtype, message: impl Into<String>) {
        self.emit(BusMessage::SystemEvent {
            subtype,
            message: message.into(),
            uuid: Uuid::new_v4(),
            session_id: self.session_id.clone(),
        });
    }
```

to:

```rust
    pub fn emit_system(&self, subtype: SystemSubtype, message: impl Into<String>) {
        self.emit(BusMessage::SystemEvent {
            subtype,
            message: message.into(),
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: self.session_id.clone(),
        });
    }
```

For any other construction sites, add `parent_tool_use_id: None`.

- [ ] **Step 5: Update `matches_filter` in transcript.rs to read the new field**

Modify `cli/src/tui/transcript.rs:173-185` from:

```rust
fn matches_filter(ev: &BusMessage, filter: Option<&str>) -> bool {
    let parent = match ev {
        BusMessage::User { parent_tool_use_id, .. }
        | BusMessage::Assistant { parent_tool_use_id, .. }
        | BusMessage::StreamEvent { parent_tool_use_id, .. }
        | BusMessage::ToolProgress { parent_tool_use_id, .. } => parent_tool_use_id.as_deref(),
        BusMessage::SystemEvent { .. } | BusMessage::Result { .. } => None,
    };
    match filter {
        None => parent.is_none(),
        Some(want) => parent == Some(want),
    }
}
```

to:

```rust
fn matches_filter(ev: &BusMessage, filter: Option<&str>) -> bool {
    let parent = match ev {
        BusMessage::User { parent_tool_use_id, .. }
        | BusMessage::Assistant { parent_tool_use_id, .. }
        | BusMessage::StreamEvent { parent_tool_use_id, .. }
        | BusMessage::ToolProgress { parent_tool_use_id, .. }
        | BusMessage::SystemEvent { parent_tool_use_id, .. } => parent_tool_use_id.as_deref(),
        BusMessage::Result { .. } => None,
    };
    match filter {
        None => parent.is_none(),
        Some(want) => parent == Some(want),
    }
}
```

- [ ] **Step 6: Run the test and the full crate test suite**

```bash
cargo test -p super-cli --lib sdk::protocol::tests::system_event_carries_parent_tool_use_id
cargo test -p super-cli --lib
```

Expected: target test PASS; full suite PASS (no regressions).

- [ ] **Step 7: Commit**

```bash
git add cli/src/sdk/protocol.rs cli/src/conversation/session_bus.rs cli/src/tui/transcript.rs
git commit -m "feat(protocol): SystemEvent carries parent_tool_use_id; add AsyncAgentDone subtype"
```

---

## Task 2: Helper accessors on `BusMessage`

**Why:** Sidechain writer and async-agent emit code both need `parent_tool_use_id()` and `session_id()` on `BusMessage` without matching every variant inline.

**Files:**
- Modify: `cli/src/sdk/protocol.rs` (add `impl BusMessage`)

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `cli/src/sdk/protocol.rs`:

```rust
    #[test]
    fn bus_message_accessors() {
        let msg = BusMessage::Result {
            stop_reason: None,
            usage: AnthropicUsage::default(),
            total_cost_usd: 0.0, duration_ms: 0, num_turns: 1,
            uuid: Uuid::new_v4(),
            session_id: "s-root".into(),
        };
        assert_eq!(msg.session_id(), "s-root");
        assert_eq!(msg.parent_tool_use_id(), None);

        let msg = BusMessage::SystemEvent {
            subtype: SystemSubtype::Notice,
            message: "x".into(),
            parent_tool_use_id: Some("tu_p".into()),
            uuid: Uuid::new_v4(),
            session_id: "s-child".into(),
        };
        assert_eq!(msg.session_id(), "s-child");
        assert_eq!(msg.parent_tool_use_id(), Some("tu_p"));
    }
```

If `AnthropicUsage` isn't imported in the test module, add to the test imports.

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-cli --lib sdk::protocol::tests::bus_message_accessors 2>&1 | head -30
```

Expected: compile error — methods don't exist.

- [ ] **Step 3: Implement the accessors**

After the `BusMessage` enum definition in `cli/src/sdk/protocol.rs` (i.e., after the closing `}` of the enum, before `#[derive(Debug, Clone, Serialize, Deserialize)] pub struct UserPayload`):

```rust
impl BusMessage {
    pub fn session_id(&self) -> &str {
        match self {
            BusMessage::User { session_id, .. }
            | BusMessage::Assistant { session_id, .. }
            | BusMessage::StreamEvent { session_id, .. }
            | BusMessage::ToolProgress { session_id, .. }
            | BusMessage::SystemEvent { session_id, .. }
            | BusMessage::Result { session_id, .. } => session_id.as_str(),
        }
    }

    pub fn parent_tool_use_id(&self) -> Option<&str> {
        match self {
            BusMessage::User { parent_tool_use_id, .. }
            | BusMessage::Assistant { parent_tool_use_id, .. }
            | BusMessage::StreamEvent { parent_tool_use_id, .. }
            | BusMessage::ToolProgress { parent_tool_use_id, .. }
            | BusMessage::SystemEvent { parent_tool_use_id, .. } => parent_tool_use_id.as_deref(),
            BusMessage::Result { .. } => None,
        }
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p super-cli --lib sdk::protocol::tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add cli/src/sdk/protocol.rs
git commit -m "feat(protocol): BusMessage::session_id / parent_tool_use_id accessors"
```

---

## Task 3: `agents::definition` — types

**Files:**
- Create: `cli/src/agents/mod.rs`
- Create: `cli/src/agents/definition.rs`
- Modify: `cli/src/lib.rs` (or `cli/src/main.rs` if no lib — verify with `ls cli/src/`)

- [ ] **Step 0: Determine module root file**

```bash
ls cli/src/lib.rs cli/src/main.rs 2>/dev/null
```

If `lib.rs` exists, the new `pub mod agents;` declaration goes there. Otherwise add it to `main.rs`. (At time of writing, the crate uses `main.rs`-with-modules style — check `main.rs` for the existing `mod` declarations and add `mod agents;` alongside them.)

- [ ] **Step 1: Write the test**

Create `cli/src/agents/mod.rs`:

```rust
pub mod definition;

#[cfg(test)]
mod tests {
    use super::definition::*;

    #[test]
    fn agent_definition_constructible() {
        let def = AgentDefinition {
            agent_type: "general-purpose".into(),
            description: "general agent".into(),
            system_prompt: "you are a general agent".into(),
            tools: Some(vec!["*".into()]),
            disallowed_tools: vec![],
            model: None,
            permission_mode: None,
            max_turns: None,
            source: AgentSource::BuiltIn,
        };
        assert_eq!(def.agent_type, "general-purpose");
        assert!(matches!(def.source, AgentSource::BuiltIn));
    }
}
```

- [ ] **Step 2: Create `definition.rs`**

Create `cli/src/agents/definition.rs`:

```rust
use crate::state::store::PermissionMode;

#[derive(Debug, Clone)]
pub struct AgentDefinition {
    pub agent_type: String,
    pub description: String,
    pub system_prompt: String,
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Vec<String>,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub max_turns: Option<u32>,
    pub source: AgentSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSource {
    BuiltIn,
    User,
    Project,
}
```

- [ ] **Step 3: Register the module**

In `cli/src/main.rs` (or `lib.rs`), add `mod agents;` near the other `mod` declarations (alphabetical placement preferred).

- [ ] **Step 4: Run tests**

```bash
cargo test -p super-cli --lib agents::tests::agent_definition_constructible
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add cli/src/agents/ cli/src/main.rs
git commit -m "feat(agents): AgentDefinition + AgentSource types"
```

---

## Task 4: `agents::model` — alias table

**Files:**
- Create: `cli/src/agents/model.rs`
- Modify: `cli/src/agents/mod.rs`

- [ ] **Step 1: Write the failing tests**

Append to `cli/src/agents/mod.rs`:

```rust
pub mod model;
```

Append to the `#[cfg(test)] mod tests` block in `cli/src/agents/mod.rs`:

```rust
    #[test]
    fn resolve_model_handles_aliases_and_inherit() {
        use super::model::resolve_model;
        let parent = "anthropic/claude-sonnet-4-6";
        assert_eq!(resolve_model(Some("sonnet"), parent), "anthropic/claude-sonnet-4-6");
        assert_eq!(resolve_model(Some("opus"), parent), "anthropic/claude-opus-4-7");
        assert_eq!(resolve_model(Some("haiku"), parent), "anthropic/claude-haiku-4-5-20251001");
        assert_eq!(resolve_model(Some("inherit"), parent), parent);
        assert_eq!(resolve_model(None, parent), parent);
        // Unknown -> pass through
        assert_eq!(resolve_model(Some("anthropic/claude-something-else"), parent), "anthropic/claude-something-else");
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-cli --lib agents::tests::resolve_model_handles_aliases_and_inherit 2>&1 | head -20
```

Expected: compile error — module / function not found.

- [ ] **Step 3: Implement `model.rs`**

Create `cli/src/agents/model.rs`:

```rust
/// Resolve an agent's `model` field (alias or full id) to a full OpenRouter
/// model id, falling back to the parent model.
pub fn resolve_model(agent_model: Option<&str>, parent_model: &str) -> String {
    match agent_model {
        None | Some("inherit") => parent_model.to_string(),
        Some("sonnet") => "anthropic/claude-sonnet-4-6".to_string(),
        Some("opus") => "anthropic/claude-opus-4-7".to_string(),
        Some("haiku") => "anthropic/claude-haiku-4-5-20251001".to_string(),
        Some(other) => other.to_string(),
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p super-cli --lib agents::tests::resolve_model_handles_aliases_and_inherit
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add cli/src/agents/model.rs cli/src/agents/mod.rs
git commit -m "feat(agents): model alias resolution (sonnet/opus/haiku/inherit)"
```

---

## Task 5: `agents::permission` — permission mode overlay

**Files:**
- Create: `cli/src/agents/permission.rs`
- Modify: `cli/src/agents/mod.rs`

- [ ] **Step 1: Append module declaration**

In `cli/src/agents/mod.rs` add:

```rust
pub mod permission;
```

- [ ] **Step 2: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block in `cli/src/agents/mod.rs`:

```rust
    #[test]
    fn permission_overlay_parent_bypass_wins() {
        use super::permission::resolve_permission_mode;
        use crate::state::store::PermissionMode;
        let out = resolve_permission_mode(&PermissionMode::Bypass, Some(&PermissionMode::Plan), false);
        assert!(matches!(out, PermissionMode::Bypass));
    }

    #[test]
    fn permission_overlay_parent_accept_edits_wins() {
        use super::permission::resolve_permission_mode;
        use crate::state::store::PermissionMode;
        let out = resolve_permission_mode(&PermissionMode::AcceptEdits, Some(&PermissionMode::Plan), false);
        assert!(matches!(out, PermissionMode::AcceptEdits));
    }

    #[test]
    fn permission_overlay_agent_override_applies_when_parent_default() {
        use super::permission::resolve_permission_mode;
        use crate::state::store::PermissionMode;
        let out = resolve_permission_mode(&PermissionMode::Default, Some(&PermissionMode::Plan), false);
        assert!(matches!(out, PermissionMode::Plan));
    }

    #[test]
    fn permission_overlay_no_agent_override_inherits_parent() {
        use super::permission::resolve_permission_mode;
        use crate::state::store::PermissionMode;
        let out = resolve_permission_mode(&PermissionMode::Default, None, false);
        assert!(matches!(out, PermissionMode::Default));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p super-cli --lib agents::tests::permission_overlay 2>&1 | head -30
```

Expected: compile error — module / function not found.

- [ ] **Step 4: Implement `permission.rs`**

Create `cli/src/agents/permission.rs`:

```rust
use crate::state::store::PermissionMode;

/// Mirrors claude-code-src/tools/AgentTool/runAgent.ts lines 415-451.
///
/// Rules:
/// 1. If parent is in Bypass or AcceptEdits, parent always wins.
/// 2. Else if the agent definition specifies a mode, use the agent's.
/// 3. Else inherit the parent's.
///
/// `is_async` is accepted for symmetry with the TS source; in v1 the flag does
/// not change the returned mode itself (auto-deny behaviour for async is
/// handled by `ToolCallContext::auto_deny_prompts`, set by the caller).
pub fn resolve_permission_mode(
    parent: &PermissionMode,
    agent: Option<&PermissionMode>,
    _is_async: bool,
) -> PermissionMode {
    match parent {
        PermissionMode::Bypass | PermissionMode::AcceptEdits => parent.clone(),
        _ => match agent {
            Some(m) => m.clone(),
            None => parent.clone(),
        },
    }
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-cli --lib agents::tests::permission_overlay
```

Expected: 4 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add cli/src/agents/permission.rs cli/src/agents/mod.rs
git commit -m "feat(agents): resolve_permission_mode overlay (parent bypass/acceptEdits wins)"
```

---

## Task 6: `agents::built_in` — port system prompts from claude-code-src

**Files:**
- Create: `cli/src/agents/built_in.rs`
- Modify: `cli/src/agents/mod.rs`

- [ ] **Step 1: Module declaration**

In `cli/src/agents/mod.rs` add:

```rust
pub mod built_in;
```

- [ ] **Step 2: Write the failing test**

Append to `#[cfg(test)] mod tests` in `cli/src/agents/mod.rs`:

```rust
    #[test]
    fn built_in_agents_list_contains_expected_types() {
        use super::built_in::built_in_agents;
        let names: Vec<&str> = built_in_agents().iter().map(|a| a.agent_type.as_str()).collect();
        assert!(names.contains(&"general-purpose"));
        assert!(names.contains(&"Explore"));
        assert!(names.contains(&"Plan"));
        assert!(names.contains(&"statusline-setup"));
        assert!(names.contains(&"claude-code-guide"));
    }

    #[test]
    fn explore_agent_is_read_only_blocking_edit_write() {
        use super::built_in::built_in_agents;
        let explore = built_in_agents().into_iter()
            .find(|a| a.agent_type == "Explore")
            .expect("Explore in built-ins");
        assert!(explore.disallowed_tools.iter().any(|t| t == "Edit"));
        assert!(explore.disallowed_tools.iter().any(|t| t == "Write"));
        assert!(explore.disallowed_tools.iter().any(|t| t == "NotebookEdit"));
        assert!(explore.disallowed_tools.iter().any(|t| t == "Task"));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p super-cli --lib agents::tests::built_in 2>&1 | head -20
```

Expected: compile error — module not found.

- [ ] **Step 4: Implement `built_in.rs`**

Create `cli/src/agents/built_in.rs`. The system prompt strings are ported byte-for-byte from `claude-code-src/tools/AgentTool/built-in/*.ts` — the exact text is in those files. Use this template; copy the prompt body from the matching TS file:

```rust
use super::definition::{AgentDefinition, AgentSource};

pub fn built_in_agents() -> Vec<AgentDefinition> {
    vec![
        general_purpose(),
        explore(),
        plan(),
        statusline_setup(),
        claude_code_guide(),
    ]
}

fn general_purpose() -> AgentDefinition {
    AgentDefinition {
        agent_type: "general-purpose".into(),
        description: "General-purpose agent for researching complex questions, searching for code, and executing multi-step tasks. When you are searching for a keyword or file and are not confident that you will find the right match in the first few tries use this agent to perform the search for you.".into(),
        system_prompt: GENERAL_PURPOSE_PROMPT.to_string(),
        tools: Some(vec!["*".into()]),
        disallowed_tools: vec![],
        model: None,
        permission_mode: None,
        max_turns: None,
        source: AgentSource::BuiltIn,
    }
}

fn explore() -> AgentDefinition {
    AgentDefinition {
        agent_type: "Explore".into(),
        description: "Fast agent specialized for exploring codebases. Use this when you need to quickly find files by patterns, search code for keywords, or answer questions about the codebase.".into(),
        system_prompt: EXPLORE_PROMPT.to_string(),
        tools: None, // inherit all and apply disallowed
        disallowed_tools: vec![
            "Task".into(),
            "ExitPlanMode".into(),
            "Edit".into(),
            "Write".into(),
            "NotebookEdit".into(),
        ],
        model: Some("haiku".into()),
        permission_mode: None,
        max_turns: None,
        source: AgentSource::BuiltIn,
    }
}

fn plan() -> AgentDefinition {
    AgentDefinition {
        agent_type: "Plan".into(),
        description: "Software architect agent for designing implementation plans. Use this when you need to plan the implementation strategy for a task. Returns step-by-step plans, identifies critical files, and considers architectural trade-offs.".into(),
        system_prompt: PLAN_PROMPT.to_string(),
        tools: None,
        disallowed_tools: vec![
            "Task".into(),
            "ExitPlanMode".into(),
            "Edit".into(),
            "Write".into(),
            "NotebookEdit".into(),
        ],
        model: Some("inherit".into()),
        permission_mode: None,
        max_turns: None,
        source: AgentSource::BuiltIn,
    }
}

fn statusline_setup() -> AgentDefinition {
    AgentDefinition {
        agent_type: "statusline-setup".into(),
        description: "Use this agent to configure the user's Claude Code status line setting.".into(),
        system_prompt: STATUSLINE_PROMPT.to_string(),
        tools: Some(vec!["Read".into(), "Edit".into()]),
        disallowed_tools: vec![],
        model: None,
        permission_mode: None,
        max_turns: None,
        source: AgentSource::BuiltIn,
    }
}

fn claude_code_guide() -> AgentDefinition {
    AgentDefinition {
        agent_type: "claude-code-guide".into(),
        description: "Use this agent when the user asks questions about Claude Code features, hooks, slash commands, MCP servers, settings, or IDE integrations.".into(),
        system_prompt: CLAUDE_CODE_GUIDE_PROMPT.to_string(),
        tools: Some(vec!["Bash".into(), "Read".into(), "WebFetch".into(), "WebSearch".into()]),
        disallowed_tools: vec![],
        model: None,
        permission_mode: None,
        max_turns: None,
        source: AgentSource::BuiltIn,
    }
}

// ---- Prompt bodies (port from claude-code-src/tools/AgentTool/built-in/*.ts) ----
//
// To port: open each TS file, locate `getXxxSystemPrompt` (or the equivalent),
// and copy the returned template-string contents verbatim into the matching
// const below. Strip TS template-string interpolation `${...}` by replacing
// tool-name placeholders with their literal super names: BASH_TOOL_NAME -> "Bash",
// FILE_READ_TOOL_NAME -> "Read", GLOB_TOOL_NAME -> "Glob", GREP_TOOL_NAME -> "Grep",
// FILE_EDIT_TOOL_NAME -> "Edit", FILE_WRITE_TOOL_NAME -> "Write",
// NOTEBOOK_EDIT_TOOL_NAME -> "NotebookEdit", AGENT_TOOL_NAME -> "Task",
// EXIT_PLAN_MODE_TOOL_NAME -> "ExitPlanMode".

const GENERAL_PURPOSE_PROMPT: &str = "You are an agent for Claude Code, Anthropic's official CLI for Claude. Given the user's message, you should use the tools available to complete the task. Complete the task fully—don't gold-plate, but don't leave it half-done. When you complete the task, respond with a concise report covering what was done and any key findings — the caller will relay this to the user, so it only needs the essentials.\n\nYour strengths:\n- Searching for code, configurations, and patterns across large codebases\n- Analyzing multiple files to understand system architecture\n- Investigating complex questions that require exploring many files\n- Performing multi-step research tasks\n\nGuidelines:\n- For file searches: search broadly when you don't know where something lives. Use Read when you know the specific file path.\n- For analysis: Start broad and narrow down. Use multiple search strategies if the first doesn't yield results.\n- Be thorough: Check multiple locations, consider different naming conventions, look for related files.\n- NEVER create files unless they're absolutely necessary for achieving your goal. ALWAYS prefer editing an existing file to creating a new one.\n- NEVER proactively create documentation files (*.md) or README files. Only create documentation files if explicitly requested.";

const EXPLORE_PROMPT: &str = "You are a file search specialist for Claude Code, Anthropic's official CLI for Claude. You excel at thoroughly navigating and exploring codebases.\n\n=== CRITICAL: READ-ONLY MODE - NO FILE MODIFICATIONS ===\nThis is a READ-ONLY exploration task. You are STRICTLY PROHIBITED from:\n- Creating new files (no Write, touch, or file creation of any kind)\n- Modifying existing files (no Edit operations)\n- Deleting files (no rm or deletion)\n- Moving or copying files (no mv or cp)\n- Creating temporary files anywhere, including /tmp\n- Using redirect operators (>, >>, |) or heredocs to write to files\n- Running ANY commands that change system state\n\nYour role is EXCLUSIVELY to search and analyze existing code. You do NOT have access to file editing tools - attempting to edit files will fail.\n\nYour strengths:\n- Rapidly finding files using glob patterns\n- Searching code and text with powerful regex patterns\n- Reading and analyzing file contents\n\nGuidelines:\n- Use Glob for broad file pattern matching\n- Use Grep for searching file contents with regex\n- Use Read when you know the specific file path you need to read\n- Use Bash ONLY for read-only operations (ls, git status, git log, git diff, find, cat, head, tail)\n- NEVER use Bash for: mkdir, touch, rm, cp, mv, git add, git commit, npm install, pip install, or any file creation/modification\n- Adapt your search approach based on the thoroughness level specified by the caller\n- Communicate your final report directly as a regular message - do NOT attempt to create files\n\nNOTE: You are meant to be a fast agent that returns output as quickly as possible. In order to achieve this you must:\n- Make efficient use of the tools that you have at your disposal: be smart about how you search for files and implementations\n- Wherever possible you should try to spawn multiple parallel tool calls for grepping and reading files\n\nComplete the user's search request efficiently and report your findings clearly.";

const PLAN_PROMPT: &str = "You are a software architect and planning specialist for Claude Code. Your role is to explore the codebase and design implementation plans.\n\n=== CRITICAL: READ-ONLY MODE - NO FILE MODIFICATIONS ===\nThis is a READ-ONLY planning task. You are STRICTLY PROHIBITED from:\n- Creating new files (no Write, touch, or file creation of any kind)\n- Modifying existing files (no Edit operations)\n- Deleting files (no rm or deletion)\n- Moving or copying files (no mv or cp)\n- Creating temporary files anywhere, including /tmp\n- Using redirect operators (>, >>, |) or heredocs to write to files\n- Running ANY commands that change system state\n\nYour role is EXCLUSIVELY to explore the codebase and design implementation plans. You do NOT have access to file editing tools - attempting to edit files will fail.\n\nYou will be provided with a set of requirements and optionally a perspective on how to approach the design process.\n\n## Your Process\n\n1. **Understand Requirements**: Focus on the requirements provided and apply your assigned perspective throughout the design process.\n\n2. **Explore Thoroughly**:\n   - Read any files provided to you in the initial prompt\n   - Find existing patterns and conventions using Glob, Grep, and Read\n   - Understand the current architecture\n   - Identify similar features as reference\n   - Trace through relevant code paths\n   - Use Bash ONLY for read-only operations (ls, git status, git log, git diff, find, cat, head, tail)\n   - NEVER use Bash for: mkdir, touch, rm, cp, mv, git add, git commit, npm install, pip install, or any file creation/modification\n\n3. **Design Solution**:\n   - Create implementation approach based on your assigned perspective\n   - Consider trade-offs and architectural decisions\n   - Follow existing patterns where appropriate\n\n4. **Detail the Plan**:\n   - Provide step-by-step implementation strategy\n   - Identify dependencies and sequencing\n   - Anticipate potential challenges\n\n## Required Output\n\nEnd your response with:\n\n### Critical Files for Implementation\nList 3-5 files most critical for implementing this plan:\n- path/to/file1.ts\n- path/to/file2.ts\n- path/to/file3.ts\n\nREMEMBER: You can ONLY explore and plan. You CANNOT and MUST NOT write, edit, or modify any files. You do NOT have access to file editing tools.";

const STATUSLINE_PROMPT: &str = "Use this agent to configure the user's Claude Code status line setting. Read the current settings.json (typically at ~/.claude/settings.json), then suggest or apply the user's desired status line configuration via Edit.";

const CLAUDE_CODE_GUIDE_PROMPT: &str = "You answer questions about Claude Code (the CLI tool) — features, hooks, slash commands, MCP servers, settings, IDE integrations, keyboard shortcuts; the Claude Agent SDK (building custom agents); and the Claude API / Anthropic SDK. Use Bash, Read, WebFetch, and WebSearch to gather authoritative information. Cite sources when relevant.";
```

The `STATUSLINE_PROMPT` and `CLAUDE_CODE_GUIDE_PROMPT` placeholders above are summarised — port the full bodies from `claude-code-src/tools/AgentTool/built-in/statuslineSetup.ts` and `claude-code-src/tools/AgentTool/built-in/claudeCodeGuideAgent.ts` respectively.

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-cli --lib agents::tests::built_in
```

Expected: both tests PASS.

- [ ] **Step 6: Commit**

```bash
git add cli/src/agents/built_in.rs cli/src/agents/mod.rs
git commit -m "feat(agents): built-in agents (general-purpose, Explore, Plan, statusline-setup, claude-code-guide)"
```

---

## Task 7: `agents::loader` — frontmatter parser

**Files:**
- Create: `cli/src/agents/loader.rs`
- Modify: `cli/src/agents/mod.rs`

- [ ] **Step 1: Module declaration**

In `cli/src/agents/mod.rs`:

```rust
pub mod loader;
```

- [ ] **Step 2: Write the failing tests**

Append to `#[cfg(test)] mod tests` in `cli/src/agents/mod.rs`:

```rust
    #[test]
    fn loader_parses_full_frontmatter() {
        use super::loader::parse_agent_md;
        use super::definition::AgentSource;
        let src = "---\ndescription: An explorer\ntools: [Read, Grep]\ndisallowedTools: [Edit]\nmodel: haiku\npermissionMode: plan\nmaxTurns: 8\n---\nYou are an explorer.\nUse the tools.\n";
        let def = parse_agent_md("Explore.md", src, AgentSource::Project)
            .expect("parses");
        assert_eq!(def.agent_type, "Explore");
        assert_eq!(def.description, "An explorer");
        assert_eq!(def.tools.as_ref().unwrap(), &vec!["Read".to_string(), "Grep".to_string()]);
        assert_eq!(def.disallowed_tools, vec!["Edit".to_string()]);
        assert_eq!(def.model.as_deref(), Some("haiku"));
        assert_eq!(def.max_turns, Some(8));
        assert!(def.system_prompt.contains("You are an explorer"));
        assert!(matches!(def.source, AgentSource::Project));
    }

    #[test]
    fn loader_rejects_missing_frontmatter() {
        use super::loader::parse_agent_md;
        use super::definition::AgentSource;
        let src = "no frontmatter here\n";
        assert!(parse_agent_md("foo.md", src, AgentSource::User).is_err());
    }

    #[test]
    fn loader_rejects_empty_description() {
        use super::loader::parse_agent_md;
        use super::definition::AgentSource;
        let src = "---\ndescription: \"\"\n---\nbody\n";
        assert!(parse_agent_md("foo.md", src, AgentSource::User).is_err());
    }

    #[test]
    fn loader_strips_md_suffix_for_agent_type() {
        use super::loader::parse_agent_md;
        use super::definition::AgentSource;
        let src = "---\ndescription: x\n---\nbody";
        let def = parse_agent_md("my-agent.md", src, AgentSource::Project).unwrap();
        assert_eq!(def.agent_type, "my-agent");
    }
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p super-cli --lib agents::tests::loader 2>&1 | head -20
```

Expected: compile errors.

- [ ] **Step 4: Implement the loader**

Create `cli/src/agents/loader.rs`:

```rust
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::definition::{AgentDefinition, AgentSource};
use crate::state::store::PermissionMode;

#[derive(Debug, Deserialize)]
struct Frontmatter {
    description: String,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default, rename = "disallowedTools")]
    disallowed_tools: Option<Vec<String>>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, rename = "permissionMode")]
    permission_mode: Option<String>,
    #[serde(default, rename = "maxTurns")]
    max_turns: Option<u32>,
}

/// Parse a single `.claude/agents/<name>.md` file's contents into an
/// `AgentDefinition`. `filename` is used only to derive `agent_type` (strip the
/// `.md` suffix). Source identifies where the file came from.
pub fn parse_agent_md(
    filename: &str,
    contents: &str,
    source: AgentSource,
) -> Result<AgentDefinition, String> {
    let (fm_src, body) = split_frontmatter(contents)
        .ok_or_else(|| format!("{filename}: missing or malformed YAML frontmatter (expected leading ---)"))?;
    let fm: Frontmatter = serde_yaml::from_str(fm_src)
        .map_err(|e| format!("{filename}: invalid frontmatter: {e}"))?;
    if fm.description.trim().is_empty() {
        return Err(format!("{filename}: description must not be empty"));
    }
    let body_trimmed = body.trim().to_string();
    if body_trimmed.is_empty() {
        return Err(format!("{filename}: body (system prompt) must not be empty"));
    }

    let permission_mode = match fm.permission_mode.as_deref() {
        None => None,
        Some("default") => Some(PermissionMode::Default),
        Some("acceptEdits") => Some(PermissionMode::AcceptEdits),
        Some("bypassPermissions") => Some(PermissionMode::Bypass),
        Some("plan") => Some(PermissionMode::Plan),
        Some(other) => return Err(format!("{filename}: unknown permissionMode '{other}'")),
    };

    let agent_type = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename)
        .to_string();

    Ok(AgentDefinition {
        agent_type,
        description: fm.description.trim().to_string(),
        system_prompt: body_trimmed,
        tools: fm.tools,
        disallowed_tools: fm.disallowed_tools.unwrap_or_default(),
        model: fm.model,
        permission_mode,
        max_turns: fm.max_turns,
        source,
    })
}

/// Returns (frontmatter_yaml, body) on success, or None when the file does not
/// begin with a `---` fence.
fn split_frontmatter(contents: &str) -> Option<(&str, &str)> {
    let rest = contents.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let fm = &rest[..end];
    let body_start = end + "\n---".len();
    let body = &rest[body_start..];
    let body = body.strip_prefix('\n').unwrap_or(body);
    Some((fm, body))
}

/// Scan a directory for `*.md` agent files. Skips files that fail to parse,
/// logging a warning. Returns whatever did parse.
pub fn load_agents_from_dir(dir: &Path, source: AgentSource) -> Vec<AgentDefinition> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out, // dir doesn't exist — that's fine
    };
    for entry in entries.flatten() {
        let path: PathBuf = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match parse_agent_md(&filename, &contents, source) {
                Ok(def) => out.push(def),
                Err(e) => tracing::warn!("agents loader: {e}"),
            },
            Err(e) => tracing::warn!("agents loader: cannot read {}: {e}", path.display()),
        }
    }
    out
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-cli --lib agents::tests::loader
```

Expected: 4 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add cli/src/agents/loader.rs cli/src/agents/mod.rs
git commit -m "feat(agents): .claude/agents/*.md frontmatter loader"
```

---

## Task 8: `agents::registry` — load all sources, resolve by type

**Files:**
- Create: `cli/src/agents/registry.rs`
- Modify: `cli/src/agents/mod.rs`

- [ ] **Step 1: Module declaration**

In `cli/src/agents/mod.rs`:

```rust
pub mod registry;
```

Re-export the registry for convenience:

```rust
pub use registry::AgentRegistry;
pub use definition::{AgentDefinition, AgentSource};
```

- [ ] **Step 2: Write the failing test**

Append to `#[cfg(test)] mod tests` in `cli/src/agents/mod.rs`:

```rust
    #[test]
    fn registry_built_ins_are_resolvable() {
        use super::registry::AgentRegistry;
        let reg = AgentRegistry::built_in_only();
        assert!(reg.resolve("general-purpose").is_some());
        assert!(reg.resolve("Explore").is_some());
        assert!(reg.resolve("does-not-exist").is_none());
    }

    #[test]
    fn registry_project_shadows_user_shadows_builtin() {
        use super::definition::{AgentDefinition, AgentSource};
        use super::registry::AgentRegistry;
        fn mk(t: &str, src: AgentSource, prompt: &str) -> AgentDefinition {
            AgentDefinition {
                agent_type: t.into(),
                description: format!("{t} {src:?}"),
                system_prompt: prompt.into(),
                tools: None,
                disallowed_tools: vec![],
                model: None,
                permission_mode: None,
                max_turns: None,
                source: src,
            }
        }
        let reg = AgentRegistry::from_layers(
            vec![mk("Explore", AgentSource::BuiltIn, "built-in body")],
            vec![mk("Explore", AgentSource::User, "user body")],
            vec![mk("Explore", AgentSource::Project, "project body")],
        );
        let resolved = reg.resolve("Explore").unwrap();
        assert!(matches!(resolved.source, AgentSource::Project));
        assert_eq!(resolved.system_prompt, "project body");
    }
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p super-cli --lib agents::tests::registry 2>&1 | head -20
```

Expected: compile errors.

- [ ] **Step 4: Implement `registry.rs`**

Create `cli/src/agents/registry.rs`:

```rust
use std::collections::HashMap;
use std::path::PathBuf;

use super::built_in::built_in_agents;
use super::definition::{AgentDefinition, AgentSource};
use super::loader::load_agents_from_dir;

pub struct AgentRegistry {
    by_type: HashMap<String, AgentDefinition>,
}

impl AgentRegistry {
    /// Load with built-ins only. Used in tests and as a degraded fallback.
    pub fn built_in_only() -> Self {
        Self::from_layers(built_in_agents(), Vec::new(), Vec::new())
    }

    /// Load with all three layers from disk. User layer scans
    /// `~/.claude/agents/*.md`; project layer scans `<cwd>/.claude/agents/*.md`.
    pub fn load(cwd: &PathBuf) -> Self {
        let user = dirs::home_dir()
            .map(|h| h.join(".claude").join("agents"))
            .map(|d| load_agents_from_dir(&d, AgentSource::User))
            .unwrap_or_default();
        let project = load_agents_from_dir(
            &cwd.join(".claude").join("agents"),
            AgentSource::Project,
        );
        Self::from_layers(built_in_agents(), user, project)
    }

    /// Merge three layers with later writes winning: built-in < user < project.
    pub fn from_layers(
        built_in: Vec<AgentDefinition>,
        user: Vec<AgentDefinition>,
        project: Vec<AgentDefinition>,
    ) -> Self {
        let mut by_type: HashMap<String, AgentDefinition> = HashMap::new();
        for layer in [built_in, user, project] {
            for def in layer {
                by_type.insert(def.agent_type.clone(), def);
            }
        }
        Self { by_type }
    }

    pub fn resolve(&self, subagent_type: &str) -> Option<&AgentDefinition> {
        self.by_type.get(subagent_type)
    }

    pub fn list(&self) -> Vec<&AgentDefinition> {
        self.by_type.values().collect()
    }
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-cli --lib agents::tests
```

Expected: all `agents` tests PASS.

- [ ] **Step 6: Commit**

```bash
git add cli/src/agents/registry.rs cli/src/agents/mod.rs
git commit -m "feat(agents): AgentRegistry with project > user > built-in precedence"
```

---

## Task 9: `ConversationEngine` — `session_id_override` + `parent_tool_use_id` parameter

**Files:**
- Modify: `cli/src/conversation/engine.rs:22-242` (the whole engine)

This task wires the parent_tool_use_id through the engine without yet adding the child constructor. Root callers still pass `None`.

- [ ] **Step 1: Write a regression test that root behaviour is unchanged**

Append to `#[cfg(test)] mod tests` in `cli/src/conversation/engine.rs`:

```rust
    #[test]
    fn engine_session_id_resolution_default_uses_bus() {
        use crate::conversation::session_bus::SessionBus;
        let bus = std::sync::Arc::new(SessionBus::new("s-root".into()));
        let engine = ConversationEngine {
            store: std::sync::Arc::new(crate::state::store::Store::new()),
            config: shared::CliConfig::default(),
            registry: std::sync::Arc::new(crate::tools::ToolRegistry::new(
                std::sync::Arc::new(crate::state::store::Store::new()),
                shared::CliConfig::default(),
            )),
            bus: bus.clone(),
            abort: None,
            session_id_override: None,
        };
        assert_eq!(engine.effective_session_id(), "s-root");
    }

    #[test]
    fn engine_session_id_override_used_when_set() {
        use crate::conversation::session_bus::SessionBus;
        let bus = std::sync::Arc::new(SessionBus::new("s-root".into()));
        let engine = ConversationEngine {
            store: std::sync::Arc::new(crate::state::store::Store::new()),
            config: shared::CliConfig::default(),
            registry: std::sync::Arc::new(crate::tools::ToolRegistry::new(
                std::sync::Arc::new(crate::state::store::Store::new()),
                shared::CliConfig::default(),
            )),
            bus,
            abort: None,
            session_id_override: Some("agent-1".into()),
        };
        assert_eq!(engine.effective_session_id(), "agent-1");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p super-cli --lib conversation::engine::tests::engine_session_id 2>&1 | head -20
```

Expected: compile errors — field and method don't exist.

- [ ] **Step 3: Add the field and accessor**

Modify the `pub struct ConversationEngine` definition (currently at `cli/src/conversation/engine.rs:22-29`):

```rust
#[derive(Clone)]
pub struct ConversationEngine {
    pub store: Arc<Store>,
    pub config: CliConfig,
    pub registry: Arc<ToolRegistry>,
    pub bus: Arc<SessionBus>,
    pub abort: Option<watch::Receiver<bool>>,
    /// When `Some`, every emit uses this as the session_id instead of
    /// `bus.session_id()`. Set by child engines spawned for subagents.
    pub session_id_override: Option<String>,
}
```

Update `ConversationEngine::new` (currently at `cli/src/conversation/engine.rs:31-39`) to:

```rust
    pub fn new(
        store: Arc<Store>,
        config: CliConfig,
        registry: Arc<ToolRegistry>,
        bus: Arc<SessionBus>,
    ) -> Self {
        Self { store, config, registry, bus, abort: None, session_id_override: None }
    }

    /// Effective session id for emits: override if set, else bus.session_id().
    pub fn effective_session_id(&self) -> String {
        self.session_id_override
            .clone()
            .unwrap_or_else(|| self.bus.session_id().to_string())
    }
```

- [ ] **Step 4: Thread `parent_tool_use_id` through `process_prompt`**

Change the signature at `cli/src/conversation/engine.rs:44-48` from:

```rust
    pub async fn process_prompt(
        &self,
        user_input: String,
        system_prompt: &SystemPrompt,
    ) -> Result<String, String> {
```

to:

```rust
    pub async fn process_prompt(
        &self,
        user_input: String,
        system_prompt: &SystemPrompt,
        parent_tool_use_id: Option<String>,
    ) -> Result<String, String> {
```

Then replace `let session_id = self.bus.session_id().to_string();` at line 49 with:

```rust
        let session_id = self.effective_session_id();
```

Replace every `parent_tool_use_id: None,` literal in `process_prompt` with `parent_tool_use_id: parent_tool_use_id.clone(),`. There are five sites in this function (User emit, StreamEvent emit, Assistant emit, the Result emit, and the synthetic User-with-tool_results emit).

Also pass `parent_tool_use_id` into `run_tool_uses` — see Task 11 for the signature change. For now, replace the call at the existing site (around line 217) with:

```rust
            let tool_results = run_tool_uses(
                &self.registry,
                tool_uses,
                cwd.clone(),
                permission_mode.clone(),
                self.abort.clone(),
                self.bus.clone(),
                parent_tool_use_id.clone(),
                session_id.clone(),
            )
            .await;
```

(This will not compile until Task 11 updates `run_tool_uses`'s signature. That's expected — we'll combine the two changes into one passing build at Task 11's end. For this task we stop after the engine changes; the tests below are scoped to fields/accessors that compile in isolation.)

Actually to keep this task self-contained, defer the call-site change to Task 11. Drop the new arguments from the call for now and leave a `let _ = parent_tool_use_id;` to silence the unused warning where the engine method ends up not using it on the `run_tool_uses` path yet.

Concretely: in `process_prompt`, just replace the five `parent_tool_use_id: None` literals with `parent_tool_use_id: parent_tool_use_id.clone()` and add `let _ = &parent_tool_use_id;` near the bottom of the function (just before the `loop { ... }` or right before the final `return Ok(last_assistant_text);` so nothing else changes shape). Then Task 11 updates `run_tool_uses` and the call site here.

- [ ] **Step 5: Update root caller**

Modify `cli/src/tui/app.rs:187` from:

```rust
            let result = engine.process_prompt(prompt, &sp).await;
```

to:

```rust
            let result = engine.process_prompt(prompt, &sp, None).await;
```

- [ ] **Step 6: Run tests**

```bash
cargo test -p super-cli --lib
```

Expected: all PASS, no warnings about unused `parent_tool_use_id`.

- [ ] **Step 7: Commit**

```bash
git add cli/src/conversation/engine.rs cli/src/tui/app.rs
git commit -m "feat(engine): session_id_override + parent_tool_use_id parameter on process_prompt"
```

---

## Task 10: `ToolCallContext::auto_deny_prompts` field

**Why:** async subagents must auto-deny tools that would otherwise prompt the user.

**Files:**
- Modify: `cli/src/tools/contract.rs:12-25`

- [ ] **Step 1: Write the test**

Append a new test module at the bottom of `cli/src/tools/contract.rs` (the file currently has no tests):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_context_carries_auto_deny_flag() {
        let ctx = ToolCallContext {
            cwd: std::path::PathBuf::from("/tmp"),
            permission_mode: crate::state::store::PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: true,
        };
        assert!(ctx.auto_deny_prompts);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-cli --lib tools::contract::tests::tool_call_context_carries_auto_deny_flag 2>&1 | head -20
```

Expected: compile error — `auto_deny_prompts` not a field.

- [ ] **Step 3: Add the field**

Modify `cli/src/tools/contract.rs:12-25` from:

```rust
pub struct ToolCallContext {
    pub cwd: std::path::PathBuf,
    pub permission_mode: crate::state::store::PermissionMode,
    pub abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
    /// When the tool is invoked from inside a subagent, this carries the
    /// parent's invoking tool_use_id so emitted events can be demuxed by
    /// consumers (TUI, server forwarder, sidechain transcript writer).
    /// Always `None` in v1 — subagent execution is out of scope.
    pub parent_tool_use_id: Option<String>,
    /// Handle to the session bus, available to tools that need to emit
    /// events (currently only AgentTool when subagents are wired up).
    /// `None` v1.
    pub bus: Option<std::sync::Arc<crate::conversation::session_bus::SessionBus>>,
}
```

to:

```rust
pub struct ToolCallContext {
    pub cwd: std::path::PathBuf,
    pub permission_mode: crate::state::store::PermissionMode,
    pub abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
    /// When the tool is invoked from inside a subagent, this carries the
    /// parent's invoking tool_use_id so emitted events can be demuxed by
    /// consumers (TUI, server forwarder, sidechain transcript writer).
    pub parent_tool_use_id: Option<String>,
    /// Handle to the session bus, available to tools that need to emit
    /// events (subagent spawn, progress tickers).
    pub bus: Option<std::sync::Arc<crate::conversation::session_bus::SessionBus>>,
    /// When true, tools that would otherwise prompt the user (AskUserQuestion,
    /// permission prompts, etc.) must auto-deny and return an error result.
    /// Set by AgentTool for async subagents.
    pub auto_deny_prompts: bool,
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p super-cli --lib
```

Expected: every `ToolCallContext { ... }` construction site fails to compile (missing field).

Fix by appending `auto_deny_prompts: false,` to each. Find them:

```bash
grep -rn "ToolCallContext {" cli/src/ | grep -v "pub struct"
```

Expected sites: `cli/src/conversation/tool_loop.rs` (both `safe` and `unsafe_` ctx constructions), any tests in tool files.

- [ ] **Step 5: Run tests again**

```bash
cargo test -p super-cli --lib
```

Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tools/contract.rs cli/src/conversation/tool_loop.rs
git commit -m "feat(tools): ToolCallContext.auto_deny_prompts (async subagent gate)"
```

---

## Task 11: `tool_loop::run_tool_uses` — populate bus, parent_tool_use_id, threaded session_id

**Files:**
- Modify: `cli/src/conversation/tool_loop.rs:14-187` (the function plus both ctx-construction sites and ticker emits)
- Modify: `cli/src/conversation/engine.rs` (call site)

- [ ] **Step 1: Write the failing test**

Append to `#[cfg(test)] mod tests` in `cli/src/conversation/tool_loop.rs`:

```rust
    #[tokio::test]
    async fn run_tool_uses_passes_parent_tool_use_id_to_context() {
        use crate::tools::contract::{Tool, ToolCallContext, ToolResult};
        use std::sync::{Arc, Mutex};

        struct CaptureTool {
            seen_parent: Arc<Mutex<Option<String>>>,
        }
        #[async_trait::async_trait]
        impl Tool for CaptureTool {
            fn name(&self) -> &str { "Capture" }
            fn description(&self) -> &str { "capture" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn call(&self, _input: serde_json::Value, ctx: &ToolCallContext) -> ToolResult {
                *self.seen_parent.lock().unwrap() = ctx.parent_tool_use_id.clone();
                ToolResult { content: "ok".into(), is_error: false, metadata: None }
            }
        }

        let seen = Arc::new(Mutex::new(None));
        let store = Arc::new(Store::new());
        let registry = ToolRegistry::new(store, CliConfig::default());
        registry.register(Arc::new(CaptureTool { seen_parent: seen.clone() }));

        let bus = Arc::new(SessionBus::new("s-root".into()));
        let _ = run_tool_uses(
            &registry,
            vec![("tu_x".into(), "Capture".into(), serde_json::json!({}))],
            std::env::current_dir().unwrap(),
            PermissionMode::Default,
            None,
            bus,
            Some("tu_parent".into()),
            "agent-1".into(),
            false,
        ).await;

        assert_eq!(seen.lock().unwrap().clone().as_deref(), Some("tu_parent"));
    }
```

(This test uses the new 9-arg signature you're about to define.)

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-cli --lib conversation::tool_loop::tests::run_tool_uses_passes_parent_tool_use_id_to_context 2>&1 | head -20
```

Expected: compile error — argument count mismatch.

- [ ] **Step 3: Update the function signature and body**

Replace `cli/src/conversation/tool_loop.rs:14-21` from:

```rust
pub async fn run_tool_uses(
    registry: &ToolRegistry,
    tool_uses: Vec<(String, String, serde_json::Value)>, // (id, name, input)
    cwd: std::path::PathBuf,
    permission_mode: PermissionMode,
    abort_signal: Option<watch::Receiver<bool>>,
    bus: Arc<SessionBus>,
) -> Vec<ContentBlockFinal> {
```

to:

```rust
pub async fn run_tool_uses(
    registry: &ToolRegistry,
    tool_uses: Vec<(String, String, serde_json::Value)>, // (id, name, input)
    cwd: std::path::PathBuf,
    permission_mode: PermissionMode,
    abort_signal: Option<watch::Receiver<bool>>,
    bus: Arc<SessionBus>,
    parent_tool_use_id: Option<String>,
    session_id: String,
    auto_deny_prompts: bool,
) -> Vec<ContentBlockFinal> {
```

Inside the function, both `ToolCallContext { ... }` literals (around lines 49-55 and 140-146) need:

```rust
        let ctx = ToolCallContext {
            cwd: cwd.clone(),
            permission_mode: permission_mode.clone(),
            abort_signal: abort_signal.clone(),
            parent_tool_use_id: parent_tool_use_id.clone(),
            bus: Some(bus.clone()),
            auto_deny_prompts,
        };
```

And both ticker emits (around lines 71-79 and 158-165) need to use `parent_tool_use_id.clone()` and `session_id.clone()`:

```rust
                    bus_for_tick.emit(BusMessage::ToolProgress {
                        tool_use_id: id_for_tick.clone(),
                        tool_name: name_for_tick.clone(),
                        elapsed_seconds: start.elapsed().as_secs_f32(),
                        parent_tool_use_id: parent_for_tick.clone(),
                        uuid: uuid::Uuid::new_v4(),
                        session_id: session_for_tick.clone(),
                    });
```

To shuttle `parent_tool_use_id` and `session_id` into the spawned ticker, clone them before the `tokio::spawn(...)` blocks:

```rust
            let parent_for_tick = parent_tool_use_id.clone();
            let session_for_tick = session_id.clone();
            let ticker = tokio::spawn(async move {
                let start = std::time::Instant::now();
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
                interval.tick().await;
                loop {
                    interval.tick().await;
                    bus_for_tick.emit(BusMessage::ToolProgress {
                        tool_use_id: id_for_tick.clone(),
                        tool_name: name_for_tick.clone(),
                        elapsed_seconds: start.elapsed().as_secs_f32(),
                        parent_tool_use_id: parent_for_tick.clone(),
                        uuid: uuid::Uuid::new_v4(),
                        session_id: session_for_tick.clone(),
                    });
                }
            });
```

Do this for both the safe (concurrency-parallel) and unsafe (serial) branches.

- [ ] **Step 4: Update existing tests in this file**

Find them:

```bash
grep -n "run_tool_uses(" cli/src/conversation/tool_loop.rs
```

Every call inside `#[cfg(test)] mod tests` (4 sites: `unknown_tool_produces_error_result`, `read_tool_executes_against_real_file`, `mixed_safe_and_sequential_preserve_emission_order`, `panicking_tool_produces_error_result_with_correct_id`) must add the new trailing args:

```rust
            None,                  // parent_tool_use_id
            "test-session".into(), // session_id
            false,                 // auto_deny_prompts
```

- [ ] **Step 5: Update the engine call site**

In `cli/src/conversation/engine.rs::process_prompt`, replace the `run_tool_uses` call (the one currently after Step 4 of Task 9 set up the parameter) so the call now passes the right arguments. From whatever shape it has, change to:

```rust
            let tool_results = run_tool_uses(
                &self.registry,
                tool_uses,
                cwd.clone(),
                permission_mode.clone(),
                self.abort.clone(),
                self.bus.clone(),
                parent_tool_use_id.clone(),
                session_id.clone(),
                false, // auto_deny_prompts — overridden by AgentTool for async children via the child engine's own field; root engines never auto-deny
            )
            .await;
```

Remove the `let _ = &parent_tool_use_id;` placeholder added in Task 9.

- [ ] **Step 6: Run all tests**

```bash
cargo test -p super-cli --lib
```

Expected: every test PASSes.

- [ ] **Step 7: Commit**

```bash
git add cli/src/conversation/tool_loop.rs cli/src/conversation/engine.rs
git commit -m "feat(tool_loop): thread parent_tool_use_id + session_id + bus into ToolCallContext"
```

---

## Task 12: `ConversationEngine::new_child` constructor

**Files:**
- Modify: `cli/src/conversation/engine.rs` (add a new constructor)

- [ ] **Step 1: Write the failing test**

Append to `#[cfg(test)] mod tests` in `cli/src/conversation/engine.rs`:

```rust
    #[test]
    fn new_child_constructs_with_overrides() {
        use crate::conversation::session_bus::SessionBus;
        let bus = std::sync::Arc::new(SessionBus::new("s-root".into()));
        let store = std::sync::Arc::new(crate::state::store::Store::new());
        let registry = std::sync::Arc::new(crate::tools::ToolRegistry::new(
            store.clone(),
            shared::CliConfig::default(),
        ));
        let child = ConversationEngine::new_child(
            store,
            shared::CliConfig::default(),
            registry,
            bus,
            "agent-xyz".into(),
            None,
        );
        assert_eq!(child.effective_session_id(), "agent-xyz");
        assert_eq!(child.session_id_override.as_deref(), Some("agent-xyz"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-cli --lib conversation::engine::tests::new_child_constructs_with_overrides 2>&1 | head -20
```

Expected: compile error — `new_child` doesn't exist.

- [ ] **Step 3: Implement `new_child`**

Add to `impl ConversationEngine` (next to `new` in `cli/src/conversation/engine.rs`):

```rust
    /// Construct a child engine for subagent execution. Shares the parent's
    /// bus and store, but stamps every emit with `agent_id` as the session_id.
    pub fn new_child(
        store: Arc<Store>,
        config: CliConfig,
        registry: Arc<ToolRegistry>,
        bus: Arc<SessionBus>,
        agent_id: String,
        abort: Option<watch::Receiver<bool>>,
    ) -> Self {
        Self {
            store,
            config,
            registry,
            bus,
            abort,
            session_id_override: Some(agent_id),
        }
    }
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p super-cli --lib conversation::engine::tests
```

Expected: all engine tests PASS.

- [ ] **Step 5: Commit**

```bash
git add cli/src/conversation/engine.rs
git commit -m "feat(engine): new_child constructor for subagent execution"
```

---

## Task 13: `ToolRegistry::filter_for_agent`

**Files:**
- Modify: `cli/src/tools/mod.rs:74-204` (the registry impl)

- [ ] **Step 1: Write the failing tests**

Append to `cli/src/tools/mod.rs` a new `#[cfg(test)] mod registry_tests` block at the very end:

```rust
#[cfg(test)]
mod registry_tests {
    use super::*;
    use crate::agents::definition::{AgentDefinition, AgentSource};
    use crate::state::store::PermissionMode;

    fn agent(tools: Option<Vec<&str>>, disallowed: Vec<&str>) -> AgentDefinition {
        AgentDefinition {
            agent_type: "x".into(),
            description: "x".into(),
            system_prompt: "x".into(),
            tools: tools.map(|v| v.into_iter().map(String::from).collect()),
            disallowed_tools: disallowed.into_iter().map(String::from).collect(),
            model: None,
            permission_mode: None,
            max_turns: None,
            source: AgentSource::BuiltIn,
        }
    }

    #[test]
    fn filter_star_keeps_all() {
        let store = std::sync::Arc::new(crate::state::store::Store::new());
        let reg = ToolRegistry::new(store, shared::CliConfig::default());
        let all = reg.assemble_for_mode(&PermissionMode::Default).len();
        let filtered = reg.filter_for_agent(&agent(Some(vec!["*"]), vec![]));
        assert_eq!(filtered.assemble_for_mode(&PermissionMode::Default).len(), all);
    }

    #[test]
    fn filter_named_subset_keeps_only_listed() {
        let store = std::sync::Arc::new(crate::state::store::Store::new());
        let reg = ToolRegistry::new(store, shared::CliConfig::default());
        let filtered = reg.filter_for_agent(&agent(Some(vec!["Read", "Grep"]), vec![]));
        let names: Vec<String> = filtered.list();
        assert!(names.contains(&"Read".to_string()));
        assert!(names.contains(&"Grep".to_string()));
        assert!(!names.iter().any(|n| n == "Edit"));
    }

    #[test]
    fn filter_disallowed_removes_listed() {
        let store = std::sync::Arc::new(crate::state::store::Store::new());
        let reg = ToolRegistry::new(store, shared::CliConfig::default());
        let filtered = reg.filter_for_agent(&agent(Some(vec!["*"]), vec!["Edit", "Write"]));
        let names: Vec<String> = filtered.list();
        assert!(!names.iter().any(|n| n == "Edit"));
        assert!(!names.iter().any(|n| n == "Write"));
    }

    #[test]
    fn filter_none_means_inherit_all() {
        let store = std::sync::Arc::new(crate::state::store::Store::new());
        let reg = ToolRegistry::new(store, shared::CliConfig::default());
        let all = reg.assemble_for_mode(&PermissionMode::Default).len();
        let filtered = reg.filter_for_agent(&agent(None, vec![]));
        assert_eq!(filtered.assemble_for_mode(&PermissionMode::Default).len(), all);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p super-cli --lib tools::registry_tests 2>&1 | head -30
```

Expected: compile error — `filter_for_agent` doesn't exist.

- [ ] **Step 3: Implement `filter_for_agent`**

In `cli/src/tools/mod.rs`, inside `impl ToolRegistry`, add (next to `assemble_for_mode`):

```rust
    pub fn filter_for_agent(&self, agent: &crate::agents::definition::AgentDefinition) -> ToolRegistry {
        let pool = self.tools.read().unwrap();
        let filtered: Vec<Arc<dyn Tool>> = pool.iter()
            .filter(|t| {
                let name = t.name();
                let in_allowed = match &agent.tools {
                    None => true,
                    Some(list) if list.iter().any(|s| s == "*") => true,
                    Some(list) => list.iter().any(|s| s == name),
                };
                let in_disallowed = agent.disallowed_tools.iter().any(|s| s == name);
                in_allowed && !in_disallowed
            })
            .cloned()
            .collect();
        ToolRegistry { tools: Arc::new(RwLock::new(filtered)) }
    }
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p super-cli --lib tools::registry_tests
```

Expected: 4 PASS.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tools/mod.rs
git commit -m "feat(tools): ToolRegistry::filter_for_agent for subagent tool whitelisting"
```

---

## Task 14: Async-agent tracking in `Store`

**Files:**
- Modify: `cli/src/state/store.rs`

- [ ] **Step 1: Write the failing tests**

Append to `cli/src/state/store.rs` (add a tests module if none exists):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_complete_async_agent() {
        let store = Store::new();
        let (tx, _rx) = tokio::sync::watch::channel(false);
        store.register_async_agent(AsyncAgentHandle {
            agent_id: "a1".into(),
            parent_tool_use_id: "tu_1".into(),
            abort: tx,
            description: "test".into(),
        });
        let list = store.list_async_agents();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].agent_id, "a1");
        store.complete_async_agent("a1");
        assert!(store.list_async_agents().is_empty());
    }

    #[test]
    fn abort_async_agent_signals_watch() {
        let store = Store::new();
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        store.register_async_agent(AsyncAgentHandle {
            agent_id: "a2".into(),
            parent_tool_use_id: "tu_2".into(),
            abort: tx,
            description: "test".into(),
        });
        assert!(store.abort_async_agent("a2"));
        // Watch should have fired
        assert!(*rx.borrow_and_update());
        assert!(store.list_async_agents().is_empty(), "abort also removes the handle");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p super-cli --lib state::store::tests 2>&1 | head -30
```

Expected: compile errors — types/methods don't exist.

- [ ] **Step 3: Add the type and AppState field**

In `cli/src/state/store.rs`, after the existing `TaskRecord`/`PermissionMode` definitions but before `pub struct Store`, add:

```rust
#[derive(Clone)]
pub struct AsyncAgentHandle {
    pub agent_id: String,
    pub parent_tool_use_id: String,
    pub abort: tokio::sync::watch::Sender<bool>,
    pub description: String,
}
```

Add to `pub struct AppState` (modifying lines 25-36):

```rust
#[derive(Clone, Default)]
pub struct AppState {
    pub messages: Vec<Message>,
    pub permission_mode: PermissionMode,
    pub model: String,
    pub thinking_enabled: bool,
    pub effort_level: Option<String>,
    pub is_streaming: bool,
    pub should_compact: bool,
    pub tasks: HashMap<String, TaskRecord>,
    pub history: Vec<crate::conversation::anthropic::HistoryEntry>,
    pub async_agents: HashMap<String, AsyncAgentHandle>,
}
```

- [ ] **Step 4: Add methods on `Store`**

Inside `impl Store` (any position is fine, after `set_state` is conventional):

```rust
    pub fn register_async_agent(&self, handle: AsyncAgentHandle) {
        let id = handle.agent_id.clone();
        self.set_state(|s| {
            s.async_agents.insert(id.clone(), handle.clone());
        });
        let _ = id;
    }

    pub fn complete_async_agent(&self, agent_id: &str) {
        self.set_state(|s| {
            s.async_agents.remove(agent_id);
        });
    }

    /// Returns true if an agent was found and signalled.
    pub fn abort_async_agent(&self, agent_id: &str) -> bool {
        // Read out the handle outside set_state so we can call its send()
        let handle = self.state.read().unwrap().async_agents.get(agent_id).cloned();
        if let Some(h) = handle {
            let _ = h.abort.send(true);
            self.set_state(|s| { s.async_agents.remove(agent_id); });
            true
        } else {
            false
        }
    }

    pub fn list_async_agents(&self) -> Vec<AsyncAgentHandle> {
        self.state.read().unwrap().async_agents.values().cloned().collect()
    }
```

Note: `AsyncAgentHandle::Clone` works because `watch::Sender` is `Clone` (it shares the underlying state).

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-cli --lib state::store::tests
```

Expected: 2 PASS.

- [ ] **Step 6: Commit**

```bash
git add cli/src/state/store.rs
git commit -m "feat(store): AsyncAgentHandle + register/complete/abort/list methods"
```

---

## Task 15: Sidechain JSONL writer

**Files:**
- Create: `cli/src/conversation/sidechain.rs`
- Modify: `cli/src/conversation/mod.rs`

- [ ] **Step 1: Register module**

In `cli/src/conversation/mod.rs`, add `pub mod sidechain;`.

- [ ] **Step 2: Write the failing test**

Create `cli/src/conversation/sidechain.rs` skeleton with the test first:

```rust
use std::path::PathBuf;
use std::sync::Arc;

use crate::conversation::session_bus::SessionBus;
use crate::sdk::protocol::BusMessage;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::protocol::{AnthropicUsage, AssistantPayload, ContentBlockFinal, SystemSubtype};
    use uuid::Uuid;

    #[tokio::test]
    async fn sidechain_writes_one_file_per_agent_id() {
        let tmp = tempfile_dir();
        let bus = Arc::new(SessionBus::new("s-root".into()));
        spawn_sidechain_writer(bus.clone(), tmp.clone());

        // Emit parent event (no parent_tool_use_id) — should NOT be written
        bus.emit(BusMessage::Assistant {
            message: AssistantPayload {
                id: "msg_root".into(),
                model: "x".into(),
                role: "assistant".into(),
                content: vec![ContentBlockFinal::Text { text: "root".into() }],
                stop_reason: None,
                usage: AnthropicUsage::default(),
            },
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s-root".into(),
        });

        // Emit child event — SHOULD be written under agent-1.jsonl
        bus.emit(BusMessage::SystemEvent {
            subtype: SystemSubtype::Notice,
            message: "child note".into(),
            parent_tool_use_id: Some("tu_1".into()),
            uuid: Uuid::new_v4(),
            session_id: "agent-1".into(),
        });

        // Let the writer drain
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let path = tmp.join("agent-1.jsonl");
        assert!(path.exists(), "expected sidechain at {:?}", path);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("child note"));
        assert!(!contents.contains("\"msg_root\""), "root event must not appear");
    }

    fn tempfile_dir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("super-sidechain-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p super-cli --lib conversation::sidechain::tests 2>&1 | head -30
```

Expected: compile error — `spawn_sidechain_writer` doesn't exist.

- [ ] **Step 4: Implement**

Replace `cli/src/conversation/sidechain.rs` with:

```rust
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::conversation::session_bus::SessionBus;
use crate::sdk::protocol::BusMessage;

/// Spawn a background task that subscribes to the bus and persists every
/// message with `parent_tool_use_id.is_some()` into a per-agent JSONL file
/// under `base_dir`. Best-effort: I/O failures are logged and the writer
/// continues running.
///
/// `base_dir` should be `~/.super/sessions/<root_session>/sidechains/`.
pub fn spawn_sidechain_writer(bus: Arc<SessionBus>, base_dir: PathBuf) {
    let mut rx = bus.subscribe();
    if let Err(e) = std::fs::create_dir_all(&base_dir) {
        tracing::warn!("sidechain: cannot create {base_dir:?}: {e}");
        return;
    }
    tokio::spawn(async move {
        let mut writers: HashMap<String, BufWriter<File>> = HashMap::new();
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if msg.parent_tool_use_id().is_none() {
                        continue;
                    }
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
                    match serde_json::to_string(&msg) {
                        Ok(line) => {
                            let _ = writer.write_all(line.as_bytes());
                            let _ = writer.write_all(b"\n");
                            let _ = writer.flush();
                        }
                        Err(e) => tracing::warn!("sidechain: serialize failed: {e}"),
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Conventional base path for sidechains.
pub fn default_sidechain_dir(root_session_id: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".super").join("sessions").join(root_session_id).join("sidechains")
}
```

Replace the test stub at the top with the test block from Step 2.

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-cli --lib conversation::sidechain::tests
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add cli/src/conversation/sidechain.rs cli/src/conversation/mod.rs
git commit -m "feat(sidechain): per-agent JSONL persistence subscriber"
```

---

## Task 16: AgentTool rewrite — sync path ("Task")

**Files:**
- Modify: `cli/src/tools/agent.rs` (full rewrite)
- Modify: `cli/src/tools/mod.rs` (pass AgentRegistry into AgentTool)

This is the unlock task. Touches multiple files but is one logical change. Split into write→register sub-tasks.

- [ ] **Step 1: Write the failing integration test**

Create `cli/tests/subagent_sync.rs` (a new integration test file at the crate root's tests/ dir; create the directory if it doesn't exist).

```bash
mkdir -p cli/tests
```

```rust
//! Integration test: sync subagent end-to-end.
//!
//! We don't have a mock OpenRouter yet, so this test only validates the
//! synchronous wiring of AgentTool::call: unknown subagent_type errors out
//! cleanly; known subagent_type constructs the child engine without panic.
//! Live HTTP end-to-end is covered by the tmux harness in acceptance.

use std::sync::Arc;

use super_cli::agents::AgentRegistry;
use super_cli::conversation::session_bus::SessionBus;
use super_cli::state::store::{PermissionMode, Store};
use super_cli::tools::agent::AgentTool;
use super_cli::tools::contract::{Tool, ToolCallContext};
use super_cli::tools::ToolRegistry;

#[tokio::test]
async fn agent_tool_errors_on_unknown_subagent_type() {
    let store = Arc::new(Store::new());
    let cfg = shared::CliConfig::default();
    let tool_reg = Arc::new(ToolRegistry::new(store.clone(), cfg.clone()));
    let agent_reg = Arc::new(AgentRegistry::built_in_only());

    let tool = AgentTool {
        store: store.clone(),
        config: cfg.clone(),
        registry: agent_reg,
        tool_registry: tool_reg.clone(),
    };

    let bus = Arc::new(SessionBus::new("s-root".into()));
    let ctx = ToolCallContext {
        cwd: std::env::current_dir().unwrap(),
        permission_mode: PermissionMode::Default,
        abort_signal: None,
        parent_tool_use_id: None,
        bus: Some(bus),
        auto_deny_prompts: false,
    };
    let input = serde_json::json!({
        "description": "do thing",
        "prompt": "thing prompt",
        "subagent_type": "does-not-exist",
    });
    let result = tool.call(input, &ctx).await;
    assert!(result.is_error);
    assert!(result.content.contains("unknown subagent"), "got: {}", result.content);
}

#[tokio::test]
async fn agent_tool_name_is_task() {
    let store = Arc::new(Store::new());
    let cfg = shared::CliConfig::default();
    let tool_reg = Arc::new(ToolRegistry::new(store.clone(), cfg.clone()));
    let agent_reg = Arc::new(AgentRegistry::built_in_only());
    let tool = AgentTool {
        store, config: cfg, registry: agent_reg, tool_registry: tool_reg,
    };
    assert_eq!(tool.name(), "Task");
}
```

This requires `super-cli` to expose a `lib.rs` so integration tests can `use super_cli::...`. If `cli/src/lib.rs` does not exist (the project uses `main.rs`-only), CREATE IT:

```bash
ls cli/src/lib.rs 2>/dev/null || echo "need to create lib.rs"
```

If creation is needed, add `cli/src/lib.rs`:

```rust
pub mod agents;
pub mod conversation;
pub mod sdk;
pub mod state;
pub mod tools;
pub mod tui;
pub mod skills;
pub mod config;
pub mod auth;
pub mod bootstrap;
```

And update `cli/src/main.rs` to use the library (`use super_cli::bootstrap;` or similar — adapt to current main.rs shape). If main.rs gets too tangled to refactor cleanly in this step, instead make the test target an in-tree unit test by adding it under `#[cfg(test)] mod` in `cli/src/tools/agent.rs`.

For simplicity in v1, **prefer the in-tree unit test** approach: put both `#[tokio::test]` functions under `#[cfg(test)] mod tests { ... }` inside `cli/src/tools/agent.rs` instead of `cli/tests/subagent_sync.rs`. Replace `super_cli::` paths with `crate::`.

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-cli --lib tools::agent::tests 2>&1 | head -30
```

Expected: compile errors — `AgentTool` doesn't have `registry`/`tool_registry`, `ToolCallContext` builder signature mismatch, etc.

- [ ] **Step 3: Replace `cli/src/tools/agent.rs` entirely**

```rust
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::watch;
use uuid::Uuid;

use super::contract::{Tool, ToolCallContext, ToolResult};
use crate::agents::definition::AgentDefinition;
use crate::agents::model::resolve_model;
use crate::agents::permission::resolve_permission_mode;
use crate::agents::AgentRegistry;
use crate::conversation::engine::ConversationEngine;
use crate::conversation::session_bus::SessionBus;
use crate::conversation::system_prompt::SystemPrompt;
use crate::sdk::protocol::{BusMessage, SystemSubtype};
use crate::state::store::{AsyncAgentHandle, Store};
use crate::tools::ToolRegistry;

pub struct AgentTool {
    pub store: Arc<Store>,
    pub config: shared::CliConfig,
    pub registry: Arc<AgentRegistry>,
    pub tool_registry: Arc<ToolRegistry>,
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &str {
        "Task"
    }

    fn description(&self) -> &str {
        "Launches a sub-agent to handle a focused task. \
         subagent_type selects the agent definition. \
         Set run_in_background to spawn an async agent."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "description":   { "type": "string", "description": "A short (3-5 word) description of the task" },
                "prompt":        { "type": "string", "description": "The task for the agent to perform" },
                "subagent_type": { "type": "string", "description": "The agent type to use (e.g. Explore, Plan, general-purpose)" },
                "model":         { "type": "string", "enum": ["sonnet", "opus", "haiku", "inherit"] },
                "run_in_background": { "type": "boolean" }
            },
            "required": ["description", "prompt", "subagent_type"]
        })
    }

    async fn call(&self, input: serde_json::Value, ctx: &ToolCallContext) -> ToolResult {
        // 1. Validate input
        let description = input.get("description").and_then(|v| v.as_str()).unwrap_or("");
        let prompt = match input.get("prompt").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p.to_string(),
            _ => return err("missing required field: prompt"),
        };
        let subagent_type = match input.get("subagent_type").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.to_string(),
            _ => return err("missing required field: subagent_type"),
        };
        let model_override = input.get("model").and_then(|v| v.as_str()).map(|s| s.to_string());
        let is_async = input.get("run_in_background").and_then(|v| v.as_bool()).unwrap_or(false);

        // 2. Resolve agent
        let Some(agent_def) = self.registry.resolve(&subagent_type) else {
            let available: Vec<String> = self.registry.list().into_iter()
                .map(|d| d.agent_type.clone())
                .collect();
            return err(&format!(
                "unknown subagent: {subagent_type} (available: {})",
                available.join(", ")
            ));
        };
        let agent_def: AgentDefinition = agent_def.clone();

        // 3. Bus must be present
        let Some(bus) = ctx.bus.clone() else {
            return err("Task tool requires bus in ToolCallContext");
        };

        let parent_tool_use_id = ctx.parent_tool_use_id.clone()
            .unwrap_or_else(|| {
                // The synthetic tool_use_id for emitting downstream events
                // when called outside an active tool_use — should never
                // happen via the engine, but defensively pick a UUID.
                format!("tu_{}", Uuid::new_v4())
            });
        // The actual invoking tool_use_id is set by tool_loop when populating
        // ctx.parent_tool_use_id BUT note that's the parent CHAIN id, not the
        // current tool_use_id. For Task tool spawning a NEW subagent, the
        // parent_tool_use_id we want stamped on the CHILD's events is the
        // tool_use_id of THIS Task call. tool_loop currently passes the chain
        // id (or None at root). We need access to "our own" tool_use_id —
        // which is the `id` from the tool_use block. Since ToolCallContext
        // doesn't carry it today, see Task 17 for the fix; for v1 of this
        // task we use parent_tool_use_id as a best-effort placeholder.
        let _ = description;

        // 4. Build child execution config
        let agent_id = Uuid::new_v4().to_string();
        let child_perm = resolve_permission_mode(
            &ctx.permission_mode,
            agent_def.permission_mode.as_ref(),
            is_async,
        );
        let child_model = resolve_model(
            model_override.as_deref().or(agent_def.model.as_deref()),
            &self.config.model,
        );
        let mut child_config = self.config.clone();
        child_config.model = child_model;

        let child_registry = Arc::new(self.tool_registry.filter_for_agent(&agent_def));

        // 5a. Sync path
        if !is_async {
            let child = ConversationEngine::new_child(
                self.store.clone(),
                child_config,
                child_registry,
                bus.clone(),
                agent_id.clone(),
                ctx.abort_signal.clone(),
            );
            let sys = build_child_system_prompt(&agent_def);

            return match child.process_prompt(prompt, &sys, Some(parent_tool_use_id)).await {
                Ok(final_text) => ToolResult {
                    content: final_text,
                    is_error: false,
                    metadata: Some({
                        let mut m = std::collections::HashMap::new();
                        m.insert("agent_id".to_string(), agent_id);
                        m.insert("agent_type".to_string(), agent_def.agent_type.clone());
                        m
                    }),
                },
                Err(e) => err(&format!("Agent failed: {e}")),
            };
        }

        // 5b. Async path
        let (abort_tx, abort_rx) = watch::channel(false);
        let handle = AsyncAgentHandle {
            agent_id: agent_id.clone(),
            parent_tool_use_id: parent_tool_use_id.clone(),
            abort: abort_tx,
            description: description.to_string(),
        };
        self.store.register_async_agent(handle);

        let child = ConversationEngine::new_child(
            self.store.clone(),
            child_config,
            child_registry,
            bus.clone(),
            agent_id.clone(),
            Some(abort_rx),
        );
        let sys = build_child_system_prompt(&agent_def);
        let store_for_task = self.store.clone();
        let bus_for_task = bus.clone();
        let agent_id_for_task = agent_id.clone();
        let parent_tu_for_task = parent_tool_use_id.clone();

        tokio::spawn(async move {
            let result = child.process_prompt(prompt, &sys, Some(parent_tu_for_task.clone())).await;
            let (text, _was_err) = match result {
                Ok(t) => (t, false),
                Err(e) => (format!("error: {e}"), true),
            };
            bus_for_task.emit(BusMessage::SystemEvent {
                subtype: SystemSubtype::AsyncAgentDone,
                message: format!("{agent_id_for_task} finished: {text}"),
                parent_tool_use_id: Some(parent_tu_for_task),
                uuid: Uuid::new_v4(),
                session_id: agent_id_for_task.clone(),
            });
            store_for_task.complete_async_agent(&agent_id_for_task);
        });

        ToolResult {
            content: format!(
                "Agent {agent_id} running in background. Completion will be reported via AsyncAgentDone."
            ),
            is_error: false,
            metadata: Some({
                let mut m = std::collections::HashMap::new();
                m.insert("agent_id".into(), agent_id);
                m.insert("agent_type".into(), agent_def.agent_type.clone());
                m.insert("async".into(), "true".into());
                m
            }),
        }
    }
}

fn err(msg: &str) -> ToolResult {
    ToolResult { content: msg.to_string(), is_error: true, metadata: None }
}

fn build_child_system_prompt(agent: &AgentDefinition) -> SystemPrompt {
    SystemPrompt { sections: vec![agent.system_prompt.clone()] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::PermissionMode;

    #[tokio::test]
    async fn agent_tool_errors_on_unknown_subagent_type() {
        let store = Arc::new(Store::new());
        let cfg = shared::CliConfig::default();
        let tool_reg = Arc::new(ToolRegistry::new(store.clone(), cfg.clone()));
        let agent_reg = Arc::new(AgentRegistry::built_in_only());

        let tool = AgentTool {
            store: store.clone(),
            config: cfg.clone(),
            registry: agent_reg,
            tool_registry: tool_reg.clone(),
        };

        let bus = Arc::new(SessionBus::new("s-root".into()));
        let ctx = ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: Some(bus),
            auto_deny_prompts: false,
        };
        let input = serde_json::json!({
            "description": "do thing",
            "prompt": "thing prompt",
            "subagent_type": "does-not-exist",
        });
        let result = tool.call(input, &ctx).await;
        assert!(result.is_error);
        assert!(result.content.contains("unknown subagent"), "got: {}", result.content);
    }

    #[tokio::test]
    async fn agent_tool_name_is_task() {
        let store = Arc::new(Store::new());
        let cfg = shared::CliConfig::default();
        let tool_reg = Arc::new(ToolRegistry::new(store.clone(), cfg.clone()));
        let agent_reg = Arc::new(AgentRegistry::built_in_only());
        let tool = AgentTool {
            store, config: cfg, registry: agent_reg, tool_registry: tool_reg,
        };
        assert_eq!(tool.name(), "Task");
    }
}
```

- [ ] **Step 4: Update `cli/src/tools/mod.rs` to construct the new AgentTool**

The current code (lines 122-126):

```rust
        tools.push(Arc::new(AgentTool {
            store: store.clone(),
            config: config.clone(),
        }));
```

…doesn't compile any more because the struct fields changed. Replace `ToolRegistry::new` to ALSO accept an `Arc<AgentRegistry>` and to use it when constructing `AgentTool`. Change the `new` signature at `cli/src/tools/mod.rs:79`:

From:

```rust
    pub fn new(store: Arc<Store>, config: shared::CliConfig) -> Self {
```

To:

```rust
    pub fn new(
        store: Arc<Store>,
        config: shared::CliConfig,
        agent_registry: Arc<crate::agents::AgentRegistry>,
    ) -> Self {
```

There's a chicken-and-egg problem: `AgentTool` needs `Arc<ToolRegistry>` and `ToolRegistry` holds `AgentTool`. Solve it by constructing the `ToolRegistry` first (with all tools EXCEPT AgentTool), wrap in `Arc`, then push `AgentTool` after with `tool_registry: arc.clone()`. Refactor `ToolRegistry::new` to return `Arc<Self>` instead of `Self`:

```rust
    pub fn new(
        store: Arc<Store>,
        config: shared::CliConfig,
        agent_registry: Arc<crate::agents::AgentRegistry>,
    ) -> Arc<Self> {
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();

        // ... all the existing pushes EXCEPT AgentTool ...

        // Wrap in registry first so AgentTool can hold an Arc to it.
        let registry = Arc::new(Self { tools: Arc::new(RwLock::new(tools)) });
        registry.register(Arc::new(AgentTool {
            store: store.clone(),
            config: config.clone(),
            registry: agent_registry,
            tool_registry: registry.clone(),
        }));
        // ... TaskCreateTool, TaskGetTool, etc — keep their pushes via registry.register() too
        registry
    }
```

Every caller of `ToolRegistry::new` must adapt:

```bash
grep -rn "ToolRegistry::new" cli/src/
```

Update `cli/src/bootstrap.rs:21-24` to construct the agent registry first and pass it in. Update test sites the same way (the tests use `ToolRegistry::new(store, CliConfig::default())` — change to `ToolRegistry::new(store, CliConfig::default(), Arc::new(AgentRegistry::built_in_only()))`).

The return type change `Self → Arc<Self>` ripples — `registry: Arc<ToolRegistry>` was already the consumer pattern, so most call sites simplify. But internal tests that do `let registry = ToolRegistry::new(...)` followed by `registry.register(...)` will keep working since `register` already takes `&self`.

- [ ] **Step 5: Run tests**

```bash
cargo test -p super-cli --lib
```

Expected: PASS. The two new tests verify Task naming and unknown-subagent error handling.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tools/agent.rs cli/src/tools/mod.rs cli/src/bootstrap.rs
git commit -m "feat(agent-tool): rewrite as Task, sync + async paths, agent registry wiring"
```

---

## Task 17: Carry the current tool_use_id into `ToolCallContext`

**Why:** AgentTool needs to know its OWN invoking `tool_use_id` so it can stamp child events with the correct `parent_tool_use_id`. Task 16's code uses `ctx.parent_tool_use_id` as a placeholder, which is wrong on the root call (it's `None` there).

**Files:**
- Modify: `cli/src/tools/contract.rs` (add `tool_use_id`)
- Modify: `cli/src/conversation/tool_loop.rs` (populate it from the `id` already in `safe`/`unsafe_`)
- Modify: `cli/src/tools/agent.rs` (use `ctx.tool_use_id` instead of `ctx.parent_tool_use_id` for the child stamp)

- [ ] **Step 1: Write the failing test**

Append to `#[cfg(test)] mod tests` in `cli/src/conversation/tool_loop.rs`:

```rust
    #[tokio::test]
    async fn run_tool_uses_passes_current_tool_use_id_to_context() {
        use crate::tools::contract::{Tool, ToolCallContext, ToolResult};
        use std::sync::{Arc, Mutex};

        struct CaptureTool { seen: Arc<Mutex<Option<String>>> }
        #[async_trait::async_trait]
        impl Tool for CaptureTool {
            fn name(&self) -> &str { "Capture2" }
            fn description(&self) -> &str { "capture" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn call(&self, _input: serde_json::Value, ctx: &ToolCallContext) -> ToolResult {
                *self.seen.lock().unwrap() = ctx.tool_use_id.clone();
                ToolResult { content: "ok".into(), is_error: false, metadata: None }
            }
        }

        let seen = Arc::new(Mutex::new(None));
        let store = Arc::new(Store::new());
        let agent_reg = Arc::new(crate::agents::AgentRegistry::built_in_only());
        let registry = ToolRegistry::new(store, CliConfig::default(), agent_reg);
        registry.register(Arc::new(CaptureTool { seen: seen.clone() }));

        let bus = Arc::new(SessionBus::new("s-root".into()));
        let _ = run_tool_uses(
            &registry,
            vec![("tu_actual".into(), "Capture2".into(), serde_json::json!({}))],
            std::env::current_dir().unwrap(),
            PermissionMode::Default,
            None,
            bus,
            None,
            "test-session".into(),
            false,
        ).await;

        assert_eq!(seen.lock().unwrap().clone().as_deref(), Some("tu_actual"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p super-cli --lib conversation::tool_loop::tests::run_tool_uses_passes_current_tool_use_id_to_context 2>&1 | head -20
```

Expected: compile error — `ctx.tool_use_id` doesn't exist.

- [ ] **Step 3: Add the field**

In `cli/src/tools/contract.rs`, inside `pub struct ToolCallContext`, add:

```rust
    /// The `tool_use_id` of the in-flight tool_use block that invoked this
    /// tool. Empty string is acceptable when constructed outside the tool
    /// loop (e.g. unit tests that aren't testing this field).
    pub tool_use_id: String,
```

Update the contract test:

```rust
        let ctx = ToolCallContext {
            cwd: std::path::PathBuf::from("/tmp"),
            permission_mode: crate::state::store::PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: true,
            tool_use_id: "tu_test".into(),
        };
        assert!(ctx.auto_deny_prompts);
        assert_eq!(ctx.tool_use_id, "tu_test");
```

- [ ] **Step 4: Populate in `tool_loop.rs`**

Both `ToolCallContext { ... }` literals in `cli/src/conversation/tool_loop.rs` must add `tool_use_id: id.clone(),`.

Find and update every other `ToolCallContext { ... }` construction site:

```bash
grep -rn "ToolCallContext {" cli/src/ | grep -v "pub struct"
```

For test sites where the value doesn't matter, use `tool_use_id: String::new(),`.

- [ ] **Step 5: Fix `AgentTool` to use `ctx.tool_use_id`**

In `cli/src/tools/agent.rs::call`, replace the `parent_tool_use_id` derivation in Task 16:

```rust
        let parent_tool_use_id = ctx.parent_tool_use_id.clone()
            .unwrap_or_else(|| { format!("tu_{}", Uuid::new_v4()) });
```

With:

```rust
        // The child's events must be stamped with the tool_use_id of THIS
        // Task invocation — not the chain id (`ctx.parent_tool_use_id`).
        // The tool_loop populates ctx.tool_use_id with the invoking block's id.
        let parent_tool_use_id = ctx.tool_use_id.clone();
```

- [ ] **Step 6: Run tests**

```bash
cargo test -p super-cli --lib
```

Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add cli/src/tools/contract.rs cli/src/conversation/tool_loop.rs cli/src/tools/agent.rs
git commit -m "feat(tools): carry invoking tool_use_id in ToolCallContext; subagents stamp it as parent_tool_use_id"
```

---

## Task 18: Wire `AgentRegistry` through `bootstrap.rs`

**Files:**
- Modify: `cli/src/bootstrap.rs`

- [ ] **Step 1: Modify bootstrap**

Replace `cli/src/bootstrap.rs:5-67` with the version that builds the agent registry and spawns the sidechain writer:

```rust
use std::sync::Arc;

use crate::config::load_config;

pub async fn run() {
    let config = load_config();

    if config.openrouter_api_key.is_none() {
        eprintln!("Not logged in. Run 'super login' first.");
        return;
    }

    let store = Arc::new(crate::state::store::Store::new());
    {
        let model = config.model.clone();
        store.set_state(|s| s.model = model);
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    let agent_registry = Arc::new(crate::agents::AgentRegistry::load(&cwd));

    let registry = crate::tools::ToolRegistry::new(
        store.clone(),
        config.clone(),
        agent_registry.clone(),
    );

    let skills = crate::skills::loader::load_all_skills();
    if !skills.is_empty() {
        registry.register(Arc::new(crate::tools::skill::SkillTool {
            skills: skills.clone(),
        }));
    }

    let bus = Arc::new(crate::conversation::session_bus::SessionBus::new(
        uuid::Uuid::new_v4().to_string(),
    ));

    // Spawn sidechain JSONL writer so any subagent activity gets persisted.
    let sidechain_dir = crate::conversation::sidechain::default_sidechain_dir(bus.session_id());
    crate::conversation::sidechain::spawn_sidechain_writer(bus.clone(), sidechain_dir);

    let engine = crate::conversation::engine::ConversationEngine::new(
        store.clone(),
        config.clone(),
        registry.clone(),
        bus.clone(),
    );

    let mut system_prompt = crate::conversation::system_prompt::SystemPrompt::build(&cwd);

    if !skills.is_empty() {
        let skill_desc: Vec<String> = skills.iter()
            .map(|s| format!("- {}: {}", s.name, s.description))
            .collect();
        system_prompt.add_section(format!("Available skills:\n{}", skill_desc.join("\n")));
    }

    let tool_descriptions =
        registry.tool_descriptions(&crate::state::store::PermissionMode::Default);
    system_prompt.add_section(format!("Available tools:\n{tool_descriptions}"));

    crate::tui::app::run_with_engine(config, store, engine, registry, bus, system_prompt).await;
}
```

- [ ] **Step 2: Build**

```bash
cargo build -p super-cli 2>&1 | tail -20
```

Expected: success.

- [ ] **Step 3: Smoke test**

```bash
cargo test -p super-cli --lib
```

Expected: all PASS.

- [ ] **Step 4: Commit**

```bash
git add cli/src/bootstrap.rs
git commit -m "feat(bootstrap): load AgentRegistry; spawn sidechain writer"
```

---

## Task 19: TUI — Task-specific summary line in `summarize_tool_call`

**Files:**
- Modify: `cli/src/tui/scroll_area.rs` (find `summarize_tool_call` — likely 30-100 lines, near the renderer)

- [ ] **Step 0: Locate the function**

```bash
grep -n "fn summarize_tool_call" cli/src/tui/scroll_area.rs
```

- [ ] **Step 1: Add a test for the Task case**

In whichever test module summarize_tool_call uses (or add one at the bottom of `cli/src/tui/scroll_area.rs`):

```rust
    #[test]
    fn summarize_tool_call_task_shows_description() {
        let input = serde_json::json!({
            "description": "find auth code",
            "prompt": "search the codebase for auth handlers",
            "subagent_type": "Explore",
        });
        let summary = summarize_tool_call("Task", &input);
        assert!(summary.contains("Explore"));
        assert!(summary.contains("find auth code"));
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p super-cli --lib tui::scroll_area::tests::summarize_tool_call_task_shows_description 2>&1 | head -20
```

Expected: FAIL — summary doesn't match.

- [ ] **Step 3: Implement the case**

At the top of `summarize_tool_call`'s match-by-name (the structure varies; if it's an `if name == ... { ... } else if ... { ... }` chain, add a branch; if it's a `match name { ... }`, add an arm):

```rust
    if name == "Task" {
        let agent = input.get("subagent_type").and_then(|v| v.as_str()).unwrap_or("?");
        let desc = input.get("description").and_then(|v| v.as_str()).unwrap_or("");
        return if desc.is_empty() {
            format!("({agent})")
        } else {
            format!("({agent}) {desc}")
        };
    }
```

Place this near the top of the function so it short-circuits before the generic fallback.

- [ ] **Step 4: Run tests**

```bash
cargo test -p super-cli --lib tui::scroll_area
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/scroll_area.rs
git commit -m "feat(tui): Task tool summary shows (subagent_type) description"
```

---

## Task 20: Integration test — sync subagent demuxed by fold filter

**Why:** End-to-end confidence: with mocked engine output, `fold(events, None)` and `fold(events, Some(TU))` produce the right views.

Since we have no mock OpenRouter in the codebase, this test exercises `fold` against hand-crafted events that mimic what a subagent would emit. It catches regressions in `matches_filter` for subagent-stamped events.

**Files:**
- Modify: `cli/src/tui/transcript.rs` (add tests)

- [ ] **Step 1: Add the test**

Append to `#[cfg(test)] mod tests` in `cli/src/tui/transcript.rs`:

```rust
    #[test]
    fn fold_demuxes_subagent_events_under_parent_tool_use_id() {
        let tu = "tu_task_1".to_string();
        let events = vec![
            // Root user prompt
            BusMessage::User {
                message: UserPayload {
                    role: "user".into(),
                    content: vec![ContentBlockFinal::Text { text: "find auth".into() }],
                },
                parent_tool_use_id: None,
                uuid: Uuid::new_v4(),
                session_id: "s-root".into(),
            },
            // Root assistant invokes Task
            env(StreamEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlockStream::ToolUse {
                    id: tu.clone(), name: "Task".into(),
                    input: serde_json::json!({"description": "find auth", "subagent_type": "Explore"}),
                },
            }),
            env(StreamEvent::ContentBlockStop { index: 0 }),
            // Subagent emits its own user prompt + assistant text — these should
            // NOT show up at the root level.
            BusMessage::User {
                message: UserPayload {
                    role: "user".into(),
                    content: vec![ContentBlockFinal::Text { text: "<subagent prompt>".into() }],
                },
                parent_tool_use_id: Some(tu.clone()),
                uuid: Uuid::new_v4(),
                session_id: "agent-xyz".into(),
            },
            BusMessage::StreamEvent {
                event: StreamEvent::ContentBlockStart { index: 0, content_block: ContentBlockStream::Text { text: "".into() } },
                parent_tool_use_id: Some(tu.clone()),
                uuid: Uuid::new_v4(),
                session_id: "agent-xyz".into(),
            },
            BusMessage::StreamEvent {
                event: StreamEvent::ContentBlockDelta { index: 0, delta: BlockDelta::TextDelta { text: "found auth in src/auth.rs".into() } },
                parent_tool_use_id: Some(tu.clone()),
                uuid: Uuid::new_v4(),
                session_id: "agent-xyz".into(),
            },
            BusMessage::StreamEvent {
                event: StreamEvent::ContentBlockStop { index: 0 },
                parent_tool_use_id: Some(tu.clone()),
                uuid: Uuid::new_v4(),
                session_id: "agent-xyz".into(),
            },
            // The tool_result that closes the Task block
            BusMessage::User {
                message: UserPayload {
                    role: "user".into(),
                    content: vec![ContentBlockFinal::ToolResult {
                        tool_use_id: tu.clone(),
                        content: "found auth in src/auth.rs".into(),
                        is_error: false,
                    }],
                },
                parent_tool_use_id: None,
                uuid: Uuid::new_v4(),
                session_id: "s-root".into(),
            },
        ];

        let root = fold(&events, None);
        // Root should have: User(prompt) + ToolCall(Task, with result attached).
        assert_eq!(root.len(), 2, "root view: {root:?}");
        assert!(matches!(&root[0], TranscriptItem::User { text } if text == "find auth"));
        match &root[1] {
            TranscriptItem::ToolCall { name, result, .. } => {
                assert_eq!(name, "Task");
                let r = result.as_ref().expect("tool result attached");
                assert!(r.content.contains("found auth"));
            }
            other => panic!("wrong root[1]: {other:?}"),
        }

        let sub = fold(&events, Some(&tu));
        // Subagent view should have: User(<subagent prompt>) + AssistantText("found auth in src/auth.rs")
        assert_eq!(sub.len(), 2, "sub view: {sub:?}");
        assert!(matches!(&sub[0], TranscriptItem::User { text } if text.contains("subagent prompt")));
        match &sub[1] {
            TranscriptItem::AssistantText { text, complete } => {
                assert_eq!(text, "found auth in src/auth.rs");
                assert!(*complete);
            }
            other => panic!("wrong sub[1]: {other:?}"),
        }
    }
```

- [ ] **Step 2: Run test**

```bash
cargo test -p super-cli --lib tui::transcript::tests::fold_demuxes_subagent_events_under_parent_tool_use_id
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add cli/src/tui/transcript.rs
git commit -m "test(transcript): fold demuxes subagent events under parent_tool_use_id"
```

---

## Task 21: Dead-code cleanup + system prompt advertises Task

**Files:**
- Modify: `cli/src/tools/contract.rs` (remove `// v1 None.` comments)
- Modify: `cli/src/tools/mod.rs` (verify `Agent` no longer appears as a tool name; only `Task`)
- Modify: `cli/src/conversation/system_prompt.rs` (no change unless tool description there mentions "Agent" by name)

- [ ] **Step 1: Sweep for stale references**

```bash
grep -rn "\"Agent\"" cli/src/ docs/
```

Anything in `cli/src/` that registers or names the old `Agent` tool: rename to `Task`. Documentation references to "Agent tool" should stay (it's the tool's role, not the name).

- [ ] **Step 2: Drop the v1-stub comments on `ToolCallContext`**

In `cli/src/tools/contract.rs`, the field docs for `parent_tool_use_id` and `bus` still say "Always None in v1." Update them to reflect that subagents are now wired:

```rust
    /// When the tool is invoked from inside a subagent, this carries the
    /// parent's invoking tool_use_id so emitted events can be demuxed by
    /// consumers (TUI, server forwarder, sidechain transcript writer).
    pub parent_tool_use_id: Option<String>,
    /// Handle to the session bus. Available to tools that need to emit
    /// events (subagent spawn, progress tickers).
    pub bus: Option<std::sync::Arc<crate::conversation::session_bus::SessionBus>>,
```

- [ ] **Step 3: Run the full test suite**

```bash
cargo test -p super-cli --lib
cargo build -p super-cli
```

Expected: all PASS, build succeeds without warnings related to subagent code (pre-existing dead-code warnings on unrelated paths are fine).

- [ ] **Step 4: Commit**

```bash
git add cli/src/tools/contract.rs cli/src/tools/mod.rs cli/src/conversation/system_prompt.rs
git commit -m "chore(tools): drop v1-stub comments on ToolCallContext; verify Task naming"
```

---

## Task 22: Acceptance check (tmux harness)

**Files:** none modified — verification step.

- [ ] **Step 1: Build release binary**

```bash
cargo build --release -p super-cli 2>&1 | tail -5
```

Expected: success.

- [ ] **Step 2: Verify the Task tool advertises subagent options**

```bash
./target/release/super --help 2>&1 | head -5
```

(super has no `--help` for tools; this is a smoke test that the binary doesn't crash on startup.)

- [ ] **Step 3: Manual tmux check (optional, skip if no Claude harness set up)**

If the tmux parity harness from `RUNNING.md` is available:

```bash
SOCK=parity-subagent
tmux -L "$SOCK" -f /dev/null new-session -d -s super -x 120 -y 40 './target/release/super'
sleep 1
tmux -L "$SOCK" send-keys -t super -l "use the Explore subagent to find where session_id is initialized in this repo"
tmux -L "$SOCK" send-keys -t super Enter
sleep 30
tmux -L "$SOCK" capture-pane -t super -p > /tmp/super-subagent.txt
grep -E "Task|⏺|⎿" /tmp/super-subagent.txt | head -20
```

Expected lines in the captured output:
- `⏺ Task (Explore) use the Explore subagent...` (or similar Task summary)
- A `⎿` indented final-text block from the Explore agent
- No panic, no stack trace

- [ ] **Step 4: Verify sidechain file**

```bash
ls ~/.super/sessions/*/sidechains/ 2>/dev/null | head
```

Expected: at least one `<agent_id>.jsonl` file from the run.

- [ ] **Step 5: Final summary commit (if any cleanup was needed)**

If steps 1–4 surfaced any issue, fix and commit. Otherwise:

```bash
git log --oneline -22
```

Should show ~22 commits matching the task numbers above.

---

## Self-review

This plan implements every section of `2026-05-15-subagent-execution-design.md`:

| Spec section | Implementing tasks |
|---|---|
| Protocol additions (SystemEvent parent_tool_use_id, AsyncAgentDone) | Tasks 1, 2 |
| `cli/src/agents/` module (definition, model, permission, built_in, loader, registry) | Tasks 3–8 |
| `ConversationEngine` (session_id_override, parent_tool_use_id, new_child) | Tasks 9, 12 |
| `tool_loop` populates bus + parent_tool_use_id + auto_deny | Tasks 10, 11, 17 |
| `ToolRegistry::filter_for_agent` | Task 13 |
| Async-agent tracking | Task 14 |
| Sidechain persistence | Task 15 |
| AgentTool rewrite | Tasks 16, 17 |
| Bootstrap wiring | Task 18 |
| TUI Task summary | Task 19 |
| Acceptance | Tasks 20, 22 |
| Cleanup | Task 21 |

**Placeholder scan:** no `TBD`s; all code blocks are concrete. Tasks 6 (built-in prompts) deliberately reference `claude-code-src/` as the source of the verbatim text, since copying multi-KB prompt bodies into this plan would be wasteful — the implementer reads the TS file and copies the string. Replacement rules for TS template variables are spelled out.

**Type consistency check:**
- `AsyncAgentHandle` fields match between Task 14 (definition) and Task 16 (construction).
- `AgentDefinition` fields match between Task 3 and every later consumer (loader, registry, filter_for_agent, AgentTool).
- `ToolCallContext` adds `auto_deny_prompts` (Task 10) and `tool_use_id` (Task 17) — every later task that constructs the context includes both.
- `run_tool_uses` signature: 6 args today → 9 args after Task 11 → still 9 after Task 17 (no further additions). Confirmed.
- `ToolRegistry::new` returns `Self` today → `Arc<Self>` after Task 16. All call sites enumerated in Task 16 Step 4.

**Spec coverage gaps:** None I can find. SendMessage integration with AsyncAgentDone is explicitly out-of-scope in the spec; the SystemEvent render is enough for v1. Worktree/MCP/hooks/skills frontmatter remain stubs as the spec specifies.

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-15-subagent-execution.md`. Two execution options:

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration
2. **Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints

Which approach?
