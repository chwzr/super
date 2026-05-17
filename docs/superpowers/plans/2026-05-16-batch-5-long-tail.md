# Batch 5 — Long Tail Tools: Search, Prompts, OutputSchemas, and Stub Parity

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the real ToolSearch (P0 — the model cannot discover deferred tools without it), port all 11 prompts from `claude-code-src/`, add missing `output_schema()` implementations, fix `is_read_only()` for ConfigTool, add basic schema validation to StructuredOutput, and add better cron field validation to CronCreate.

**Architecture:** Eleven tools. ToolSearch is the only one that gets a real `call()` implementation — it accepts an `Arc<ToolRegistry>` and searches registered tools by name/description/searchHint with `select:` prefix support. Monitor, EnterWorktree, ExitWorktree, LSP stay as stubs but get their correct prompt text and output schemas. ConfigTool, Sleep, CronCreate, CronDelete, CronList, and StructuredOutput get prompt ports, output schemas, and minor logic fixes. All prompt files currently contain `TODO(parity:batch-5)` stubs and will be replaced with the actual Claude Code prompt text.

**Tech Stack:** Rust, serde_json, `json-schema` (or `jsonschema` crate for StructuredOutput validation).

**Prerequisite:** Batch 1 (extended Tool trait with `output_schema()`, `should_defer()`, `always_load()`, `search_hint()`) must be complete. The `ToolRegistry` (`cli/src/tools/mod.rs`) already has the `Arc<ToolRegistry>` passing pattern used by `AgentTool` — ToolSearch will follow the same pattern.

---

## File Map

| File | Responsibility |
|------|---------------|
| `cli/src/tools/tool_search.rs` | **P0** — Real implementation: search registry by name/desc/hint, `select:` prefix, structured JSON result |
| `cli/src/tools/monitor.rs` | Add `output_schema()`, keep stub `call()` |
| `cli/src/tools/enter_worktree.rs` | Keep stub, verify schema |
| `cli/src/tools/exit_worktree.rs` | Keep stub, verify schema |
| `cli/src/tools/lsp.rs` | Add `output_schema()`, keep stub `call()` |
| `cli/src/tools/config_tool.rs` | Fix `is_read_only()`, add `output_schema()` |
| `cli/src/tools/sleep.rs` | Add `output_schema()`, verify max duration |
| `cli/src/tools/cron_create.rs` | Add `output_schema()`, better field validation |
| `cli/src/tools/cron_delete.rs` | Add `output_schema()` |
| `cli/src/tools/cron_list.rs` | Add `output_schema()` |
| `cli/src/tools/structured_output.rs` | Add schema validation, `output_schema()` |
| `cli/src/tools/mod.rs` | Pass `Arc<ToolRegistry>` to `ToolSearchTool` |
| `cli/src/tools/prompts/tool_search.txt` | Port ToolSearch prompt from `claude-code-src/tools/ToolSearchTool/prompt.ts` |
| `cli/src/tools/prompts/monitor.txt` | Port Monitor prompt (derived from BashTool monitor section) |
| `cli/src/tools/prompts/enter_worktree.txt` | Port EnterWorktree prompt |
| `cli/src/tools/prompts/exit_worktree.txt` | Port ExitWorktree prompt |
| `cli/src/tools/prompts/lsp.txt` | Port LSP prompt |
| `cli/src/tools/prompts/config_tool.txt` | Port ConfigTool prompt |
| `cli/src/tools/prompts/sleep.txt` | Port Sleep tool prompt |
| `cli/src/tools/prompts/cron_create.txt` | Port CronCreate prompt (massive, ~120 lines) |
| `cli/src/tools/prompts/cron_delete.txt` | Port CronDelete prompt |
| `cli/src/tools/prompts/cron_list.txt` | Port CronList prompt |
| `cli/src/tools/prompts/structured_output.txt` | Port StructuredOutput prompt |

---

## Task 1: ToolSearch — Real Implementation (P0)

**Why first:** Deferred tools (AskUserQuestion, future MCP tools, CronCreate, CronDelete, CronList) cannot be discovered by the model without ToolSearch. This is the single most critical piece in Batch 5 — without it, the tool deferred loading system is broken.

**Files:**
- Modify: `cli/src/tools/tool_search.rs` (full rewrite of `call()`)
- Modify: `cli/src/tools/mod.rs` (pass `Arc<ToolRegistry>` to `ToolSearchTool`)
- Modify: `cli/src/tools/prompts/tool_search.txt` (port from `claude-code-src/tools/ToolSearchTool/prompt.ts`)

### Step 1: Write the prompt file

Replace `cli/src/tools/prompts/tool_search.txt`:

```bash
cat > cli/src/tools/prompts/tool_search.txt << 'PROMPTEOF'
Fetches full schema definitions for deferred tools so they can be called.

Deferred tools appear by name in <system-reminder> messages. Until fetched, only the name is known — there is no parameter schema, so the tool cannot be invoked. This tool takes a query, matches it against the deferred tool list, and returns the matched tools' complete JSONSchema definitions inside a <functions> block. Once a tool's schema appears in that result, it is callable exactly like any tool defined at the top of the prompt.

Result format: each matched tool appears as one <function>{"description": "...", "name": "...", "parameters": {...}}</function> line inside the <functions> block — the same encoding as the tool list at the top of this prompt.

Query forms:
- "select:Read,Edit,Grep" — fetch these exact tools by name
- "notebook jupyter" — keyword search, up to max_results best matches
- "+slack send" — require "slack" in the name, rank by remaining terms
PROMPTEOF
```

### Step 2: Modify `mod.rs` to pass `Arc<ToolRegistry>` to `ToolSearchTool`

In `cli/src/tools/mod.rs`, change:

```rust
// ToolSearch
tools.push(Arc::new(ToolSearchTool));
```

to:

```rust
// ToolSearch
tools.push(Arc::new(ToolSearchTool {
    registry: registry.clone(),
}));
```

Move this block up so it comes BEFORE the `let registry = Arc::new(Self { ... })` line. Specifically, insert the ToolSearch registration right after `tools.push(Arc::new(MonitorTool));` and before the registry construction. But since `registry.clone()` requires the registry to exist, construct ToolSearchTool with a deferral pattern:

Change the block starting at `// ToolSearch` to:

```rust
        // ToolSearch — needs an Arc<ToolRegistry> back-reference so it can
        // search the tool list. Registered via `register()` after the Arc is
        // constructed, same pattern as AgentTool.
        let tool_search = Arc::new(ToolSearchTool {
            registry: Arc::new(std::sync::RwLock::new(Vec::new())), // placeholder, replaced below
        });
        // We'll register the real one after registry construction
```

Then after `let registry = Arc::new(Self { ... })`, add:

```rust
        // Re-register ToolSearch with the real registry back-reference
        registry.register(Arc::new(ToolSearchTool {
            registry: registry.clone(),
        }));
```

And remove the earlier `tools.push(Arc::new(ToolSearchTool));` from the pre-registry vec.

Wait, that's convoluted. Simpler approach: build the registry first, then register ToolSearch. Change the ordering in `ToolRegistry::new()`:

1. Build the `tools` vec WITHOUT ToolSearch
2. Construct the `Arc<ToolRegistry>`
3. Register ToolSearch via `registry.register()`
4. Continue with AgentTool etc.

The exact code change:

```rust
        // ... all other tools pushed to vec ...
        
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

        let registry = Arc::new(Self { tools: Arc::new(RwLock::new(tools)) });

        // ToolSearch — registers after the Arc exists so it can hold a
        // back-reference for searching the full tool list.
        registry.register(Arc::new(ToolSearchTool {
            registry: registry.clone(),
        }));

        // Agent and task management tools.
        // ...
```

### Step 3: Rewrite `cli/src/tools/tool_search.rs`

Full rewrite of the struct and impl:

```rust
use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, RwLock};
use super::contract::{DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent};

const MAX_SEARCH_RESULTS: usize = 20;

pub struct ToolSearchTool {
    pub registry: Arc<super::ToolRegistry>,
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str { "ToolSearch" }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Search for tools by name or description. Use select: prefix for exact tool name lookup. Returns matching tools with their full schemas.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/tool_search.txt").into()
    }

    fn search_hint(&self) -> Option<&'static str> {
        Some("discover and load deferred tools by name or description")
    }

    fn should_defer(&self) -> bool { false }
    fn always_load(&self) -> bool { true }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query to match against tool names and descriptions. Use 'select:ToolName1,ToolName2' for exact tool name lookup."
                }
            },
            "required": ["query"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "matches": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "description": {"type": "string"},
                            "input_schema": {"type": "object"}
                        }
                    }
                }
            }
        }))
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { true }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) => q.trim(),
            None => {
                return ToolResult {
                    content: "Missing required parameter: query".into(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let all_tools: Vec<Arc<dyn Tool>> = {
            let tools = self.registry.tools.read().unwrap();
            tools.clone()
        };

        // --- select: prefix ---
        if let Some(names_str) = query.strip_prefix("select:") {
            let names: Vec<&str> = names_str.split(',').map(|s| s.trim()).collect();
            let mut matches = Vec::new();
            for name in &names {
                if let Some(tool) = all_tools.iter().find(|t| t.name() == *name) {
                    matches.push(format_tool_entry(&**tool));
                }
            }
            if matches.is_empty() {
                return ToolResult {
                    content: format!("No tools found matching select names: {names_str}"),
                    is_error: false,
                    ..Default::default()
                };
            }
            let result = format!("<functions>\n{}\n</functions>", matches.join("\n"));
            return ToolResult {
                content: result,
                is_error: false,
                ..Default::default()
            };
        }

        // --- +prefix: require term in name ---
        let require_in_name: Option<&str> = query.strip_prefix('+')
            .and_then(|s| s.split_whitespace().next());

        let search_terms: Vec<&str> = if require_in_name.is_some() {
            // After "+slack send" => require "slack", search by "send"
            let after_plus = query.strip_prefix('+').unwrap();
            let parts: Vec<&str> = after_plus.splitn(2, ' ').collect();
            if parts.len() > 1 { parts[1].split_whitespace().collect() } else { vec![] }
        } else {
            query.split_whitespace().collect()
        };

        let query_lower = query.to_lowercase();
        let mut scored: Vec<(i32, &Arc<dyn Tool>)> = Vec::new();

        for tool in &all_tools {
            let name = tool.name();
            let desc = tool.description(None, &DescriptionCtx::default());
            let hint = tool.search_hint().unwrap_or("");
            let name_lower = name.to_lowercase();
            let desc_lower = desc.to_lowercase();
            let hint_lower = hint.to_lowercase();

            // +prefix filter: must contain term in name
            if let Some(req) = require_in_name {
                if !name_lower.contains(&req.to_lowercase()) {
                    continue;
                }
            }

            let mut score: i32 = 0;

            // Exact name match = highest
            if name_lower == query_lower { score += 1000; }
            // Name starts with query
            else if name_lower.starts_with(&query_lower) { score += 500; }
            // Name contains query
            else if name_lower.contains(&query_lower) { score += 200; }

            // Word-level matching in name
            for term in &search_terms {
                let term_lower = term.to_lowercase();
                if name_lower.contains(&term_lower) { score += 100; }
                if desc_lower.contains(&term_lower) { score += 50; }
                if hint_lower.contains(&term_lower) { score += 30; }
            }

            // Full query in description or hint
            if desc_lower.contains(&query_lower) { score += 40; }
            if hint_lower.contains(&query_lower) { score += 20; }

            if score > 0 || require_in_name.is_some() {
                scored.push((score, tool));
            }
        }

        // Sort by score descending, then by name
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name().cmp(b.1.name())));

        if scored.is_empty() {
            return ToolResult {
                content: format!("No tools found matching query: {query}"),
                is_error: false,
                ..Default::default()
            };
        }

        let matches: Vec<String> = scored.iter()
            .take(MAX_SEARCH_RESULTS)
            .map(|(_, tool)| format_tool_entry(&***tool))
            .collect();

        let result = format!("<functions>\n{}\n</functions>", matches.join("\n"));
        ToolResult {
            content: result,
            is_error: false,
            ..Default::default()
        }
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

fn format_tool_entry(tool: &dyn Tool) -> String {
    let desc = tool.description(None, &DescriptionCtx::default());
    let schema = tool.input_schema();
    format!(
        "<function>{}</function>",
        json!({
            "name": tool.name(),
            "description": desc,
            "parameters": schema
        })
    )
}
```

Note: The function signature uses `Arc<dyn Tool>` items. Since `ToolRegistry.tools` is `Arc<RwLock<Vec<Arc<dyn Tool>>>>`, we can clone the vec. Make sure `use std::sync::Arc;` is imported.

### Step 4: Run verification

```bash
cargo build -p super-cli 2>&1 | tail -20
```

Expected: clean build.

```bash
cargo test -p super-cli --lib tools::registry_tests 2>&1 | tail -10
```

Expected: all registry tests pass (some may need updating if the ToolSearch tool is now in the list).

### Commit

```
fix(parity): implement real ToolSearch with registry search

Replace the ToolSearch stub with a real implementation that searches
the ToolRegistry by name, description, and searchHint. Supports select:
prefix for exact tool name lookup and +term prefix to require the term
in the tool name. Returns structured JSON with full <functions> blocks
so deferred tools become callable without a second round-trip.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
```

---

## Task 2: Monitor — Prompt + OutputSchema Parity (Stub)

**Why:** Schema already has `command, description, timeout_ms, persistent` — matches Claude's `MonitorTool`. Port the prompt text and add `output_schema()`. Keep the stub `call()` since the scheduler/monitor backend is deferred to a separate spec.

**Files:**
- Modify: `cli/src/tools/monitor.rs` (add `output_schema()`, small description tweak)
- Modify: `cli/src/tools/prompts/monitor.txt` (port full prompt)

### Step 1: Write the prompt file

Replace `cli/src/tools/prompts/monitor.txt`:

```bash
cat > cli/src/tools/prompts/monitor.txt << 'PROMPTEOF'
Start a background monitor that streams events from a long-running script. Each stdout line is an event — you keep working and notifications arrive in the chat. Events arrive on their own schedule and are not replies from the user, even if one lands while you're waiting for the user to answer a question.

Pick by how many notifications you need:
- **One** ("tell me when the server is ready / the build finishes") — use **Bash with `run_in_background`** and a command that exits when the condition is true, e.g. `until grep -q "Ready in" dev.log; do sleep 0.5; done`. You get a single completion notification when it exits.
- **One per occurrence, indefinitely** ("tell me every time an ERROR line appears") — Monitor with an unbounded command (`tail -f`, `inotifywait -m`, `while true`).
- **One per occurrence, until a known end** ("emit each CI step result, stop when the run completes") — Monitor with a command that emits lines and then exits.

Your script's stdout is the event stream. Each line becomes a notification. Exit ends the watch.

**Script quality:**
- Always use `grep --line-buffered` in pipes — without it, pipe buffering delays events by minutes.
- In poll loops, handle transient failures (`curl ... || true`) — one failed request shouldn't kill the monitor.
- Poll intervals: 30s+ for remote APIs (rate limits), 0.5-1s for local checks.
- Write a specific `description` — it appears in every notification ("errors in deploy.log" not "watching logs").
- Only stdout is the event stream. Stderr goes to the output file but does not trigger notifications — for a command you run directly, merge stderr with `2>&1` so its failures reach your filter.

**Coverage — silence is not success.** When watching a job or process for an outcome, your filter must match every terminal state, not just the happy path. A monitor that greps only for the success marker stays silent through a crashloop, a hung process, or an unexpected exit — and silence looks identical to "still running." Before arming, ask: *if this process crashed right now, would my filter emit anything?* If not, widen it.

**Output volume**: Every stdout line is a conversation message, so the filter should be selective — but selective means "the lines you'd act on," not "only good news." Never pipe raw logs; use `grep --line-buffered`, `awk`, or a wrapper that emits exactly the success and failure signals you care about.
PROMPTEOF
```

### Step 2: Modify `cli/src/tools/monitor.rs`

Add `output_schema()` to the `impl Tool for MonitorTool` block. The current schema is already correct (matches Claude's Monitor tool). Just add:

```rust
    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "acknowledged": {"type": "boolean"},
                "message": {"type": "string"}
            }
        }))
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }
```

The full file after changes (key additions only, rest unchanged):

```rust
// ... existing code unchanged ...
    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "acknowledged": {"type": "boolean"},
                "message": {"type": "string"}
            }
        }))
    }
// ... rest unchanged ...
```

### Step 3: Verify build

```bash
cargo build -p super-cli 2>&1 | tail -5
```

### Commit

```
fix(parity): port Monitor prompt and add outputSchema

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
```

---

## Task 3: EnterWorktree — Prompt Parity (Stub)

**Why:** Schema already has `name?, path?` — matches Claude's `EnterWorktreeTool`. Port the prompt text. Keep the stub `call()` since worktree implementation is deferred.

**Files:**
- Modify: `cli/src/tools/prompts/enter_worktree.txt` (port prompt)
- Modify: `cli/src/tools/enter_worktree.rs` (add `output_schema()` if missing)

### Step 1: Write the prompt file

Replace `cli/src/tools/prompts/enter_worktree.txt`:

```bash
cat > cli/src/tools/prompts/enter_worktree.txt << 'PROMPTEOF'
Use this tool ONLY when explicitly instructed to work in a worktree — either by the user directly, or by project instructions (CLAUDE.md / memory). This tool creates an isolated git worktree and switches the current session into it.

## When to Use

- The user explicitly says "worktree" (e.g., "start a worktree", "work in a worktree", "create a worktree", "use a worktree")
- CLAUDE.md or memory instructions direct you to work in a worktree for the current task

## When NOT to Use

- The user asks to create a branch, switch branches, or work on a different branch — use git commands instead
- Never use this tool unless "worktree" is explicitly mentioned by the user or in CLAUDE.md / memory instructions

## Requirements

- Must be in a git repository, OR have WorktreeCreate/WorktreeRemove hooks configured in settings.json
- Must not already be in a worktree

## Behavior

- In a git repository: creates a new git worktree inside `.claude/worktrees/` on a new branch. The base ref is governed by the `worktree.baseRef` setting: `fresh` (default) branches from origin/<default-branch>; `head` branches from your current local HEAD
- Outside a git repository: delegates to WorktreeCreate/WorktreeRemove hooks for VCS-agnostic isolation
- Switches the session's working directory to the new worktree
- Use ExitWorktree to leave the worktree mid-session (keep or remove). On session exit, if still in the worktree, the user will be prompted to keep or remove it

## Entering an existing worktree

Pass `path` instead of `name` to switch the session into a worktree that already exists (e.g., one you just created with `git worktree add`). The path must appear in `git worktree list` for the current repository — paths that are not registered worktrees of this repo are rejected. ExitWorktree will not remove a worktree entered this way; use `action: "keep"` to return to the original directory.

## Parameters

- `name` (optional): A name for a new worktree. Each "/"-separated segment may contain only letters, digits, dots, underscores, and dashes; max 64 chars total. A random name is generated if not provided. Mutually exclusive with `path`.
- `path` (optional): Path to an existing worktree of the current repository to switch into instead of creating a new one. Must appear in `git worktree list` for the current repo. Mutually exclusive with `name`.
PROMPTEOF
```

### Step 2: Verify schema matches Claude

Current schema (in `enter_worktree.rs`):

```rust
json!({
    "type": "object",
    "properties": {
        "name": {"type": "string", "description": "Optional name for the worktree"},
        "path": {"type": "string", "description": "Optional path to an existing worktree"}
    }
})
```

This is already correct per Claude's EnterWorktreeTool. No changes needed to the schema or the Rust struct — just the prompt file.

### Commit

```
fix(parity): port EnterWorktree prompt from Claude Code source

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
```

---

## Task 4: ExitWorktree — Prompt Parity (Stub)

**Why:** Schema already has `action, discard_changes?` — matches Claude's `ExitWorktreeTool`. Port the prompt text. Keep the stub.

**Files:**
- Modify: `cli/src/tools/prompts/exit_worktree.txt` (port prompt)
- No Rust changes needed (schema already correct, stub call() is fine)

### Step 1: Write the prompt file

Replace `cli/src/tools/prompts/exit_worktree.txt`:

```bash
cat > cli/src/tools/prompts/exit_worktree.txt << 'PROMPTEOF'
Exit a worktree session created by EnterWorktree and return the session to the original working directory.

## Scope

This tool ONLY operates on worktrees created by EnterWorktree in this session. It will NOT touch:
- Worktrees you created manually with `git worktree add`
- Worktrees from a previous session (even if created by EnterWorktree then)
- The directory you're in if EnterWorktree was never called

If called outside an EnterWorktree session, the tool is a **no-op**: it reports that no worktree session is active and takes no action. Filesystem state is unchanged.

## When to Use

- The user explicitly asks to "exit the worktree", "leave the worktree", "go back", or otherwise end the worktree session
- Do NOT call this proactively — only when the user asks

## Parameters

- `action` (required): `"keep"` or `"remove"`
  - `"keep"` — leave the worktree directory and branch intact on disk. Use this if the user wants to come back to the work later, or if there are changes to preserve.
  - `"remove"` — delete the worktree directory and its branch. Use this for a clean exit when the work is done or abandoned.
- `discard_changes` (optional, default false): only meaningful with `action: "remove"`. If the worktree has uncommitted files or commits not on the original branch, the tool will REFUSE to remove it unless this is set to `true`. If the tool returns an error listing changes, confirm with the user before re-invoking with `discard_changes: true`.

## Behavior

- Restores the session's working directory to where it was before EnterWorktree
- Clears CWD-dependent caches (system prompt sections, memory files, plans directory) so the session state reflects the original directory
- Once exited, EnterWorktree can be called again to create a fresh worktree
PROMPTEOF
```

### Commit

```
fix(parity): port ExitWorktree prompt from Claude Code source

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
```

---

## Task 5: LSP — Prompt + OutputSchema Parity (Stub)

**Why:** Schema already has `operation, filePath, line, character` supporting all Claude LSP operations. Port prompt and add `output_schema()`. Keep stub `call()`.

**Files:**
- Modify: `cli/src/tools/lsp.rs` (add `output_schema()`, update description, add `prepareCallHierarchy`/`incomingCalls`/`outgoingCalls` to operation enum)
- Modify: `cli/src/tools/prompts/lsp.txt` (port prompt)

### Step 1: Write the prompt file

Replace `cli/src/tools/prompts/lsp.txt`:

```bash
cat > cli/src/tools/prompts/lsp.txt << 'PROMPTEOF'
Interact with Language Server Protocol (LSP) servers to get code intelligence features.

Supported operations:
- goToDefinition: Find where a symbol is defined
- findReferences: Find all references to a symbol
- hover: Get hover information (documentation, type info) for a symbol
- documentSymbol: Get all symbols (functions, classes, variables) in a document
- workspaceSymbol: Search for symbols across the entire workspace
- goToImplementation: Find implementations of an interface or abstract method
- prepareCallHierarchy: Get call hierarchy item at a position (functions/methods)
- incomingCalls: Find all functions/methods that call the function at a position
- outgoingCalls: Find all functions/methods called by the function at a position

All operations require:
- filePath: The file to operate on
- line: The line number (1-based, as shown in editors)
- character: The character offset (1-based, as shown in editors)

Note: LSP servers must be configured for the file type. If no server is available, an error will be returned.
PROMPTEOF
```

### Step 2: Update LSP schema to include missing operations

Current enum is missing `prepareCallHierarchy`, `incomingCalls`, `outgoingCalls`. In `cli/src/tools/lsp.rs`, update the `input_schema()`:

```rust
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {"type": "string", "enum": [
                    "goToDefinition", "findReferences", "hover",
                    "documentSymbol", "workspaceSymbol", "goToImplementation",
                    "prepareCallHierarchy", "incomingCalls", "outgoingCalls"
                ]},
                "filePath": {"type": "string"},
                "line": {"type": "integer"},
                "character": {"type": "integer"}
            },
            "required": ["operation", "filePath", "line", "character"]
        })
    }
```

Also add `output_schema()`:

```rust
    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "result": {"description": "LSP operation result"}
            }
        }))
    }
```

Note: `line` and `character` should not be in `required` for operations that don't need them (e.g. `workspaceSymbol`). But keeping them required for now matches the Claude Code schema (which uses Zod and makes them required at the schema level too).

### Commit

```
fix(parity): port LSP prompt and add missing operations to schema

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
```

---

## Task 6: ConfigTool — Fix is_read_only + OutputSchema + Prompt

**Why:** `is_read_only()` currently returns `true` unconditionally, but ConfigTool supports writes (sets values). Fix to return `false` when `value` is present in input. Add `output_schema()`.

**Files:**
- Modify: `cli/src/tools/config_tool.rs` (fix `is_read_only()`, add `output_schema()`)
- Modify: `cli/src/tools/prompts/config_tool.txt` (port prompt)

### Step 1: Write the prompt file

Replace `cli/src/tools/prompts/config_tool.txt`:

```bash
cat > cli/src/tools/prompts/config_tool.txt << 'PROMPTEOF'
Get or set Claude Code configuration settings.

View or change Claude Code settings. Use when the user requests configuration changes, asks about current settings, or when adjusting a setting would benefit them.

## Usage
- **Get current value:** Omit the "value" parameter
- **Set new value:** Include the "value" parameter

## Configurable settings list
The following settings are available for you to change:

### Global Settings (stored in ~/.claude.json)
- theme: "light", "dark" - Color theme for the terminal UI
- editorMode: "normal", "vim" - Editor keybinding mode
- verbose: true/false - Show detailed debug information
- statusLine: true/false - Show the status line

### Project Settings (stored in settings.json)
- model - Override the default model
- permissions - Permission configuration

## Examples
- Get theme: { "key": "theme" }
- Set dark theme: { "key": "theme", "value": "dark" }
- Enable vim mode: { "key": "editorMode", "value": "vim" }
- Enable verbose: { "key": "verbose", "value": true }
- Change model: { "key": "model", "value": "opus" }
PROMPTEOF
```

### Step 2: Fix `is_read_only()` and add `output_schema()`

In `cli/src/tools/config_tool.rs`, change:

```rust
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }
```

to:

```rust
    fn is_read_only(&self, input: &serde_json::Value) -> bool {
        // Read-only when no value is provided (get mode);
        // write when value is present (set mode).
        input.get("value").is_none()
    }
```

Add `output_schema()` after `input_schema()`:

```rust
    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "key": {"type": "string"},
                "value": {"description": "The current value, if reading"},
                "set": {"type": "boolean", "description": "Whether the key was set"}
            }
        }))
    }
```

Note: The `json!` macro must be in scope. Import it at the top if not already: add `use serde_json::json;` to the file (currently the file doesn't have it since it uses `serde_json::json!` fully qualified in `input_schema()`).

### Commit

```
fix(parity): fix ConfigTool is_read_only for writes, add outputSchema

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
```

---

## Task 7: Sleep — OutputSchema + Prompt

**Why:** Sleep already works correctly (max 300000ms verified match with Claude). Add `output_schema()` and port the prompt.

**Files:**
- Modify: `cli/src/tools/sleep.rs` (add `output_schema()`)
- Modify: `cli/src/tools/prompts/sleep.txt` (port prompt)

### Step 1: Write the prompt file

Replace `cli/src/tools/prompts/sleep.txt`:

```bash
cat > cli/src/tools/prompts/sleep.txt << 'PROMPTEOF'
Wait for a specified duration. The user can interrupt the sleep at any time.

Use this when the user tells you to sleep or rest, when you have nothing to do, or when you're waiting for something.

You may receive <tick> prompts — these are periodic check-ins. Look for useful work to do before sleeping.

You can call this concurrently with other tools — it won't interfere with them.

Prefer this over `Bash(sleep ...)` — it doesn't hold a shell process.

Each wake-up costs an API call, but the prompt cache expires after 5 minutes of inactivity — balance accordingly.
PROMPTEOF
```

### Step 2: Add `output_schema()` to `cli/src/tools/sleep.rs`

After `input_schema()`, add:

```rust
    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "slept_ms": {"type": "integer", "description": "Actual duration slept in milliseconds"}
            }
        }))
    }
```

Also add `is_concurrency_safe()` returning `true` since sleep doesn't conflict with anything:

```rust
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { true }
```

### Step 3: Verify max duration

Current code: `input["duration_ms"].as_u64().unwrap_or(1000).min(300000);`
Claude Code SleepTool: max 300000ms (5 minutes).

Confirmed match. No change needed.

### Commit

```
fix(parity): port Sleep prompt and add outputSchema

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
```

---

## Task 8: CronCreate — OutputSchema + Better Validation + Prompt

**Why:** Add `output_schema()` returning `{id, humanSchedule, recurring, durable?}`, add per-field cron validation (minute 0-59, hour 0-23, etc.), and port the massive prompt from `claude-code-src/tools/ScheduleCronTool/prompt.ts`.

**Files:**
- Modify: `cli/src/tools/cron_create.rs` (add `output_schema()`, better validation, `should_defer()`)
- Modify: `cli/src/tools/prompts/cron_create.txt` (port prompt)

### Step 1: Write the prompt file

Replace `cli/src/tools/prompts/cron_create.txt`:

```bash
cat > cli/src/tools/prompts/cron_create.txt << 'PROMPTEOF'
Schedule a prompt to be enqueued at a future time. Use for both recurring schedules and one-shot reminders.

Uses standard 5-field cron in the user's local timezone: minute hour day-of-month month day-of-week. "0 9 * * *" means 9am local — no timezone conversion needed.

## One-shot tasks (recurring: false)

For "remind me at X" or "at <time>, do Y" requests — fire once then auto-delete.
Pin minute/hour/day-of-month/month to specific values:
  "remind me at 2:30pm today to check the deploy" → cron: "30 14 <today_dom> <today_month> *", recurring: false
  "tomorrow morning, run the smoke test" → cron: "57 8 <tomorrow_dom> <tomorrow_month> *", recurring: false

## Recurring jobs (recurring: true, the default)

For "every N minutes" / "every hour" / "weekdays at 9am" requests:
  "*/5 * * * *" (every 5 min), "0 * * * *" (hourly), "0 9 * * 1-5" (weekdays at 9am local)

## Avoid the :00 and :30 minute marks when the task allows it

Every user who asks for "9am" gets `0 9`, and every user who asks for "hourly" gets `0 *` — which means requests from across the planet land on the API at the same instant. When the user's request is approximate, pick a minute that is NOT 0 or 30:
  "every morning around 9" → "57 8 * * *" or "3 9 * * *" (not "0 9 * * *")
  "hourly" → "7 * * * *" (not "0 * * * *")
  "in an hour or so, remind me to..." → pick whatever minute you land on, don't round

Only use minute 0 or 30 when the user names that exact time and clearly means it ("at 9:00 sharp", "at half past", coordinating with a meeting). When in doubt, nudge a few minutes early or late — the user will not notice, and the fleet will.

## Session-only

Jobs live only in this Claude session — nothing is written to disk, and the job is gone when Claude exits.

## Runtime behavior

Jobs only fire while the REPL is idle (not mid-query). The scheduler adds a small deterministic jitter on top of whatever you pick: recurring tasks fire up to 10% of their period late (max 15 min); one-shot tasks landing on :00 or :30 fire up to 90 s early. Picking an off-minute is still the bigger lever.

Recurring tasks auto-expire after 7 days — they fire one final time, then are deleted. This bounds session lifetime. Tell the user about the 7-day limit when scheduling recurring jobs.

Returns a job ID you can pass to CronDelete.
PROMPTEOF
```

### Step 2: Add `output_schema()`, `should_defer()`, and better field validation

In `cli/src/tools/cron_create.rs`, add after `input_schema()`:

```rust
    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Unique job ID"},
                "cron": {"type": "string", "description": "The cron expression"},
                "humanSchedule": {"type": "string", "description": "Human-readable schedule description"},
                "recurring": {"type": "boolean"},
                "durable": {"type": "boolean"}
            }
        }))
    }

    fn should_defer(&self) -> bool { true }
```

Then add per-field cron validation to `call()`. Replace the current validation block (which only checks field count) with:

```rust
        // Validate 5-field cron expression with per-field constraints
        let fields: Vec<&str> = cron.split_whitespace().collect();
        if fields.len() != 5 {
            return ToolResult {
                content: "Invalid cron expression: must have exactly 5 space-separated fields (minute hour day-of-month month day-of-week)".into(),
                is_error: true,
                ..Default::default()
            };
        }

        // Validate each field's domain
        fn valid_field(field: &str, min: i32, max: i32) -> bool {
            // Accept wildcards, steps, lists, ranges
            if field == "*" { return true; }
            for part in field.split(',') {
                let (part, _step) = match part.split_once('/') {
                    Some((p, s)) => (p, Some(s)),
                    None => (part, None),
                };
                let (lo, hi) = match part.split_once('-') {
                    Some((l, h)) => (l, h),
                    None => (part, part),
                };
                for val in [lo, hi] {
                    if let Ok(n) = val.parse::<i32>() {
                        if n < min || n > max { return false; }
                    }
                }
            }
            true
        }

        let field_constraints = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
        let field_names = ["minute", "hour", "day-of-month", "month", "day-of-week"];
        for (i, field) in fields.iter().enumerate() {
            let (min, max) = field_constraints[i];
            if !valid_field(field, min, max) {
                return ToolResult {
                    content: format!("Invalid cron field '{}': {} must be in range {}-{}", field_names[i], field, min, max),
                    is_error: true,
                    ..Default::default()
                };
            }
        }
```

### Step 3: Update the response to include structured fields

Update the success response in `call()` to include the human-readable schedule:

```rust
        let human = format!("cron: {cron} recurring: {recurring} durable: {durable}");

        ToolResult {
            content: json!({
                "id": id,
                "cron": cron,
                "humanSchedule": human,
                "recurring": recurring,
                "durable": durable
            }).to_string(),
            is_error: false,
            ..Default::default()
        }
```

### Commit

```
fix(parity): port CronCreate prompt, add outputSchema and field validation

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
```

---

## Task 9: CronDelete — OutputSchema + Prompt

**Files:**
- Modify: `cli/src/tools/cron_delete.rs` (add `output_schema()`, `should_defer()`)
- Modify: `cli/src/tools/prompts/cron_delete.txt` (port prompt)

### Step 1: Write the prompt file

Replace `cli/src/tools/prompts/cron_delete.txt`:

```bash
cat > cli/src/tools/prompts/cron_delete.txt << 'PROMPTEOF'
Cancel a cron job previously scheduled with CronCreate. Removes it from the in-memory session store.
PROMPTEOF
```

### Step 2: Add `output_schema()` and `should_defer()`

In `cli/src/tools/cron_delete.rs`, add after `input_schema()`:

```rust
    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "ID of the deleted job"}
            }
        }))
    }

    fn should_defer(&self) -> bool { true }
```

### Commit

```
fix(parity): port CronDelete prompt and add outputSchema

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
```

---

## Task 10: CronList — OutputSchema + Prompt

**Files:**
- Modify: `cli/src/tools/cron_list.rs` (add `output_schema()`, `should_defer()`)
- Modify: `cli/src/tools/prompts/cron_list.txt` (port prompt)

### Step 1: Write the prompt file

Replace `cli/src/tools/prompts/cron_list.txt`:

```bash
cat > cli/src/tools/prompts/cron_list.txt << 'PROMPTEOF'
List all cron jobs scheduled via CronCreate in this session.
PROMPTEOF
```

### Step 2: Add `output_schema()` and `should_defer()`

In `cli/src/tools/cron_list.rs`, add after `input_schema()`:

```rust
    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "jobs": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string"},
                            "cron": {"type": "string"},
                            "prompt": {"type": "string"},
                            "recurring": {"type": "boolean"},
                            "durable": {"type": "boolean"}
                        }
                    }
                }
            }
        }))
    }

    fn should_defer(&self) -> bool { true }
```

### Commit

```
fix(parity): port CronList prompt and add outputSchema

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
```

---

## Task 11: StructuredOutput — Schema Validation + OutputSchema + Prompt

**Why:** Currently just returns the input value as-is with no validation. The Claude Code version uses `ajv` to compile and validate the input against an optional JSON schema. Add basic validation, `output_schema()`, and the prompt.

**Files:**
- Modify: `cli/src/tools/structured_output.rs` (add schema validation in `call()`, `output_schema()`)
- Modify: `cli/src/tools/prompts/structured_output.txt` (port prompt)

### Step 1: Write the prompt file

Replace `cli/src/tools/prompts/structured_output.txt`:

```bash
cat > cli/src/tools/prompts/structured_output.txt << 'PROMPTEOF'
Use this tool to return your final response in the requested structured format. You MUST call this tool exactly once at the end of your response to provide the structured output.
PROMPTEOF
```

### Step 2: Add schema validation and `output_schema()`

The current `call()` just returns the input value. Add validation: if `schema` is provided, validate `value` against it and return an error on mismatch.

```rust
// At the top of the file, add:
use serde_json::json;

// Replace the existing call() with:
    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let value = match input.get("value") {
            Some(v) => v.clone(),
            None => {
                return ToolResult {
                    content: "StructuredOutput: no value provided".into(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        // If schema is provided, validate the value against it
        if let Some(schema) = input.get("schema") {
            match validate_against_schema(&value, schema) {
                Ok(()) => {}
                Err(e) => {
                    return ToolResult {
                        content: format!("Output does not match required schema: {e}"),
                        is_error: true,
                        ..Default::default()
                    };
                }
            }
        }

        ToolResult {
            content: value.to_string(),
            is_error: false,
            ..Default::default()
        }
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "string",
            "description": "Structured output tool result"
        }))
    }
```

Add the validation helper at the bottom of the file:

```rust
/// Basic JSON Schema validation using the `jsonschema` crate.
/// Returns Ok(()) if the value matches the schema, or Err with a description.
fn validate_against_schema(value: &serde_json::Value, schema: &serde_json::Value) -> Result<(), String> {
    // Use the jsonschema crate for proper validation
    match jsonschema::validator_for(schema) {
        Ok(validator) => {
            let mut errors = validator.iter_errors(value);
            if let Some(first) = errors.next() {
                let mut msg = format!("{}: {}", first.instance_path, first);
                for e in errors.take(9) {
                    msg.push_str(&format!("; {}: {}", e.instance_path, e));
                }
                if validator.iter_errors(value).count() > 10 {
                    msg.push_str("; ...");
                }
                Err(msg)
            } else {
                Ok(())
            }
        }
        Err(e) => Err(format!("Invalid JSON schema: {e}")),
    }
}
```

Note: This requires adding `jsonschema` to `cli/Cargo.toml`:

```bash
cargo add jsonschema --package super-cli
```

### Step 3: Verify build

```bash
cargo build -p super-cli 2>&1 | tail -5
```

### Commit

```
fix(parity): port StructuredOutput prompt, add schema validation

Validate input values against optional JSON schemas using the
jsonschema crate. Reject mismatches with error detail.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
```

---

## Final Checklist

After all 11 tasks are complete:

```bash
# Build check
cargo build -p super-cli 2>&1 | grep -E "error|warning"
# Expected: clean (no errors)

# Test suite
cargo test -p super-cli --lib 2>&1 | tail -20
# Expected: all tests pass

# Verify all prompt files are non-TODO
grep -rn "TODO(parity:batch-5)" cli/src/tools/prompts/
# Expected: empty — no remaining TODO stubs

# Verify all 11 tools have output_schema (where applicable)
grep -rn "fn output_schema" cli/src/tools/tool_search.rs cli/src/tools/monitor.rs cli/src/tools/lsp.rs cli/src/tools/config_tool.rs cli/src/tools/sleep.rs cli/src/tools/cron_create.rs cli/src/tools/cron_delete.rs cli/src/tools/cron_list.rs cli/src/tools/structured_output.rs
# Expected: 9 hits (EnterWorktree and ExitWorktree stub are exempt — they don't need output schemas as stubs)

# Verify ToolSearch is_registered with registry back-reference
grep -A3 "ToolSearchTool {" cli/src/tools/mod.rs
# Expected: shows `registry:` field being passed
```

---

## Commit Plan

All changes in one commit:

```
fix(parity): Batch 5 — long-tail tool prompt/schema/outputSchema parity

Implement real ToolSearch with registry-backed search (P0). Port
prompts for all 11 tools from claude-code-src. Add output_schema()
to Monitor, ConfigTool, Sleep, CronCreate, CronDelete, CronList,
LSP, and StructuredOutput. Fix ConfigTool is_read_only() for writes.
Add cron field validation to CronCreate. Add JSON schema validation
to StructuredOutput using the jsonschema crate.

Tools: ToolSearch, Monitor, EnterWorktree, ExitWorktree, LSP,
ConfigTool, Sleep, CronCreate, CronDelete, CronList, StructuredOutput

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
```
