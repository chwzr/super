# Subagent execution — design

**Status:** approved (pending user sign-off on this file)
**Issue:** #7 — *subagents: implement AgentTool::call against the existing protocol*
**Depends on:** `2026-05-15-scroll-area-and-tool-execution-parity-design.md` (protocol & TUI carry the hierarchy fields)

---

## Goal

Replace the `AgentTool::call` stub with a working subagent execution path that matches Claude Code's `AgentTool` 1:1 in architecture and behaviour. Per CLAUDE.md, "Subagents must match Claude Code's subagent surface 1:1 — same invocation semantics, configuration shape, and lifecycle."

Scope: full parity in one shot — sync + async, built-in + `.claude/agents/*.md` loader, sidechain JSONL persistence, permission overlays, abort isolation.

---

## Background

The protocol scaffolding for subagents already landed in #5:

- Every `BusMessage` variant carries `parent_tool_use_id: Option<String>` and `session_id: String`.
- `cli/src/tui/transcript.rs::fold(events, filter: Option<&str>)` filters by `parent_tool_use_id` — `None` returns the root view; `Some(tool_use_id)` returns a subagent's transcript.
- `ToolCallContext` already has fields `bus: Option<Arc<SessionBus>>` and `parent_tool_use_id: Option<String>` (today always `None`).

Authoritative reference: `claude-code-src/tools/AgentTool/`. Key files:
- `AgentTool.tsx` — tool schema, permissions, entrypoint.
- `runAgent.ts` — spawn logic, context cloning, cleanup. Critically: `runAgent` is a generator that re-invokes the top-level `query()` function with a child `toolUseContext`. **Same loop function, child context.**
- `loadAgentsDir.ts` — `.claude/agents/*.md` parser.
- `built-in/*.ts` — built-in agent definitions (system prompts).
- `agentToolUtils.ts` — `resolveAgentTools` (whitelist/blacklist intersection).

The architectural translation to Rust: **reuse `ConversationEngine` as the loop function**, construct a child engine with a child config, and stamp `parent_tool_use_id` on every emitted event.

---

## Architecture

```
AgentTool::call (invoked from parent's run_tool_uses with tool_use_id = TU)
   │
   ├── 1. Validate input (description, prompt, subagent_type, model?, run_in_background?)
   │
   ├── 2. Resolve agent definition via AgentRegistry::resolve(subagent_type)
   │        Precedence: project (.claude/agents/) > user (~/.claude/agents/) > built-in
   │
   ├── 3. Build child execution config:
   │        • agent_id = Uuid::new_v4()   (becomes child session_id)
   │        • child_bus = parent ctx.bus.clone()       (SAME SessionBus, different session_id stamped on emits)
   │        • child_registry = filter parent registry by agent.tools / agent.disallowed_tools
   │        • child_system_prompt = agent.system_prompt (with env enhancement)
   │        • child_permission_mode = resolve_permission_mode(parent_mode, agent.permission_mode, is_async)
   │        • child_abort: sync → ctx.abort_signal.clone(); async → new watch::channel(false)
   │
   ├── 4. Construct ConversationEngine via new constructor `new_child`
   │
   ├── 5a. Sync path:
   │        let final_text = child.process_prompt(prompt, &sys, parent_tool_use_id=Some(TU)).await?;
   │        return ToolResult { content: final_text, is_error: false, metadata: { agent_id } }
   │
   └── 5b. Async path (run_in_background = true):
            register handle in Store.async_agents[agent_id];
            tokio::spawn(async move {
                let result = child.process_prompt(...).await;
                // wrap: emit AsyncAgentDone with the result text or error
                bus_clone.emit(BusMessage::SystemEvent {
                    subtype: SystemSubtype::AsyncAgentDone,
                    message: format!("<agent_id> finished: {}", result_text),
                    parent_tool_use_id: Some(TU),
                    session_id: agent_id,
                    uuid: Uuid::new_v4(),
                });
                store.complete_async_agent(&agent_id);
            });
            return ToolResult { content: "Agent <agent_id> running in background.", is_error: false }
```

The child engine **shares the parent's `Arc<SessionBus>`**. Every event it emits carries:
- `session_id = <agent_id>` (the child's, distinct from parent's root session id)
- `parent_tool_use_id = Some(TU)` (the parent's invoking tool_use_id)

The TUI's existing fold demuxes on these fields without modification.

---

## Module layout

### New: `cli/src/agents/`

```
cli/src/agents/
├── mod.rs              -- public re-exports
├── definition.rs       -- AgentDefinition, AgentSource enums + structs
├── built_in.rs         -- hardcoded built-ins (system prompts ported from claude-code-src)
├── loader.rs           -- frontmatter parser, scans .claude/agents/*.md
├── registry.rs         -- AgentRegistry: resolve by subagent_type, precedence
└── permission.rs       -- resolve_permission_mode(parent, agent, is_async) -> PermissionMode
```

### Modified

- `cli/src/tools/agent.rs` — replace stub. Renamed tool: `"Agent"` → `"Task"` (Claude Code parity).
- `cli/src/tools/contract.rs` — no shape changes; documentation updates only.
- `cli/src/tools/mod.rs` — pass `Arc<AgentRegistry>` into AgentTool at registry construction time.
- `cli/src/conversation/engine.rs` — add `new_child` constructor; `process_prompt` gains `parent_tool_use_id: Option<String>` parameter; every internal emit stamps it.
- `cli/src/conversation/tool_loop.rs` — populate `ctx.bus` and `ctx.parent_tool_use_id` (today both hardcoded `None`); add the parent_tool_use_id to ToolProgress emits.
- `cli/src/conversation/mod.rs` — `pub mod sidechain;`
- `cli/src/conversation/sidechain.rs` — new file. Spawned task subscribes to bus; writes per-agent JSONL.
- `cli/src/state/store.rs` — add `async_agents: HashMap<AgentId, AsyncAgentHandle>` to `AppState`.
- `cli/src/tui/scroll_area.rs` — Task-specific summary line in `summarize_tool_call` ("Task <description>"). No new widget.

---

## Component details

### `AgentDefinition` (cli/src/agents/definition.rs)

```rust
#[derive(Debug, Clone)]
pub struct AgentDefinition {
    pub agent_type: String,                  // "general-purpose", "Explore", etc.
    pub description: String,                 // surfaced in Task tool's input schema docs
    pub system_prompt: String,               // body of agent .md, or hardcoded for built-ins
    pub tools: Option<Vec<String>>,          // None == inherit all; ["*"] == all
    pub disallowed_tools: Vec<String>,       // subtracted from tools
    pub model: Option<String>,               // model override (alias or full id)
    pub permission_mode: Option<PermissionMode>,
    pub max_turns: Option<u32>,
    pub source: AgentSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSource { BuiltIn, User, Project }
```

Out of scope for v1 (fields ignored in `.md` even if present, with a warn log):
`mcpServers`, `hooks`, `skills`, `initialPrompt`, `memory`, `background`, `isolation`, `criticalSystemReminder_EXPERIMENTAL`.

### Built-in agents (cli/src/agents/built_in.rs)

Port byte-for-byte system prompts from:
- `claude-code-src/tools/AgentTool/built-in/generalPurposeAgent.ts` → `general-purpose`
- `claude-code-src/tools/AgentTool/built-in/exploreAgent.ts` → `Explore` (read-only; disallowed: Edit, Write, NotebookEdit, ExitPlanMode, Task)
- `claude-code-src/tools/AgentTool/built-in/planAgent.ts` → `Plan` (read-only; same disallowed set as Explore)
- `claude-code-src/tools/AgentTool/built-in/statuslineSetup.ts` → `statusline-setup` (limited tools: Read, Edit)
- `claude-code-src/tools/AgentTool/built-in/claudeCodeGuideAgent.ts` → `claude-code-guide`

The exact disallowed_tools list per agent is copied from the TS source. Each built-in is a `const FOO_AGENT: AgentDefinition` returned by a `builtin_agents()` function — mirrors `getBuiltInAgents()`.

Skipped: `VERIFICATION_AGENT` (gated behind a GrowthBook flag in Claude Code; not shipping).

### Loader (cli/src/agents/loader.rs)

For each path in `[./.claude/agents/, ~/.claude/agents/]`:
1. Glob `*.md`.
2. Parse with `serde_yaml` (already a transitive dep) for `---`-fenced frontmatter, then markdown body as `system_prompt`.
3. Required frontmatter: `description` (non-empty), `prompt` body (markdown body, non-empty).
4. Optional: `tools` (array of strings), `disallowedTools` (array), `model` (string), `permissionMode` (one of `default|plan|acceptEdits|bypassPermissions`), `maxTurns` (positive int).
5. `agent_type` derived from filename stem (`general-purpose.md` → `general-purpose`).
6. Unknown keys: log debug, ignore.
7. Parse failures: log warn with file path + reason, skip the file (don't fail loader).

Returns `Vec<AgentDefinition>` with `source` set appropriately.

### Registry (cli/src/agents/registry.rs)

```rust
pub struct AgentRegistry {
    by_type: HashMap<String, AgentDefinition>,
}

impl AgentRegistry {
    pub fn load() -> Self { /* built-ins, then user, then project — later writes win */ }
    pub fn resolve(&self, subagent_type: &str) -> Option<&AgentDefinition>;
    pub fn list(&self) -> Vec<&AgentDefinition>;
}
```

Loaded once at startup in `main.rs`, wrapped in `Arc`, passed into `ToolRegistry::new` so `AgentTool` can hold a reference.

### Permission overlay (cli/src/agents/permission.rs)

```rust
pub fn resolve_permission_mode(
    parent: &PermissionMode,
    agent: Option<&PermissionMode>,
    is_async: bool,
) -> PermissionMode { ... }
```

Rules (from `runAgent.ts:415-451`):
1. If `parent == BypassPermissions || parent == AcceptEdits`, parent wins. Return parent.
2. Otherwise if agent specifies a mode, return agent's.
3. Otherwise return parent's.
4. Async note: `is_async = true` sets a flag that tools should auto-deny anything requiring user prompts. (Implementation: a new field on `ToolCallContext`, `auto_deny_prompts: bool`. Tools that show prompts check this and return `is_error: true, content: "Permission denied (async agent cannot prompt)"`.)

### AgentTool (cli/src/tools/agent.rs)

```rust
pub struct AgentTool {
    pub registry: Arc<AgentRegistry>,
    pub store: Arc<Store>,
    pub config: shared::CliConfig,
    pub tool_registry: Arc<ToolRegistry>,   // so we can build a filtered view
}

impl Tool for AgentTool {
    fn name(&self) -> &str { "Task" }
    fn description(&self) -> &str { /* "Launches a sub-agent. ..." — matches Claude's wording */ }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "description": { "type": "string", "description": "A short (3-5 word) description of the task" },
                "prompt":      { "type": "string", "description": "The task for the agent to perform" },
                "subagent_type": { "type": "string", "description": "The type of specialized agent to use" },
                "model":       { "type": "string", "enum": ["sonnet", "opus", "haiku"] },
                "run_in_background": { "type": "boolean" }
            },
            "required": ["description", "prompt", "subagent_type"]
        })
    }

    async fn call(&self, input: Value, ctx: &ToolCallContext) -> ToolResult {
        // 1. validate input
        // 2. resolve agent
        // 3. build child engine
        // 4. dispatch sync or async
    }
}
```

The tool description string and the `subagent_type` enum advertised in the tool schema are generated dynamically from `registry.list()` so user-defined agents appear automatically — matches Claude Code's behaviour where the tool's input enum is the union of built-in + loaded agents.

### Child engine construction

`AgentTool::call` builds the child like so:

```rust
let agent_id = Uuid::new_v4().to_string();
let child_bus = ctx.bus.clone().expect("Task tool requires bus in context");
// Note: child_bus is the parent's SessionBus arc. session_id stamped per emit
// is the agent's, NOT bus.session_id(). We pass agent_id explicitly into the
// child engine and have it stamp that on every emit instead of reading the
// bus's session_id field.
let child_registry = self.tool_registry.filter_for_agent(&agent_def);
let child_perm = resolve_permission_mode(&ctx.permission_mode, agent_def.permission_mode.as_ref(), is_async);
let child_abort = if is_async {
    let (tx, rx) = watch::channel(false);
    self.store.register_async_agent(agent_id.clone(), tx);
    Some(rx)
} else {
    ctx.abort_signal.clone()
};

let child = ConversationEngine::new_child(
    self.store.clone(),
    self.config_with_model_override(&agent_def, model_override),
    Arc::new(child_registry),
    child_bus,
    agent_id.clone(),
    child_perm,
    child_abort,
);
```

### Engine changes (cli/src/conversation/engine.rs)

The bus today reads `self.bus.session_id()` to stamp emits. That's the parent's id and is hardcoded into the bus. We need to decouple "the bus we emit on" from "the session_id we stamp."

Two options:
- **A. Add a `session_id_override: Option<String>` on the engine.** Simpler. When set, every emit uses the override instead of `bus.session_id()`. Default `None` for root.
- **B. Change `SessionBus` so session_id is per-emit, not per-bus.** Cleaner but ripples into existing callers.

**Chosen: A.** Smaller blast radius; the bus continues to be a shared transport.

Implementation note: `process_prompt` today reads `self.bus.session_id()` once near the top and clones into a local `session_id` used throughout the function. With the override:

```rust
let session_id = self.session_id_override
    .clone()
    .unwrap_or_else(|| self.bus.session_id().to_string());
```

Every existing emit site that uses `session_id` is correct without further change.

New constructor:

```rust
pub fn new_child(
    store: Arc<Store>,
    config: CliConfig,
    registry: Arc<ToolRegistry>,
    bus: Arc<SessionBus>,                  // shared with parent
    agent_id: String,                       // becomes session_id on emits
    permission_mode: PermissionMode,
    abort: Option<watch::Receiver<bool>>,
) -> Self
```

`process_prompt` signature change:

```rust
pub async fn process_prompt(
    &self,
    user_input: String,
    system_prompt: &SystemPrompt,
    parent_tool_use_id: Option<String>,     // NEW
) -> Result<String, String>
```

All `session_id.clone()` inside this method use the engine's `session_id_override` if set, else `bus.session_id()`. All `parent_tool_use_id: None` literals become `parent_tool_use_id.clone()`.

The child engine uses `self.config.model` for the request — and `AgentTool` clones `self.config` into the child with `model` overridden if the user passed `model` in tool input OR if `agent_def.model` is set.

### Tool loop changes (cli/src/conversation/tool_loop.rs)

Today's `ToolCallContext` construction in `run_tool_uses`:

```rust
let ctx = ToolCallContext {
    cwd: cwd.clone(),
    permission_mode: permission_mode.clone(),
    abort_signal: abort_signal.clone(),
    parent_tool_use_id: None,    // ← hardcoded
    bus: None,                   // ← hardcoded
};
```

`run_tool_uses` needs to accept a `parent_tool_use_id: Option<String>` (the current `process_prompt`'s value) and a `bus: Arc<SessionBus>` (already present), and populate `ctx.parent_tool_use_id = parent_tool_use_id.clone()` and `ctx.bus = Some(bus.clone())`.

The 1Hz ToolProgress ticker also needs to stamp the parent's `parent_tool_use_id`:

```rust
bus_for_tick.emit(BusMessage::ToolProgress {
    tool_use_id: id_for_tick.clone(),
    tool_name: name_for_tick.clone(),
    elapsed_seconds: start.elapsed().as_secs_f32(),
    parent_tool_use_id: parent_for_tick.clone(),   // NEW: parent's parent_tool_use_id, not the current tool's id
    uuid: uuid::Uuid::new_v4(),
    session_id: bus_for_tick.session_id().to_string(),
});
```

**Critical:** `parent_tool_use_id` carried into the ticker is the engine's *parent_tool_use_id*, not the running tool's `tool_use_id`. The hierarchy is "events emitted by the child engine carry the parent's invoking tool_use_id." A subagent's internal tool call still carries `parent_tool_use_id = TU` (the original Task tool_use_id), not its own immediate tool_use_id. Nesting works because subagents-of-subagents will set their own parent_tool_use_id when they invoke their nested AgentTool::call.

### Tool registry filtering (cli/src/tools/mod.rs)

```rust
impl ToolRegistry {
    pub fn filter_for_agent(&self, agent: &AgentDefinition) -> ToolRegistry {
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
}
```

### Sidechain persistence (cli/src/conversation/sidechain.rs)

```rust
pub fn spawn_sidechain_writer(bus: Arc<SessionBus>, root_session_id: String) {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        let base = sidechain_dir(&root_session_id);
        let mut writers: HashMap<String, BufWriter<File>> = HashMap::new();
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    let Some(parent_tu) = msg.parent_tool_use_id() else { continue };
                    let session_id = msg.session_id();
                    let writer = writers.entry(session_id.to_string()).or_insert_with(|| {
                        let path = base.join(format!("{}.jsonl", session_id));
                        // ensure dir; open append
                        ...
                    });
                    let line = serde_json::to_string(&msg).unwrap_or_default();
                    let _ = writer.write_all(line.as_bytes());
                    let _ = writer.write_all(b"\n");
                    let _ = writer.flush();
                    let _ = parent_tu;  // already used via has-check above
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn sidechain_dir(root: &str) -> PathBuf {
    let home = dirs::home_dir().expect("home");
    home.join(".super").join("sessions").join(root).join("sidechains")
}
```

Spawned from `main.rs` after the bus is constructed. Failures (disk full, permission denied) log and continue — persistence is best-effort.

Add helper methods on `BusMessage`:
```rust
impl BusMessage {
    pub fn parent_tool_use_id(&self) -> Option<&str> { ... }
    pub fn session_id(&self) -> &str { ... }
}
```

### Async agent registry (cli/src/state/store.rs)

```rust
pub struct AsyncAgentHandle {
    pub agent_id: String,
    pub parent_tool_use_id: String,
    pub abort: watch::Sender<bool>,
    pub started_at: Instant,
    pub description: String,
}

// In AppState:
pub async_agents: HashMap<String, AsyncAgentHandle>,
```

Methods: `register_async_agent`, `complete_async_agent`, `abort_async_agent(agent_id)`, `list_async_agents`.

On root session shutdown, iterate handles and send `true` to each abort to clean up.

### System event subtype

Add `SystemSubtype::AsyncAgentDone` to `cli/src/sdk/protocol.rs`. Format: `"<agent_id> finished: <final_text>"`. The TUI's existing System rendering picks it up automatically.

**Protocol caveat:** `BusMessage::SystemEvent` today does NOT carry `parent_tool_use_id` (only `subtype`, `message`, `uuid`, `session_id`). The matches_filter helper in `cli/src/tui/transcript.rs` short-circuits SystemEvents to `parent: None`. For AsyncAgentDone we need the parent_tool_use_id so a) sidechain writer can route the event, b) future drill-in views show completion under the right Task card. Two paths:

- **Path A (preferred): add `parent_tool_use_id: Option<String>` to `SystemEvent`.** Additive protocol change; update `matches_filter` to read it; update existing `SessionBus::emit_system` to default `None`. Minimal blast radius.
- Path B: encode the parent_tool_use_id in `message` as a structured suffix. Hacky and surfaces in the rendered text.

Going with Path A.

(If protocol additions feel risky: alternative is to reuse `SystemSubtype::Notice` with a known prefix. The new variant is cleaner and is additive — no backwards-compat hazard.)

---

## Data flow walkthroughs

### Sync subagent — happy path

1. User: "use Explore to find auth code"
2. Parent assistant emits `tool_use { name: "Task", id: TU_1, input: {description: "find auth", prompt: "...", subagent_type: "Explore"} }`.
3. `run_tool_uses` calls `AgentTool::call(input, ctx)` with `ctx.bus = Some(bus)`, `ctx.parent_tool_use_id = None`.
4. AgentTool resolves "Explore", builds child engine with `agent_id = a-xyz`, shared bus, filtered registry (no Write/Edit), shared abort.
5. `child.process_prompt(prompt, &explore_sys, Some(TU_1)).await`.
6. Child emits `BusMessage::User { parent_tool_use_id: Some(TU_1), session_id: "a-xyz", content: [Text { text: prompt }] }`.
7. Child runs its tool loop. Internal `Grep` call produces:
   - `StreamEvent { ContentBlockStart { ToolUse { id: TU_2, name: "Grep", ... } }, parent_tool_use_id: Some(TU_1), session_id: "a-xyz" }`
   - `ToolProgress { tool_use_id: TU_2, parent_tool_use_id: Some(TU_1), session_id: "a-xyz", ... }`
   - `User { content: [ToolResult { tool_use_id: TU_2, ... }], parent_tool_use_id: Some(TU_1), session_id: "a-xyz" }`
8. Child returns final assistant text "Found auth in `src/auth.rs:...`".
9. AgentTool::call returns `ToolResult { content: "Found auth in...", is_error: false, metadata: { agent_id: "a-xyz" } }`.
10. Parent's `run_tool_uses` builds `tool_result { tool_use_id: TU_1, content: "Found auth in..." }` and parent loop continues.

Fold view:
- `fold(events, None)` → User + Assistant + ToolCall(Task, result=`"Found auth in..."`) + Assistant
- `fold(events, Some(TU_1))` → User(prompt) + Assistant(child's thinking + final text) + ToolCall(Grep, result=...)

### Async subagent — happy path

Steps 1–4 same as sync, but `run_in_background = true` in input.

5. AgentTool registers `AsyncAgentHandle` in store, spawns the child loop on a tokio task with a fresh abort watch, returns `ToolResult { content: "Agent a-xyz running in background.", is_error: false, metadata: { agent_id: "a-xyz" } }` immediately.
6. Parent loop continues with the placeholder result.
7. Child task runs over the next N seconds, emitting bus events with `parent_tool_use_id: Some(TU_1)`.
8. On completion, child emits `SystemEvent { subtype: AsyncAgentDone, message: "a-xyz finished: <final_text>", parent_tool_use_id: Some(TU_1), session_id: "a-xyz" }`.
9. TUI's existing System renderer shows `※ a-xyz finished: ...` in the scroll-area.
10. (Future) SendMessage tool surfaces this into the parent's next prompt.

### Abort — sync agent

Parent's abort_signal fires (user hit Esc). The child engine's shared `watch::Receiver<bool>` sees `true`. The child's request stream is dropped, `process_prompt` returns `Err("aborted")`. AgentTool returns `ToolResult { is_error: true, content: "Agent aborted" }`. Parent loop sees the error result and continues (or also aborts, depending on the parent's stop reason).

### Abort — async agent

Async children have independent watch channels. Parent abort does NOT cancel them. Killing an async agent requires:
- Root session shutdown → iterates `store.async_agents`, sends `true` to every abort.
- Future: explicit `TaskStop` tool with the agent_id.

---

## Error handling

| Failure | AgentTool::call result |
|---|---|
| unknown `subagent_type` | `is_error: true, content: "unknown subagent: X (available: a, b, c)"` |
| missing `prompt` or `description` | `is_error: true, content: "missing required field: X"` |
| child loop hits max_turns | `is_error: true, content: "Agent reached max turns ({max_turns}): <partial final text>"` |
| child API error | `is_error: true, content: "Agent failed: {error}"` |
| abort | `is_error: true, content: "Agent aborted"` |
| async spawn failure | `is_error: true, content: "Failed to spawn background agent: {error}"` (synchronous, no spawn) |

All paths return a tagged `ToolResult` — never panic, never drop the tool_use_id (Anthropic rejects turns with orphaned tool_use_ids).

---

## Testing strategy

### Unit

- `agents::loader::parse_md` — happy path with full frontmatter, missing frontmatter (no `---`), empty body, unknown keys (ignored with warn), invalid `maxTurns`.
- `agents::registry::resolve` — precedence project > user > built-in; unknown returns `None`.
- `agents::permission::resolve_permission_mode` — every combination of (parent ∈ {default, plan, acceptEdits, bypassPermissions}, agent ∈ {None, default, plan, ...}, is_async ∈ {true, false}).
- `tools::ToolRegistry::filter_for_agent` — `tools: None` (inherit all), `tools: ["*"]`, `tools: ["Read", "Grep"]`, `disallowed_tools: ["Edit"]` intersection.

### Integration (mock OpenRouter)

- **Sync Task → text result.** Mock responds to parent with `tool_use { name: "Task" }`; responds to child with text. Assert:
  - `fold(events, None)` contains one ToolCall(Task) with result equal to child's text.
  - `fold(events, Some(TU_1))` contains the child's User+Assistant.
  - Sidechain JSONL at `~/.super/sessions/<root>/sidechains/a-xyz.jsonl` exists with the expected events.
- **Sync Task → child runs a nested tool.** Mock parent → Task; mock child → tool_use{Grep} then text. Assert the Grep ToolCall is folded under `filter=Some(TU_1)`, not at the root.
- **Async Task — returns immediately.** Mock parent → Task with `run_in_background: true`. Assert AgentTool returns within 100ms with the placeholder text; assert the spawned task is registered in `store.async_agents`; wait for completion; assert `SystemEvent::AsyncAgentDone` fires.
- **Abort propagation.** Sync child mid-stream; trigger parent's abort; assert child returns error within 1s.
- **Unknown subagent_type.** Assert `is_error: true` with a helpful message.
- **Permission mode overlay.** Parent in `default`, Explore agent has `permissionMode: plan` (via .md) → child runs in `plan`. Parent in `bypassPermissions` → child stays in `bypassPermissions` regardless of agent.

### Acceptance (tmux harness)

Same harness as #5 design doc, but with a prompt that exercises Task:

```bash
tmux ... send-keys -t super -l "use the Explore agent to find where session_id is initialized"
```

Acceptance:
- A `⏺ Task Explore find session_id` line appears in the scroll-area.
- An elapsed counter ticks under it.
- A `⎿ ...` indented final text appears with Explore's findings.
- Sidechain JSONL exists at the expected path.
- Diff against Claude Code's behaviour for the same prompt is limited to model identity, glyphs (per CLAUDE.md), and the actual content of the search.

---

## Migration order

Each step must leave the build green and the existing scroll-area path working.

1. **Add `cli/src/agents/` module** — built-ins, loader, registry, permission overlay. Pure additive. Unit tests pass; nothing wired up yet.
2. **Plumb `parent_tool_use_id` through engine.** Add the parameter on `process_prompt`; thread `None` from `App::run`. Stamp on every internal emit. No behaviour change for root sessions.
3. **Plumb `bus` and `parent_tool_use_id` through tool_loop.** Populate `ToolCallContext` fields. Add to ToolProgress emit. Still no behaviour change (no tool reads them yet).
4. **Add `ConversationEngine::new_child` + `session_id_override`.** Refactor to use override when set. Root still passes `None` everywhere.
5. **Add `ToolRegistry::filter_for_agent`.** Pure helper; not wired yet.
6. **Add `SystemSubtype::AsyncAgentDone`.** Protocol additive.
7. **Add `Store::async_agents` and methods.** Storage scaffolding.
8. **Add `sidechain::spawn_sidechain_writer`.** Spawn from main.rs; sidechain files start appearing for any future subagent activity (none yet, so they're empty).
9. **Rename `AgentTool` → `"Task"`, replace stub with real implementation.** End-to-end sync subagents work after this step.
10. **Add async path.** `run_in_background: true` works after this step.
11. **TUI polish.** Task-specific summary line ("Task <description>" instead of raw JSON dump).
12. **Cleanup.** Drop dead-code annotations on the now-used fields (`ToolCallContext::bus`, `::parent_tool_use_id`). Update `cli/src/tools/mod.rs` Vec ordering / construction order.

Steps 1–8 are demoable but don't change user-visible behaviour. Step 9 is the unlock. Steps 10–11 are polish.

---

## Out of scope (deferred)

These map to fields that already have field-level scaffolding in the protocol or agent definition but are not implemented:

- **MCP servers per agent** (`mcpServers:` frontmatter) — MCP itself is unshipped in super.
- **Agent-defined hooks** (`hooks:`) — needs the broader hook system (CLAUDE.md forbids in v1).
- **Agent-preloaded skills** (`skills:`) — Skill tool is a stub in super.
- **Worktree isolation** (`isolation: "worktree"`) — uses worktree tools that are stubs.
- **Model alias resolution** (`sonnet`/`opus`/`haiku`) — v1 ships a minimal alias table in `cli/src/agents/`: `sonnet` → `anthropic/claude-sonnet-4-6`, `opus` → `anthropic/claude-opus-4-7`, `haiku` → `anthropic/claude-haiku-4-5-20251001`, `inherit` → parent's model. Full alias-resolution parity with Claude Code's getAgentModel comes later. If `agent.model` is a string not in the alias table, pass through verbatim (assumed full OpenRouter id).
- **Multi-agent / teammate spawning** (`name`, `team_name`, `mode` frontmatter).
- **Resume after restart** for async agents — once super exits, async children die. Resuming them would require persisting state beyond JSONL.
- **TUI drill-in keybinding** — fold-by-tool_use_id works; no widget to surface it yet. Track in a follow-up issue.
- **SendMessage delivery of async completion** — for v1 the AsyncAgentDone SystemEvent renders in the scroll-area; integrating with SendMessage to surface as a user message is a follow-up.

---

## Open questions

None at time of writing. If implementation surfaces ambiguity (e.g. how to wire `Arc<AgentRegistry>` through main.rs cleanly, how exactly to spell the YAML frontmatter parse for `tools: ["*"]` vs absent vs empty), resolve per CLAUDE.md ("ask, don't improvise") and update this doc inline.

---

## Acceptance checklist

- [ ] `cli/src/agents/` module compiles, unit tests pass.
- [ ] `cli/src/agents/built_in.rs` ports general-purpose, Explore, Plan, statusline-setup, claude-code-guide system prompts verbatim from `claude-code-src/`.
- [ ] `.claude/agents/<name>.md` files are picked up; precedence project > user > built-in verified by test.
- [ ] `ConversationEngine::process_prompt` accepts `parent_tool_use_id`; all internal emits stamp it.
- [ ] `tool_loop::run_tool_uses` populates `ctx.bus` and `ctx.parent_tool_use_id`.
- [ ] `AgentTool` registered as `"Task"`; old `"Agent"` name removed.
- [ ] Sync Task call end-to-end: child runs, emits events with hierarchy fields, returns final text as ToolResult.
- [ ] Async Task call: returns immediately, child runs in background, AsyncAgentDone fires on completion.
- [ ] Sidechain JSONL at `~/.super/sessions/<root>/sidechains/<agent_id>.jsonl` is populated for both sync and async children.
- [ ] Abort: sync child cancels with parent; async child has independent lifetime.
- [ ] Permission overlay: parent bypass/acceptEdits wins; otherwise agent override applies.
- [ ] Acceptance tmux test shows Task tool-call line and `⎿` result in super matching Claude Code's behaviour modulo documented divergences.
