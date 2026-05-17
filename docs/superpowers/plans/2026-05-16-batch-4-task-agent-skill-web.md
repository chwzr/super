# Batch 4 — Task, Agent, Skill & Web Tools Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the 6 task-management tools (TaskCreate, TaskGet, TaskList, TaskOutput, TaskStop, TaskUpdate) and the 4 specialized tools (Agent, Skill, WebFetch, WebSearch) to full Claude Code parity — output schemas, schema field expansions, prompt ports, and behavioral fixes.

**Architecture:** TaskRecord in state/store.rs gets 3 new Option fields (active_form, owner, metadata). TaskCreate wires activeForm into the record. TaskUpdate gets a massive 8-field expansion. WebSearch gets domain filtering. WebFetch gets a preapproved URL module. Every tool gets output_schema(). All 10 prompts ported from claude-code-src.

**Tech Stack:** Rust, serde_json, uuid, tokio, shared::RenderSpec (Batch 1).

**Prerequisite:** Batch 1 must be complete. Batch 4 is independent of Batches 2 and 3.

---

## File Map

### New files
| Path | Purpose |
|------|---------|
| cli/src/tools/web_fetch_preapproved.rs | Preapproved URL host list |

### Modified files
| Path | Change |
|------|--------|
| cli/src/state/store.rs | Add active_form, owner, metadata to TaskRecord |
| cli/src/tools/task_create.rs | Add metadata to schema, wire activeForm, output_schema |
| cli/src/tools/task_get.rs | output_schema |
| cli/src/tools/task_list.rs | output_schema |
| cli/src/tools/task_output.rs | Fix field name, add block/timeout, output_schema |
| cli/src/tools/task_stop.rs | Add shell_id alias, output_schema |
| cli/src/tools/task_update.rs | 8-field expansion, full call() wiring, output_schema |
| cli/src/tools/agent.rs | output_schema (discriminated union), searchHint |
| cli/src/tools/skill.rs | output_schema |
| cli/src/tools/web_fetch.rs | Prompt-application, preapproved module, output_schema |
| cli/src/tools/web_search.rs | Domain filtering, output_schema |
| cli/src/tools/mod.rs | Add web_fetch_preapproved module |

### Prompt files replaced (10 files)
| Path | Claude source |
|------|---------------|
| prompts/task_create.txt | TaskCreateTool/prompt.ts |
| prompts/task_get.txt | TaskGetTool/prompt.ts |
| prompts/task_list.txt | TaskListTool/prompt.ts |
| prompts/task_output.txt | TaskOutputTool/prompt.ts |
| prompts/task_stop.txt | TaskStopTool/prompt.ts |
| prompts/task_update.txt | TaskUpdateTool/prompt.ts |
| prompts/agent.txt | AgentTool/prompt.ts |
| prompts/skill.txt | SkillTool/prompt.ts |
| prompts/web_fetch.txt | WebFetchTool/prompt.ts |
| prompts/web_search.txt | WebSearchTool/prompt.ts |

---

### Task 1: Expand TaskRecord in state/store.rs

**Files:**
- Modify: cli/src/state/store.rs

- [ ] **Step 1: Add new fields to TaskRecord**

Change the TaskRecord struct to add active_form, owner, and metadata as Option fields:

```rust
#[derive(Clone, Default)]
pub struct TaskRecord {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub active_form: Option<String>,
    pub status: TaskStatus,
    pub owner: Option<String>,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
    pub metadata: Option<serde_json::Value>,
}
```

The existing Default derive handles the new Option fields (defaults to None).

- [ ] **Step 2: Build to verify struct compiles**

Run: cargo check -p cli 2>&1 | head -20

Expected: errors only in task_create.rs and task_update.rs (struct literal sites need the new fields). No other file should break.

- [ ] **Step 3: Commit**

```
git add cli/src/state/store.rs
git commit -m "feat(store): add activeForm, owner, metadata fields to TaskRecord"
```

---

### Task 2: TaskCreate — metadata + activeForm + output_schema

**Files:**
- Modify: cli/src/tools/task_create.rs
- Replace: cli/src/tools/prompts/task_create.txt

- [ ] **Step 1: Port the prompt text**

Replace prompts/task_create.txt with the prompt from ../claude-code-src/tools/TaskCreateTool/prompt.ts (the PROMPT constant). Skip the isAgentSwarmsEnabled() conditional team sections.

- [ ] **Step 2: Update input_schema to include metadata**

Add the metadata field to the existing input_schema properties:

```rust
fn input_schema(&self) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "subject": {"type": "string", "description": "A brief title for the task"},
            "description": {"type": "string", "description": "What needs to be done"},
            "activeForm": {"type": "string", "description": "Present continuous form shown in spinner when in_progress"},
            "metadata": {"type": "object", "description": "Arbitrary metadata to attach to the task"}
        },
        "required": ["subject", "description"]
    })
}
```

- [ ] **Step 3: Wire activeForm into TaskRecord in call()**

In call(), read the activeForm field and set it on the record:

```rust
let active_form = input.get("activeForm").and_then(|v| v.as_str()).map(String::from);
let meta = input.get("metadata").cloned();

let task = TaskRecord {
    id: uuid::Uuid::new_v4().to_string(),
    subject: subject.clone(),
    description: description.clone(),
    active_form,
    status: TaskStatus::Pending,
    owner: None,
    blocks: vec![],
    blocked_by: vec![],
    metadata: meta,
};
```

- [ ] **Step 4: Add output_schema()**

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(json!({
        "type": "object",
        "properties": {
            "task": {
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "subject": {"type": "string"}
                }
            }
        },
        "required": ["task"]
    }))
}
```

- [ ] **Step 5: Update call() to return structured JSON output**

```rust
ToolResult {
    content: json!({"task": {"id": task.id, "subject": task.subject}}).to_string(),
    is_error: false,
    ..Default::default()
}
```

- [ ] **Step 6: Compile check and commit**

Run: cargo check -p cli 2>&1

```
git add cli/src/tools/task_create.rs cli/src/tools/prompts/task_create.txt
git commit -m "feat(parity): add metadata, activeForm, and output_schema to TaskCreate"
```

---

### Task 3: TaskGet — output_schema + prompt

**Files:**
- Modify: cli/src/tools/task_get.rs
- Replace: cli/src/tools/prompts/task_get.txt

- [ ] **Step 1: Port prompt**

Replace prompts/task_get.txt with ../claude-code-src/tools/TaskGetTool/prompt.ts PROMPT text.

- [ ] **Step 2: Add output_schema()**

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(json!({
        "type": ["object", "null"],
        "properties": {
            "id": {"type": "string"},
            "subject": {"type": "string"},
            "description": {"type": "string"},
            "status": {"type": "string"},
            "blocks": {"type": "array", "items": {"type": "string"}},
            "blockedBy": {"type": "array", "items": {"type": "string"}}
        }
    }))
}
```

- [ ] **Step 3: Commit**

```
git add cli/src/tools/task_get.rs cli/src/tools/prompts/task_get.txt
git commit -m "feat(parity): add output_schema and prompt to TaskGet"
```

---

### Task 4: TaskList — output_schema + prompt

**Files:**
- Modify: cli/src/tools/task_list.rs
- Replace: cli/src/tools/prompts/task_list.txt

- [ ] **Step 1: Port prompt**

Replace prompts/task_list.txt with ../claude-code-src/tools/TaskListTool/prompt.ts getPrompt() text.

- [ ] **Step 2: Add output_schema()**

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "subject": {"type": "string"},
                "status": {"type": "string"},
                "owner": {"type": "string"},
                "blockedBy": {"type": "array", "items": {"type": "string"}}
            }
        }
    }))
}
```

- [ ] **Step 3: Commit**

```
git add cli/src/tools/task_list.rs cli/src/tools/prompts/task_list.txt
git commit -m "feat(parity): add output_schema and prompt to TaskList"
```

---

### Task 5: TaskOutput — block/timeout fields + prompt

**Files:**
- Modify: cli/src/tools/task_output.rs
- Replace: cli/src/tools/prompts/task_output.txt

- [ ] **Step 1: Port prompt**

Replace prompts/task_output.txt with ../claude-code-src/tools/TaskOutputTool/prompt.ts text.

- [ ] **Step 2: Add block and timeout to input_schema**

Change input field name to match Claude (task_id, not taskId) and add block/timeout:

```rust
fn input_schema(&self) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string", "description": "The task ID to get output from"},
            "block": {"type": "boolean", "default": true, "description": "Whether to wait for completion"},
            "timeout": {"type": "integer", "minimum": 0, "maximum": 600000, "default": 30000, "description": "Max wait time in ms"}
        },
        "required": ["task_id", "block", "timeout"]
    })
}
```

- [ ] **Step 3: Add output_schema()**

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string"},
            "status": {"type": "string"},
            "output": {"type": "string"}
        }
    }))
}
```

- [ ] **Step 4: Commit**

```
git add cli/src/tools/task_output.rs cli/src/tools/prompts/task_output.txt
git commit -m "feat(parity): add block, timeout fields and output_schema to TaskOutput"
```

---

### Task 6: TaskStop — shell_id alias + prompt

**Files:**
- Modify: cli/src/tools/task_stop.rs
- Replace: cli/src/tools/prompts/task_stop.txt

- [ ] **Step 1: Port prompt**

Replace prompts/task_stop.txt with ../claude-code-src/tools/TaskStopTool/prompt.ts DESCRIPTION.

- [ ] **Step 2: Add shell_id as deprecated alias in input_schema**

```rust
fn input_schema(&self) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string", "description": "The ID of the background task to stop"},
            "shell_id": {"type": "string", "description": "Deprecated: use task_id instead"}
        }
    })
}
```

- [ ] **Step 3: Update call() to accept either field**

```rust
let task_id = input.get("task_id")
    .or_else(|| input.get("shell_id"))
    .and_then(|v| v.as_str())
    .unwrap_or("");
```

- [ ] **Step 4: Add output_schema()**

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(json!({
        "type": "object",
        "properties": {
            "message": {"type": "string"},
            "task_id": {"type": "string"},
            "task_type": {"type": "string"}
        }
    }))
}
```

- [ ] **Step 5: Commit**

```
git add cli/src/tools/task_stop.rs cli/src/tools/prompts/task_stop.txt
git commit -m "feat(parity): add shell_id alias and output_schema to TaskStop"
```

---

### Task 7: TaskUpdate — massive 8-field expansion

**Files:**
- Modify: cli/src/tools/task_update.rs
- Replace: cli/src/tools/prompts/task_update.txt

- [ ] **Step 1: Port prompt**

Replace prompts/task_update.txt with ../claude-code-src/tools/TaskUpdateTool/prompt.ts PROMPT text.

- [ ] **Step 2: Expand input_schema with all missing fields**

Replace the current schema (which only has taskId + status) with the full schema that includes subject, description, activeForm, owner, addBlocks, addBlockedBy, metadata, and deleted status:

```rust
fn input_schema(&self) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "taskId": {"type": "string"},
            "subject": {"type": "string", "description": "New subject for the task"},
            "description": {"type": "string", "description": "New description for the task"},
            "activeForm": {"type": "string", "description": "Present continuous form shown in spinner when in_progress"},
            "status": {
                "type": "string",
                "enum": ["pending", "in_progress", "completed", "deleted"]
            },
            "owner": {"type": "string", "description": "New owner for the task"},
            "addBlocks": {"type": "array", "items": {"type": "string"}, "description": "Task IDs that this task blocks"},
            "addBlockedBy": {"type": "array", "items": {"type": "string"}, "description": "Task IDs that block this task"},
            "metadata": {"type": "object", "description": "Metadata keys to merge into the task"}
        },
        "required": ["taskId"]
    })
}
```

- [ ] **Step 3: Rewrite call() to update all fields**

Replace the call() method to handle ALL the new optional fields:

```rust
async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
    let task_id = input["taskId"].as_str().unwrap_or("");
    let mut updated_fields: Vec<String> = Vec::new();

    let mut store = self.store.write().unwrap();
    let task = match store.tasks.get_mut(task_id) {
        Some(t) => t,
        None => return ToolResult {
            content: format!("Task not found: {}", task_id),
            is_error: true,
            ..Default::default()
        },
    };

    if let Some(v) = input.get("subject").and_then(|v| v.as_str()) {
        task.subject = v.to_string();
        updated_fields.push("subject".into());
    }
    if let Some(v) = input.get("description").and_then(|v| v.as_str()) {
        task.description = v.to_string();
        updated_fields.push("description".into());
    }
    if let Some(v) = input.get("activeForm").and_then(|v| v.as_str()) {
        task.active_form = Some(v.to_string());
        updated_fields.push("activeForm".into());
    }
    if let Some(v) = input.get("status").and_then(|v| v.as_str()) {
        task.status = match v {
            "in_progress" => TaskStatus::InProgress,
            "completed" => TaskStatus::Completed,
            "deleted" => TaskStatus::Deleted,
            _ => TaskStatus::Pending,
        };
        updated_fields.push("status".into());
    }
    if let Some(v) = input.get("owner").and_then(|v| v.as_str()) {
        task.owner = Some(v.to_string());
        updated_fields.push("owner".into());
    }
    if let Some(arr) = input.get("addBlocks").and_then(|v| v.as_array()) {
        for id in arr {
            if let Some(s) = id.as_str() {
                task.blocks.push(s.to_string());
            }
        }
        updated_fields.push("addBlocks".into());
    }
    if let Some(arr) = input.get("addBlockedBy").and_then(|v| v.as_array()) {
        for id in arr {
            if let Some(s) = id.as_str() {
                task.blocked_by.push(s.to_string());
            }
        }
        updated_fields.push("addBlockedBy".into());
    }
    if let Some(meta) = input.get("metadata").cloned() {
        task.metadata = Some(meta);
        updated_fields.push("metadata".into());
    }

    drop(store);

    ToolResult {
        content: json!({
            "success": true,
            "taskId": task_id,
            "updatedFields": updated_fields
        }).to_string(),
        is_error: false,
        ..Default::default()
    }
}
```

Note: This uses `self.store.write().unwrap()` which requires the Store to use RwLock instead of Mutex. If the current store uses Mutex, change `.write()` to `.lock()`.

- [ ] **Step 4: Add output_schema()**

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(json!({
        "type": "object",
        "properties": {
            "success": {"type": "boolean"},
            "taskId": {"type": "string"},
            "updatedFields": {"type": "array", "items": {"type": "string"}},
            "error": {"type": "string"},
            "statusChange": {"type": "string"}
        },
        "required": ["success", "taskId"]
    }))
}
```

- [ ] **Step 5: Compile check — fix any store API mismatches**

Run: cargo check -p cli 2>&1

If the Store uses `std::sync::Mutex` instead of `tokio::sync::RwLock`, adjust `.write().unwrap()` to `.lock().unwrap()`. Fix any other compilation errors.

- [ ] **Step 6: Commit**

```
git add cli/src/tools/task_update.rs cli/src/tools/prompts/task_update.txt
git commit -m "feat(parity): expand TaskUpdate with 8 new fields and output_schema"
```

---

### Task 8: Agent (Task) — output_schema + searchHint + prompt

**Files:**
- Modify: cli/src/tools/agent.rs
- Replace: cli/src/tools/prompts/agent.txt

- [ ] **Step 1: Port prompt**

Replace prompts/agent.txt with ../claude-code-src/tools/AgentTool/prompt.ts getPrompt() text. Use the non-swarm path since Super has no coordinator mode yet.

- [ ] **Step 2: Add searchHint**

```rust
fn search_hint(&self) -> Option<&'static str> {
    Some("launch a new agent to handle complex, multi-step tasks autonomously")
}
```

- [ ] **Step 3: Add output_schema() — discriminated union**

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "status": {"const": "completed"},
                    "prompt": {"type": "string"}
                },
                "required": ["status", "prompt"]
            },
            {
                "type": "object",
                "properties": {
                    "status": {"const": "async_launched"},
                    "agentId": {"type": "string"},
                    "prompt": {"type": "string"}
                },
                "required": ["status", "agentId", "prompt"]
            }
        ]
    }))
}
```

- [ ] **Step 4: Commit**

```
git add cli/src/tools/agent.rs cli/src/tools/prompts/agent.txt
git commit -m "feat(parity): add output_schema, searchHint, and prompt to Agent tool"
```

---

### Task 9: Skill — output_schema + prompt

**Files:**
- Modify: cli/src/tools/skill.rs
- Replace: cli/src/tools/prompts/skill.txt

- [ ] **Step 1: Port prompt**

Replace prompts/skill.txt with ../claude-code-src/tools/SkillTool/prompt.ts getPrompt() text.

- [ ] **Step 2: Add output_schema()**

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "inline": {"type": "boolean", "const": true},
                    "message": {"type": "string"}
                },
                "required": ["inline", "message"]
            },
            {
                "type": "object",
                "properties": {
                    "forked": {"type": "boolean", "const": true},
                    "message": {"type": "string"}
                },
                "required": ["forked", "message"]
            }
        ]
    }))
}
```

- [ ] **Step 3: Commit**

```
git add cli/src/tools/skill.rs cli/src/tools/prompts/skill.txt
git commit -m "feat(parity): add output_schema and prompt to Skill tool"
```

---

### Task 10: WebSearch — domain filtering + output_schema + prompt

**Files:**
- Modify: cli/src/tools/web_search.rs
- Replace: cli/src/tools/prompts/web_search.txt

- [ ] **Step 1: Port prompt**

Replace prompts/web_search.txt with ../claude-code-src/tools/WebSearchTool/prompt.ts getWebSearchPrompt() text.

- [ ] **Step 2: Implement domain filtering in call()**

After fetching results from DuckDuckGo, apply the allowed_domains and blocked_domains filters. The fields are parsed from the input but currently ignored:

```rust
let allowed: Option<Vec<&str>> = input.get("allowed_domains")
    .and_then(|v| v.as_array())
    .map(|a| a.iter().filter_map(|v| v.as_str()).collect());

let blocked: Option<Vec<&str>> = input.get("blocked_domains")
    .and_then(|v| v.as_array())
    .map(|a| a.iter().filter_map(|v| v.as_str()).collect());

// After collecting results, filter:
let results: Vec<_> = results.into_iter().filter(|r| {
    if let Some(allow) = &allowed {
        if !allow.iter().any(|domain| r.url.contains(domain)) {
            return false;
        }
    }
    if let Some(block) = &blocked {
        if block.iter().any(|domain| r.url.contains(domain)) {
            return false;
        }
    }
    true
}).collect();
```

- [ ] **Step 3: Add output_schema()**

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(json!({
        "type": "object",
        "properties": {
            "query": {"type": "string"},
            "results": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "url": {"type": "string"},
                        "snippet": {"type": "string"}
                    }
                }
            },
            "durationMs": {"type": "number"}
        },
        "required": ["query", "results"]
    }))
}
```

- [ ] **Step 4: Return structured output**

In call(), return structured JSON instead of plain text:

```rust
let start = std::time::Instant::now();
// ... fetch and filter ...
let duration_ms = start.elapsed().as_millis() as f64;

ToolResult {
    content: json!({
        "query": query,
        "results": results,
        "durationMs": duration_ms
    }).to_string(),
    is_error: false,
    ..Default::default()
}
```

- [ ] **Step 5: Commit**

```
git add cli/src/tools/web_search.rs cli/src/tools/prompts/web_search.txt
git commit -m "feat(parity): implement domain filtering, output_schema for WebSearch"
```

---

### Task 11: WebFetch — preapproved module + prompt-application + output_schema

**Files:**
- Create: cli/src/tools/web_fetch_preapproved.rs
- Modify: cli/src/tools/web_fetch.rs
- Replace: cli/src/tools/prompts/web_fetch.txt
- Modify: cli/src/tools/mod.rs

- [ ] **Step 1: Port prompt**

Replace prompts/web_fetch.txt with ../claude-code-src/tools/WebFetchTool/prompt.ts text.

- [ ] **Step 2: Create preapproved URL list module**

Create cli/src/tools/web_fetch_preapproved.rs:

```rust
/// Preapproved URL hosts for WebFetch. Claude Code has a list of trusted
/// hosts that bypass the permission check. Super starts with an empty list;
/// hosts can be added as needed.

pub fn is_preapproved(_url: &str) -> bool {
    // TODO: populate from Claude Code's WebFetchTool/preapproved.ts
    // Currently empty — all URLs require permission approval
    false
}
```

- [ ] **Step 3: Add module declaration in mod.rs**

Add to cli/src/tools/mod.rs:
```rust
pub mod web_fetch_preapproved;
```

- [ ] **Step 4: Add output_schema() to WebFetch**

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(json!({
        "type": "object",
        "properties": {
            "bytes": {"type": "integer"},
            "code": {"type": "integer"},
            "codeText": {"type": "string"},
            "result": {"type": "string", "description": "AI-processed result of the fetch"},
            "durationMs": {"type": "number"},
            "url": {"type": "string"}
        },
        "required": ["bytes", "code", "result", "durationMs", "url"]
    }))
}
```

- [ ] **Step 5: Wire prompt-application note**

In call(), after fetching and stripping HTML from the URL content, include both the raw stripped text and the user's prompt in the result. Since Super doesn't have a small local model for content summarization yet, note that full LLM processing of the prompt against fetched content is deferred:

```rust
// In call(), after content is fetched and stripped:
let prompt = input["prompt"].as_str().unwrap_or("");
let start = std::time::Instant::now();
// ... fetch and strip content ...
let byte_count = content.len();
let duration_ms = start.elapsed().as_millis() as f64;

ToolResult {
    content: json!({
        "url": url,
        "bytes": byte_count,
        "code": 200,
        "codeText": "OK",
        "result": format!("Content fetched ({} bytes). Prompt '{}' will be processed in a follow-up.", byte_count, prompt),
        "durationMs": duration_ms
    }).to_string(),
    is_error: false,
    ..Default::default()
}
```

- [ ] **Step 6: Commit**

```
git add cli/src/tools/web_fetch.rs cli/src/tools/prompts/web_fetch.txt cli/src/tools/web_fetch_preapproved.rs cli/src/tools/mod.rs
git commit -m "feat(parity): add preapproved module, output_schema, and prompt to WebFetch"
```

---

## Acceptance Criteria

1. All 10 tools compile against the extended Tool trait: `cargo check -p cli`
2. All 10 prompts match Claude byte-for-byte: verified by parity harness
3. TaskCreate call() writes activeForm + metadata to the store
4. TaskUpdate call() updates subject, description, activeForm, owner, addBlocks, addBlockedBy, metadata
5. TaskUpdate supports status "deleted"
6. TaskStop accepts both task_id and shell_id
7. WebSearch filters results by allowed_domains / blocked_domains
8. TaskRecord in the store has active_form, owner, metadata fields with Default
