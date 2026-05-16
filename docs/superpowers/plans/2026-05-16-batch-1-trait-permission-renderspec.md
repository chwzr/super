# Batch 1 — Tool Trait + PermissionResult + RenderSpec Foundations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lay the foundation for Claude-Code tool parity. Extend Super's `Tool` trait to mirror Claude's `Tool` interface 1:1, replace the binary `Decision` permission enum with a richer `PermissionResult`, introduce the `RenderSpec` framework-agnostic render description, and migrate every existing tool to the new contract — *without changing any tool's observable behavior*.

**Architecture:** Three new modules (`shared/src/render_spec.rs`, extended `cli/src/tools/contract.rs`, rewritten `cli/src/tools/permission.rs`), a `BusMessage::RenderEvent` variant on the session bus, and a behavior-preserving rewrite of all ~36 existing tools. The render side gets a no-op dispatcher in the TUI so tools can emit `RenderSpec::Nothing` until Batches 2–5 fill in real rendering. Each batched fix spec that comes after this one depends on the trait, types, and bus plumbing introduced here.

**Tech Stack:** Rust 2021, `async-trait`, `serde`, `tokio::sync::broadcast` (bus), `tokio::sync::mpsc` (per-call progress), ratatui (TUI), the existing `shared` workspace crate.

**Reference spec:** `docs/superpowers/specs/2026-05-16-tool-parity-claude-code-design.md`

---

## File Structure

### New files

| Path | Purpose |
|---|---|
| `shared/src/render_spec.rs` | The `RenderSpec` enum + supporting types. Lives in `shared` so server and (future) web frontend can serde-decode the same shapes. |
| `cli/src/tools/defaults.rs` | `build_tool!` declarative macro + default trait impls. Keeps per-tool files short. |
| `cli/tests/parity_smoke.rs` | Integration tests for Batch 1 contract migration. |

### Modified files

| Path | Change |
|---|---|
| `shared/src/lib.rs` | Convert flat module to a small mod tree exposing the existing types from `shared/src/auth.rs` (move) and adding `pub mod render_spec;`. |
| `cli/src/tools/contract.rs` | Extend `Tool` trait to full Claude-mirror surface; add `ValidationResult`, `ColorHint`, `InterruptBehavior`, `SearchReadKind`, `ProgressEvent`, `ProgressSink`, `GroupedCall`, `ToolResultBlock` types; add `progress_sink` and remove the dead `bus` deduplication on `ToolCallContext`; extend `ToolResult` with `mcp_meta` and `new_messages`. |
| `cli/src/tools/permission.rs` | Replace `Decision` with `PermissionResult`; add `DecisionReason`, `RuleSuggestion`, `RuleBehavior`; rewrite `PermissionRule` to carry input-pattern `content`; rewrite `evaluate()` to take `&dyn Tool, &Value`. |
| `cli/src/state/store.rs` | Rename `PermissionMode::Bypass` → `BypassPermissions`; remove `DontAsk` (semantics fold into `Auto`). |
| `cli/src/sdk/protocol.rs` | Add `BusMessage::RenderEvent` variant. |
| `cli/src/conversation/tool_loop.rs` | Pass tool input to `is_concurrency_safe(&Value)`; update `MissingTool` to new trait; rebuild `ToolCallContext` with `progress_sink`. |
| `cli/src/tools/mod.rs` | Update `assemble_for_mode` to call `is_read_only(&Value::Null)` for the schema-level filter (since the per-input variant requires an input). |
| `cli/src/tools/{agent,ask_user_question,bash,config_tool,cron_create,cron_delete,cron_list,edit,enter_plan_mode,enter_worktree,exit_plan_mode,exit_worktree,glob_tool,grep,lsp,monitor,notebook_edit,read,send_message,skill,sleep,structured_output,task_create,task_get,task_list,task_output,task_stop,task_update,todo_write,tool_search,web_fetch,web_search,write}.rs` | Migrate each `impl Tool` to the new contract. Behavior-preserving: keep current `call()` body; add `prompt()` placeholder, `check_permissions()` defaulting to `Allow`, `render_*` returning `RenderSpec::Nothing`, etc. |
| `cli/src/agents/{loader,mod,permission}.rs` | Update enum match arms after `PermissionMode` rename. |
| `cli/src/bootstrap.rs` | Update `PermissionMode` reference. |
| `cli/src/tui/render/mod.rs` | Add a stub `render_spec()` dispatcher that handles `RenderSpec::Nothing` (and nothing else, for now). |
| `cli/src/tui/app.rs` | Subscribe to `BusMessage::RenderEvent` on the existing bus reader; no-op for `Nothing`. |

### File responsibilities

- `render_spec.rs` owns the entire render-description schema. No tool-specific types leak in.
- `permission.rs` owns the permission decision logic. The rule store and matcher selection live here; tool-specific input matching lives in each tool's `prepare_permission_matcher`.
- `contract.rs` is the single source of truth for the `Tool` trait. Every tool implements it; nothing else does.
- `defaults.rs` exists so per-tool files stay readable. If a tool only needs the basics, it uses `build_tool!`.

---

## Phase A — `RenderSpec` foundation (shared crate)

### Task A.1 — Convert `shared/src/lib.rs` to a module tree

**Files:**
- Modify: `shared/src/lib.rs`
- Create: `shared/src/auth.rs` (moved from `lib.rs`)

- [ ] **Step 1: Create `shared/src/auth.rs` with the auth/config types currently in `lib.rs`**

Move everything from current `shared/src/lib.rs` lines 1–137 (auth + config + AuthError) into `shared/src/auth.rs` unchanged. The new file content is the entire current `lib.rs` body.

- [ ] **Step 2: Rewrite `shared/src/lib.rs` to declare the module tree**

```rust
pub mod auth;
pub mod render_spec;

pub use auth::*;
```

The `pub use auth::*;` preserves the existing flat re-export surface so no caller has to change `use shared::CliConfig;` → `use shared::auth::CliConfig;`.

- [ ] **Step 3: Build to verify nothing broke**

Run: `cargo build -p shared`
Expected: clean build, no warnings about unused items.

Run: `cargo build -p super-cli && cargo build -p server`
Expected: clean build — no callers should need changes.

- [ ] **Step 4: Run existing shared tests**

Run: `cargo test -p shared`
Expected: all `config_*` tests in the moved `auth.rs` still pass.

- [ ] **Step 5: Commit**

```bash
git add shared/src/lib.rs shared/src/auth.rs
git commit -m "refactor(shared): split lib.rs into auth.rs + module tree

Preserves the flat re-export surface via 'pub use auth::*;' so no
callers need updating. Prepares the crate for the upcoming
render_spec module."
```

### Task A.2 — Add `RenderSpec` core types

**Files:**
- Create: `shared/src/render_spec.rs`

- [ ] **Step 1: Write the failing test**

Create `shared/src/render_spec.rs` with this initial test (no `RenderSpec` defined yet — the test fails to compile):

```rust
use serde::{Deserialize, Serialize};

// (types defined below)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_round_trips_through_json() {
        let spec = RenderSpec::Nothing;
        let json = serde_json::to_string(&spec).unwrap();
        let back: RenderSpec = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, RenderSpec::Nothing));
        assert_eq!(json, r#"{"kind":"nothing"}"#);
    }

    #[test]
    fn header_serializes_with_kind_tag() {
        let spec = RenderSpec::Header {
            verb: "Reading".into(),
            target: Some("src/foo.rs".into()),
            tag: None,
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains(r#""kind":"header""#));
        assert!(json.contains(r#""verb":"Reading""#));
    }

    #[test]
    fn diff_round_trips() {
        let spec = RenderSpec::Diff {
            file_path: "/tmp/a".into(),
            hunks: vec![DiffHunk {
                old_start: 1, new_start: 1,
                lines: vec![DiffLine::Add("+x".into()), DiffLine::Remove("-y".into())],
            }],
        };
        let json = serde_json::to_string(&spec).unwrap();
        let back: RenderSpec = serde_json::from_str(&json).unwrap();
        match back {
            RenderSpec::Diff { hunks, .. } => assert_eq!(hunks.len(), 1),
            _ => panic!("wrong variant"),
        }
    }
}
```

- [ ] **Step 2: Run the test to verify it fails (compile error)**

Run: `cargo test -p shared --lib render_spec -- --nocapture`
Expected: compile error — `RenderSpec`, `DiffHunk`, `DiffLine` undefined.

- [ ] **Step 3: Add the type definitions**

Add to `shared/src/render_spec.rs` *above* the `#[cfg(test)]` block:

```rust
use std::path::PathBuf;

/// Framework-agnostic render description. Tools emit this; TUI / server /
/// web frontend interpret it. The bridge between Claude's React-typed
/// render hooks and Super's multi-frontend rendering surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderSpec {
    /// Inline header line with a verb + target (e.g. "Reading src/foo.rs").
    Header {
        verb: String,
        target: Option<String>,
        tag: Option<Tag>,
    },

    /// Multiline plain text. Used for fallback/raw output.
    Text { body: String, dim: bool },

    /// A code block with optional language; rendered monospace.
    Code {
        language: Option<String>,
        body: String,
        truncated: bool,
    },

    /// A unified diff. Renderer applies +/- colors and gutter.
    Diff {
        file_path: String,
        hunks: Vec<DiffHunk>,
    },

    /// File-path-prefixed list (Grep/Glob results).
    PathList {
        entries: Vec<PathEntry>,
        total: usize,
        truncated: bool,
    },

    /// Key/value pairs (e.g. tool-use tag, web search result metadata).
    KeyValues { rows: Vec<(String, String)> },

    /// Collapsible block. Children rendered only when expanded.
    Collapsible {
        summary: String,
        expanded_by_default: bool,
        children: Vec<RenderSpec>,
    },

    /// Vertical group of specs (rendered as a column).
    Group { children: Vec<RenderSpec> },

    /// Horizontal group of specs (rendered as a row).
    Row { children: Vec<RenderSpec> },

    /// Status badge: in-progress / success / error / rejected / queued.
    Status {
        state: StatusState,
        message: Option<String>,
    },

    /// Interactive widget. The renderer maps `widget` to its own input
    /// component; the agent loop suspends the turn until the user completes
    /// it. Defined further in render_spec::interactive below; Batch 1 only
    /// declares the variant so the enum is closed.
    Interactive {
        widget: InteractiveWidget,
        /// Renderer-agnostic, serializable response shape. Tools that
        /// produce an Interactive spec also document what response payload
        /// they expect to receive back.
        response_schema: serde_json::Value,
    },

    /// Empty — explicitly render nothing.
    Nothing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiffLine {
    Context(String),
    Add(String),
    Remove(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathEntry {
    pub path: PathBuf,
    pub line: Option<u32>,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Tag {
    Timeout { ms: u64 },
    Model { id: String },
    Truncated,
    ResumeId(String),
    Custom(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusState {
    Queued,
    InProgress,
    Success,
    Error,
    Rejected,
}

// InteractiveWidget is the only variant Batch 1 does not actually emit;
// included so the enum is forward-compatible with Batch 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractiveWidget {
    /// AskUserQuestion. 1–4 questions, each with options (single- or
    /// multi-select), optional preview, and an "Other" text fallback.
    MultiQuestion { questions: Vec<Question> },

    /// ExitPlanMode. Renderer shows the plan markdown and asks accept/reject.
    PlanApproval { plan_markdown: String },

    /// Permission prompt for a tool call.
    PermissionPrompt {
        tool_name: String,
        input_summary: String,
        rule_suggestions: Vec<RuleSuggestion>,
    },

    /// SendMessage to a peer agent / channel.
    MessageCompose {
        recipients: Vec<String>,
        draft: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    pub question: String,
    pub header: String,
    pub multi_select: bool,
    pub options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
    /// Markdown rendered in a monospace box (renderer-dependent).
    pub preview: Option<String>,
}

/// Permission rule suggestion shown on a permission prompt. Mirrors
/// Claude's `ruleSuggestions`. The actual `PermissionRule` type lives in
/// `cli/src/tools/permission.rs`; we duplicate the *transport* shape here
/// so `shared` doesn't depend on the cli crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSuggestion {
    pub label: String,
    pub rule_json: serde_json::Value,
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p shared --lib render_spec`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add shared/src/render_spec.rs
git commit -m "feat(shared): add RenderSpec framework-agnostic render schema

RenderSpec is the bridge from tool render hooks to the TUI / server /
web frontend. All consumers decode the same serde shape.

This commit defines the schema only — no consumers wired up yet."
```

### Task A.3 — Re-export `render_spec` from the shared crate root

**Files:**
- Modify: `shared/src/lib.rs`

- [ ] **Step 1: Add re-exports**

Update `shared/src/lib.rs`:

```rust
pub mod auth;
pub mod render_spec;

pub use auth::*;
pub use render_spec::{
    DiffHunk, DiffLine, InteractiveWidget, PathEntry, Question,
    QuestionOption, RenderSpec, RuleSuggestion, StatusState, Tag,
};
```

- [ ] **Step 2: Build to verify**

Run: `cargo build -p shared`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add shared/src/lib.rs
git commit -m "feat(shared): re-export RenderSpec types from crate root"
```

---

## Phase B — `PermissionResult` rewrite (cli)

### Task B.1 — Add `PermissionResult` and supporting types

**Files:**
- Modify: `cli/src/tools/permission.rs`

- [ ] **Step 1: Write the failing test**

Add to the end of `cli/src/tools/permission.rs` (above any existing `#[cfg(test)]`):

```rust
#[cfg(test)]
mod permission_result_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn allow_serializes_with_behavior_tag() {
        let r = PermissionResult::Allow {
            updated_input: Some(json!({"path": "/tmp/x"})),
            decision_reason: Some(DecisionReason::ToolDefault),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""behavior":"allow""#));
        assert!(s.contains(r#""updated_input""#));
    }

    #[test]
    fn deny_serializes_with_reason() {
        let r = PermissionResult::Deny {
            reason: "blocked".into(),
            decision_reason: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""behavior":"deny""#));
        assert!(s.contains(r#""reason":"blocked""#));
    }

    #[test]
    fn ask_includes_empty_rule_suggestions_when_none() {
        let r = PermissionResult::Ask {
            updated_input: None,
            rule_suggestions: vec![],
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""behavior":"ask""#));
        assert!(s.contains(r#""rule_suggestions":[]"#));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p super-cli --lib tools::permission::permission_result_tests`
Expected: compile error — types undefined.

- [ ] **Step 3: Add the types**

Insert at the top of `cli/src/tools/permission.rs` (after the existing `use` lines):

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum PermissionResult {
    Allow {
        #[serde(skip_serializing_if = "Option::is_none")]
        updated_input: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        decision_reason: Option<DecisionReason>,
    },
    Deny {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        decision_reason: Option<DecisionReason>,
    },
    Ask {
        #[serde(skip_serializing_if = "Option::is_none")]
        updated_input: Option<Value>,
        rule_suggestions: Vec<RuleSuggestion>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecisionReason {
    Rule { source: RuleSource, pattern: String },
    Hook { hook_name: String },
    Mode { mode: crate::state::store::PermissionMode },
    Classifier { confidence: f32 },
    ToolDefault,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSuggestion {
    /// Human-readable label for the prompt UI ("Always allow `git *`").
    pub label: String,
    pub rule: PermissionRule,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleBehavior {
    Allow,
    Deny,
    Ask,
}
```

Add `Serialize, Deserialize` to the existing `RuleSource` derive list (it currently is `#[derive(Debug, Clone, PartialEq, Eq)]`):

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    User, Project, Local, Flag, Policy, Session, BuiltIn,
}
```

Update the existing `PermissionRule` struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub tool_name: String,
    /// Input-matcher pattern. None = match any input for this tool.
    /// Tool-specific syntax interpreted by `prepare_permission_matcher`.
    /// Examples: "git *", "git status", "Read(/etc/*)".
    pub content: Option<String>,
    pub behavior: RuleBehavior,
    pub source: RuleSource,
}
```

The old `pattern: String, decision: Decision, compiled: Regex` fields go away — `compiled` is rebuilt per-evaluate in the new flow.

- [ ] **Step 4: Keep the legacy `Decision` enum but mark it deprecated**

For migration safety, keep `Decision` defined but unused, with a deprecation marker:

```rust
#[deprecated(note = "Use PermissionResult instead. To be removed once all tools migrate.")]
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}
```

We'll delete it in Task B.5 after the migration completes.

- [ ] **Step 5: Run the new tests to verify pass**

Run: `cargo test -p super-cli --lib tools::permission::permission_result_tests`
Expected: 3 passed.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tools/permission.rs
git commit -m "feat(permissions): add PermissionResult + DecisionReason + RuleSuggestion

Adds the richer permission decision shape from the parity spec.
Legacy Decision enum kept (deprecated) until all tools migrate."
```

### Task B.2 — Rename `PermissionMode::Bypass` → `BypassPermissions`; fold `DontAsk` into `Auto`

**Files:**
- Modify: `cli/src/state/store.rs`
- Modify: `cli/src/tools/permission.rs`
- Modify: `cli/src/agents/loader.rs`
- Modify: `cli/src/agents/mod.rs`
- Modify: `cli/src/agents/permission.rs`

- [ ] **Step 1: Update the enum**

In `cli/src/state/store.rs`, replace:

```rust
#[derive(Debug, Clone, Default, PartialEq)]
pub enum PermissionMode {
    #[default]
    Default,
    AcceptEdits,
    Bypass,
    Plan,
    DontAsk,
    Auto,
}
```

with:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    #[default]
    Default,
    AcceptEdits,
    /// Was `Bypass`. Renamed to match Claude Code's `bypassPermissions`.
    BypassPermissions,
    Plan,
    /// Was both `DontAsk` and `Auto`. Today's semantics: anything that
    /// would Ask is auto-Denied. A future spec wires the classifier in.
    Auto,
}
```

Note: `Copy` is safe because all variants are unit. `Serialize/Deserialize` lets the SDK protocol round-trip the mode by name.

- [ ] **Step 2: Update permission.rs match arms**

In `cli/src/tools/permission.rs`, replace:

```rust
PermissionMode::Bypass => return Decision::Allow,
PermissionMode::DontAsk => {
    // In DontAsk, auto-deny anything that would normally ask
}
```

with:

```rust
PermissionMode::BypassPermissions => return Decision::Allow,
PermissionMode::Auto => {
    // In Auto, auto-deny anything that would normally ask
    // (classifier wiring is a future spec; for now Auto ≡ legacy DontAsk).
}
```

(This block will be entirely rewritten in Task B.3 — for now we just keep it compiling.)

- [ ] **Step 3: Update agents/loader.rs**

In `cli/src/agents/loader.rs`, replace:

```rust
Some("bypassPermissions") => Some(PermissionMode::Bypass),
```

with:

```rust
Some("bypassPermissions") => Some(PermissionMode::BypassPermissions),
```

If `auto` was being parsed elsewhere, leave it alone — the variant is unchanged. Remove any `dontAsk` parse arm if present (none exists currently per the audit).

- [ ] **Step 4: Update agents/mod.rs and agents/permission.rs**

Replace every `PermissionMode::Bypass` with `PermissionMode::BypassPermissions` in:
- `cli/src/agents/mod.rs` (test cases on lines 64, 65)
- `cli/src/agents/permission.rs` (line 19)

Use `git grep -l 'PermissionMode::Bypass\b'` to find all remaining call sites (note the word boundary `\b` to avoid matching `BypassPermissions`).

- [ ] **Step 5: Build the whole workspace**

Run: `cargo build --workspace`
Expected: clean. If a stray `PermissionMode::Bypass` remains, fix it.

- [ ] **Step 6: Run the agents tests**

Run: `cargo test -p super-cli --lib agents`
Expected: all pass — the rename is name-only and tests asserted via `matches!` patterns that we updated.

- [ ] **Step 7: Commit**

```bash
git add cli/src/state/store.rs cli/src/tools/permission.rs cli/src/agents/
git commit -m "refactor(permissions): rename Bypass→BypassPermissions, fold DontAsk into Auto

Aligns Super's PermissionMode variant names with Claude Code's.
Auto inherits the old DontAsk behavior (Ask→Deny); classifier
wiring is deferred to a future spec.

PermissionMode now derives Serialize/Deserialize for round-trip
through the SDK protocol."
```

### Task B.3 — Rewrite `PermissionSystem::evaluate` to return `PermissionResult` and be input-aware

**Files:**
- Modify: `cli/src/tools/permission.rs`

- [ ] **Step 1: Write the failing test for the new evaluator shape**

Append to `cli/src/tools/permission.rs`:

```rust
#[cfg(test)]
mod evaluate_v2_tests {
    use super::*;
    use crate::state::store::PermissionMode;
    use crate::tools::contract::{Tool, ToolCallContext};
    use async_trait::async_trait;
    use serde_json::{json, Value};

    struct DummyTool {
        name: &'static str,
        read_only: bool,
    }

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str { self.name }
        fn description(&self, _input: Option<&Value>, _ctx: &crate::tools::contract::DescriptionCtx) -> String {
            "test".into()
        }
        fn prompt(&self, _ctx: &crate::tools::contract::PromptCtx) -> String { "".into() }
        fn input_schema(&self) -> Value { json!({"type":"object"}) }
        fn is_read_only(&self, _input: &Value) -> bool { self.read_only }
        async fn check_permissions(&self, _input: &Value, _ctx: &ToolCallContext) -> PermissionResult {
            PermissionResult::Allow { updated_input: None, decision_reason: Some(DecisionReason::ToolDefault) }
        }
        async fn call(
            &self,
            _input: Value,
            _ctx: &ToolCallContext,
            _progress: Option<crate::tools::contract::ProgressSink>,
        ) -> crate::tools::contract::ToolResult {
            unreachable!("not called in this test")
        }
        fn render_tool_use_message(
            &self,
            _input: &Value,
            _opts: &crate::tools::contract::RenderOpts,
        ) -> shared::RenderSpec {
            shared::RenderSpec::Nothing
        }
        fn map_tool_result_to_block(
            &self,
            _output: &Value,
            tool_use_id: &str,
        ) -> crate::tools::contract::ToolResultBlock {
            crate::tools::contract::ToolResultBlock {
                tool_use_id: tool_use_id.into(),
                content: crate::tools::contract::ToolResultContent::Text("ok".into()),
                is_error: false,
            }
        }
    }

    #[tokio::test]
    async fn bypass_mode_returns_allow_with_mode_reason() {
        let sys = PermissionSystem::new(PermissionMode::BypassPermissions);
        let tool = DummyTool { name: "X", read_only: false };
        let result = sys.evaluate(&tool, &json!({}), &dummy_ctx()).await;
        match result {
            PermissionResult::Allow { decision_reason: Some(DecisionReason::Mode { mode }), .. } => {
                assert_eq!(mode, PermissionMode::BypassPermissions);
            }
            other => panic!("expected Allow via Mode reason, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn plan_mode_denies_non_read_only_tool() {
        let sys = PermissionSystem::new(PermissionMode::Plan);
        let tool = DummyTool { name: "Y", read_only: false };
        let result = sys.evaluate(&tool, &json!({}), &dummy_ctx()).await;
        assert!(matches!(result, PermissionResult::Deny { .. }));
    }

    #[tokio::test]
    async fn plan_mode_allows_read_only_tool() {
        let sys = PermissionSystem::new(PermissionMode::Plan);
        let tool = DummyTool { name: "Z", read_only: true };
        let result = sys.evaluate(&tool, &json!({}), &dummy_ctx()).await;
        assert!(matches!(result, PermissionResult::Allow { .. }));
    }

    #[tokio::test]
    async fn defaults_to_tool_check_when_no_rules_match() {
        let sys = PermissionSystem::new(PermissionMode::Default);
        let tool = DummyTool { name: "W", read_only: false };
        let result = sys.evaluate(&tool, &json!({}), &dummy_ctx()).await;
        // DummyTool.check_permissions returns Allow with ToolDefault.
        assert!(matches!(result, PermissionResult::Allow { decision_reason: Some(DecisionReason::ToolDefault), .. }));
    }

    fn dummy_ctx() -> ToolCallContext {
        ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: false,
            tool_use_id: "tu_test".into(),
            progress_sink: None,
        }
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p super-cli --lib tools::permission::evaluate_v2_tests`
Expected: compile error — the new `Tool` trait methods don't exist yet. **This is the gate** — these tests will only pass once Tasks C.1–C.5 (trait extension) land. Mark them `#[ignore]` for now so they don't gate the commit:

Add `#[ignore = "blocked on Task C.5: Tool trait extension"]` to every `#[tokio::test]` above.

- [ ] **Step 3: Rewrite `evaluate` (still returning the OLD `Decision` for now)**

This step is *behavior-preserving*: keep the existing match-on-tool-name flow, but rename `DontAsk` → `Auto` and `Bypass` → `BypassPermissions`:

```rust
impl PermissionSystem {
    pub fn evaluate_legacy(&self, tool_name: &str, _input: &Value) -> Decision {
        match self.mode {
            PermissionMode::BypassPermissions => return Decision::Allow,
            PermissionMode::Auto => {
                // Auto-deny anything that would normally ask
            }
            _ => {}
        }

        let mut rules = self.rules.clone();
        rules.sort_by_key(|r| -(r.source.priority() as i32));

        for rule in &rules {
            // Legacy compiled-regex matching on tool name. The new
            // input-aware evaluate() will replace this in Task C.6 once
            // the Tool trait carries prepare_permission_matcher.
            if rule.tool_name == tool_name {
                return match rule.behavior {
                    RuleBehavior::Allow => Decision::Allow,
                    RuleBehavior::Deny => Decision::Deny,
                    RuleBehavior::Ask => Decision::Ask,
                };
            }
        }

        Decision::Ask
    }
}
```

Rename the existing `evaluate` → `evaluate_legacy` so callers temporarily still compile, and leave a `pub async fn evaluate` stub returning a `PermissionResult` that delegates to `evaluate_legacy` until we wire the new trait. The stub goes here:

```rust
impl PermissionSystem {
    /// Input-aware evaluation. Returns Claude-shaped PermissionResult.
    /// Stub during Batch 1: defers to evaluate_legacy and lifts Decision
    /// to PermissionResult. The full input-aware path (rule pattern
    /// matching via prepare_permission_matcher) is wired in Task D.5
    /// once the Tool trait carries prepare_permission_matcher.
    pub async fn evaluate(
        &self,
        tool: &dyn crate::tools::contract::Tool,
        input: &Value,
        _ctx: &crate::tools::contract::ToolCallContext,
    ) -> PermissionResult {
        // Mode shortcuts (with new naming):
        match self.mode {
            PermissionMode::BypassPermissions => {
                return PermissionResult::Allow {
                    updated_input: None,
                    decision_reason: Some(DecisionReason::Mode { mode: self.mode }),
                };
            }
            PermissionMode::Plan if !tool.is_read_only(input) => {
                return PermissionResult::Deny {
                    reason: "Plan mode allows read-only tools only.".into(),
                    decision_reason: Some(DecisionReason::Mode { mode: self.mode }),
                };
            }
            _ => {}
        }

        // Defer to the tool's own check.
        tool.check_permissions(input, _ctx).await
    }
}
```

- [ ] **Step 4: Update the existing tests for `evaluate_legacy`**

The existing tests in `permission.rs` call the old `evaluate(&str, &Value) -> Decision`. Update them to call `evaluate_legacy(&str, &Value) -> Decision`. Or add a thin `#[allow(dead_code)] fn legacy` wrapper.

Run: `cargo test -p super-cli --lib tools::permission`
Expected: existing tests pass; `permission_result_tests` pass; `evaluate_v2_tests` skipped (`#[ignore]`).

- [ ] **Step 5: Commit**

```bash
git add cli/src/tools/permission.rs
git commit -m "feat(permissions): add input-aware evaluate() returning PermissionResult

Adds the new evaluate(&dyn Tool, &Value, &ToolCallContext) signature
that returns PermissionResult. Today it short-circuits to the tool's
own check_permissions() — the full rule-pattern matching via
prepare_permission_matcher is wired once the Tool trait exposes
that method (Task D.5).

evaluate_legacy() preserved for callers still on Decision."
```

---

## Phase C — Tool trait extension (cli)

### Task C.1 — Add supporting types in contract.rs

**Files:**
- Modify: `cli/src/tools/contract.rs`

- [ ] **Step 1: Write the failing test**

Append to `cli/src/tools/contract.rs`:

```rust
#[cfg(test)]
mod new_types_tests {
    use super::*;

    #[test]
    fn validation_result_ok_is_default() {
        let v = ValidationResult::Ok;
        assert!(matches!(v, ValidationResult::Ok));
    }

    #[test]
    fn interrupt_behavior_defaults_to_block() {
        assert_eq!(InterruptBehavior::default(), InterruptBehavior::Block);
    }

    #[test]
    fn search_read_kind_default_is_all_false() {
        let k = SearchReadKind::default();
        assert!(!k.is_search && !k.is_read && !k.is_list);
    }

    #[test]
    fn tool_result_block_text_round_trip() {
        let b = ToolResultBlock {
            tool_use_id: "tu_1".into(),
            content: ToolResultContent::Text("ok".into()),
            is_error: false,
        };
        let json = serde_json::to_string(&b).unwrap();
        let back: ToolResultBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_use_id, "tu_1");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p super-cli --lib tools::contract::new_types_tests`
Expected: compile error — types undefined.

- [ ] **Step 3: Add the types**

Insert into `cli/src/tools/contract.rs` (just below the existing `pub struct ToolResult { ... }` definition):

```rust
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Outcome of `Tool::validate_input` — pre-flight validation that the model
/// sees as a failed-tool result so it can self-correct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    Ok,
    Err { message: String, error_code: u32 },
}

/// User-facing color hint for a tool name pill in the TUI / web frontend.
/// Renderer maps to its own palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorHint {
    Default,
    Permission,
    Subagent,
    Tool,
}

/// What should happen when the user sends a new message while this tool
/// is mid-flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterruptBehavior {
    /// Stop the tool and discard its result.
    Cancel,
    /// Keep running; the new message waits.
    #[default]
    Block,
}

/// UI collapse hints. Tools that are "search-like" or "read-like" get
/// folded together in compact transcript views.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchReadKind {
    pub is_search: bool,
    pub is_read: bool,
    pub is_list: bool,
}

/// Streamed progress event from a long-running tool. Tools push these
/// into the `ProgressSink` channel during `call()`; the executor relays
/// them onto the session bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressEvent {
    Started,
    Update { data: serde_json::Value },
    Finished,
}

pub type ProgressSink = mpsc::Sender<ProgressEvent>;

/// Snapshot of a group of parallel tool calls for `render_grouped_tool_use`.
#[derive(Debug, Clone)]
pub struct GroupedCall {
    pub tool_use_id: String,
    pub input: serde_json::Value,
    pub is_resolved: bool,
    pub is_error: bool,
    pub is_in_progress: bool,
    pub result: Option<ToolResultBlock>,
    pub progress: Vec<ProgressEvent>,
}

/// Anthropic-shaped tool result block. Produced by `Tool::map_tool_result_to_block`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultBlock {
    pub tool_use_id: String,
    pub content: ToolResultContent,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Multi(Vec<ToolResultPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultPart {
    Text { text: String },
    Image { source: ImageSource },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub kind: String, // "base64"
    pub media_type: String,
    pub data: String,
}

/// Light context bags passed to the description / prompt / render hooks.
#[derive(Debug, Clone, Default)]
pub struct DescriptionCtx {
    pub is_non_interactive_session: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PromptCtx {
    pub is_non_interactive_session: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RenderOpts {
    pub verbose: bool,
    pub is_transcript_mode: bool,
}
```

- [ ] **Step 4: Run the tests to verify pass**

Run: `cargo test -p super-cli --lib tools::contract::new_types_tests`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tools/contract.rs
git commit -m "feat(tools): add supporting types for the extended Tool trait

Adds ValidationResult, ColorHint, InterruptBehavior, SearchReadKind,
ProgressEvent + ProgressSink, GroupedCall, ToolResultBlock /
ToolResultContent / ToolResultPart / ImageSource, and the context
bags DescriptionCtx / PromptCtx / RenderOpts. No trait changes yet —
that's Task C.2."
```

### Task C.2 — Extend `Tool` trait to the full Claude-mirror surface

**Files:**
- Modify: `cli/src/tools/contract.rs`

- [ ] **Step 1: Add the extended trait + ToolResult enrichment + ToolCallContext field**

Replace the existing `Tool` trait in `cli/src/tools/contract.rs` with:

```rust
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    // ── Identity ──────────────────────────────────────────────────────────
    fn name(&self) -> &str;
    fn aliases(&self) -> &[&'static str] { &[] }
    fn user_facing_name(&self, _input: Option<&serde_json::Value>) -> String {
        self.name().into()
    }
    fn user_facing_name_bg_color(&self, _input: Option<&serde_json::Value>) -> Option<ColorHint> {
        None
    }

    // ── Discovery / loading ───────────────────────────────────────────────
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String;
    fn prompt(&self, _ctx: &PromptCtx) -> String;
    fn search_hint(&self) -> Option<&'static str> { None }
    fn should_defer(&self) -> bool { false }
    fn always_load(&self) -> bool { false }
    fn is_enabled(&self) -> bool { true }

    // ── Schemas ───────────────────────────────────────────────────────────
    fn input_schema(&self) -> serde_json::Value;
    fn output_schema(&self) -> Option<serde_json::Value> { None }
    fn max_result_size_chars(&self) -> usize { 200_000 }
    fn strict(&self) -> bool { false }

    // ── Behavior flags (per-input) ────────────────────────────────────────
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { false }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool { false }
    fn is_destructive(&self, _input: &serde_json::Value) -> bool { false }
    fn is_open_world(&self, _input: &serde_json::Value) -> bool { false }
    fn is_mcp(&self) -> bool { false }
    fn is_lsp(&self) -> bool { false }
    fn requires_user_interaction(&self) -> bool { false }
    fn interrupt_behavior(&self) -> InterruptBehavior { InterruptBehavior::Block }
    fn inputs_equivalent(&self, a: &serde_json::Value, b: &serde_json::Value) -> bool {
        a == b
    }
    fn get_path(&self, _input: &serde_json::Value) -> Option<std::path::PathBuf> { None }
    fn is_search_or_read_command(&self, _input: &serde_json::Value) -> SearchReadKind {
        SearchReadKind::default()
    }
    fn is_transparent_wrapper(&self) -> bool { false }

    // ── Activity / spinner ────────────────────────────────────────────────
    fn get_activity_description(&self, _input: &serde_json::Value) -> Option<String> { None }
    fn get_tool_use_summary(&self, _input: &serde_json::Value) -> Option<String> { None }

    // ── Validation + permissions ──────────────────────────────────────────
    async fn validate_input(
        &self,
        _input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> ValidationResult {
        ValidationResult::Ok
    }
    async fn check_permissions(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> crate::tools::permission::PermissionResult {
        // Default: allow unchanged input with ToolDefault reason.
        crate::tools::permission::PermissionResult::Allow {
            updated_input: None,
            decision_reason: Some(
                crate::tools::permission::DecisionReason::ToolDefault,
            ),
        }
    }
    async fn prepare_permission_matcher(
        &self,
        _input: &serde_json::Value,
    ) -> Option<Box<dyn Fn(&str) -> bool + Send + Sync>> {
        None
    }
    fn to_auto_classifier_input(&self, _input: &serde_json::Value) -> serde_json::Value {
        serde_json::Value::String(String::new())
    }
    fn backfill_observable_input(&self, _input: &mut serde_json::Value) {}

    // ── Execution ─────────────────────────────────────────────────────────
    async fn call(
        &self,
        input: serde_json::Value,
        context: &ToolCallContext,
        on_progress: Option<ProgressSink>,
    ) -> ToolResult;

    // ── Render hooks ──────────────────────────────────────────────────────
    fn render_tool_use_message(
        &self,
        _input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> shared::RenderSpec {
        shared::RenderSpec::Nothing
    }
    fn render_tool_use_tag(&self, _input: &serde_json::Value) -> Option<shared::RenderSpec> {
        None
    }
    fn render_tool_use_progress_message(
        &self,
        _progress: &[ProgressEvent],
        _opts: &RenderOpts,
    ) -> Option<shared::RenderSpec> {
        None
    }
    fn render_tool_use_queued_message(&self) -> Option<shared::RenderSpec> { None }
    fn render_tool_result_message(
        &self,
        _output: &serde_json::Value,
        _progress: &[ProgressEvent],
        _opts: &RenderOpts,
    ) -> Option<shared::RenderSpec> {
        None
    }
    fn render_tool_use_rejected_message(
        &self,
        _input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> Option<shared::RenderSpec> {
        None
    }
    fn render_tool_use_error_message(
        &self,
        _err: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> Option<shared::RenderSpec> {
        None
    }
    fn render_grouped_tool_use(
        &self,
        _calls: &[GroupedCall],
        _opts: &RenderOpts,
    ) -> Option<shared::RenderSpec> {
        None
    }
    fn is_result_truncated(&self, _output: &serde_json::Value) -> bool { false }
    fn extract_search_text(&self, _output: &serde_json::Value) -> Option<String> { None }

    // ── Result mapping ────────────────────────────────────────────────────
    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        // Default: stringify the JSON. Tools with richer outputs override.
        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(
                output.as_str().map(String::from).unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}
```

Notes:
- Every method except `name`, `description`, `prompt`, `input_schema`, and `call` has a sensible default. Tools that need richer behavior override; tools that don't get the default for free.
- The default `check_permissions` returns `Allow{..ToolDefault}`. Combined with `PermissionSystem::evaluate`'s mode shortcuts (Bypass / Plan), this preserves current behavior for every tool that doesn't override.
- `render_tool_use_message` defaults to `RenderSpec::Nothing` so the existing tool-call render path (which doesn't consume `RenderSpec` yet) keeps working.

- [ ] **Step 2: Update `ToolResult` with `mcp_meta` and `new_messages`**

Replace the existing `ToolResult` struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<std::collections::HashMap<String, String>>,
    /// Additional text blocks to inject into the conversation alongside
    /// the tool_result. In-process only.
    #[serde(skip)]
    pub inject_messages: Vec<String>,
    /// MCP-shaped passthrough metadata (structuredContent, _meta) for
    /// SDK consumers. Empty for non-MCP tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_meta: Option<serde_json::Value>,
    /// New conversation messages to inject. Used by tools whose effect
    /// includes adding user/assistant/system messages (TodoWrite,
    /// SendMessage). In-process only.
    #[serde(skip)]
    pub new_messages: Vec<serde_json::Value>,
}

impl Default for ToolResult {
    fn default() -> Self {
        Self {
            content: String::new(),
            is_error: false,
            metadata: None,
            inject_messages: Vec::new(),
            mcp_meta: None,
            new_messages: Vec::new(),
        }
    }
}
```

- [ ] **Step 3: Add `progress_sink` to `ToolCallContext`**

Replace the existing struct:

```rust
pub struct ToolCallContext {
    pub cwd: std::path::PathBuf,
    pub permission_mode: crate::state::store::PermissionMode,
    pub abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
    pub parent_tool_use_id: Option<String>,
    pub bus: Option<std::sync::Arc<crate::conversation::session_bus::SessionBus>>,
    pub auto_deny_prompts: bool,
    pub tool_use_id: String,
    /// Optional per-call progress channel. When set, the tool can push
    /// `ProgressEvent`s and the executor relays them onto the session bus.
    pub progress_sink: Option<ProgressSink>,
}
```

The existing `tool_call_context_carries_auto_deny_flag` test needs the new field — update it:

```rust
let ctx = ToolCallContext {
    // ... existing fields ...
    progress_sink: None,
};
```

- [ ] **Step 4: Build**

Run: `cargo build -p super-cli`
Expected: **many compile errors** in the individual tool files (they still implement the old trait shape). That's expected — Phase E migrates them. Do *not* commit yet.

- [ ] **Step 5: Confirm only tool files fail**

The errors should all be of the form:
- `error[E0046]: not all trait items implemented` in each `tools/<name>.rs`
- `error[E0050]: method has 3 parameters but the declaration in the trait has 4` (for `is_concurrency_safe`, etc.)

If there are errors *outside* the `tools/` directory, fix them in this step. Expected: only `tool_loop.rs` (the `MissingTool` impl and the `is_concurrency_safe()` call) needs touching.

In `cli/src/conversation/tool_loop.rs` line 31, change:

```rust
Some(tool) if tool.is_concurrency_safe() => {
```

to:

```rust
Some(tool) if tool.is_concurrency_safe(&input) => {
```

Then leave the `MissingTool` impl alone — it will be migrated in Task D.4.

- [ ] **Step 6: Disable the tool-registry assembly tests temporarily**

In `cli/src/tools/mod.rs`, the `registry_tests` module's tests instantiate the full registry. They'll fail until all tools migrate. Add `#[ignore = "blocked on Phase E tool migration"]` to each `#[test]` in `registry_tests`.

- [ ] **Step 7: Build to confirm only tool-file errors remain**

Run: `cargo build -p super-cli 2>&1 | grep -E "^error" | head -50`
Expected: every error is in `cli/src/tools/<name>.rs` (one of the 33 per-tool files). Nothing else.

- [ ] **Step 8: Do NOT commit yet**

Phase E completes this work. We commit at the end of Task C.5 with a stub message and ship the migration as one continuous PR-batch.

### Task C.3 — Add the `build_tool!` defaults helper

**Files:**
- Create: `cli/src/tools/defaults.rs`
- Modify: `cli/src/tools/mod.rs`

- [ ] **Step 1: Create the defaults module**

`cli/src/tools/defaults.rs`:

```rust
//! Default helpers for `Tool` impls so per-tool files stay short.
//!
//! Usage pattern: a tool struct implements only the methods it needs to
//! override; the rest come from the trait defaults defined in
//! `crate::tools::contract::Tool`.
//!
//! For tools that want to be even more concise, the `tool_defaults!`
//! macro can be expanded inside an impl block to spell out the most
//! commonly-overridden defaults. Use only when it saves real
//! boilerplate — most tools are fine relying on the trait defaults.

/// Expand inside an `impl Tool for X` block to add the four most-overridden
/// flag methods with read-only / safe / non-destructive defaults.
#[macro_export]
macro_rules! tool_read_only_defaults {
    () => {
        fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { true }
        fn is_destructive(&self, _input: &serde_json::Value) -> bool { false }
        fn is_open_world(&self, _input: &serde_json::Value) -> bool { false }
    };
}

/// Expand for tools that perform writes (file edits, shell side-effects).
#[macro_export]
macro_rules! tool_write_defaults {
    () => {
        fn is_read_only(&self, _input: &serde_json::Value) -> bool { false }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { false }
        fn is_destructive(&self, _input: &serde_json::Value) -> bool { true }
        fn is_open_world(&self, _input: &serde_json::Value) -> bool { false }
    };
}
```

- [ ] **Step 2: Wire the module**

Add to `cli/src/tools/mod.rs`:

```rust
pub mod defaults;
```

- [ ] **Step 3: Build to verify the module compiles**

Run: `cargo build -p super-cli 2>&1 | grep "tools/defaults" | head -10`
Expected: no errors mentioning the new file (rest of the workspace still has the Phase C migration errors).

### Task C.4 — Migrate `Read` tool (the canonical pattern)

**Files:**
- Modify: `cli/src/tools/read.rs`

This task documents the *pattern* for migrating every tool. The same pattern repeats in Phase E for each remaining tool.

- [ ] **Step 1: Read the current shape of `read.rs`**

Open `cli/src/tools/read.rs` to refresh memory; the migration changes:
1. `description(&self) -> &str` → `description(&self, _input: Option<&Value>, _ctx: &DescriptionCtx) -> String`
2. `is_concurrency_safe(&self) -> bool` → `is_concurrency_safe(&self, _input: &Value) -> bool`
3. `is_read_only(&self) -> bool` → `is_read_only(&self, _input: &Value) -> bool`
4. `call(&self, input, _context) -> ToolResult` → `call(&self, input, _context, _on_progress: Option<ProgressSink>) -> ToolResult`
5. Add `fn prompt(&self, _ctx: &PromptCtx) -> String { include_str!("../../../docs/superpowers/specs/ported-prompts/read.md").into() }` as a placeholder — for Batch 1 we write the placeholder file with a TODO marker; Batch 3 actually ports the Claude prompt content into it.

- [ ] **Step 2: Create the per-tool prompt placeholder file**

Create directory: `cli/src/tools/prompts/`
Create file: `cli/src/tools/prompts/read.txt` with content:

```
TODO(parity:batch-3): port the FileReadTool prompt from
claude-code-src/tools/FileReadTool/prompt.ts (PROMPT export).
```

- [ ] **Step 3: Update the `Read` tool impl**

Replace the existing `impl Tool for ReadTool` block:

```rust
use crate::tools::contract::{
    DescriptionCtx, PromptCtx, RenderOpts, Tool, ToolCallContext,
    ToolResult, ToolResultBlock, ToolResultContent, ProgressSink,
};

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str { "Read" }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Reads a file from the local filesystem. Supports text, images, PDFs, and Jupyter notebooks.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/read.txt").into()
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "The line number to start reading from (0-based for internal processing, used as 1-based in output)",
                    "minimum": 0
                },
                "limit": {
                    "type": "integer",
                    "description": "The number of lines to read",
                    "minimum": 1
                }
            },
            "required": ["file_path"]
        })
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { true }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }

    fn get_path(&self, input: &serde_json::Value) -> Option<std::path::PathBuf> {
        input.get("file_path").and_then(|v| v.as_str()).map(std::path::PathBuf::from)
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        // ── Body unchanged from the previous implementation ──
        let file_path = match input.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return ToolResult {
                    content: "Missing required parameter: file_path".to_string(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };
        // ... rest of the existing body ...
        // (Engineer: copy from the existing call() body verbatim. The
        //  ONLY signature change is the trailing _on_progress parameter.)
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(
                output.as_str().map(String::from).unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}
```

- [ ] **Step 4: Build just this file**

Run: `cargo build -p super-cli 2>&1 | grep "tools/read.rs" | head -10`
Expected: no errors for `read.rs`. (Other tool files still fail; that's fine.)

- [ ] **Step 5: Pause — Read is the template**

Do not commit yet. The remaining tools migrate the same way in Phase E. Commit at the end of Task C.5 once Phase E completes.

### Task C.5 — Migrate all remaining tools to the extended trait

For each tool below, apply the exact same pattern as Task C.4:
1. Add the new `use` line for `DescriptionCtx, PromptCtx, RenderOpts, ToolResultBlock, ToolResultContent, ProgressSink`.
2. Change `description(&self) -> &str` to `description(&self, _input: Option<&Value>, _ctx: &DescriptionCtx) -> String` (return owned `String`, with the same content).
3. Add `fn prompt(&self, _ctx: &PromptCtx) -> String { include_str!("prompts/<tool>.txt").into() }`.
4. Create `cli/src/tools/prompts/<tool>.txt` with `TODO(parity:batch-<N>): port the <ToolName> prompt from claude-code-src/tools/<ToolName>/prompt.ts.`
5. Change every behavior flag method to take `_input: &Value`.
6. Add `_on_progress: Option<ProgressSink>` to `call()`'s signature.
7. Keep the `call()` body verbatim.
8. Add `fn map_tool_result_to_block(...)` using the default-style impl from Task C.4 unless the tool produces a richer shape (none do in Batch 1 — all tools currently return a `content: String`).

**Files to migrate (and which Batch they ultimately belong to, for the prompt placeholder marker):**

- [ ] **Step 1: File tools** — Migrate one tool per step using the pattern from Task C.4.

  - [ ] `cli/src/tools/edit.rs` — batch 3
  - [ ] `cli/src/tools/write.rs` — batch 3
  - [ ] `cli/src/tools/glob_tool.rs` — batch 3
  - [ ] `cli/src/tools/grep.rs` — batch 3
  - [ ] `cli/src/tools/notebook_edit.rs` — batch 3
  - [ ] Build: `cargo build -p super-cli 2>&1 | grep "tools/\(edit\|write\|glob_tool\|grep\|notebook_edit\)" | head -10` — expect no errors in these files.

- [ ] **Step 2: Shell + web tools**

  - [ ] `cli/src/tools/bash.rs` — batch 3
  - [ ] `cli/src/tools/web_fetch.rs` — batch 4
  - [ ] `cli/src/tools/web_search.rs` — batch 4
  - [ ] Build check.

- [ ] **Step 3: Task / todo tools**

  - [ ] `cli/src/tools/task_create.rs` — batch 4
  - [ ] `cli/src/tools/task_get.rs` — batch 4
  - [ ] `cli/src/tools/task_list.rs` — batch 4
  - [ ] `cli/src/tools/task_output.rs` — batch 4
  - [ ] `cli/src/tools/task_stop.rs` — batch 4
  - [ ] `cli/src/tools/task_update.rs` — batch 4
  - [ ] `cli/src/tools/todo_write.rs` — batch 2
  - [ ] Build check.

- [ ] **Step 4: Interactive tools** — these stay behavior-identical in Batch 1; Batch 2 adds the rich interactive widget flow.

  - [ ] `cli/src/tools/ask_user_question.rs` — batch 2
  - [ ] `cli/src/tools/enter_plan_mode.rs` — batch 2
  - [ ] `cli/src/tools/exit_plan_mode.rs` — batch 2
  - [ ] `cli/src/tools/send_message.rs` — batch 2
  - [ ] Build check.

- [ ] **Step 5: Agent + Skill**

  - [ ] `cli/src/tools/agent.rs` — batch 4
  - [ ] `cli/src/tools/skill.rs` — batch 4
  - [ ] Build check.

- [ ] **Step 6: Cron tools**

  - [ ] `cli/src/tools/cron_create.rs` — batch 5
  - [ ] `cli/src/tools/cron_delete.rs` — batch 5
  - [ ] `cli/src/tools/cron_list.rs` — batch 5
  - [ ] Build check.

- [ ] **Step 7: Long tail / stubs**

  - [ ] `cli/src/tools/sleep.rs` — batch 5
  - [ ] `cli/src/tools/monitor.rs` — batch 5
  - [ ] `cli/src/tools/tool_search.rs` — batch 5
  - [ ] `cli/src/tools/structured_output.rs` — batch 5
  - [ ] `cli/src/tools/lsp.rs` — batch 5
  - [ ] `cli/src/tools/config_tool.rs` — batch 5
  - [ ] `cli/src/tools/enter_worktree.rs` — batch 5
  - [ ] `cli/src/tools/exit_worktree.rs` — batch 5
  - [ ] Build check.

- [ ] **Step 8: Final build of the cli crate**

Run: `cargo build -p super-cli`
Expected: zero errors. If `tool_loop.rs`'s `MissingTool` still fails, leave it — Task D.4 fixes it.

- [ ] **Step 9: Commit the trait migration**

```bash
git add cli/src/tools/contract.rs \
        cli/src/tools/defaults.rs \
        cli/src/tools/mod.rs \
        cli/src/tools/prompts/ \
        cli/src/tools/*.rs
git commit -m "feat(tools): extend Tool trait to Claude-mirror surface; migrate all tools

Tool trait now exposes the full Claude-Code Tool interface: prompt(),
validate_input(), check_permissions() returning PermissionResult,
prepare_permission_matcher(), get_path(), get_activity_description(),
the render_*() family returning RenderSpec, output_schema(),
max_result_size_chars(), search_hint(), should_defer(),
requires_user_interaction(), interrupt_behavior(),
to_auto_classifier_input(), backfill_observable_input(),
inputs_equivalent(), is_concurrency_safe/read_only/destructive/
open_world per-input.

Every existing tool migrated to the new contract. Behavior is
preserved: each tool keeps its original call() body, prompt() loads
a TODO-marker placeholder from cli/src/tools/prompts/<tool>.txt (real
prompt text comes in Batches 2-5), render hooks return None /
RenderSpec::Nothing.

ToolResult gains mcp_meta and new_messages fields (in-process only,
not yet wired). ToolCallContext gains progress_sink for streaming
progress events.

Co-authored-by: parity-spec docs/superpowers/specs/2026-05-16-tool-parity-claude-code-design.md"
```

---

## Phase D — Bus + executor plumbing (cli)

### Task D.1 — Add `BusMessage::RenderEvent` variant

**Files:**
- Modify: `cli/src/sdk/protocol.rs`

- [ ] **Step 1: Write the failing test**

In `cli/src/sdk/protocol.rs`, append at the bottom (above any existing `#[cfg(test)]`):

```rust
#[cfg(test)]
mod render_event_tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn render_event_serializes_with_type_tag() {
        let msg = BusMessage::RenderEvent {
            tool_use_id: "tu_1".into(),
            spec: shared::RenderSpec::Nothing,
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"render_event""#));
        assert!(json.contains(r#""tool_use_id":"tu_1""#));
        assert!(json.contains(r#""kind":"nothing""#));
    }

    #[test]
    fn render_event_round_trips() {
        let msg = BusMessage::RenderEvent {
            tool_use_id: "tu_2".into(),
            spec: shared::RenderSpec::Status {
                state: shared::StatusState::Success,
                message: Some("done".into()),
            },
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: BusMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, BusMessage::RenderEvent { .. }));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p super-cli --lib sdk::protocol::render_event_tests`
Expected: compile error — `RenderEvent` variant not found.

- [ ] **Step 3: Add the variant**

In `cli/src/sdk/protocol.rs`, in the `pub enum BusMessage` definition, add this variant alongside the existing ones (before the closing `}`):

```rust
#[serde(rename = "render_event")]
RenderEvent {
    tool_use_id: String,
    spec: shared::RenderSpec,
    parent_tool_use_id: Option<String>,
    uuid: Uuid,
    session_id: String,
},
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p super-cli --lib sdk::protocol::render_event_tests`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add cli/src/sdk/protocol.rs
git commit -m "feat(sdk): add BusMessage::RenderEvent carrying a RenderSpec

Lets tools emit RenderSpec payloads onto the session bus, keyed by
tool_use_id. Consumed by the TUI render dispatcher and forwarded by
the server. Batch 1 only emits RenderSpec::Nothing; Batches 2-5 emit
real specs."
```

### Task D.2 — Migrate `MissingTool` to the new trait

**Files:**
- Modify: `cli/src/conversation/tool_loop.rs`

- [ ] **Step 1: Find the existing `MissingTool` impl**

It's around line 440–470 of `tool_loop.rs`. The current shape uses the old `description(&self) -> &str` etc.

- [ ] **Step 2: Rewrite the impl**

Replace the `impl Tool for MissingTool` block with:

```rust
use crate::tools::contract::{
    DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult,
};
use serde_json::Value;

#[async_trait::async_trait]
impl Tool for MissingTool {
    fn name(&self) -> &str { &self.name }

    fn description(&self, _input: Option<&Value>, _ctx: &DescriptionCtx) -> String {
        format!("unknown tool: {}", self.name)
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String { String::new() }

    fn input_schema(&self) -> Value {
        serde_json::json!({"type": "object"})
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool { true }
    fn is_read_only(&self, _input: &Value) -> bool { true }

    async fn call(
        &self,
        _input: Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        ToolResult {
            content: format!("Unknown tool: {}", self.name),
            is_error: true,
            ..Default::default()
        }
    }
}
```

- [ ] **Step 3: Update all `ToolCallContext { ... }` construction sites in `tool_loop.rs`**

Find every `ToolCallContext { ... }` literal and add `progress_sink: None,` to the field list. The pattern:

```rust
let ctx = ToolCallContext {
    cwd: cwd.clone(),
    permission_mode: permission_mode.clone(),
    abort_signal: abort_signal.clone(),
    parent_tool_use_id: parent_tool_use_id.clone(),
    bus: Some(bus.clone()),
    auto_deny_prompts,
    tool_use_id: id.clone(),
    progress_sink: None,   // <-- new field
};
```

Also update every `tool.call(input, &ctx)` to `tool.call(input, &ctx, None)`.

- [ ] **Step 4: Build the cli crate**

Run: `cargo build -p super-cli`
Expected: zero errors.

- [ ] **Step 5: Commit**

```bash
git add cli/src/conversation/tool_loop.rs
git commit -m "refactor(tool-loop): migrate MissingTool + ctx construction to new trait

MissingTool now implements the extended Tool trait. Every
ToolCallContext construction in tool_loop.rs sets progress_sink:
None and tool.call() gets an explicit None for on_progress."
```

### Task D.3 — Re-enable the ignored `evaluate_v2_tests` and `registry_tests`

**Files:**
- Modify: `cli/src/tools/permission.rs`
- Modify: `cli/src/tools/mod.rs`

- [ ] **Step 1: Remove `#[ignore]` from `evaluate_v2_tests`**

In `cli/src/tools/permission.rs`, delete every `#[ignore = "blocked on Task C.5: Tool trait extension"]` attribute from `mod evaluate_v2_tests`.

- [ ] **Step 2: Remove `#[ignore]` from `registry_tests`**

In `cli/src/tools/mod.rs`, delete every `#[ignore = "blocked on Phase E tool migration"]` attribute from `mod registry_tests`.

- [ ] **Step 3: Update `mod.rs` `assemble_for_mode` for the new `is_read_only(&Value)` signature**

The current code:

```rust
.filter(|t| match mode {
    PermissionMode::Plan => t.is_read_only(),
    _ => true,
})
```

becomes:

```rust
.filter(|t| match mode {
    PermissionMode::Plan => t.is_read_only(&serde_json::Value::Null),
    _ => true,
})
```

Schema-level filtering is approximate by design — we pass `Value::Null` because we don't have a real input at registry-listing time. Tools whose read-only-ness genuinely depends on input must override `is_read_only` to handle the `Null` case sensibly (today: only `Bash` would care, and Batch 3 handles it).

- [ ] **Step 4: Run all the previously-ignored tests**

Run: `cargo test -p super-cli --lib tools::permission::evaluate_v2_tests tools::registry_tests`
Expected: all pass.

- [ ] **Step 5: Run the full cli test suite**

Run: `cargo test -p super-cli`
Expected: every test passes. No `Decision`-related regressions; no panics; no removed-variant errors. If any test fails, **debug before continuing** — Phase E's migration is supposed to be behavior-preserving.

- [ ] **Step 6: Commit**

```bash
git add cli/src/tools/permission.rs cli/src/tools/mod.rs
git commit -m "test(tools): re-enable evaluate_v2 and registry tests after trait migration

Now that every tool implements the extended Tool trait and the
ToolCallContext carries progress_sink, the gated tests pass.
assemble_for_mode passes Value::Null when filtering by is_read_only
since no input is available at registry listing time."
```

### Task D.4 — Wire input-aware permission matching via `prepare_permission_matcher`

**Files:**
- Modify: `cli/src/tools/permission.rs`

- [ ] **Step 1: Write the failing test**

Append to `cli/src/tools/permission.rs`:

```rust
#[cfg(test)]
mod matcher_tests {
    use super::*;
    use crate::state::store::PermissionMode;
    use crate::tools::contract::{
        DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult,
    };
    use async_trait::async_trait;
    use serde_json::{json, Value};

    /// Stand-in for a Bash-style tool whose permission rules match on
    /// the `command` field rather than the tool name.
    struct CommandTool;

    #[async_trait]
    impl Tool for CommandTool {
        fn name(&self) -> &str { "Bash" }
        fn description(&self, _input: Option<&Value>, _ctx: &DescriptionCtx) -> String {
            "Bash test stand-in".into()
        }
        fn prompt(&self, _ctx: &PromptCtx) -> String { String::new() }
        fn input_schema(&self) -> Value { json!({"type":"object"}) }

        async fn prepare_permission_matcher(
            &self,
            input: &Value,
        ) -> Option<Box<dyn Fn(&str) -> bool + Send + Sync>> {
            let command = input.get("command").and_then(|v| v.as_str())?.to_string();
            // Pattern syntax: "git *" matches commands whose first word is "git".
            // "git status" matches exactly that. Plain "Bash" (no parens) matches all.
            Some(Box::new(move |pattern: &str| {
                let cmd = command.as_str();
                let first = cmd.split_whitespace().next().unwrap_or("");
                match pattern.split_once(' ') {
                    None => first == pattern,                       // exact stem match
                    Some((stem, "*")) => first == stem,            // prefix wildcard
                    Some(_) => cmd == pattern,                     // exact command match
                }
            }))
        }

        async fn call(
            &self,
            _input: Value,
            _ctx: &ToolCallContext,
            _on_progress: Option<ProgressSink>,
        ) -> ToolResult {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn rule_with_input_pattern_matches_command_stem() {
        let mut sys = PermissionSystem::new(PermissionMode::Default);
        sys.add_rule(PermissionRule {
            tool_name: "Bash".into(),
            content: Some("git *".into()),
            behavior: RuleBehavior::Allow,
            source: RuleSource::User,
        });
        let tool = CommandTool;
        let result = sys
            .evaluate(&tool, &json!({"command": "git status"}), &dummy_ctx())
            .await;
        match result {
            PermissionResult::Allow { decision_reason: Some(DecisionReason::Rule { pattern, .. }), .. } => {
                assert_eq!(pattern, "git *");
            }
            other => panic!("expected Allow via Rule(git *), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn rule_with_input_pattern_does_not_match_other_command() {
        let mut sys = PermissionSystem::new(PermissionMode::Default);
        sys.add_rule(PermissionRule {
            tool_name: "Bash".into(),
            content: Some("git *".into()),
            behavior: RuleBehavior::Allow,
            source: RuleSource::User,
        });
        let tool = CommandTool;
        let result = sys
            .evaluate(&tool, &json!({"command": "rm -rf /tmp/x"}), &dummy_ctx())
            .await;
        // Falls through to the tool's check_permissions (default: Allow ToolDefault).
        assert!(matches!(
            result,
            PermissionResult::Allow { decision_reason: Some(DecisionReason::ToolDefault), .. }
        ));
    }

    fn dummy_ctx() -> ToolCallContext {
        ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: false,
            tool_use_id: "tu_test".into(),
            progress_sink: None,
        }
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p super-cli --lib tools::permission::matcher_tests`
Expected: both tests fail — `evaluate` doesn't yet consult `prepare_permission_matcher`.

- [ ] **Step 3: Wire the matcher into `evaluate`**

Replace the existing `evaluate` body with:

```rust
pub async fn evaluate(
    &self,
    tool: &dyn crate::tools::contract::Tool,
    input: &Value,
    ctx: &crate::tools::contract::ToolCallContext,
) -> PermissionResult {
    // 1. Mode shortcuts.
    match self.mode {
        PermissionMode::BypassPermissions => {
            return PermissionResult::Allow {
                updated_input: None,
                decision_reason: Some(DecisionReason::Mode { mode: self.mode }),
            };
        }
        PermissionMode::Plan if !tool.is_read_only(input) => {
            return PermissionResult::Deny {
                reason: "Plan mode allows read-only tools only.".into(),
                decision_reason: Some(DecisionReason::Mode { mode: self.mode }),
            };
        }
        _ => {}
    }

    // 2. Rule matching with input-aware patterns.
    let tool_name = tool.name();
    let matcher = tool.prepare_permission_matcher(input).await;
    let mut sorted_rules = self.rules.clone();
    sorted_rules.sort_by_key(|r| -(r.source.priority() as i32));

    for rule in &sorted_rules {
        if rule.tool_name != tool_name {
            continue;
        }
        let matches = match (&rule.content, &matcher) {
            (None, _) => true,                                       // no pattern = match any
            (Some(_), None) => false,                                // pattern set but tool has no matcher → can't match
            (Some(pat), Some(m)) => m(pat),
        };
        if !matches {
            continue;
        }
        let reason = Some(DecisionReason::Rule {
            source: rule.source,
            pattern: rule.content.clone().unwrap_or_else(|| tool_name.into()),
        });
        return match rule.behavior {
            RuleBehavior::Allow => PermissionResult::Allow {
                updated_input: None,
                decision_reason: reason,
            },
            RuleBehavior::Deny => PermissionResult::Deny {
                reason: format!("Denied by {:?} rule.", rule.source),
                decision_reason: reason,
            },
            RuleBehavior::Ask if ctx.auto_deny_prompts => PermissionResult::Deny {
                reason: "Permission denied: async subagents cannot prompt the user.".into(),
                decision_reason: reason,
            },
            RuleBehavior::Ask => PermissionResult::Ask {
                updated_input: None,
                rule_suggestions: vec![],
            },
        };
    }

    // 3. Auto mode (was DontAsk): anything that would normally Ask is auto-denied.
    if matches!(self.mode, PermissionMode::Auto) && matches!(
        tool.check_permissions(input, ctx).await,
        PermissionResult::Ask { .. }
    ) {
        return PermissionResult::Deny {
            reason: "Auto mode: prompts are auto-denied.".into(),
            decision_reason: Some(DecisionReason::Mode { mode: self.mode }),
        };
    }

    // 4. Defer to the tool's own check.
    tool.check_permissions(input, ctx).await
}
```

- [ ] **Step 4: Run the matcher tests**

Run: `cargo test -p super-cli --lib tools::permission::matcher_tests`
Expected: 2 passed.

- [ ] **Step 5: Run the full cli test suite**

Run: `cargo test -p super-cli`
Expected: all pass.

- [ ] **Step 6: Delete the deprecated `Decision` enum and `evaluate_legacy`**

Both are now unused. Remove them from `cli/src/tools/permission.rs` along with any old tests that depended on them. The remaining tests should already cover the new flow.

If `git grep -n "\\bDecision::\\|evaluate_legacy" cli/` returns any hits, fix them. Expected: zero hits after the deletion.

- [ ] **Step 7: Final build + test**

Run: `cargo test -p super-cli`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add cli/src/tools/permission.rs
git commit -m "feat(permissions): input-aware rule matching via prepare_permission_matcher

evaluate() now consults each tool's prepare_permission_matcher(input)
closure to decide whether a rule pattern matches the input (e.g.
'git *' matches Bash {command: 'git status'}). Patterns without a
matcher fail closed.

Auto mode preserves the legacy DontAsk behavior: anything that would
Ask is auto-denied.

auto_deny_prompts on the context short-circuits Ask → Deny so async
subagents never hang waiting for the user.

Deprecated Decision enum and evaluate_legacy removed."
```

---

## Phase E — TUI render dispatcher stub

### Task E.1 — Add a `render_spec()` dispatcher that handles `Nothing`

**Files:**
- Modify: `cli/src/tui/render/mod.rs`

- [ ] **Step 1: Write the failing test**

Append to `cli/src/tui/render/mod.rs`:

```rust
#[cfg(test)]
mod render_spec_tests {
    use super::*;
    use shared::RenderSpec;

    #[test]
    fn nothing_renders_empty_vec() {
        let lines = render_spec(&RenderSpec::Nothing);
        assert!(lines.is_empty());
    }

    #[test]
    fn unhandled_variant_renders_placeholder() {
        let spec = RenderSpec::Header {
            verb: "Reading".into(),
            target: Some("src/foo.rs".into()),
            tag: None,
        };
        let lines = render_spec(&spec);
        // Batch 1 stub: any non-Nothing variant produces a one-line
        // dim placeholder. Batches 2-5 add real renderers.
        assert_eq!(lines.len(), 1);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p super-cli --lib tui::render::render_spec_tests`
Expected: compile error — `render_spec` undefined.

- [ ] **Step 3: Add the dispatcher**

Append to `cli/src/tui/render/mod.rs`:

```rust
/// Stub dispatcher: turns a `RenderSpec` into TUI lines. Batch 1 only
/// handles `Nothing`; every other variant produces a one-line dim
/// placeholder. Batches 2-5 fill in real renderers per variant.
pub fn render_spec(spec: &shared::RenderSpec) -> Vec<Line<'static>> {
    match spec {
        shared::RenderSpec::Nothing => Vec::new(),
        other => vec![Line::from(Span::styled(
            format!("[render_spec stub: {:?}]", std::mem::discriminant(other)),
            dim_style(),
        ))],
    }
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p super-cli --lib tui::render::render_spec_tests`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/render/mod.rs
git commit -m "feat(tui): add render_spec() stub dispatcher

Handles RenderSpec::Nothing (renders empty). All other variants
produce a debug placeholder line until Batches 2-5 implement them.

The function exists so tools can start emitting RenderSpec without
breaking the TUI; full rendering ships per-tool in later batches."
```

### Task E.2 — Subscribe to `BusMessage::RenderEvent` in the TUI app

**Files:**
- Modify: `cli/src/tui/app.rs`

- [ ] **Step 1: Find the existing `BusMessage` match arm in the TUI bus reader**

Grep: `grep -nE "BusMessage::(User|Assistant|StreamEvent|ToolProgress|SystemEvent|Result)" cli/src/tui/app.rs | head -20`

There should be a `match msg { ... }` block consuming `BusMessage`. Add a no-op arm for `RenderEvent`:

```rust
BusMessage::RenderEvent { .. } => {
    // Batch 1: tools only emit RenderSpec::Nothing, which renders to
    // nothing. Batches 2-5 wire this into the scrollback / live region.
}
```

- [ ] **Step 2: Build**

Run: `cargo build -p super-cli`
Expected: clean.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test -p super-cli`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add cli/src/tui/app.rs
git commit -m "feat(tui): handle BusMessage::RenderEvent (no-op stub)

The new variant must be matched exhaustively. Batch 1's no-op arm
lets the existing scrollback writer continue to render tools the
same way it does today; Batches 2-5 wire RenderSpec into the
scrollback writer."
```

---

## Phase F — Integration smoke test

### Task F.1 — End-to-end test exercising the new contract

**Files:**
- Create: `cli/tests/parity_smoke.rs`

- [ ] **Step 1: Write the test**

```rust
//! End-to-end smoke test for the Batch 1 trait/permission/renderspec migration.
//!
//! Verifies that:
//!   1. A registered tool can be looked up and called through the new contract.
//!   2. The permission system returns PermissionResult::Allow for a default-mode read.
//!   3. The session bus accepts a RenderEvent carrying RenderSpec::Nothing.

use std::sync::Arc;

use shared::RenderSpec;
use super_cli::conversation::session_bus::SessionBus;
use super_cli::sdk::protocol::BusMessage;
use super_cli::state::store::{PermissionMode, Store};
use super_cli::tools::contract::{
    DescriptionCtx, PromptCtx, RenderOpts, Tool, ToolCallContext,
};
use super_cli::tools::permission::{PermissionResult, PermissionSystem};
use super_cli::tools::ToolRegistry;

#[tokio::test]
async fn read_tool_round_trips_through_new_contract() {
    let store = Arc::new(Store::new());
    let registry = ToolRegistry::new(
        store.clone(),
        shared::CliConfig::default(),
        Arc::new(super_cli::agents::AgentRegistry::built_in_only()),
    );

    let read = registry.get("Read").expect("Read tool registered");
    let desc = read.description(None, &DescriptionCtx::default());
    assert!(desc.contains("Reads"));

    let prompt = read.prompt(&PromptCtx::default());
    // Placeholder marker until Batch 3 ports the real prompt text.
    assert!(prompt.contains("TODO(parity:batch-3)"));

    assert!(read.is_read_only(&serde_json::Value::Null));
    assert!(read.is_concurrency_safe(&serde_json::Value::Null));
    assert!(matches!(
        read.render_tool_use_message(&serde_json::Value::Null, &RenderOpts::default()),
        RenderSpec::Nothing
    ));
}

#[tokio::test]
async fn default_mode_permission_check_returns_allow() {
    let store = Arc::new(Store::new());
    let registry = ToolRegistry::new(
        store.clone(),
        shared::CliConfig::default(),
        Arc::new(super_cli::agents::AgentRegistry::built_in_only()),
    );
    let read = registry.get("Read").unwrap();
    let sys = PermissionSystem::new(PermissionMode::Default);
    let ctx = ToolCallContext {
        cwd: std::env::current_dir().unwrap(),
        permission_mode: PermissionMode::Default,
        abort_signal: None,
        parent_tool_use_id: None,
        bus: None,
        auto_deny_prompts: false,
        tool_use_id: "tu_smoke".into(),
        progress_sink: None,
    };
    let result = sys.evaluate(read.as_ref(), &serde_json::json!({"file_path":"/tmp/x"}), &ctx).await;
    assert!(matches!(result, PermissionResult::Allow { .. }));
}

#[tokio::test]
async fn bus_round_trips_render_event() {
    let bus = SessionBus::new("smoke".into());
    let mut rx = bus.subscribe();
    bus.emit(BusMessage::RenderEvent {
        tool_use_id: "tu_1".into(),
        spec: RenderSpec::Nothing,
        parent_tool_use_id: None,
        uuid: uuid::Uuid::new_v4(),
        session_id: "smoke".into(),
    });
    let received = rx.recv().await.unwrap();
    assert!(matches!(received, BusMessage::RenderEvent { .. }));
}

#[tokio::test]
async fn bypass_mode_short_circuits_to_allow() {
    let store = Arc::new(Store::new());
    let registry = ToolRegistry::new(
        store.clone(),
        shared::CliConfig::default(),
        Arc::new(super_cli::agents::AgentRegistry::built_in_only()),
    );
    let bash = registry.get("Bash").unwrap();
    let sys = PermissionSystem::new(PermissionMode::BypassPermissions);
    let ctx = ToolCallContext {
        cwd: std::env::current_dir().unwrap(),
        permission_mode: PermissionMode::BypassPermissions,
        abort_signal: None,
        parent_tool_use_id: None,
        bus: None,
        auto_deny_prompts: false,
        tool_use_id: "tu_smoke".into(),
        progress_sink: None,
    };
    let result = sys.evaluate(bash.as_ref(), &serde_json::json!({"command":"echo hi"}), &ctx).await;
    match result {
        PermissionResult::Allow { decision_reason, .. } => {
            assert!(decision_reason.is_some());
        }
        other => panic!("expected Allow, got {:?}", other),
    }
}
```

- [ ] **Step 2: Make `super_cli` available as a lib crate path for the integration test**

Check `cli/Cargo.toml` — there's no `[lib]` entry, only `[[bin]]`. Integration tests live in `cli/tests/` and compile against the bin's exported items.

If `cargo test -p super-cli --test parity_smoke` fails with "can't find crate `super_cli`", add a `[lib]` entry:

```toml
[lib]
name = "super_cli"
path = "src/main.rs"
```

Actually, having both a `[[bin]]` and a `[lib]` pointing at `main.rs` won't work. The cleanest fix is to split `main.rs` into a thin shim that calls into `lib.rs`:

If `cli/src/lib.rs` does not exist, create it:

```rust
//! Library entry for the super CLI. Re-exports the public modules so
//! integration tests (and future external consumers) can reach them.

pub mod agents;
pub mod auth;
pub mod bootstrap;
pub mod commands;
pub mod config;
pub mod conversation;
pub mod mcp;
pub mod providers;
pub mod sdk;
pub mod skills;
pub mod state;
pub mod tools;
pub mod tui;
```

Then update `cli/src/main.rs` to use the library:

```rust
fn main() {
    super_cli::bootstrap::run();
}
```

(If `bootstrap::run` doesn't exist with that exact name, use whatever the current entry-point function is called — `git grep -n "fn main" cli/src/main.rs` shows the current shape.)

Add a `[lib]` block to `cli/Cargo.toml`:

```toml
[lib]
name = "super_cli"
path = "src/lib.rs"
```

Keep the existing `[[bin]]` pointing at `src/main.rs`.

- [ ] **Step 3: Run the integration test**

Run: `cargo test -p super-cli --test parity_smoke`
Expected: 4 passed.

- [ ] **Step 4: Run the entire workspace test suite**

Run: `cargo test --workspace`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add cli/Cargo.toml cli/src/lib.rs cli/src/main.rs cli/tests/parity_smoke.rs
git commit -m "test(parity): add Batch 1 smoke test exercising the new contract

End-to-end checks that:
  - A registered tool can be looked up and queried through the new
    Tool trait (description, prompt, is_read_only, render hook).
  - The permission system returns PermissionResult::Allow for a
    default-mode read.
  - BypassPermissions mode short-circuits to Allow with a Mode reason.
  - BusMessage::RenderEvent round-trips through the session bus.

Refactors cli/src/main.rs to delegate to a new cli/src/lib.rs so
integration tests under cli/tests/ can reach internal modules."
```

---

## Self-Review Notes

After all tasks ship, the engineer should run:

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

All three must pass before the Batch 1 PR is opened.

### Spec coverage

| Spec §7 Batch 1 line item | Covered by |
|---|---|
| Extend `cli/src/tools/contract.rs` to the full `Tool` trait | Tasks C.1, C.2 |
| Add `cli/src/tools/defaults.rs` with macro/helpers | Task C.3 |
| Add `shared/src/render_spec.rs` | Tasks A.1, A.2, A.3 |
| Replace `permission::Decision` with `PermissionResult` | Tasks B.1, B.3, D.4 (and `Decision` deletion in D.4 step 6) |
| Update `evaluate()` to take `&dyn Tool, &Value` | Tasks B.3, D.4 |
| Wire `updated_input` through the executor | The `updated_input` field is plumbed in PermissionResult; executor honors it during Batch 2 when a real Ask-resolution flow runs. Batch 1 only ensures Allow's `updated_input` is *available* — it's not yet acted on because no tool produces it in Batch 1. **Covered as type-only in Task B.1; behavioral wiring in Batch 2.** |
| Add `ProgressSink` plumbing through `ToolCallContext` | Task C.2 (adds field), Task D.2 (threads `None` through every call site) |
| Migrate every existing tool to the new trait | Tasks C.4, C.5 |
| Update `ToolResult` with `mcp_meta` and `new_messages` | Task C.2 |
| Update `permission.rs` tests + Bash matcher smoke test | Tasks B.3, D.4 |

### Placeholder scan

Done. The only `TODO` markers are in `cli/src/tools/prompts/<tool>.txt` placeholder files, which are explicit handoffs to Batches 2–5 for actual prompt porting — this is the spec's stated plan, not a plan failure.

### Type consistency

- `PermissionResult` variant names (`Allow`/`Deny`/`Ask`) used consistently across permission.rs, contract.rs's `check_permissions`, evaluator tests, and matcher tests.
- `PermissionMode::BypassPermissions` (not `Bypass`) used consistently after Task B.2.
- `ToolCallContext.progress_sink` (not `progress`) used in every context construction (Tasks C.2, D.2, F.1).
- `Tool::is_concurrency_safe(&Value)` (not `&self`-only) used in tool_loop.rs (Task D.2) and registry (already-deferred mod.rs changes in D.3).
- `RenderSpec::Nothing` used at every render-hook default site.

---

Plan complete and saved to `docs/superpowers/plans/2026-05-16-batch-1-trait-permission-renderspec.md`. Two execution options:

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
