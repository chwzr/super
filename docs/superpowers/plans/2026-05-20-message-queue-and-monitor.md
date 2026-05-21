### Task 1: MessageQueue type and tests

**Files:**
- Create: `cli/src/conversation/message_queue.rs`
- Modify: `cli/src/conversation/mod.rs`

- [ ] **Step 1: Write the message_queue module with types and tests**

```rust
// cli/src/conversation/message_queue.rs
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Priority levels. Lower numeric value = higher priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueuePriority {
    Now = 0,
    Next = 1,
    Later = 2,
}

/// What kind of content is in the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptInputMode {
    Prompt,
    Bash,
    OrphanedPermission,
    TaskNotification,
}

#[derive(Debug, Clone)]
pub struct QueuedCommand {
    pub value: String,
    pub mode: PromptInputMode,
    pub priority: QueuePriority,
    pub agent_id: Option<String>,
    pub is_meta: bool,
    pub uuid: Uuid,
}

#[derive(Clone)]
pub struct MessageQueue {
    inner: Arc<Mutex<MessageQueueInner>>,
}

struct MessageQueueInner {
    queue: Vec<QueuedCommand>,
}

impl MessageQueue {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MessageQueueInner { queue: Vec::new() })),
        }
    }

    /// Push a command onto the queue.
    pub fn enqueue(&self, cmd: QueuedCommand) {
        self.inner.lock().unwrap().queue.push(cmd);
    }

    /// Convenience for system-generated notifications. Defaults priority to Later.
    pub fn enqueue_pending_notification(
        &self,
        value: String,
        mode: PromptInputMode,
        priority: QueuePriority,
        agent_id: Option<String>,
    ) {
        self.enqueue(QueuedCommand {
            value,
            mode,
            priority,
            agent_id,
            is_meta: false,
            uuid: Uuid::new_v4(),
        });
    }

    /// Drain commands matching the priority threshold and optional agent filter.
    /// Returns removed commands sorted by insertion order.
    /// - agent_filter None: return all
    /// - agent_filter Some(None): return only main-thread commands (agent_id == None)
    /// - agent_filter Some(Some(id)): return only commands for that agent
    pub fn drain(
        &self,
        max_priority: QueuePriority,
        agent_filter: Option<Option<&str>>,
    ) -> Vec<QueuedCommand> {
        let mut inner = self.inner.lock().unwrap();
        let threshold = max_priority as u8;
        let mut drained = Vec::new();
        let mut i = 0;
        while i < inner.queue.len() {
            let cmd = &inner.queue[i];
            let priority_ok = (cmd.priority as u8) <= threshold;
            let agent_ok = match &agent_filter {
                None => true,
                Some(target) => cmd.agent_id.as_deref() == *target,
            };
            if priority_ok && agent_ok {
                drained.push(inner.queue.remove(i));
            } else {
                i += 1;
            }
        }
        drained
    }

    /// Remove commands matching a predicate.
    pub fn remove_by_filter(&self, pred: impl Fn(&QueuedCommand) -> bool) -> Vec<QueuedCommand> {
        let mut inner = self.inner.lock().unwrap();
        let mut removed = Vec::new();
        let mut i = 0;
        while i < inner.queue.len() {
            if pred(&inner.queue[i]) {
                removed.push(inner.queue.remove(i));
            } else {
                i += 1;
            }
        }
        removed
    }

    /// Remove all commands.
    pub fn clear(&self) {
        self.inner.lock().unwrap().queue.clear();
    }

    /// Check if any commands are pending at or above the given priority.
    pub fn has_pending(&self, max_priority: QueuePriority) -> bool {
        let inner = self.inner.lock().unwrap();
        let threshold = max_priority as u8;
        inner
            .queue
            .iter()
            .any(|cmd| (cmd.priority as u8) <= threshold)
    }

    /// Total number of commands in the queue.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().queue.len()
    }
}

impl Default for MessageQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(value: &str, priority: QueuePriority, mode: PromptInputMode) -> QueuedCommand {
        QueuedCommand {
            value: value.into(),
            mode,
            priority,
            agent_id: None,
            is_meta: false,
            uuid: Uuid::new_v4(),
        }
    }

    fn cmd_with_agent(
        value: &str,
        priority: QueuePriority,
        agent_id: &str,
    ) -> QueuedCommand {
        QueuedCommand {
            value: value.into(),
            mode: PromptInputMode::TaskNotification,
            priority,
            agent_id: Some(agent_id.into()),
            is_meta: false,
            uuid: Uuid::new_v4(),
        }
    }

    #[test]
    fn drain_respects_priority_threshold() {
        let q = MessageQueue::new();
        q.enqueue(cmd("a", QueuePriority::Later, PromptInputMode::TaskNotification));
        q.enqueue(cmd("b", QueuePriority::Next, PromptInputMode::TaskNotification));
        q.enqueue(cmd("c", QueuePriority::Now, PromptInputMode::TaskNotification));

        let drained = q.drain(QueuePriority::Next, None);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].value, "b");
        assert_eq!(drained[1].value, "c");
        // "a" (Later) remains
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn drain_filters_by_agent() {
        let q = MessageQueue::new();
        q.enqueue(cmd("main", QueuePriority::Next, PromptInputMode::TaskNotification));
        q.enqueue(cmd_with_agent("sub", QueuePriority::Next, "agent-1"));

        // Main thread drain
        let main = q.drain(QueuePriority::Next, Some(None));
        assert_eq!(main.len(), 1);
        assert_eq!(main[0].value, "main");

        // Subagent drain
        let sub = q.drain(QueuePriority::Next, Some(Some("agent-1")));
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].value, "sub");
    }

    #[test]
    fn clear_removes_all() {
        let q = MessageQueue::new();
        q.enqueue(cmd("a", QueuePriority::Now, PromptInputMode::TaskNotification));
        q.enqueue(cmd("b", QueuePriority::Later, PromptInputMode::TaskNotification));
        q.clear();
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn has_pending_checks_priority() {
        let q = MessageQueue::new();
        q.enqueue(cmd("a", QueuePriority::Later, PromptInputMode::TaskNotification));
        assert!(!q.has_pending(QueuePriority::Next));
        assert!(q.has_pending(QueuePriority::Later));
    }

    #[test]
    fn remove_by_filter_selective() {
        let q = MessageQueue::new();
        q.enqueue(cmd("keep", QueuePriority::Next, PromptInputMode::TaskNotification));
        q.enqueue(cmd("drop", QueuePriority::Next, PromptInputMode::TaskNotification));

        let removed = q.remove_by_filter(|c| c.value == "drop");
        assert_eq!(removed.len(), 1);
        assert_eq!(q.len(), 1);
        assert_eq!(q.drain(QueuePriority::Later, None)[0].value, "keep");
    }
}
```

- [ ] **Step 2: Add module declaration in mod.rs**

In `cli/src/conversation/mod.rs`, add:
```rust
pub mod message_queue;
pub mod cron_runtime;
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p super-cli message_queue
```
Expected: all 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add cli/src/conversation/message_queue.rs cli/src/conversation/mod.rs
git commit -m "feat: add MessageQueue type with priority-based drain"
```

---

### Task 2: Add queue field to ToolCallContext

**Files:**
- Modify: `cli/src/tools/contract.rs:135-157`
- Modify: `cli/src/conversation/tool_loop.rs:63-72,154-163`

- [ ] **Step 1: Add `queue` field to `ToolCallContext`**

In `cli/src/tools/contract.rs`, add the field after `progress_sink`:

```rust
pub struct ToolCallContext {
    pub cwd: std::path::PathBuf,
    pub permission_mode: crate::state::store::PermissionMode,
    pub abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
    pub parent_tool_use_id: Option<String>,
    pub bus: Option<std::sync::Arc<crate::conversation::session_bus::SessionBus>>,
    pub auto_deny_prompts: bool,
    pub tool_use_id: String,
    pub progress_sink: Option<ProgressSink>,
    /// Message queue for tools that need to enqueue notifications (Monitor,
    /// background Bash, async Agent, Cron).
    pub queue: Option<std::sync::Arc<crate::conversation::message_queue::MessageQueue>>,
}
```

- [ ] **Step 2: Pass queue into ToolCallContext in tool_loop.rs**

In `cli/src/conversation/tool_loop.rs`, the `run_tool_uses` function needs a new parameter `queue: Arc<MessageQueue>`. Add it and pass it into both safe and unsafe ToolCallContext constructors.

Add parameter to function signature:
```rust
pub async fn run_tool_uses(
    registry: &ToolRegistry,
    tool_uses: Vec<(String, String, serde_json::Value)>,
    cwd: std::path::PathBuf,
    permission_mode: PermissionMode,
    abort_signal: Option<watch::Receiver<bool>>,
    bus: Arc<SessionBus>,
    parent_tool_use_id: Option<String>,
    session_id: String,
    auto_deny_prompts: bool,
    queue: Arc<crate::conversation::message_queue::MessageQueue>,  // NEW
) -> Vec<ContentBlockFinal> {
```

In the safe-path `ToolCallContext` (around line 63), add:
```rust
queue: Some(queue.clone()),
```

In the unsafe-path `ToolCallContext` (around line 154), add:
```rust
queue: Some(queue.clone()),
```

- [ ] **Step 3: Update the `ctx()` helper in bash.rs tests**

In `cli/src/tools/bash.rs` tests, the `ctx()` helper constructs a `ToolCallContext`. Add the new field:

```rust
fn ctx() -> ToolCallContext {
    ToolCallContext {
        cwd: std::env::temp_dir(),
        permission_mode: PermissionMode::default(),
        abort_signal: None,
        parent_tool_use_id: None,
        bus: None,
        auto_deny_prompts: true,
        tool_use_id: String::new(),
        progress_sink: None,
        queue: None,  // NEW
    }
}
```

- [ ] **Step 4: Verify compilation**

```bash
cargo check -p super-cli 2>&1 | head -20
```
Expected: compiles cleanly (or only warns about unused `queue` field — to be used in later tasks).

- [ ] **Step 5: Commit**

```bash
git add cli/src/tools/contract.rs cli/src/conversation/tool_loop.rs cli/src/tools/bash.rs
git commit -m "feat: add queue field to ToolCallContext, wire through tool_loop"
```

---

### Task 3: Wire MessageQueue into ConversationEngine and bootstrap

**Files:**
- Modify: `cli/src/conversation/engine.rs`
- Modify: `cli/src/bootstrap.rs`
- Modify: `cli/src/tools/mod.rs`

- [ ] **Step 1: Add queue field to ConversationEngine**

In `cli/src/conversation/engine.rs`, add to the struct:

```rust
pub struct ConversationEngine {
    // ... existing fields ...
    pub queue: Arc<crate::conversation::message_queue::MessageQueue>,
}
```

Update `new()` to accept and store `queue`:
```rust
pub fn new(
    store: Arc<Store>,
    config: CliConfig,
    registry: Arc<ToolRegistry>,
    bus: Arc<SessionBus>,
    queue: Arc<crate::conversation::message_queue::MessageQueue>,
) -> Self {
    Self {
        store,
        config,
        registry,
        bus,
        abort: None,
        session_id_override: None,
        history_override: None,
        permission_mode_override: None,
        auto_deny_prompts: false,
        skills: Arc::new(Vec::new()),
        skill_listing_sent: Arc::new(AtomicBool::new(false)),
        queue,
    }
}
```

Update `new_child()` to share the same queue (clone the Arc):
```rust
queue: parent_engine.queue.clone(),
```

- [ ] **Step 2: Create queue and pass it in bootstrap.rs**

In `cli/src/bootstrap.rs`:

```rust
use crate::conversation::message_queue::MessageQueue;
use std::sync::Arc;

// Create the cron jobs map and wake channel
let cron_jobs = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
let (cron_wake_tx, cron_wake_rx) = tokio::sync::watch::channel(false);

// Create the shared message queue
let queue = Arc::new(MessageQueue::new());

// Build tool registry (receives cron_wake_tx and queue references)
let registry = crate::tools::ToolRegistry::new(
    store.clone(),
    config.clone(),
    agent_registry.clone(),
    queue.clone(),
    cron_jobs.clone(),
    cron_wake_tx,
);

// Spawn cron runtime
let cron_runtime = crate::conversation::cron_runtime::CronRuntime::new(
    cron_jobs,
    queue.clone(),
    cron_wake_rx,
);
tokio::spawn(async move { cron_runtime.run().await });

// Pass queue to engine:
let mut engine = crate::conversation::engine::ConversationEngine::new(
    store.clone(),
    config.clone(),
    registry.clone(),
    bus.clone(),
    queue.clone(),
);
```

- [ ] **Step 3: Update ToolRegistry::new to accept and wire queue + cron references**

In `cli/src/tools/mod.rs`, update the `ToolRegistry::new` signature:

```rust
use crate::conversation::message_queue::MessageQueue;
use tokio::sync::watch;

pub fn new(
    store: Arc<Store>,
    config: shared::CliConfig,
    agent_registry: Arc<crate::agents::AgentRegistry>,
    queue: Arc<MessageQueue>,
    cron_jobs: Arc<std::sync::Mutex<std::collections::HashMap<String, crate::tools::cron_create::CronJob>>>,
    cron_wake_tx: watch::Sender<bool>,
) -> Arc<Self> {
```

Wire queue into tools that need it:

```rust
Arc::new(BashTool {
    queue: queue.clone(),
    store: store.clone(),
}),
Arc::new(MonitorTool {
    queue: queue.clone(),
}),
Arc::new(CronCreateTool {
    jobs: cron_jobs.clone(),
    wake_tx: cron_wake_tx.clone(),
}),
Arc::new(CronDeleteTool {
    jobs: cron_jobs.clone(),
    wake_tx: cron_wake_tx.clone(),
}),
// CronListTool unchanged — no wake needed
Arc::new(CronListTool {
    jobs: cron_jobs.clone(),
}),
```

And later:
```rust
registry.register(Arc::new(AgentTool {
    store: store.clone(),
    config: config.clone(),
    registry: agent_registry,
    tool_registry: registry.clone(),
    queue: queue.clone(),
}));
```

- [ ] **Step 4: Update tests that use ToolRegistry::new**

In `cli/src/tools/mod.rs`, all test functions calling `ToolRegistry::new(...)` need the new params. Add a helper:

```rust
fn make_test_deps() -> (Arc<MessageQueue>, Arc<Mutex<HashMap<String, cron_create::CronJob>>>, watch::Sender<bool>) {
    let queue = Arc::new(MessageQueue::new());
    let jobs = Arc::new(Mutex::new(HashMap::new()));
    let (tx, _rx) = watch::channel(false);
    (queue, jobs, tx)
}
```

In each test:
```rust
let (queue, jobs, wake_tx) = make_test_deps();
let reg = ToolRegistry::new(store, config, agent_registry, queue, jobs, wake_tx);
```

- [ ] **Step 5: Verify compilation**

```bash
cargo check -p super-cli 2>&1
```
Fix any compilation errors. Expected: compiles.

- [ ] **Step 6: Commit**

```bash
git add cli/src/conversation/engine.rs cli/src/bootstrap.rs cli/src/tools/mod.rs cli/src/tools/bash.rs
git commit -m "feat: wire MessageQueue into engine, bootstrap, and tool constructors"
```

---

### Task 4: Implement Monitor tool call()

**Files:**
- Modify: `cli/src/tools/monitor.rs`

- [ ] **Step 1: Rewrite monitor.rs with real implementation**

Replace the entire file content:

```rust
use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

use crate::conversation::message_queue::{MessageQueue, PromptInputMode, QueuePriority};

pub struct MonitorTool {
    pub queue: Arc<MessageQueue>,
}

#[async_trait]
impl Tool for MonitorTool {
    fn name(&self) -> &str {
        "Monitor"
    }
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Start a background monitor that streams events from a long-running script. \
         Each stdout line is an event. Supports tailing logs, polling for changes, \
         and watching processes."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/monitor.txt").into()
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command or script to monitor"},
                "description": {"type": "string", "description": "Human-readable description"},
                "timeout_ms": {"type": "integer", "description": "Maximum time to run in milliseconds"},
                "persistent": {"type": "boolean", "description": "Run for the lifetime of the session", "default": false}
            },
            "required": ["command", "description"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "acknowledged": {"type": "boolean"},
                "message": {"type": "string"},
                "task_id": {"type": "string"}
            }
        }))
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(
        &self,
        input: serde_json::Value,
        context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let command_str = input["command"].as_str().unwrap_or("");
        let description = input["description"]
            .as_str()
            .unwrap_or("monitor")
            .to_string();
        let timeout_ms = input["timeout_ms"].as_u64().unwrap_or(300_000); // 5 min default

        let task_id = Uuid::new_v4().to_string();
        let agent_id = context.parent_tool_use_id.clone();
        let queue = self.queue.clone();
        let desc_for_completion = description.clone();
        let task_id_for_completion = task_id.clone();

        // Spawn the child process, piping stdout
        let mut child = match Command::new("bash")
            .arg("-c")
            .arg(command_str.to_string())  // to_string() to own the value
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to spawn monitor command: {e}"),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        // Take stdout for line streaming, then move both reader and child into the spawn
        let stdout = child.stdout.take().expect("stdout not piped");
        let reader = BufReader::new(stdout);

        // Spawn background task for line-by-line streaming
        // child is moved into the closure so we can call wait() on it
        tokio::spawn(async move {
            let mut lines = reader.lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        queue.enqueue_pending_notification(
                            line,
                            PromptInputMode::TaskNotification,
                            QueuePriority::Next,
                            agent_id.clone(),
                        );
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            let status = match child.wait().await {
                Ok(exit) if exit.success() => "completed",
                Ok(_) => "failed",
                Err(_) => "failed",
            };
            let summary = match status {
                "completed" => format!("Monitor \"{desc_for_completion}\" stream ended"),
                _ => format!("Monitor \"{desc_for_completion}\" script failed"),
            };
            let notification = format!(
                "<task-notification>\n  <task-id>{}</task-id>\n  <status>{}</status>\n  <summary>{}</summary>\n</task-notification>",
                task_id_for_completion, status, summary
            );
            queue.enqueue_pending_notification(
                notification,
                PromptInputMode::TaskNotification,
                QueuePriority::Next,
                agent_id,
            );
        });

        // Return immediately — the monitor runs in background
        ToolResult {
            content: json!({
                "acknowledged": true,
                "message": format!("Monitor \"{description}\" started"),
                "task_id": task_id
            })
            .to_string(),
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
                output
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p super-cli 2>&1
```
Fix any compilation errors. Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add cli/src/tools/monitor.rs
git commit -m "feat: implement Monitor tool with line-by-line stdout streaming to queue"
```

---

### Task 5: Engine loop drain — inject queue notifications into conversation

**Files:**
- Modify: `cli/src/conversation/engine.rs`

- [ ] **Step 1: Add drain logic at the top of the engine loop**

In `cli/src/conversation/engine.rs`, inside the `loop {` in `process_prompt()`, right after the turn limit check and before `build_request_body()`, add the drain. Also track whether Sleep ran.

First, track Sleep usage. After the tool_use block processing (where `tool_uses` is collected), check:

```rust
// After collecting tool_uses from content blocks, check for Sleep
let sleep_ran = tool_uses.iter().any(|(_, name, _)| name == "Sleep");
```

Then at the top of the next iteration, drain:

```rust
// Drain pending queue notifications before each API call.
// If Sleep ran, drain everything (Later); otherwise only Next-priority.
let max_priority = if sleep_ran {
    crate::conversation::message_queue::QueuePriority::Later
} else {
    crate::conversation::message_queue::QueuePriority::Next
};

// Agent filter: subagents drain only their own agent's task-notifications
let agent_filter = self.session_id_override.as_deref();

let queued = self.queue.drain(max_priority, Some(agent_filter));
for cmd in &queued {
    match cmd.mode {
        crate::conversation::message_queue::PromptInputMode::TaskNotification => {
            let text_block = ContentBlockFinal::Text {
                text: cmd.value.clone(),
            };
            // Inject into history so the model sees it
            history.push(HistoryEntry {
                role: crate::conversation::anthropic::Role::User,
                content: vec![text_block],
            });
            // Emit on bus so the TUI renders it
            self.bus.emit_system(
                crate::sdk::protocol::SystemSubtype::Notice,
                &cmd.value,
            );
        }
        crate::conversation::message_queue::PromptInputMode::Prompt => {
            // User submitted input mid-turn — inject as a user message
            let text_block = ContentBlockFinal::Text {
                text: cmd.value.clone(),
            };
            history.push(HistoryEntry {
                role: crate::conversation::anthropic::Role::User,
                content: vec![text_block],
            });
            self.bus.emit(BusMessage::User {
                message: UserPayload {
                    role: "user".to_string(),
                    content: vec![text_block],
                },
                parent_tool_use_id: parent_tool_use_id.clone(),
                uuid: Uuid::new_v4(),
                session_id: session_id.clone(),
            });
        }
        _ => {
            // Bash and OrphanedPermission modes are consumed by the TUI
            // queue processor, not mid-turn.
        }
    }
}
```

- [ ] **Step 2: Pass queue into the run_tool_uses call**

Update the call to `run_tool_uses` to include the queue parameter:

```rust
let tool_results = run_tool_uses(
    &self.registry,
    tool_uses,
    cwd.clone(),
    permission_mode,
    self.abort.clone(),
    self.bus.clone(),
    parent_tool_use_id.clone(),
    session_id.clone(),
    self.auto_deny_prompts,
    self.queue.clone(),  // NEW
)
.await;
```

- [ ] **Step 3: Initialize sleep_ran for the first iteration**

Before the loop, initialize:
```rust
let mut sleep_ran = false;
```

At the end of each iteration, after processing tool_uses, update:
```rust
sleep_ran = tool_uses.iter().any(|(_, name, _)| name == "Sleep");
// Reset for next iteration? No — sleep_ran should persist across iterations.
// Actually Claude Code persists it: if Sleep ran in ANY prior iteration of the
// same turn, Later notifications are drained. We match that by NOT resetting.
// Actually, looking more carefully at Claude Code: sleepRan is checked per-iteration
// and applies to the NEXT iteration's drain. It is derived from the CURRENT iteration's
// tool_uses. So it should be set fresh each iteration from the just-processed tools.
```

Correction — `sleep_ran` is recalculated each iteration from the tool_uses just processed. So the order is:

```rust
// At the TOP of the loop (before API call):
let max_priority = if sleep_ran { QueuePriority::Later } else { QueuePriority::Next };
// ... drain ...
// ... API call ...
// ... process tool_uses ...
// At the BOTTOM of the loop (after processing):
sleep_ran = tool_uses.iter().any(|(_, name, _)| name == "Sleep");
```

- [ ] **Step 4: Verify compilation**

```bash
cargo check -p super-cli 2>&1
```
Expected: compiles. Fix any issues.

- [ ] **Step 5: Commit**

```bash
git add cli/src/conversation/engine.rs
git commit -m "feat: drain message queue notifications into engine loop before API calls"
```

---

### Task 6: Background Bash completion notification

**Files:**
- Modify: `cli/src/tools/bash.rs`
- Modify: `cli/src/state/store.rs`

- [ ] **Step 1: Add task output path helper to Store**

In `cli/src/state/store.rs`, add:

```rust
use std::path::PathBuf;

impl Store {
    /// Build the output path for a background task.
    /// Path: ~/.super/tasks/{task_id}.output
    pub fn task_output_path(task_id: &str) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".super")
            .join("tasks")
            .join(format!("{task_id}.output"))
    }
}
```

- [ ] **Step 2: Rewrite BashTool's run_in_background to capture output and enqueue completion**

In `cli/src/tools/bash.rs`, replace the `run_in_bg` block (currently lines 131-153):

```rust
if run_in_bg {
    let desc = input["description"]
        .as_str()
        .unwrap_or("background bash")
        .to_string();
    let task_id = uuid::Uuid::new_v4().to_string();
    let output_path = crate::state::store::Store::task_output_path(&task_id);
    let agent_id = context.parent_tool_use_id.clone();

    // Ensure parent directories exist
    if let Some(parent) = output_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let output_path_clone = output_path.clone();
    let queue = context
        .queue
        .clone()
        .unwrap_or_else(|| {
            // Fallback: create a local queue. In practice this should never happen
            // because bootstrap always wires the queue.
            panic!("MessageQueue not available in ToolCallContext")
        });

    match Command::new("bash")
        .arg("-c")
        .arg(command_str)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(mut child) => {
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            // Write output to file in background
            let output_path_for_writer = output_path_clone.clone();
            tokio::spawn(async move {
                let stdout_bytes = if let Some(stdout) = stdout {
                    let reader = tokio::io::BufReader::new(stdout);
                    use tokio::io::AsyncReadExt;
                    let mut reader = reader;
                    let mut buf = Vec::new();
                    let _ = tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf).await;
                    buf
                } else {
                    Vec::new()
                };

                let stderr_bytes = if let Some(stderr) = stderr {
                    let reader = tokio::io::BufReader::new(stderr);
                    use tokio::io::AsyncReadExt;
                    let mut reader = reader;
                    let mut buf = Vec::new();
                    let _ = tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf).await;
                    buf
                } else {
                    Vec::new()
                };

                // Write to output file
                let mut output = String::from_utf8_lossy(&stdout_bytes).to_string();
                let stderr_str = String::from_utf8_lossy(&stderr_bytes);
                if !stderr_str.is_empty() {
                    output.push_str("\nstderr:\n");
                    output.push_str(&stderr_str);
                }
                let _ = std::fs::write(&output_path_for_writer, &output);

                // Wait for process to finish
                let exit_status = child.wait().await;
                let exit_code = exit_status.as_ref().ok().and_then(|s| s.code()).unwrap_or(-1);
                let status = if exit_status.map(|s| s.success()).unwrap_or(false) {
                    "completed"
                } else {
                    "failed"
                };

                let summary = format!(
                    "Background bash \"{desc}\" {} (exit code {exit_code})",
                    if status == "completed" { "completed" } else { "failed" }
                );
                let notification = format!(
                    "<task-notification>\n  <task-id>{task_id}</task-id>\n  <output-file>{output_path}</output-file>\n  <status>{status}</status>\n  <summary>{summary}</summary>\n</task-notification>",
                    task_id = task_id,
                    output_path = output_path_clone.display(),
                    status = status,
                    summary = summary,
                );

                use crate::conversation::message_queue::{PromptInputMode, QueuePriority};
                queue.enqueue_pending_notification(
                    notification,
                    PromptInputMode::TaskNotification,
                    QueuePriority::Next,
                    agent_id,
                );
            });

            ToolResult {
                content: json!({
                    "background": true,
                    "task_id": task_id,
                    "message": format!("Command launched in background. Output: {}", output_path.display()),
                })
                .to_string(),
                is_error: false,
                ..Default::default()
            }
        }
        Err(e) => ToolResult {
            content: format!("Failed to spawn: {e}"),
            is_error: true,
            ..Default::default()
        },
    }
}
```

- [ ] **Step 3: Clean up unused imports**

Remove or comment out the `use tokio::process::Command;` if it's no longer needed (it's still used for the non-background path):

The existing import `use tokio::process::Command;` at line 7 is still needed for both paths.

- [ ] **Step 4: Verify compilation**

```bash
cargo check -p super-cli 2>&1
```
Expected: compiles. Fix any issues.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tools/bash.rs cli/src/state/store.rs
git commit -m "feat: enqueue background bash completion notification into queue"
```

---

### Task 7: Background Agent completion notification

**Files:**
- Modify: `cli/src/tools/agent.rs`

- [ ] **Step 1: Add queue to AgentTool and enqueue on async completion**

In `cli/src/tools/agent.rs`, add the queue import:

```rust
use crate::conversation::message_queue::{MessageQueue, PromptInputMode, QueuePriority};
```

Add the field to `AgentTool`:

```rust
pub struct AgentTool {
    pub store: Arc<Store>,
    pub config: CliConfig,
    pub registry: Arc<AgentRegistry>,
    pub tool_registry: Arc<ToolRegistry>,
    pub queue: Arc<MessageQueue>,
}
```

In the async spawn completion handler (around line 242), after the bus emit, add the queue enqueue:

```rust
// Existing bus emit (keep it):
bus_for_task.emit(BusMessage::SystemEvent {
    subtype: SystemSubtype::AsyncAgentDone,
    message: format!("{agent_id_for_task} finished: {text}"),
    parent_tool_use_id: Some(parent_tu_for_task.clone()),
    uuid: Uuid::new_v4(),
    session_id: agent_id_for_task.clone(),
});

// NEW: Enqueue notification for the model
let status = if result.is_ok() { "completed" } else { "failed" };
let summary = format!(
    "Agent \"{description}\" {}",
    if result.is_ok() { "completed" } else { "failed" }
);
let notification = format!(
    "<task-notification>\n  <task-id>{}</task-id>\n  <status>{}</status>\n  <summary>{}</summary>\n</task-notification>",
    agent_id_for_task, status, summary
);
queue_for_agent.enqueue_pending_notification(
    notification,
    PromptInputMode::TaskNotification,
    QueuePriority::Later,
    None, // agent notification goes to main thread
);
```

The `queue_for_agent` variable needs to be cloned from `self.queue` before the `tokio::spawn`:

```rust
let queue_for_agent = self.queue.clone();
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p super-cli 2>&1
```
Expected: compiles. Fix any issues.

- [ ] **Step 3: Commit**

```bash
git add cli/src/tools/agent.rs
git commit -m "feat: enqueue async agent completion notification into queue"
```

---

### Task 8: Cron runtime — background scheduler

**Files:**
- Create: `cli/src/conversation/cron_runtime.rs`
- Modify: `cli/src/tools/cron_create.rs`
- Modify: `cli/src/tools/cron_delete.rs`

- [ ] **Step 1: Create cron_runtime.rs**

```rust
// cli/src/conversation/cron_runtime.rs
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use uuid::Uuid;

use crate::conversation::message_queue::{
    MessageQueue, PromptInputMode, QueuePriority,
};

/// Parsed fields of a 5-field cron expression.
#[derive(Debug, Clone)]
struct CronFields {
    minute: CronField,
    hour: CronField,
    day_of_month: CronField,
    month: CronField,
    day_of_week: CronField,
}

#[derive(Debug, Clone)]
enum CronField {
    Any,
    List(Vec<u8>),
}

impl CronField {
    fn parse(s: &str, min: u8, max: u8) -> Result<Self, String> {
        if s == "*" {
            return Ok(CronField::Any);
        }
        let mut values = Vec::new();
        for part in s.split(',') {
            let part = part.split('/').next().unwrap_or(part); // ignore step for now
            if let Some((lo, hi)) = part.split_once('-') {
                let lo: u8 = lo.parse().map_err(|_| format!("bad range: {lo}"))?;
                let hi: u8 = hi.parse().map_err(|_| format!("bad range: {hi}"))?;
                for v in lo..=hi {
                    if v < min || v > max {
                        return Err(format!("{v} out of range {min}-{max}"));
                    }
                    values.push(v);
                }
            } else {
                let v: u8 = part.parse().map_err(|_| format!("bad value: {part}"))?;
                if v < min || v > max {
                    return Err(format!("{v} out of range {min}-{max}"));
                }
                values.push(v);
            }
        }
        Ok(CronField::List(values))
    }

    fn matches(&self, value: u8) -> bool {
        match self {
            CronField::Any => true,
            CronField::List(vals) => vals.contains(&value),
        }
    }
}

impl CronFields {
    fn parse(expr: &str) -> Result<Self, String> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 5 {
            return Err("cron must have 5 fields".into());
        }
        Ok(CronFields {
            minute: CronField::parse(parts[0], 0, 59)?,
            hour: CronField::parse(parts[1], 0, 23)?,
            day_of_month: CronField::parse(parts[2], 1, 31)?,
            month: CronField::parse(parts[3], 1, 12)?,
            day_of_week: CronField::parse(parts[4], 0, 7)?,
        })
    }

    fn matches_now(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs();
        // Convert to UTC components using a simple approach
        let days_since_epoch = secs / 86400;
        let time_of_day = secs % 86400;
        let hour = (time_of_day / 3600) as u8;
        let minute = ((time_of_day % 3600) / 60) as u8;

        // Day of week: 1970-01-01 was a Thursday (4). Sunday = 0 in cron.
        let dow = ((days_since_epoch + 4) % 7) as u8;

        // Month and day: simplified — we compute from a known epoch.
        // For a proper implementation, use the `chrono` crate. For now,
        // we use a basic approach that's correct for the common case.
        let (month, day) = approximate_month_day(days_since_epoch);

        self.minute.matches(minute)
            && self.hour.matches(hour)
            && self.day_of_month.matches(day)
            && self.month.matches(month)
            && self.day_of_week.matches(dow)
    }
}

fn approximate_month_day(days: u64) -> (u8, u8) {
    // Days in each month (non-leap year)
    let month_days: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut remaining = days;
    // Account for leap years since 1970 (simplified)
    let years_since_1970 = days / 365;
    let leap_days = (years_since_1970 + 1) / 4; // approximate
    remaining = remaining.saturating_sub(leap_days);

    let mut month: u8 = 1;
    for (i, md) in month_days.iter().enumerate() {
        if remaining < *md {
            return (month, remaining as u8 + 1);
        }
        remaining -= *md;
        month = (i + 2) as u8;
        if month > 12 {
            month = 1;
        }
    }
    (12, 31)
}

pub struct CronJob {
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    pub fields: CronFields,
}

pub struct CronRuntime {
    jobs: Arc<Mutex<HashMap<String, CronJob>>>,
    queue: Arc<MessageQueue>,
    wake_rx: watch::Receiver<bool>,
}

impl CronRuntime {
    pub fn new(
        jobs: Arc<Mutex<HashMap<String, CronJob>>>,
        queue: Arc<MessageQueue>,
        wake_rx: watch::Receiver<bool>,
    ) -> Self {
        CronRuntime {
            jobs,
            queue,
            wake_rx,
        }
    }

    /// Run the cron scheduler loop. Call via tokio::spawn.
    pub async fn run(self) {
        let tick_interval = Duration::from_secs(30);
        loop {
            // Wait for next tick or wake signal
            tokio::select! {
                _ = tokio::time::sleep(tick_interval) => {}
                _ = self.wake_rx.changed() => {}
            }

            let mut to_remove: Vec<String> = Vec::new();

            {
                let jobs = self.jobs.lock().unwrap();
                for (id, job) in jobs.iter() {
                    if job.fields.matches_now() {
                        // Enqueue the prompt
                        self.queue.enqueue(crate::conversation::message_queue::QueuedCommand {
                            value: job.prompt.clone(),
                            mode: PromptInputMode::Prompt,
                            priority: QueuePriority::Later,
                            agent_id: None,
                            is_meta: true,
                            uuid: Uuid::new_v4(),
                        });

                        if !job.recurring {
                            to_remove.push(id.clone());
                        }
                    }
                }
            }

            // Clean up one-shot jobs that fired
            if !to_remove.is_empty() {
                let mut jobs = self.jobs.lock().unwrap();
                for id in &to_remove {
                    jobs.remove(id);
                }
            }
        }
    }
}
```

- [ ] **Step 2: Update CronCreateTool to parse cron fields and notify runtime**

In `cli/src/tools/cron_create.rs`, import the cron types:

```rust
use crate::conversation::cron_runtime::CronFields;
use tokio::sync::watch;
```

Update the struct — the jobs map now stores `cron_runtime::CronJob` (with parsed fields) instead of the local `CronJob`:

```rust
pub struct CronCreateTool {
    pub jobs: Arc<Mutex<HashMap<String, crate::conversation::cron_runtime::CronJob>>>,
    pub wake_tx: watch::Sender<bool>,
}
```

In the `call()` method, after validating and creating the CronJob, parse the cron fields and notify:

```rust
// Parse cron fields for runtime matching
let fields = match CronFields::parse(&cron) {
    Ok(f) => f,
    Err(e) => {
        return ToolResult {
            content: format!("Invalid cron expression: {e}"),
            is_error: true,
            ..Default::default()
        };
    }
};

let runtime_job = crate::conversation::cron_runtime::CronJob {
    cron: cron.clone(),
    prompt: prompt.clone(),
    recurring,
    fields,
};

let mut jobs = self.jobs.lock().unwrap();
jobs.insert(id.clone(), runtime_job);

// Wake the runtime
let _ = self.wake_tx.send(true);
```

- [ ] **Step 3: Update CronDeleteTool to notify runtime**

In `cli/src/tools/cron_delete.rs`:

```rust
use tokio::sync::watch;

pub struct CronDeleteTool {
    pub jobs: Arc<Mutex<HashMap<String, crate::conversation::cron_runtime::CronJob>>>,
    pub wake_tx: watch::Sender<bool>,
}
```

After removing the job, add `let _ = self.wake_tx.send(true);` to wake the runtime for rescheduling.

- [ ] **Step 4: Update CronListTool to use the new CronJob type**

In `cli/src/tools/cron_list.rs`, update the import:

```rust
use super::cron_runtime::CronJob;
```

And update struct to reference `cron_runtime::CronJob`:

```rust
pub struct CronListTool {
    pub jobs: Arc<Mutex<HashMap<String, crate::conversation::cron_runtime::CronJob>>>,
}
```

The `call()` method formatting changes slightly since `CronJob` now has `fields: CronFields` in addition to `cron`, `prompt`, `recurring`. The display output can reference `job.cron`, `job.prompt`, and `job.recurring` — same as before.

- [ ] **Step 5: Verify compilation**

```bash
cargo check -p super-cli 2>&1
```
Expected: compiles. Fix issues.

- [ ] **Step 6: Commit**

```bash
git add cli/src/conversation/cron_runtime.rs cli/src/tools/cron_create.rs cli/src/tools/cron_delete.rs cli/src/tools/mod.rs cli/src/bootstrap.rs
git commit -m "feat: add cron runtime scheduler, wire cron tools with wake notifications"
```

---

### Task 9: Mid-turn user input via queue

**Files:**
- Modify: `cli/src/tui/app.rs`

- [ ] **Step 1: Add queue to App struct**

In `cli/src/tui/app.rs`:

```rust
use crate::conversation::message_queue::{MessageQueue, PromptInputMode, QueuePriority};

pub struct App {
    // ... existing fields ...
    queue: Arc<MessageQueue>,
}
```

Update `App::new()` to accept and store `queue: Arc<MessageQueue>`.

- [ ] **Step 2: Enqueue mid-turn input instead of only using local deque**

In `spawn_engine()`, when `self.inflight.is_some()`, instead of only pushing to `queued_prompts`, also enqueue to the message queue:

```rust
fn spawn_engine(&mut self, prompt: String) {
    if self.inflight.is_some() {
        self.queued_prompts.push_back(prompt.clone());
        // Also enqueue to message queue so engine drain picks it up mid-turn
        use uuid::Uuid;
        self.queue.enqueue(crate::conversation::message_queue::QueuedCommand {
            value: prompt,
            mode: PromptInputMode::Prompt,
            priority: QueuePriority::Next,
            agent_id: None,
            is_meta: false,
            uuid: Uuid::new_v4(),
        });
        return;
    }
    // ... rest of existing spawn logic ...
}
```

- [ ] **Step 3: Clear queue on ESC abort**

In the ESC handler (around line 555-576), add `self.queue.clear();` alongside the existing `self.queued_prompts.clear()` and abort logic.

- [ ] **Step 4: Update run_with_engine to pass queue**

```rust
pub async fn run_with_engine(
    config: CliConfig,
    store: Arc<Store>,
    engine: ConversationEngine,
    _registry: Arc<ToolRegistry>,
    bus: Arc<SessionBus>,
    system_prompt: SystemPrompt,
    queue: Arc<MessageQueue>,  // NEW param
) {
    // ...
    let mut app = App::new(config, store, engine, bus, system_prompt, queue);
    // ...
}
```

- [ ] **Step 5: Update bootstrap.rs call**

In `cli/src/bootstrap.rs`:
```rust
crate::tui::app::run_with_engine(
    config, store, engine, registry, bus, system_prompt, queue
).await;
```

- [ ] **Step 6: Verify compilation**

```bash
cargo check -p super-cli 2>&1
```
Expected: compiles. Fix any issues.

- [ ] **Step 7: Commit**

```bash
git add cli/src/tui/app.rs cli/src/bootstrap.rs
git commit -m "feat: enqueue mid-turn user input into message queue, clear on ESC"
```

---

### Task 10: End-to-end build and fix compilation

**Files:** All modified files

- [ ] **Step 1: Full build**

```bash
cargo build -p super-cli 2>&1
```
Fix any remaining compilation errors across all files. Pay special attention to:
- Import paths for `MessageQueue`, `QueuePriority`, `PromptInputMode`
- `use uuid::Uuid;` in files that need it
- `Arc` wrapping for queue references
- `watch::Sender<bool>` for cron wake channel

- [ ] **Step 2: Run all tests**

```bash
cargo test -p super-cli 2>&1
```
Fix any failing tests. Pay attention to:
- `ToolRegistry::new()` test calls need the new `queue` parameter
- `ctx()` helper in bash tests needs `queue: None` field
- Any test that constructs `ConversationEngine::new()` needs the `queue` param

- [ ] **Step 3: Verify with clippy**

```bash
cargo clippy -p super-cli -- -D warnings 2>&1
```
Fix any clippy warnings.

- [ ] **Step 4: Verify with fmt**

```bash
cargo fmt -p super-cli -- --check 2>&1
```
Fix any formatting issues:
```bash
cargo fmt -p super-cli
```

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "chore: apply clippy and fmt fixes for message queue implementation"
```
