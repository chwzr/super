//! The engine every child session runs on: the agents the `Agent` tool
//! starts, the agents of a workflow and the iterations of a loop.
//!
//! A child is an [`AgentSession`] of its own, made by the parent; one run of
//! it is [`run_child_turn`]. A run lasts until the child has nothing left to
//! do: the turns its own background work starts (the notice of a command it
//! left running, a message for it) belong to the run, so their outcome is
//! the run's outcome.

use crate::session_runner::AgentSession;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use super_agent::AgentMessage;
use super_ai::{StopReason, Usage};
use tokio_util::sync::CancellationToken;

/// How a child run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Completed,
    Failed,
    Interrupted,
}

/// How much of the parent's conversation a workflow or loop child starts
/// with: nothing, or the user and assistant text without tool calls and
/// results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkTurns {
    None,
    All,
}

pub(crate) struct ChildTurnOutcome {
    pub status: AgentStatus,
    pub result: Option<String>,
    pub error: Option<String>,
    pub usage: Usage,
    /// Tool calls the child made during the run.
    pub tool_uses: u64,
    pub duration: Duration,
}

/// Run `child` with `prompt` until it has nothing left to do, account for
/// its cost in the parent, and read its final answer.
///
/// When `cancel` fires or `timeout_ms` passes, the child is told to abort
/// and its own background tasks are stopped; the run then counts as
/// interrupted. The prompt future is still awaited so the child's session
/// stays consistent.
pub(crate) async fn run_child_turn(
    parent: &Weak<AgentSession>,
    child: &Arc<AgentSession>,
    prompt: AgentMessage,
    cancel: Option<CancellationToken>,
    timeout_ms: Option<u64>,
) -> ChildTurnOutcome {
    let run = ChildRun::begin(child, cancel, timeout_ms);
    run.prompt(child, prompt).await;
    run.end(parent, child)
}

/// One run of a child, from its prompt until it has nothing left to do:
/// [`run_child_turn`] in steps, for a caller that decides under a lock of
/// its own when the run is over (a message may reach the child after its
/// last turn and before the caller records the end).
pub(crate) struct ChildRun {
    /// Fired by the run's cancel token or its time limit.
    stop: CancellationToken,
    guard: Option<tokio::task::JoinHandle<()>>,
    started: Instant,
    usage_before: Usage,
    tool_uses_before: u64,
}

impl ChildRun {
    /// Start a run of `child`. When `cancel` fires or `timeout_ms` passes,
    /// the child is told to abort and its own background tasks are stopped.
    pub(crate) fn begin(
        child: &Arc<AgentSession>,
        cancel: Option<CancellationToken>,
        timeout_ms: Option<u64>,
    ) -> ChildRun {
        let stop = CancellationToken::new();
        let guard = (cancel.is_some() || timeout_ms.is_some()).then(|| {
            let guard_child = child.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                match (cancel, timeout_ms) {
                    (Some(cancel), Some(ms)) => {
                        tokio::select! {
                            _ = cancel.cancelled() => {}
                            _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
                        }
                    }
                    (Some(cancel), None) => cancel.cancelled().await,
                    (None, Some(ms)) => tokio::time::sleep(Duration::from_millis(ms)).await,
                    (None, None) => return,
                }
                stop.cancel();
                guard_child.abort();
                guard_child.stop_background_tasks();
            })
        });
        ChildRun {
            stop,
            guard,
            started: Instant::now(),
            usage_before: child.totals(),
            tool_uses_before: tool_results(child),
        }
    }

    /// Give the child `prompt`, then wait until it has nothing left to do.
    pub(crate) async fn prompt(&self, child: &Arc<AgentSession>, prompt: AgentMessage) {
        child.prompt(vec![prompt]).await;
        self.settle(child).await;
    }

    /// Wait until the child has nothing left to do, or the run is stopped.
    /// The notices of the child's own background work start turns of their
    /// own; the run is over when the last of them is.
    pub(crate) async fn settle(&self, child: &AgentSession) {
        if self.stop.is_cancelled() {
            return;
        }
        tokio::select! {
            _ = child.wait_for_background_work() => {}
            _ = self.stop.cancelled() => {}
        }
    }

    /// Whether the run's cancel token or its time limit stopped it.
    pub(crate) fn is_stopped(&self) -> bool {
        self.stop.is_cancelled()
    }

    /// End the run: account for its cost in the parent and read the
    /// child's final answer.
    pub(crate) fn end(
        mut self,
        parent: &Weak<AgentSession>,
        child: &AgentSession,
    ) -> ChildTurnOutcome {
        if let Some(guard) = self.guard.take() {
            guard.abort();
        }
        let usage = usage_delta(child.totals(), self.usage_before);
        if let Some(parent) = parent.upgrade() {
            parent.record_subagent_usage(usage);
        }
        let (status, result, error) = if self.stop.is_cancelled() {
            (AgentStatus::Interrupted, None, Some("interrupted".into()))
        } else {
            turn_outcome(child)
        };
        ChildTurnOutcome {
            status,
            result,
            error,
            usage,
            tool_uses: tool_results(child).saturating_sub(self.tool_uses_before),
            duration: self.started.elapsed(),
        }
    }
}

impl Drop for ChildRun {
    fn drop(&mut self) {
        if let Some(guard) = self.guard.take() {
            guard.abort();
        }
    }
}

/// How many tool results the child's conversation holds.
fn tool_results(child: &AgentSession) -> u64 {
    child
        .manager
        .lock()
        .unwrap()
        .build_session_context()
        .messages
        .iter()
        .filter(|message| matches!(message, AgentMessage::ToolResult(_)))
        .count() as u64
}

/// The status and final answer of the child's last turn.
pub(crate) fn turn_outcome(
    session: &AgentSession,
) -> (AgentStatus, Option<String>, Option<String>) {
    let manager = session.manager.lock().unwrap();
    let assistant = manager
        .build_session_context()
        .messages
        .into_iter()
        .rev()
        .find_map(|message| match message {
            AgentMessage::Assistant(assistant) => Some(assistant),
            _ => None,
        });
    let Some(assistant) = assistant else {
        return (
            AgentStatus::Failed,
            None,
            Some("the child returned no assistant message".into()),
        );
    };
    match assistant.stop_reason {
        StopReason::Error => (
            AgentStatus::Failed,
            None,
            Some(
                assistant
                    .error_message
                    .unwrap_or_else(|| "the child request failed".into()),
            ),
        ),
        StopReason::Aborted => (AgentStatus::Interrupted, None, Some("interrupted".into())),
        _ => {
            let text = assistant.text();
            if text.trim().is_empty() {
                (
                    AgentStatus::Failed,
                    None,
                    Some("the child returned an empty result".into()),
                )
            } else {
                (AgentStatus::Completed, Some(text), None)
            }
        }
    }
}

/// The user and assistant text of `messages` that a workflow or loop child
/// starts with; tool calls, tool results and notices are left out.
pub(crate) fn fork_messages(messages: &[AgentMessage], fork: ForkTurns) -> Vec<AgentMessage> {
    let safe = messages
        .iter()
        .filter(|message| match message {
            AgentMessage::User(_)
            | AgentMessage::BranchSummary(_)
            | AgentMessage::CompactionSummary(_) => true,
            AgentMessage::Assistant(assistant) => assistant.tool_calls().next().is_none(),
            AgentMessage::ToolResult(_)
            | AgentMessage::BashExecution(_)
            | AgentMessage::Custom(_) => false,
        })
        .cloned()
        .collect::<Vec<_>>();

    match fork {
        ForkTurns::None => Vec::new(),
        ForkTurns::All => safe,
    }
}

/// `messages` as a fork starts with them: all of them, tool calls and
/// results included. A tool call without a result yet (the parent's call
/// that starts the fork, and the calls beside it) gets a stand-in result,
/// so the copy is a conversation a provider takes.
pub(crate) fn exact_copy(messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
    let answered: std::collections::HashSet<String> = messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::ToolResult(result) => Some(result.tool_call_id.clone()),
            _ => None,
        })
        .collect();
    let mut copy = Vec::with_capacity(messages.len());
    let mut open: Vec<(String, String)> = Vec::new();
    for message in messages {
        if !matches!(message, AgentMessage::ToolResult(_)) {
            close(&mut copy, &mut open);
        }
        if let AgentMessage::Assistant(assistant) = &message {
            open = assistant
                .tool_calls()
                .filter(|call| !answered.contains(&call.id))
                .map(|call| (call.id.clone(), call.name.clone()))
                .collect();
        }
        copy.push(message);
    }
    close(&mut copy, &mut open);
    copy
}

/// Append the stand-in results of the `open` tool calls.
fn close(copy: &mut Vec<AgentMessage>, open: &mut Vec<(String, String)>) {
    for (id, name) in open.drain(..) {
        copy.push(AgentMessage::ToolResult(super_ai::ToolResultMessage {
            tool_call_id: id,
            tool_name: name,
            content: vec![super_ai::ContentBlock::text(
                super_agent::placeholder::prompt_todo("AgentType.fork.pending_result"),
            )],
            details: None,
            usage: None,
            is_error: false,
            timestamp: super_ai::now_ms(),
        }));
    }
}

fn usage_delta(after: Usage, before: Usage) -> Usage {
    Usage {
        input: after.input.saturating_sub(before.input),
        output: after.output.saturating_sub(before.output),
        cache_read: after.cache_read.saturating_sub(before.cache_read),
        cache_write: after.cache_write.saturating_sub(before.cache_write),
        cache_write_1h: after.cache_write_1h.saturating_sub(before.cache_write_1h),
        cache_read_available: after.cache_read_available,
        reasoning: after
            .reasoning
            .map(|after| after.saturating_sub(before.reasoning.unwrap_or_default())),
        total_tokens: after.total_tokens.saturating_sub(before.total_tokens),
        cost: super_ai::Cost {
            input: (after.cost.input - before.cost.input).max(0.0),
            output: (after.cost.output - before.cost.output).max(0.0),
            cache_read: (after.cost.cache_read - before.cost.cache_read).max(0.0),
            cache_write: (after.cost.cache_write - before.cost.cache_write).max(0.0),
            total: (after.cost.total - before.cost.total).max(0.0),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use super_ai::{AssistantMessage, ContentBlock, ToolCall};

    fn assistant(text: &str) -> AgentMessage {
        let mut message = AssistantMessage::empty("fake", "fake", "fake");
        message.content.push(ContentBlock::text(text));
        AgentMessage::Assistant(message)
    }

    #[test]
    fn context_fork_removes_tool_pairs() {
        let mut tool_message = AssistantMessage::empty("fake", "fake", "fake");
        tool_message.content.push(ContentBlock::ToolCall(ToolCall {
            id: "call".into(),
            name: "Read".into(),
            arguments: json!({}),
            thought_signature: None,
        }));
        let history = vec![
            AgentMessage::user("first"),
            assistant("one"),
            AgentMessage::Assistant(tool_message),
            AgentMessage::user("second"),
            assistant("two"),
        ];

        assert!(fork_messages(&history, ForkTurns::None).is_empty());
        let all = fork_messages(&history, ForkTurns::All);
        assert_eq!(all.len(), 4);
        assert!(
            all.iter()
                .all(|message| !matches!(message, AgentMessage::ToolResult(_)))
        );
    }
}
