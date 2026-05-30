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

#[derive(Clone, Debug)]
pub struct MessageQueue {
    inner: Arc<Mutex<MessageQueueInner>>,
}

#[derive(Debug)]
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
    #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
    pub fn enqueue(&self, cmd: QueuedCommand) {
        self.inner.lock().unwrap().queue.push(cmd);
    }

    /// Convenience for system-generated notifications.
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
    #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
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
    #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
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
    #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
    pub fn clear(&self) {
        self.inner.lock().unwrap().queue.clear();
    }

    /// Check if any commands are pending at or above the given priority.
    #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
    pub fn has_pending(&self, max_priority: QueuePriority) -> bool {
        let inner = self.inner.lock().unwrap();
        let threshold = max_priority as u8;
        inner
            .queue
            .iter()
            .any(|cmd| (cmd.priority as u8) <= threshold)
    }

    /// Total number of commands in the queue.
    #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().queue.len()
    }

    /// True when the queue has no commands.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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

    fn cmd_with_agent(value: &str, priority: QueuePriority, agent_id: &str) -> QueuedCommand {
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
        q.enqueue(cmd(
            "a",
            QueuePriority::Later,
            PromptInputMode::TaskNotification,
        ));
        q.enqueue(cmd(
            "b",
            QueuePriority::Next,
            PromptInputMode::TaskNotification,
        ));
        q.enqueue(cmd(
            "c",
            QueuePriority::Now,
            PromptInputMode::TaskNotification,
        ));

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
        q.enqueue(cmd(
            "main",
            QueuePriority::Next,
            PromptInputMode::TaskNotification,
        ));
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
        q.enqueue(cmd(
            "a",
            QueuePriority::Now,
            PromptInputMode::TaskNotification,
        ));
        q.enqueue(cmd(
            "b",
            QueuePriority::Later,
            PromptInputMode::TaskNotification,
        ));
        q.clear();
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn has_pending_checks_priority() {
        let q = MessageQueue::new();
        q.enqueue(cmd(
            "a",
            QueuePriority::Later,
            PromptInputMode::TaskNotification,
        ));
        assert!(!q.has_pending(QueuePriority::Next));
        assert!(q.has_pending(QueuePriority::Later));
    }

    #[test]
    fn remove_by_filter_selective() {
        let q = MessageQueue::new();
        q.enqueue(cmd(
            "keep",
            QueuePriority::Next,
            PromptInputMode::TaskNotification,
        ));
        q.enqueue(cmd(
            "drop",
            QueuePriority::Next,
            PromptInputMode::TaskNotification,
        ));

        let removed = q.remove_by_filter(|c| c.value == "drop");
        assert_eq!(removed.len(), 1);
        assert_eq!(q.len(), 1);
        assert_eq!(q.drain(QueuePriority::Later, None)[0].value, "keep");
    }
}
