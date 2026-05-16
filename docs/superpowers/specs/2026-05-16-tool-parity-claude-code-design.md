# Tool Parity with Claude Code — Design

**Status:** Design accepted, awaiting user spec review
**Date:** 2026-05-16
**Owner:** felix.koppe@cavorit.de

## 1. Goals & Non-Goals

### Goals

1. Produce a **per-tool divergence audit** for every tool currently registered in Super, scored against its Claude counterpart on six axes: name, input schema, output schema, system prompt, behavior / side effects, and UX surface (permission flow, interactive prompts, render hooks).
2. Define the **extended `Tool` trait** in Rust that mirrors Claude's `Tool` interface 1:1, including render hooks adapted via a `RenderSpec` bridge.
3. Define the **`RenderSpec` schema** that tools emit and the TUI / server / web frontend consume.
4. Define the **`PermissionResult` + permission-rule** parity (input-aware matching, `updatedInput`, `decisionReason`, `ruleSuggestions`, mode interactions).
5. Carve the fix work into **5 implementation-ready batches**, ordered so each batch unblocks the next.
6. Define a **parity-test harness** so divergence is regression-prevented, not just one-shot fixed.

### Non-Goals

- Adding tools that don't exist in Super today (BriefTool, ListMcpResources, ReadMcpResource, RemoteTrigger, SendUserFile, PushNotification, Workflow, PowerShell, REPL, Tungsten, ExitPlanModeV2, TeamCreate/Delete, Subscribe, SuggestBackgroundPR, etc.). These get captured in a *missing-tools follow-up list*, separate from this spec.
- MCP tool support (Super has none registered today).
- Touching the agent loop, context-builder, or system-prompt assembly — only the per-tool surface and the permission / render plumbing they depend on.
- Web frontend rendering work. The `RenderSpec` is defined and emitted; consuming it in the web is its own future spec.
- iOS / E2B / coordinator-mode integrations.

---

## 2. Per-Tool Audit Rubric

For every tool in Super, we produce one row in the audit table with these columns:

| Axis | What we check | "Pass" bar |
|---|---|---|
| Name | Tool name string + any aliases | Identical to Claude's `name` and `aliases` |
| Input schema | Field names, types, required-ness, descriptions, defaults, min/max, conditional omissions (e.g. Bash's `run_in_background` gated by `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS`) | Byte-for-byte equivalent JSON Schema when both are normalized; descriptions match text |
| Output schema | Whether Claude declares an `outputSchema` and what shape | Super declares matching schema (new field on extended `Tool` trait) |
| System prompt | Long-form `prompt()` text Claude ships per tool (separate from one-line `description()`) | Identical prompt text, ported as Rust `pub const PROMPT: &str` per tool |
| Description | Short one-line capability phrase + optional `searchHint` for ToolSearch | Identical |
| Behavior flags | `isReadOnly`, `isConcurrencySafe`, `isDestructive`, `isOpenWorld`, `requiresUserInteraction`, `interruptBehavior`, `shouldDefer`, `alwaysLoad`, `maxResultSizeChars` | Same values for same inputs |
| Validation | `validateInput(input, ctx) -> ValidationResult` — model-facing pre-flight checks | Super implements; same failure messages where Claude has them |
| Permission flow | `checkPermissions(input, ctx) -> PermissionResult` returning behavior + `updatedInput` + `decisionReason` + `ruleSuggestions`; `preparePermissionMatcher(input)` for input-aware rule patterns (e.g. `Bash(git *)`) | Same decisions and same `ruleSuggestions` for same input + context |
| Activity / spinner | `getActivityDescription(input)` — present-tense verb string | Identical strings |
| Render hooks | `renderToolUseMessage`, `renderToolResultMessage`, `renderToolUseProgressMessage`, `renderToolUseRejectedMessage`, `renderToolUseErrorMessage`, `renderGroupedToolUse`, `renderToolUseTag` | Each emits a `RenderSpec` whose semantic content matches Claude's React output (header text, body blocks, kind, hints) |
| Result shape | `ToolResult` content + `mcpMeta` + `newMessages` + `contextModifier` | Same fields wired through |
| Auto-classifier input | `toAutoClassifierInput(input)` — compact security-classifier string | Same string for same input |
| Path tools | `getPath(input)` for file-path-aware tools | Same path extraction |
| Search/read collapse | `isSearchOrReadCommand(input)` for UI collapsing | Same flags |

### Severity Labels

- **P0 — protocol divergence** (model sees a different surface): missing required field, wrong schema, wrong tool name, wrong prompt text.
- **P1 — behavior divergence** (model sees same surface but tool acts differently): wrong validation, wrong permission flow, missing render hook with user-visible consequence.
- **P2 — internal divergence** (no model- or user-visible effect): missing flag wired through, missing classifier input, etc.

P0/P1 must be fixed; P2 is best-effort.

---

## 3. Extended `Tool` Trait

Mirrors every method from Claude's `Tool` interface that has a protocol or behavior consequence. Render methods are translated from `React.ReactNode` returns to `RenderSpec` returns (see §4). React-only types become Super-native types built around `serde_json::Value` payloads on the session bus.

```rust
// cli/src/tools/contract.rs (extended)

#[async_trait]
pub trait Tool: Send + Sync {
    // ── Identity ──────────────────────────────────────────────────────────
    fn name(&self) -> &str;
    fn aliases(&self) -> &[&str] { &[] }
    fn user_facing_name(&self, input: Option<&Value>) -> String { self.name().into() }
    fn user_facing_name_bg_color(&self, _input: Option<&Value>) -> Option<ColorHint> { None }

    // ── Discovery / loading ───────────────────────────────────────────────
    fn description(&self, input: Option<&Value>, ctx: &DescriptionCtx) -> String;
    fn prompt(&self, ctx: &PromptCtx) -> String;             // long-form
    fn search_hint(&self) -> Option<&str> { None }
    fn should_defer(&self) -> bool { false }
    fn always_load(&self) -> bool { false }
    fn is_enabled(&self) -> bool { true }

    // ── Schemas ───────────────────────────────────────────────────────────
    fn input_schema(&self) -> Value;
    fn output_schema(&self) -> Option<Value> { None }
    fn max_result_size_chars(&self) -> usize { 200_000 }
    fn strict(&self) -> bool { false }

    // ── Behavior flags (per-input) ────────────────────────────────────────
    fn is_concurrency_safe(&self, _input: &Value) -> bool { false }
    fn is_read_only(&self, _input: &Value) -> bool { false }
    fn is_destructive(&self, _input: &Value) -> bool { false }
    fn is_open_world(&self, _input: &Value) -> bool { false }
    fn is_mcp(&self) -> bool { false }
    fn is_lsp(&self) -> bool { false }
    fn requires_user_interaction(&self) -> bool { false }
    fn interrupt_behavior(&self) -> InterruptBehavior { InterruptBehavior::Block }
    fn inputs_equivalent(&self, a: &Value, b: &Value) -> bool { a == b }
    fn get_path(&self, _input: &Value) -> Option<PathBuf> { None }
    fn is_search_or_read_command(&self, _input: &Value) -> SearchReadKind {
        SearchReadKind::default()
    }
    fn is_transparent_wrapper(&self) -> bool { false }

    // ── Activity / spinner ────────────────────────────────────────────────
    fn get_activity_description(&self, input: &Value) -> Option<String> { None }
    fn get_tool_use_summary(&self, _input: &Value) -> Option<String> { None }

    // ── Validation + permissions ──────────────────────────────────────────
    async fn validate_input(&self, _input: &Value, _ctx: &ToolCallContext)
        -> ValidationResult { ValidationResult::Ok }
    async fn check_permissions(&self, input: &Value, ctx: &ToolCallContext)
        -> PermissionResult;
    async fn prepare_permission_matcher(&self, _input: &Value)
        -> Option<Box<dyn Fn(&str) -> bool + Send + Sync>> { None }
    fn to_auto_classifier_input(&self, _input: &Value) -> Value { Value::String(String::new()) }
    fn backfill_observable_input(&self, _input: &mut Value) {}

    // ── Execution ─────────────────────────────────────────────────────────
    async fn call(
        &self,
        input: Value,
        context: &ToolCallContext,
        on_progress: Option<ProgressSink>,
    ) -> ToolResult;

    // ── Render hooks (return RenderSpec, see §4) ──────────────────────────
    fn render_tool_use_message(&self, input: &Value, opts: &RenderOpts) -> RenderSpec;
    fn render_tool_use_tag(&self, _input: &Value) -> Option<RenderSpec> { None }
    fn render_tool_use_progress_message(
        &self, _progress: &[ProgressEvent], _opts: &RenderOpts,
    ) -> Option<RenderSpec> { None }
    fn render_tool_use_queued_message(&self) -> Option<RenderSpec> { None }
    fn render_tool_result_message(
        &self, _output: &Value, _progress: &[ProgressEvent], _opts: &RenderOpts,
    ) -> Option<RenderSpec> { None }
    fn render_tool_use_rejected_message(
        &self, _input: &Value, _opts: &RenderOpts,
    ) -> Option<RenderSpec> { None }
    fn render_tool_use_error_message(
        &self, _err: &Value, _opts: &RenderOpts,
    ) -> Option<RenderSpec> { None }
    fn render_grouped_tool_use(
        &self, _calls: &[GroupedCall], _opts: &RenderOpts,
    ) -> Option<RenderSpec> { None }
    fn is_result_truncated(&self, _output: &Value) -> bool { false }
    fn extract_search_text(&self, _output: &Value) -> Option<String> { None }

    // ── Result mapping ────────────────────────────────────────────────────
    fn map_tool_result_to_block(&self, output: &Value, tool_use_id: &str) -> ToolResultBlock;
}
```

### Supporting Types

- `PermissionResult` — defined in §5.
- `RenderSpec` — defined in §4.
- `ProgressSink` — `mpsc::Sender<ProgressEvent>` so tools can stream progress (replaces Claude's `onProgress` callback).
- `InterruptBehavior::{Cancel, Block}` — what happens when user sends a new message mid-tool.
- `SearchReadKind { is_search, is_read, is_list }` — UI collapse hints.
- `DescriptionCtx`, `PromptCtx`, `RenderOpts` — light context bags (interactive flag, perm context, tools list, theme, verbose, terminal size).

The trait references a few more types defined alongside it in `cli/src/tools/contract.rs`:

```rust
pub enum ValidationResult {
    Ok,
    Err { message: String, error_code: u32 },
}

pub enum ColorHint { Default, Permission, Subagent, Tool, Custom(String) }

pub struct GroupedCall {
    pub tool_use_id: String,
    pub input: Value,
    pub is_resolved: bool,
    pub is_error: bool,
    pub is_in_progress: bool,
    pub result: Option<ToolResultBlock>,
    pub progress: Vec<ProgressEvent>,
}

pub enum ProgressEvent {
    Started,
    Update { data: Value },         // tool-defined payload (matches Claude's ToolProgressData)
    Finished,
}

/// Maps to the Anthropic SDK ToolResultBlockParam. The `content` field is
/// either a plain string or a list of multimodal blocks (text + image).
pub struct ToolResultBlock {
    pub tool_use_id: String,
    pub content: ToolResultContent,
    pub is_error: bool,
}

pub enum ToolResultContent {
    Text(String),
    Multi(Vec<ToolResultPart>),
}

pub enum ToolResultPart {
    Text { text: String },
    Image { source: ImageSource },
}
```

### Defaults

A `ToolDefaults` blanket-impl helper (or a `build_tool!` macro) so a tool that only needs the basics doesn't have to spell out every method. Mirrors Claude's `buildTool({...})` filling in `TOOL_DEFAULTS`.

### Conditional Input Schemas

Bash's `run_in_background` omission and AskUserQuestion's `_simulatedSedEdit` omission are supported because `input_schema(&self) -> Value` is a function, not a static const. Each tool can read its config to produce the right schema.

---

## 4. `RenderSpec` Schema (the Render-Hook Bridge)

`RenderSpec` is a serializable, framework-agnostic description of what a tool wants rendered. The TUI maps it to ratatui widgets; the server forwards it over the wire; the web frontend maps it to shadcn / assistant-ui / tool-ui components. All three render the *same* spec, satisfying the CLAUDE.md remote-control-protocol constraint ("client-agnostic, no web-only assumptions").

```rust
// shared/src/render_spec.rs
//
// One Rust crate, shared between cli + server. serde-derived.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderSpec {
    /// Inline header line with a verb + target (e.g. "Reading src/foo.rs").
    Header { verb: String, target: Option<String>, tag: Option<Tag> },

    /// Multiline plain text. Used for fallback/raw output.
    Text { body: String, dim: bool },

    /// A code block with optional language; rendered monospace.
    Code { language: Option<String>, body: String, truncated: bool },

    /// A unified diff. Renderer applies +/- colors and gutter.
    Diff { file_path: String, hunks: Vec<DiffHunk> },

    /// File-path-prefixed list (Grep/Glob results).
    PathList { entries: Vec<PathEntry>, total: usize, truncated: bool },

    /// Key/value pairs (e.g. tool-use tag, web search result metadata).
    KeyValues { rows: Vec<(String, String)> },

    /// Collapsible block. Children are rendered when expanded.
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
    Status { state: StatusState, message: Option<String> },

    /// Interactive widget — tells the renderer "this needs a UI handler."
    /// The renderer (TUI / web) maps `widget` to its own input component;
    /// the agent loop suspends the turn until the user completes it.
    Interactive {
        widget: InteractiveWidget,
        /// Renderer-agnostic, serializable response shape.
        response_schema: serde_json::Value,
    },

    /// Empty — explicitly render nothing (vs. None which means "use default").
    Nothing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractiveWidget {
    /// AskUserQuestion. Renderer presents 1-4 questions, each with options
    /// (single- or multi-select), optional preview pane (right side), and
    /// a fallback "Other" text input.
    MultiQuestion { questions: Vec<Question> },

    /// ExitPlanMode. Renderer shows the plan and asks accept/reject.
    PlanApproval { plan_markdown: String },

    /// Permission prompt for a tool call. Renderer shows tool name + input
    /// summary + options [allow once / always allow this / always allow
    /// matching rule / deny].
    PermissionPrompt {
        tool_name: String,
        input_summary: String,
        rule_suggestions: Vec<RuleSuggestion>,
    },

    /// SendMessage to a peer agent / channel.
    MessageCompose { recipients: Vec<String>, draft: String },
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
    pub preview: Option<String>,   // markdown rendered in monospace box
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk { pub old_start: u32, pub new_start: u32, pub lines: Vec<DiffLine> }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiffLine { Context(String), Add(String), Remove(String) }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathEntry { pub path: PathBuf, pub line: Option<u32>, pub preview: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Tag {
    Timeout { ms: u64 },
    Model { id: String },
    Truncated,
    ResumeId(String),
    Custom(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusState { Queued, InProgress, Success, Error, Rejected }
```

### Three Rendering Rules

1. **Tools never reach into ratatui types.** They construct a `RenderSpec` and return it. The TUI module owns a single `pub fn render(spec: &RenderSpec, frame: &mut Frame, area: Rect)` that handles every variant.
2. **Interactive widgets pause the agent loop.** When a tool returns a `RenderSpec::Interactive`, the executor records the pending interaction on the session bus, waits for a `UserInteractionResponse` event whose payload conforms to `response_schema`, then resumes by passing that payload back as the tool's result (or, for permission prompts, as the resolved decision). This is how `AskUserQuestion`, `ExitPlanMode`, and the permission system all flow through a single primitive.
3. **The server forwards `RenderSpec` verbatim.** No HTML, no React. The web frontend has a `<RenderSpec spec={…} />` component that switches on `kind` and renders shadcn / assistant-ui / tool-ui pieces. iOS does the same with SwiftUI later.

### Renderer Fidelity Test

For every tool, given a representative `(input, output)` fixture, the `RenderSpec` emitted by Super must be semantically equivalent to the React node Claude renders. "Semantically equivalent" = same header verb, same body text, same diff hunks, same code-block contents, same status. Color / spacing / font are allowed to diverge.

---

## 5. Permission System Parity

Today's Super: `Decision::{Allow, Deny, Ask}`, matches on **tool name only**, no `updatedInput`, no `decisionReason`, no `ruleSuggestions`. That means `Bash(git *)`-style rules can't work, hooks can't rewrite a tool's input before it runs, and the UI can't offer "always allow this *specific* call" affordances.

We replace `Decision` with `PermissionResult` and rewrite the matcher to be input-aware.

```rust
// cli/src/tools/permission.rs (rewrite)

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum PermissionResult {
    Allow {
        /// Tool runs with this input. May differ from what the model sent
        /// (e.g. a hook normalized a path, a permission rule pinned a flag).
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
        /// What the permission prompt should show. May differ from the
        /// raw model input (e.g. expanded paths).
        #[serde(skip_serializing_if = "Option::is_none")]
        updated_input: Option<Value>,
        /// Rule suggestions the prompt offers as "always allow" affordances.
        /// E.g. for `Bash {command: "git status"}`, suggest `Bash(git *)`,
        /// `Bash(git status)`, `Bash`.
        rule_suggestions: Vec<RuleSuggestion>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecisionReason {
    /// Matched a permission rule (allow/deny/ask).
    Rule { source: RuleSource, pattern: String },
    /// A pre-tool hook returned this decision.
    Hook { hook_name: String },
    /// Permission mode shortcut (bypass / plan / acceptEdits).
    Mode { mode: PermissionMode },
    /// Auto-classifier returned this decision (auto mode).
    Classifier { confidence: f32 },
    /// Tool-defined default (e.g. read-only tools always allow).
    ToolDefault,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSuggestion {
    /// Human-readable label for the prompt UI ("Always allow `git *`").
    pub label: String,
    /// The actual rule that would be added if accepted.
    pub rule: PermissionRule,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub tool_name: String,
    /// Input matcher pattern. None = match any input for that tool.
    /// Tool-specific syntax interpreted by `prepare_permission_matcher`.
    /// Examples: "git *", "git status", "Read(/etc/*)".
    pub content: Option<String>,
    pub behavior: RuleBehavior,
    pub source: RuleSource,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleBehavior { Allow, Deny, Ask }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    Policy, Flag, Session, Local, Project, User, BuiltIn,
}
// priority(): Policy > Flag > Session > Local > Project > User > BuiltIn
// Already correct in Super; preserved.

pub enum PermissionMode { Default, AcceptEdits, Plan, BypassPermissions, Auto }
```

### `PermissionMode` migration note

Super's current `state::store::PermissionMode` enum has variants `{Default, Bypass, DontAsk, Plan, ...}`. The above enum renames these to match Claude's naming:
- `Bypass` → `BypassPermissions`
- `DontAsk` → `Auto` (semantics drift slightly — `DontAsk` short-circuits anything that would `Ask` to `Deny`; Claude's `Auto` short-circuits to the classifier's decision. Batch 1 keeps `Auto` ≡ today's `DontAsk` behavior — i.e. ask → deny — with the classifier wiring deferred to a later spec.)
- `AcceptEdits` is new (Claude has it, Super does not).

Rename is part of Batch 1. Any code that pattern-matches the old variant names is updated in the same change.

### Evaluator Changes

```rust
impl PermissionSystem {
    pub async fn evaluate(
        &self,
        tool: &dyn Tool,
        input: &Value,
        ctx: &ToolCallContext,
    ) -> PermissionResult {
        // 1. Mode shortcuts.
        match self.mode {
            PermissionMode::BypassPermissions => return PermissionResult::Allow {
                updated_input: None,
                decision_reason: Some(DecisionReason::Mode { mode: self.mode }),
            },
            PermissionMode::Plan if !tool.is_read_only(input) => {
                return PermissionResult::Deny {
                    reason: "Plan mode allows read-only tools only.".into(),
                    decision_reason: Some(DecisionReason::Mode { mode: self.mode }),
                };
            }
            _ => {}
        }

        // 2. Always-deny rules (highest priority bucket).
        // 3. Always-allow rules.
        // 4. Always-ask rules.
        // For each: use tool.prepare_permission_matcher(input) to get a
        //   pattern->bool fn, so e.g. Bash matches on `command`, not tool name.
        // ...

        // 5. Defer to tool.check_permissions(input, ctx).
        tool.check_permissions(input, ctx).await
    }
}
```

### Key Parity Points

- **`updated_input` plumbing**: the executor uses the `PermissionResult::Allow.updated_input` (or `Ask`'s, after user resolves) as the *actual* input the tool sees. Mirrors Claude's `permissions.ts` rewrite path. Required for hook-rewritten inputs and for `_simulatedSedEdit`-style fields populated after user approval.
- **`rule_suggestions`**: the `RenderSpec::Interactive::PermissionPrompt` carries these. The prompt UI offers them as "always allow" affordances. Matches Claude's `ruleSuggestions` flow.
- **Input-aware matching via `prepare_permission_matcher`**: each tool returns a closure that knows how to match its own input against a pattern. Bash matches command stems; Read/Edit/Write match path globs; AskUserQuestion ignores patterns. This is how `Bash(git *)` works at all.
- **`auto_deny_prompts`** (already on `ToolCallContext`): preserved. When set, any path that would `Ask` short-circuits to `Deny` with `reason = "async subagents cannot prompt the user"`. Mirrors Claude's `shouldAvoidPermissionPrompts`.

### Out of Scope (deliberate)

- The hook system that produces `DecisionReason::Hook`. Super doesn't have a generic hook system per CLAUDE.md. The variant exists for forward-compat with the Superpowers auto-start logic, but no current hook wires through it.
- Auto-mode classifier (`DecisionReason::Classifier`). Plumbed in the type but no Super-side implementation.

Both are still defined in the type so future work doesn't have to rev `PermissionResult`.

---

## 6. Per-Tool Divergence Summary (Audit Findings)

Severity: **P0** protocol divergence, **P1** behavior divergence, **P2** internal-only.

### Interactive Tools

**AskUserQuestion** — *catastrophic divergence*
- **P0** Input schema is `{question: string}`. Claude's is `{questions: [{question, header, multiSelect, options: [{label, description, preview?}]}], answers?, annotations?, metadata?}` with min 1 / max 4 questions and a uniqueness refinement on labels. Output schema completely missing (Claude returns `{questions, answers, annotations}`).
- **P0** No `prompt()` text. Claude ships a multi-paragraph prompt covering "Other" fallback, recommended-first ordering, multiSelect guidance, plan-mode interaction with ExitPlanMode, plus a `PREVIEW_FEATURE_PROMPT` conditional on render format.
- **P0** No `searchHint`, no `shouldDefer: true`.
- **P0** No `outputSchema`.
- **P1** No actual interactive widget. Today the tool returns a string and metadata flags — the TUI is on its own. Needs to return `RenderSpec::Interactive{MultiQuestion}` and the executor must suspend until response.
- **P1** No `isEnabled()` gating (Claude disables when running on channels).

**ExitPlanMode** — *near-stub*
- **P0** Input schema is `{}`. Claude's (V2) is `{plan: string, plan_path: string, bashPrompts?: [{tool, prompt, fallbackBehavior?}]}` with the plan content injected from disk by `normalizeToolInput`. Output includes `isAgent`, `planPath`, `agentToolAvailable`, `requestId`.
- **P0** No `prompt()` text.
- **P1** Just flips a flag in the store. No plan presentation, no user approval flow. Should emit `RenderSpec::Interactive{PlanApproval}` and only flip the mode after user accepts.

**EnterPlanMode** — *near-stub*
- **P0** Input schema is `{}`. Matches Claude shape but no `prompt()` text.
- **P1** No description of plan-mode semantics in the user-visible activity (no `getActivityDescription`).

**SendMessage** — *wrong shape*
- **P0** Schema is `{message, channel?}`. Claude's is `{to: string, summary?: string, message: string | StructuredMessage}` where `to` is a teammate name or "*" for broadcast, and `StructuredMessage` is a discriminated union of `shutdown_request | shutdown_response | plan_approval_response`. Output is one of `MessageOutput | BroadcastOutput | RequestOutput` (Super has none).
- **P0** No `prompt()` text. No request/response correlation via `request_id`.
- **P1** Super's call just stringifies and returns; no actual routing or peer addressing.

**TodoWrite** — *missing required field*
- **P0** Schema: `{todos: [{content, status}]}`. Claude's `TodoItemSchema` requires `{content, status, activeForm}` (activeForm = present-continuous spinner text). Super omits `activeForm` entirely.
- **P0** No output schema. Claude returns `{oldTodos, newTodos, verificationNudgeNeeded?}`.
- **P0** No `prompt()` text.
- **P1** Returns a one-liner string; doesn't emit a render that updates the todo panel. Renders nothing in the TUI today.

### File Tools

**Read**
- **P0** Missing `pages` field for PDF page range (`"1-5"`, `"3"`, etc.). Claude has it.
- **P1** Code comment says offset is "0-based for internal processing, used as 1-based in output" — Claude's `offset` is consistently 1-based-line semantics. Verify and align.
- **P0** No `prompt()` text. No `outputSchema` (Claude returns a discriminated union: text file, image, PDF, notebook).
- **P2** No `searchHint`, no `maxResultSizeChars`, no `extract_search_text`.

**Edit**
- **P0** Input schema fields match. No `prompt()` text (Claude's `getEditToolDescription()` is long, covers strict-string matching rules).
- **P0** No `outputSchema` (Claude returns FileEditOutput with diff patch).
- **P1** No `getPath(input)` for path-aware permission matching. No `getActivityDescription("Editing <file>")`. No `toAutoClassifierInput` returning `"<path>: <new_string>"` (Claude uses this for the security classifier).
- **P1** No `backfill_observable_input` for `~`/relative-path expansion before hooks see the input.
- **P1** No render of a diff after success. Need `RenderSpec::Diff`.

**Write**
- **P0** Schema matches (`file_path`, `content`). No `prompt()` text.
- **P0** No `outputSchema`.
- **P1** Same `getPath`, `getActivityDescription`, `toAutoClassifierInput` gaps as Edit.

**Glob**
- **P0** Schema has `pattern, path?`. Claude's matches in field names. No `prompt()` text.
- **P0** No `outputSchema` (Claude returns `{durationMs, numFiles, filenames, truncated}`).
- **P1** No truncation indicator on result.

**Grep**
- **P0** Super has `pattern, path?, glob?, head_limit?`. Claude has those plus `output_mode ('content'|'files_with_matches'|'count')`, `-B`/`-A`/`-C`/`context`, `-n`, `-i`, `type`, `offset`, `multiline`. **Massive missing surface area.**
- **P0** No `outputSchema` (Claude returns `{mode, numFiles, filenames, content?, numLines?, numMatches?, appliedLimit?, appliedOffset?}`).
- **P0** No `prompt()` text.

**NotebookEdit**
- **P0** Schema mostly matches. Required is `[notebook_path, new_source]` on both sides.
- **P0** No `outputSchema` (Claude returns `{newSource, cellId, cellType, language, editMode, error?, notebookPath, originalNotebook, updatedNotebook}`).
- **P0** No `prompt()` text.

### Shell

**Bash**
- **P0** Missing `dangerouslyDisableSandbox: boolean` (escapes sandbox). Missing the model-facing multi-paragraph `description` content (rules about preferring dedicated tools, git commit protocol, PR creation protocol, etc. — currently a short two-line string).
- **P0** No `prompt()` text. Claude's is huge.
- **P0** No `outputSchema`.
- **P0** Conditional schema omission gated by `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` is missing. Super always advertises `run_in_background`.
- **P1** No sandbox enforcement (`shouldUseSandbox` decision tree). No `bashSecurity` / `bashPermissions` / `commandSemantics` modules. No sed-edit auto-detection (`_simulatedSedEdit` flow). No destructive-command warning.
- **P1** No `prepare_permission_matcher` for `Bash(git *)`-style rules.
- **P1** No `getActivityDescription` returning the `description` field.

### Web

**WebFetch**
- **P0** Schema matches (`url, prompt`). Claude validates `url` as URL type; Super does string + length check. Behavior diverges (Super hard-fails on creds in URL; Claude doesn't).
- **P0** No `prompt()` text. No `outputSchema` (Claude returns `{bytes, code, codeText, result, durationMs, url}`).
- **P1** No `preapproved` URL list (Claude has one for trusted hosts). No model-side processing of the fetched content with the `prompt` field — Super ignores `prompt` entirely.

**WebSearch**
- **P0** Schema matches. No `prompt()` text.
- **P0** No `outputSchema` (Claude returns `{query, results, durationMs}` with discriminated hit shapes).
- **P1** Uses DuckDuckGo Instant Answer API as a stand-in. The *output shape* must match Claude's even if the provider differs.

### Tasks / Todos

**TaskCreate** — *missing fields*
- **P0** Schema has `subject, description, activeForm?`. Claude has `subject, description, activeForm?, metadata?`. Missing `metadata`.
- **P0** No `outputSchema` (Claude returns `{task: {id, subject, ...}}`).
- **P0** No `prompt()` text.

**TaskGet / TaskList / TaskOutput / TaskStop / TaskUpdate**
- Per-tool audit pending. Likely missing `metadata`, `owner`, `addBlockedBy`/`addBlocks`, status set including `deleted`. Captured as a single batch item: "audit each against Claude's `tools/TaskCreateTool/`, `TaskGetTool/`, etc."

### Agent

**AgentTool** (named `Task`)
- **P0** Schema has `description, prompt, subagent_type, model, run_in_background`. Claude's V1 also has those; V2 (multi-agent / coordinator) adds `name`, `team_name`, `mode`, `isolation`, `cwd`. V2 is out of scope per non-goals. V1 surface is close to parity.
- **P0** No `prompt()` text. Claude ships a detailed agent prompt covering when to use, parallel execution rules, isolation modes.
- **P0** No `outputSchema` (Claude has `{status: 'completed', prompt}` and `{status: 'async_launched', agentId, ...}`).
- **P1** No `searchHint`, no `userFacingName` override.

### Skills

**Skill**
- **P0** Schema matches (`skill, args?`). No `prompt()` text. No `outputSchema` (Claude has inline + forked output shapes).

### Scheduling

**CronCreate / CronDelete / CronList**
- **P0** CronCreate schema: Super has `cron, prompt, recurring?, durable?` — matches Claude. Missing `outputSchema` `{id, humanSchedule, recurring, durable?}`. Missing `prompt()` text.
- CronDelete / CronList per-tool audit pending.

### Stubs / Placeholders

**Sleep, Monitor, ToolSearch, StructuredOutput, LSP, Config, EnterWorktree, ExitWorktree**
- All stub-ish or short. Each needs schema + prompt parity check. Most are P0 on `prompt()` text and `outputSchema`, P1 on behavior (Monitor / ToolSearch / LSP explicitly return "not implemented" strings).
- **ToolSearch is special**: it's the *deferred-tool loader*. The model uses it to fetch schemas for `shouldDefer: true` tools. If Super doesn't actually implement deferred-tool loading, no tool can be deferred — which means `AskUserQuestion`, MCP tools, etc. can't be optimized. **P0 in practice once `shouldDefer` is wired up.**

### Cross-Cutting Findings

- **No `outputSchema` anywhere in Super.** Every tool that returns structured data needs one.
- **No `prompt()` method on any tool.** Every Claude tool has long-form per-tool prompt text. CLAUDE.md mandates "exact same system prompts" — these per-tool prompts are part of the system surface (assembled into the tools section of the API call) and must be ported.
- **No `searchHint`, `shouldDefer`, `alwaysLoad`, `maxResultSizeChars`** on any tool — needed once ToolSearch is real and once tool-result-budget enforcement is added.
- **No `requires_user_interaction()`** anywhere — needed for channel relay / non-interactive session detection.
- **No `interrupt_behavior()`** — needed so a long Bash can cancel on user interrupt while AskUserQuestion blocks.
- **No `is_open_world()`** — needed for caching decisions.
- **No `inputs_equivalent()`** — needed for de-dup of parallel identical calls.

---

## 7. Batched Fix Specs (Sequencing & Contents)

Five batches. Each becomes its own implementation plan downstream. Batches 1 and 2 are foundational and gate everything that follows; 3–5 can be parallelized after them.

### Batch 1 — Trait + Permission + RenderSpec Foundations (gates everything)

**Scope:**
- Extend `cli/src/tools/contract.rs` to the full `Tool` trait from §3.
- Add `cli/src/tools/defaults.rs` with a `buildTool!`-style macro (or `TOOL_DEFAULTS` const-fn helpers) so per-tool impls stay short.
- Add `shared/src/render_spec.rs` (§4). Wire its serde types through the session bus, server forwarding, and TUI's tool-render dispatcher.
- Replace `permission::Decision` with `PermissionResult` (§5). Update `evaluate()` to take `&dyn Tool, &Value` and call `prepare_permission_matcher`. Wire `updated_input` through the tool executor so the tool sees the rewritten input.
- Add `ProgressSink` (mpsc channel) plumbing through `ToolCallContext`.
- Update every existing tool's `impl Tool` block to the new trait. *Behavior-preserving rewrite only* — fill in `prompt()` with a placeholder pointing at the per-tool ported prompt (TODO comment with the Claude file path), wire `is_read_only(input)` etc., return `RenderSpec::Nothing` from render hooks. No new logic; just the contract migration.
- Update `ToolResult` to carry `mcp_meta` and `new_messages` fields so future MCP / multi-turn flows have somewhere to land.
- Update `permission.rs` tests; add a smoke test that `Bash(git *)` matches against `{command: "git status"}`.

**Acceptance:** code compiles, all existing tool unit tests pass, no observable behavior change to any tool.

### Batch 2 — Interactive Tools Parity (the user-facing class)

**Scope (depends on Batch 1):**
- `AskUserQuestion`: full schema rewrite (`questions[]`, options, preview, multiSelect, annotations, metadata); port `ASK_USER_QUESTION_TOOL_PROMPT` and the `PREVIEW_FEATURE_PROMPT` markdown/html variants; add `outputSchema`; return `RenderSpec::Interactive{MultiQuestion}`; set `shouldDefer = true`; gate `isEnabled` (channels future-compat); wire executor to suspend on `Interactive` results and await a `UserInteractionResponse`.
- `ExitPlanMode`: full schema rewrite (V2 — `plan`, `plan_path`, `bashPrompts?`); port `EXIT_PLAN_MODE_TOOL_PROMPT`; return `RenderSpec::Interactive{PlanApproval}`; only flip the store flag after user accepts; emit `isAgent`/`planPath`/`requestId` in output.
- `EnterPlanMode`: port `ENTER_PLAN_MODE_TOOL_PROMPT`; add `getActivityDescription`.
- `SendMessage`: rewrite schema to `{to, summary?, message: string | StructuredMessage}` with discriminated message union; port prompt; define output union (`MessageOutput`/`BroadcastOutput`/`RequestOutput`). Out of scope: actual peer routing (UDS_INBOX is feature-flagged; teammate addressing requires multi-agent runtime). Schema parity now; routing impl tracked as separate spec.
- `TodoWrite`: add required `activeForm` field; add `outputSchema {oldTodos, newTodos, verificationNudgeNeeded?}`; port `TODO_WRITE_TOOL_PROMPT`; return `RenderSpec::Group{...}` updating the todo panel widget.
- **Permission-prompt UI**: when `PermissionResult::Ask` reaches the executor, the executor emits `RenderSpec::Interactive{PermissionPrompt}` with `rule_suggestions` derived from the tool's `prepare_permission_matcher`. The TUI shows the prompt; user selects an option; the result resolves the `Ask`.

**Acceptance:** running the parity harness (§8) against these five tools shows matching schemas, matching prompts, and behaviorally-equivalent interactive flows in the TUI. Permission prompts offer the same `rule_suggestions` as Claude for the same input.

### Batch 3 — File & Shell Tools Parity

**Scope (depends on Batch 1, parallelizable with Batch 2):**
- Read: add `pages`; verify offset semantics; port prompt; add `outputSchema` discriminated union (text/image/pdf/notebook); add `extract_search_text`; set `maxResultSizeChars = Infinity` (per Claude comment about Read→file→Read loop).
- Edit: port prompt (long); add `outputSchema` (FileEditOutput with diff patch); implement `getPath`, `getActivityDescription`, `toAutoClassifierInput`, `backfillObservableInput` (path expansion); emit `RenderSpec::Diff` on success.
- Write: port prompt; add `outputSchema`; same `getPath`/`getActivityDescription`/`toAutoClassifierInput` set as Edit.
- Glob: port prompt; add `outputSchema` `{durationMs, numFiles, filenames, truncated}`; emit `RenderSpec::PathList`.
- Grep: **major** schema expansion — add `output_mode`, `-B`/`-A`/`-C`/`context`, `-n`, `-i`, `type`, `offset`, `multiline`; port prompt; add `outputSchema` discriminated by `mode`; emit `RenderSpec::PathList` or `RenderSpec::Code` depending on mode.
- NotebookEdit: port prompt; add `outputSchema`.
- Bash: add `dangerouslyDisableSandbox`; port long prompt; add conditional schema omission for `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS`; add `outputSchema`; implement `prepare_permission_matcher` matching command stems (so `Bash(git *)` works); add `getActivityDescription` using `description` field; **defer** sandbox enforcement, sed-edit auto-detect, destructive-command warning to a follow-up sub-batch (Bash is large; ship schema / prompt / perms-matcher first).

**Acceptance:** parity harness passes for all seven tools. Bash's `prepare_permission_matcher` correctly matches `Bash(git *)` against `{command: "git status"}` and rejects against `{command: "rm -rf /"}`.

### Batch 4 — Tasks, Agent, Skill, Web Parity

**Scope (depends on Batch 1):**
- TaskCreate / TaskGet / TaskList / TaskOutput / TaskStop / TaskUpdate — per-tool schema audit against Claude's counterparts; add missing fields (`metadata`, `owner`, `addBlockedBy`/`addBlocks`, `deleted` status); add `outputSchema` per tool; port each prompt.
- AgentTool (Task) — port prompt; add `outputSchema` (completed | async_launched discriminated union); hold V2 fields (`name`, `team_name`, `mode`, `isolation`, `cwd`) for a coordinator-mode spec (non-goals exclude coordinator/swarm).
- Skill — port `SKILL_TOOL_PROMPT`; add `outputSchema` (inline / forked union).
- WebFetch — port prompt; add `outputSchema`; add `preapproved` URL list module; implement actual prompt-application to fetched content (currently ignored).
- WebSearch — port prompt; add `outputSchema` matching Claude's `{query, results[discriminated], durationMs}`.

**Acceptance:** parity harness passes for all of these.

### Batch 5 — Long Tail (stubs, ConfigTool, LSP, Cron, Sleep, Monitor, ToolSearch, StructuredOutput, Worktree)

**Scope (depends on Batch 1):**
- Stubs (`Monitor`, `EnterWorktree`, `ExitWorktree`, `LSP`, `ToolSearch`) — schema + prompt parity. **ToolSearch needs real implementation** to make `shouldDefer = true` work on AskUserQuestion and future deferred MCP tools; otherwise the model can never call deferred tools. Implement actual deferred-tool registry + `select:` + keyword search.
- ConfigTool — port prompt; add `outputSchema`; verify the ant-only gating (Claude only ships this for `USER_TYPE === 'ant'`) — for Super, ship unconditionally (Super has no ant gate).
- Sleep — port prompt; verify max duration semantics match.
- CronCreate/Delete/List — port prompts; add output schemas; verify cron-expression parsing matches Claude's.
- StructuredOutput — verify Claude has an analog (`SyntheticOutputTool` exists; check shape).

**Acceptance:** parity harness passes for everything in this batch.

### Sequencing

```
Batch 1 (foundations)
   ├── Batch 2 (interactive)   ─┐
   ├── Batch 3 (files + shell) ─┼── parity harness green across all tools
   ├── Batch 4 (agent/task/skill/web) ─┤
   └── Batch 5 (long tail)     ─┘
```

Batches 2–5 are independent of each other once Batch 1 lands. Recommended serial order: 1 → 2 → 3 → 4 → 5 (interactive tools first because they're the most user-painful gap today).

---

## 8. Testing Strategy & Deliverables

### Testing Strategy

Three layers, all under `cli/tests/parity/`. The point is that "did we drift?" is answered by `cargo test`, not by humans reading source.

**Layer 1 — Schema parity tests (the lock).** For every tool in Super, a test that:
1. Reads the corresponding Claude tool file from `../claude-code-src/tools/<X>Tool/<X>Tool.{ts,tsx}`.
2. Extracts the zod input schema and output schema via a small build-time script that runs `bun` against a Claude-side dumper, producing `claude-code-src/.cache/tool-schemas.json` (committed). The dumper is a ~30-line `bun` script that imports each tool and emits `{name, inputSchema: zodToJsonSchema(t.inputSchema), outputSchema, prompt, searchHint, shouldDefer, alwaysLoad, maxResultSizeChars}` per tool.
3. Compares the Super tool's `input_schema()`, `output_schema()`, `prompt()`, `search_hint()`, `should_defer()`, `always_load()`, `max_result_size_chars()` against the JSON dump after light normalization (sort object keys, normalize JSON Schema dialect quirks, strip Claude-side `$schema` injection).
4. Fails with a structured diff showing the divergence.

This is the canonical regression gate. Drift in Claude is detected at the next cache regen; drift in Super is detected immediately.

**Layer 2 — Behavior parity tests (golden fixtures).** Per-tool fixtures under `cli/tests/parity/fixtures/<tool>/<case-name>/{input.json, expected_output.json, expected_render.json}`. The test:
1. Calls `tool.call(input, ctx)` with a synthetic `ToolCallContext`.
2. Asserts the output matches `expected_output.json` (modulo timestamps, durations, paths normalized).
3. Asserts `tool.render_tool_result_message(output, &[], &opts)` equals `expected_render.json` (RenderSpec is serde, so this is a JSON diff).

Fixtures generated once from running the same input through the tool in Claude Code and capturing its rendered text via the transcript test infra (`test/utils/transcriptSearch.renderFidelity.test.tsx` pattern). Per tool: at least one happy-path fixture, one error fixture, one rejected fixture for tools with rejection rendering.

**Layer 3 — Interactive flow tests (executor-level).** For the 5 interactive tools (AskUserQuestion, ExitPlanMode, EnterPlanMode, SendMessage, TodoWrite) plus the permission-prompt flow:
1. Spin up the tool executor with a mock `UserInteractionResponse` provider.
2. Call the tool; assert it emits the expected `RenderSpec::Interactive` on the session bus.
3. Feed the mock response; assert the tool's final `ToolResult` matches.
4. Per-permission-flow test: invoke a tool with input that should trigger `Ask`; assert `RenderSpec::Interactive{PermissionPrompt}` carries the right `rule_suggestions`; respond with each option (allow once / always allow rule / deny) and assert the rule store mutation matches Claude's.

**Pre-commit hook (out of scope to wire, in scope to specify):** a `parity-check` target runs Layer 1 against a fresh schema dump if `claude-code-src/` changed. Out of scope to *implement* in this spec — but the test layout supports it.

**What we don't test:**
- Pixel-level TUI rendering (ratatui pixel diffs are flaky and don't add value over RenderSpec diffs).
- Network/web responses (WebFetch/WebSearch use deterministic fakes).
- Cron timing (fake clock).

### Deliverables

**This brainstorm produces one design doc:**
- `docs/superpowers/specs/2026-05-16-tool-parity-claude-code-design.md` — this document.

**Five follow-up implementation plans (written by `writing-plans` skill, one per batch):**
- `docs/superpowers/plans/2026-05-16-batch-1-trait-permission-renderspec.md`
- `docs/superpowers/plans/2026-05-16-batch-2-interactive-tools.md`
- `docs/superpowers/plans/2026-05-16-batch-3-file-and-shell-tools.md`
- `docs/superpowers/plans/2026-05-16-batch-4-task-agent-skill-web.md`
- `docs/superpowers/plans/2026-05-16-batch-5-long-tail.md`

**Two artifacts produced inside Batch 1's work (referenced by all later batches):**
- `claude-code-src/.cache/tool-schemas.json` — the schema dump for Layer 1 tests.
- `cli/tests/parity/fixtures/<tool>/<case>/...` — fixture skeletons (filled in per batch).

**One artifact NOT produced in this spec but committed for future work:**
- `docs/superpowers/specs/2026-05-16-tool-parity-missing-tools-followup.md` — a one-page list of tools Claude ships that Super lacks (BriefTool, ListMcpResources, ReadMcpResource, RemoteTrigger, SendUserFile, PushNotification, Workflow, PowerShell, REPL, Tungsten, ExitPlanModeV2 if we held V1, TeamCreate/Delete, Subscribe, SuggestBackgroundPR, VerifyPlanExecution, ListPeers, MCPTool, McpAuthTool, CtxInspect, OverflowTest, TerminalCapture, WebBrowser, Snip, plus their feature-flag dependencies). Per non-goals, no design here — just the list, so it's not lost.
