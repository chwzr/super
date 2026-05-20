# Message Queue + Monitor Tool Design

## Summary

Add a centralized message queue (parity with Claude Code's `messageQueueManager`) and wire all existing background-result paths into it. Implement the Monitor tool to stream stdout lines from long-running scripts as conversation notifications the model sees mid-turn.

## Architecture

```
MessageQueue (Arc<Mutex<Vec<QueuedCommand>>>)
  |
  ├── Writers enqueue:
  │   ├── Monitor: each stdout line → TaskNotification(Next)
  │   ├── Monitor: on exit → TaskNotification(Next)
  │   ├── Bash bg: on exit → TaskNotification(Next)
  │   ├── Agent bg: on exit → TaskNotification(Later)
  │   ├── Cron runtime: on fire → Prompt(Later, is_meta)
  │   └── TUI mid-turn input → Prompt(Next)
  │
  └── Readers drain:
      ├── Engine loop (before each API call): drain Next (or Later if Sleep ran)
      ├── TUI queue processor (when idle): drain all main-thread
      └── Abort-on-Now: subscriber checks for Now-priority interrupts
```

## Component 1: `message_queue.rs` (NEW)

### Types

```rust
enum QueuePriority { Now = 0, Next = 1, Later = 2 }

enum PromptInputMode { Prompt, Bash, OrphanedPermission, TaskNotification }

struct QueuedCommand {
    value: String,           // notification body (XML for task-notification)
    mode: PromptInputMode,
    priority: QueuePriority,
    agent_id: Option<String>, // None = main thread
    is_meta: bool,
    uuid: Uuid,
}
```

### Operations

- `enqueue(cmd)` — push with given priority/mode
- `enqueue_pending_notification(value, mode, priority, agent_id)` — convenience for task-notification
- `drain(max_priority, agent_filter)` — return all commands matching priority threshold and agent scope, remove from queue
- `remove_by_filter(predicate)` — selective removal (ESC cancel, session backgrounding)
- `clear()` — empty the queue
- `has_pending(max_priority)` — check without draining

### Location

Stored as `Arc<MessageQueue>` on `ConversationEngine`. Shared via clone to tools (through `ToolCallContext`), the TUI, and the cron runtime. Not a global static — Arc handles sharing.

## Component 2: Engine Loop Drain (MODIFY `engine.rs`)

At the top of each loop iteration, before `build_request_body()`:

1. Determine max priority: `Later` if any tool_use this turn was `Sleep`, otherwise `Next`
2. Determine agent filter: main thread drains `agent_id == None`, subagents drain `agent_id == current_agent_id`
3. Call `queue.drain(max_priority, agent_filter)`
4. Convert each `task-notification` command value into `ContentBlockFinal::Text` blocks
5. Push into `history` so the model sees them this turn
6. Emit on session bus so TUI renders them

Also track whether Sleep was called in the current tool-use batch via a boolean flag, matching Claude Code's `sleepRan` variable.

## Component 3: Monitor Tool (REWRITE `monitor.rs`)

### Input schema (unchanged)

`command`, `description`, `timeout_ms`, `persistent` — matches current definition and Claude Code.

### call() implementation

1. Spawn child process via `tokio::process::Command`, pipe stdout
2. Wrap stdout in `BufReader::lines()`
3. For each line: enqueue `TaskNotification` with priority `Next` and current agent_id
4. Return immediately with `{ task_id, acknowledged: true }`
5. Spawned task continues: on process exit, enqueue completion/failure notification
6. Completion summary: `"Monitor \"{description}\" stream ended"` (not the bash prefix, to avoid collapsing)
7. If `persistent: false` (default), monitor stops when the current session ends or on timeout

### Notification format

Each line: text string (not XML-wrapped — just the line content). Line notifications are plain text injected as user content so the model reads them naturally.

Completion: XML-wrapped like bash/agent notifications:
```xml
<task-notification>
  <task-id>{id}</task-id>
  <output-file>{path}</output-file>
  <status>completed|failed</status>
  <summary>Monitor "{description}" stream ended</summary>
</task-notification>
```
This matches Claude Code's distinction: streaming lines are content, completion is a structured notification.

### Output file

Monitor writes all stdout to `~/.super/tasks/{session_id}/{task_id}.output` for later retrieval via `TaskOutput`. Stderr is discarded (per Claude Code prompt: "Only stdout is the event stream").

### Struct change

```rust
pub struct MonitorTool {
    pub queue: Arc<MessageQueue>,
}
```

## Component 4: Background Bash Tasks (MODIFY `bash.rs`)

### Current behavior

`run_in_background` spawns a process and drops the result. No model notification.

### New behavior

1. `BashTool` holds `queue: Arc<MessageQueue>`
2. On `run_in_background`: spawn process with stdout/stderr piped, write to a temp output file at `~/.super/tasks/{session_id}/{task_id}.output`
3. On process exit: enqueue `TaskNotification` with priority `Next`:
   - Summary: `"Background bash \"{description}\" completed (exit code {code})"`
   - Include output file path for `TaskOutput` to read
4. The notification format matches Claude Code's `enqueueShellNotification`: XML with `<task-notification>`, `<task-id>`, `<output-file>`, `<status>`, `<summary>` tags
5. Output file path uses `~/.super/tasks/{session_id}/` — the session_id comes from the ToolCallContext's bus

### Concurrent background tasks

Multiple background tasks can run concurrently. Each gets a unique task_id (UUID). The `TaskOutput` tool reads output by task_id. The `TaskStop` tool kills by task_id. Task records are registered in the Store so `TaskList` shows them.

### Struct change

```rust
pub struct BashTool {
    pub queue: Arc<MessageQueue>,
    pub store: Arc<Store>,          // NEW — to register task records
    pub session_id: String,         // NEW — for output path construction
}
```

## Component 5: Background Agents (MODIFY `agent.rs`)

### Current behavior

Async agent emits `BusMessage::SystemEvent { subtype: AsyncAgentDone }` on the bus. The TUI ignores this event type — the model is never notified.

### New behavior

1. `AgentTool` holds `queue: Arc<MessageQueue>`
2. In the async spawn completion handler: enqueue `TaskNotification` with priority `Later`:
   - Summary: `"Agent \"{description}\" completed"`
   - Include agent output file path (agent output is written to `~/.super/tasks/{session_id}/{agent_id}.output`)
3. Keep the bus emit for TUI progress display
4. Both paths run: bus event for TUI, queue for model notification
5. Agent task records are already registered in the Store via `register_async_agent()`

### Notification format

```xml
<task-notification>
  <task-id>{agent_id}</task-id>
  <output-file>{output_path}</output-file>
  <status>completed</status>
  <summary>Agent "{description}" completed</summary>
</task-notification>
```

On failure: status `failed`, summary includes error.

### Struct change

```rust
pub struct AgentTool {
    pub store: Arc<Store>,
    pub config: CliConfig,
    pub registry: Arc<AgentRegistry>,
    pub tool_registry: Arc<ToolRegistry>,
    pub queue: Arc<MessageQueue>,  // NEW
}
```

## Component 6: Cron Runtime (MODIFY `cron_create.rs`, NEW scheduler)

### Current behavior

`CronCreateTool` inserts jobs into `Arc<Mutex<HashMap>>`. No scheduler runs them.

### New behavior

1. New `cron_runtime.rs` — a background task spawned in `bootstrap.rs`
2. Runtime holds `Arc<Mutex<HashMap<String, CronJob>>>` and `Arc<MessageQueue>`
3. On each tick (every ~30s): parse cron expressions, find due jobs, enqueue prompts
4. Enqueued as `Prompt` mode, priority `Later`, `is_meta: true`
5. One-shot jobs: auto-delete after firing. Recurring: re-schedule.
6. `CronCreateTool`: registers job + notifies runtime to wake/reschedule
7. `CronDeleteTool`: marks job deleted + notifies runtime
8. `CronListTool`: unchanged (reads the same HashMap)

### Runtime lifecycle

Spawned in `bootstrap.rs` after queue creation. Runs for session lifetime. Uses `tokio::time::sleep` for scheduling, woken early via a `watch` channel when jobs change.

## Component 7: Mid-Turn User Input (MODIFY `app.rs`)

### Current behavior

`queued_prompts: VecDeque<String>` — if `inflight.is_some()`, push to deque. Dispatched when engine finishes.

### New behavior

1. TUI `App` holds `queue: Arc<MessageQueue>`
2. When user submits text while engine is running: enqueue as `Prompt` mode, priority `Next`
3. Engine loop drain picks it up on the next iteration — the model sees the new input mid-turn
4. Keep `queued_prompts` as fallback for dispatch when engine is not running
5. ESC abort: also call `queue.clear()` to discard pending notifications

## Component 8: ToolCallContext Extension (MODIFY `contract.rs`)

Add to `ToolCallContext`:

```rust
pub queue: Option<Arc<MessageQueue>>,
```

All tools that need to enqueue get it from context. Constructed by `tool_loop.rs` from the engine's queue.

## What Is NOT In Scope

- MCP channel notifications — transport is stubbed
- Generic hook system — intentionally out of scope per CLAUDE.md
- Permission orphans — no orphan concept exists
- Proactive/autonomous ticks — Sleep integration provides the foundation; full KAIROS mode is separate
- Slash command background execution — all slash commands are currently synchronous

## Files Changed

| File | Change | Description |
|------|--------|-------------|
| `cli/src/conversation/message_queue.rs` | NEW | Queue module with QueuedCommand, enqueue, drain |
| `cli/src/conversation/engine.rs` | MODIFY | Drain queue before API calls; carry queue field |
| `cli/src/conversation/mod.rs` | MODIFY | Add `mod message_queue` |
| `cli/src/conversation/tool_loop.rs` | MODIFY | Pass queue into ToolCallContext |
| `cli/src/tools/monitor.rs` | REWRITE | Real implementation with line streaming |
| `cli/src/tools/bash.rs` | MODIFY | Background completion → queue notification |
| `cli/src/tools/agent.rs` | MODIFY | Async completion → queue notification |
| `cli/src/tools/cron_create.rs` | MODIFY | Wires into cron runtime |
| `cli/src/tools/cron_delete.rs` | MODIFY | Notifies runtime of deletion |
| `cli/src/tools/mod.rs` | MODIFY | Wire queue into all tools that need it |
| `cli/src/tools/contract.rs` | MODIFY | Add queue field to ToolCallContext |
| `cli/src/conversation/cron_runtime.rs` | NEW | Background cron scheduler |
| `cli/src/conversation/mod.rs` | MODIFY | Add `mod cron_runtime` |
| `cli/src/bootstrap.rs` | MODIFY | Create queue, spawn cron runtime, pass to engine/tools |
| `cli/src/tui/app.rs` | MODIFY | Mid-turn input → queue; ESC clears queue |
| `cli/src/state/store.rs` | MODIFY | Add task output path helper |
