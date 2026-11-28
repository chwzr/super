//! Bounded loop and autoresearch jobs in persistent child sessions, and the
//! self-paced loop that runs in the session itself.
//!
//! `/loop` with an interval, `/loop` in the basic mode and `/autoresearch`
//! run their iterations back to back (or every interval) in a child session
//! ([`IterativeRuntime::start`]). `/loop` without an interval in the advanced
//! mode is self-paced ([`IterativeRuntime::start_self_paced`]): each round is
//! a turn of the session itself, started with the loop's prompt
//! ([`crate::notifications::loop_round`]), and the model decides with
//! `ScheduleWakeup` when the next round comes or that the loop is over
//! ([`crate::wakeup`]).

use crate::child_turn;
use crate::child_turn::{AgentStatus, ForkTurns};
use crate::notifications::Priority;
use crate::session_runner::AgentSession;
use crate::wakeup::Decision;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, Semaphore, watch};
use tokio_util::sync::CancellationToken;

pub type JobId = u64;

const MAX_ACTIVE_JOBS: usize = 4;
const MAX_RETAINED_JOBS: usize = 20;
pub const MAX_ITERATIONS: u32 = 100;

pub fn format_interval(interval: Duration) -> String {
    let mut nanos = interval.as_nanos();
    let units = [
        (86_400_000_000_000_u128, "d"),
        (3_600_000_000_000, "h"),
        (60_000_000_000, "m"),
        (1_000_000_000, "s"),
        (1_000_000, "ms"),
        (1_000, "us"),
        (1, "ns"),
    ];
    let mut output = String::new();
    for (size, suffix) in units {
        let count = nanos / size;
        if count > 0 {
            output.push_str(&format!("{count}{suffix}"));
            nanos %= size;
        }
    }
    if output.is_empty() {
        output.push_str("0s");
    }
    output
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Loop,
    Autoresearch,
}

impl JobKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Loop => "loop",
            Self::Autoresearch => "autoresearch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Stopped,
}

impl JobStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    pub fn is_finished(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Stopped)
    }
}

#[derive(Debug, Clone)]
pub struct JobSnapshot {
    pub id: JobId,
    pub kind: JobKind,
    pub goal: String,
    pub status: JobStatus,
    pub iteration: u32,
    pub limit: Option<u32>,
    pub interval: Option<Duration>,
    pub tokens: u64,
    pub elapsed: Duration,
    pub session_id: String,
    pub latest_result: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct JobSummary {
    pub id: JobId,
    pub kind: JobKind,
    pub goal: String,
    pub status: JobStatus,
    pub iteration: u32,
    pub limit: Option<u32>,
    pub interval: Option<Duration>,
    pub tokens: u64,
    pub elapsed: Duration,
}

struct JobState {
    status: JobStatus,
    iteration: u32,
    tokens: u64,
    latest_result: Option<String>,
    error: Option<String>,
}

pub struct JobRecord {
    id: JobId,
    kind: JobKind,
    goal: String,
    limit: Option<u32>,
    interval: Option<Duration>,
    session_id: String,
    /// The child session the iterations run in; `None` for a self-paced
    /// loop, whose rounds are turns of the session itself.
    child: Option<Arc<AgentSession>>,
    started: Instant,
    state: Mutex<JobState>,
    cancel: CancellationToken,
    wake: Notify,
    version: watch::Sender<u64>,
    parent: Weak<AgentSession>,
}

impl JobRecord {
    pub fn snapshot(&self) -> JobSnapshot {
        let state = self.state.lock().unwrap();
        JobSnapshot {
            id: self.id,
            kind: self.kind,
            goal: self.goal.clone(),
            status: state.status,
            iteration: state.iteration,
            limit: self.limit,
            interval: self.interval,
            tokens: state.tokens,
            elapsed: self.started.elapsed(),
            session_id: self.session_id.clone(),
            latest_result: state.latest_result.clone(),
            error: state.error.clone(),
        }
    }

    fn summary(&self) -> JobSummary {
        let state = self.state.lock().unwrap();
        JobSummary {
            id: self.id,
            kind: self.kind,
            goal: self.goal.clone(),
            status: state.status,
            iteration: state.iteration,
            limit: self.limit,
            interval: self.interval,
            tokens: state.tokens,
            elapsed: self.started.elapsed(),
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.version.subscribe()
    }

    pub fn pause(&self) {
        let mut state = self.state.lock().unwrap();
        if matches!(state.status, JobStatus::Running | JobStatus::Queued) {
            state.status = JobStatus::Paused;
            drop(state);
            self.signal();
        }
    }

    pub fn resume(&self) {
        let mut state = self.state.lock().unwrap();
        if state.status == JobStatus::Paused {
            state.status = JobStatus::Running;
            drop(state);
            self.wake.notify_waiters();
            self.signal();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.state.lock().unwrap().status == JobStatus::Paused
    }

    pub fn stop(&self) {
        if self.state.lock().unwrap().status.is_finished() {
            return;
        }
        self.cancel.cancel();
        if let Some(child) = &self.child {
            child.abort();
        }
        self.wake.notify_waiters();
    }

    fn signal(&self) {
        self.version
            .send_modify(|version| *version = version.wrapping_add(1));
        if let Some(parent) = self.parent.upgrade() {
            parent.emit_iterative(self.id, *self.version.borrow());
        }
    }

    fn set_status(&self, status: JobStatus) {
        self.state.lock().unwrap().status = status;
        self.signal();
    }

    /// Whether the rounds run in the session itself: a self-paced loop.
    pub fn is_self_paced(&self) -> bool {
        self.child.is_none()
    }

    async fn wait_until_ready(&self) -> bool {
        loop {
            if self.cancel.is_cancelled() {
                return false;
            }
            if !self.is_paused() {
                return true;
            }
            tokio::select! {
                _ = self.cancel.cancelled() => return false,
                _ = self.wake.notified() => {}
            }
        }
    }
}

pub struct IterativeRuntime {
    parent: Weak<AgentSession>,
    jobs: Mutex<Vec<Arc<JobRecord>>>,
    next_id: AtomicU64,
    permits: Arc<Semaphore>,
}

impl IterativeRuntime {
    pub(crate) fn new(parent: Weak<AgentSession>) -> Arc<Self> {
        Arc::new(Self {
            parent,
            jobs: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            permits: Arc::new(Semaphore::new(MAX_ACTIVE_JOBS)),
        })
    }

    pub fn start(
        self: &Arc<Self>,
        kind: JobKind,
        goal: impl Into<String>,
        limit: Option<u32>,
        interval: Option<Duration>,
    ) -> anyhow::Result<Arc<JobRecord>> {
        let goal = goal.into();
        if goal.trim().is_empty() {
            anyhow::bail!("the job goal cannot be empty");
        }
        let parent = self
            .parent
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("the parent session has closed"))?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        if limit.is_some() && interval.is_some() {
            anyhow::bail!("an interval and --iterations cannot be used together");
        }
        if kind == JobKind::Autoresearch && interval.is_some() {
            anyhow::bail!("autoresearch does not support an interval");
        }
        if interval.is_some_and(|interval| interval.is_zero()) {
            anyhow::bail!("the interval must be greater than zero");
        }
        let limit = limit.map(|limit| limit.clamp(1, MAX_ITERATIONS));
        let task_name = format!("{}_{}", kind.label(), id);
        let child = parent.create_subagent_session(
            &task_name,
            &format!("/root/{task_name}"),
            ForkTurns::All,
            None,
            None,
        )?;
        let session_id = child.manager.lock().unwrap().session_id().to_string();
        let (version, _) = watch::channel(0);
        let record = Arc::new(JobRecord {
            id,
            kind,
            goal,
            limit,
            interval,
            session_id,
            child: Some(child),
            started: Instant::now(),
            state: Mutex::new(JobState {
                status: JobStatus::Queued,
                iteration: 0,
                tokens: 0,
                latest_result: None,
                error: None,
            }),
            cancel: CancellationToken::new(),
            wake: Notify::new(),
            version,
            parent: self.parent.clone(),
        });

        self.remember(&record)?;
        let runtime = self.clone();
        let running = record.clone();
        tokio::spawn(async move { runtime.run(running).await });
        record.signal();
        Ok(record)
    }

    /// Start a self-paced loop with `goal` in the session itself: round 1
    /// starts at once (after the turn that runs, if one does), and every
    /// further round when the wake-up the model scheduled is due. It ends
    /// when the model stops it, when the session has nothing left to do and
    /// no wake-up is pending, or after `limit` rounds (at most
    /// [`MAX_ITERATIONS`], the default). A session runs one at a time.
    pub fn start_self_paced(
        self: &Arc<Self>,
        goal: impl Into<String>,
        limit: Option<u32>,
    ) -> anyhow::Result<Arc<JobRecord>> {
        let goal = goal.into();
        if goal.trim().is_empty() {
            anyhow::bail!("the job goal cannot be empty");
        }
        let parent = self
            .parent
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("the parent session has closed"))?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let limit = limit.unwrap_or(MAX_ITERATIONS).clamp(1, MAX_ITERATIONS);
        parent
            .wakeups()
            .begin_loop(id, limit)
            .map_err(anyhow::Error::msg)?;
        let session_id = parent.manager.lock().unwrap().session_id().to_string();
        let (version, _) = watch::channel(0);
        let record = Arc::new(JobRecord {
            id,
            kind: JobKind::Loop,
            goal,
            limit: Some(limit),
            interval: None,
            session_id,
            child: None,
            started: Instant::now(),
            state: Mutex::new(JobState {
                status: JobStatus::Queued,
                iteration: 0,
                tokens: 0,
                latest_result: None,
                error: None,
            }),
            cancel: CancellationToken::new(),
            wake: Notify::new(),
            version,
            parent: self.parent.clone(),
        });
        if let Err(error) = self.remember(&record) {
            parent.wakeups().end_loop(id);
            return Err(error);
        }
        let running = record.clone();
        tokio::spawn(async move { run_self_paced(running).await });
        record.signal();
        Ok(record)
    }

    fn remember(&self, record: &Arc<JobRecord>) -> anyhow::Result<()> {
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| anyhow::anyhow!("the job list is unavailable"))?;
        jobs.push(record.clone());
        while jobs.len() > MAX_RETAINED_JOBS {
            let Some(position) = jobs
                .iter()
                .position(|job| job.state.lock().unwrap().status.is_finished())
            else {
                break;
            };
            jobs.remove(position);
        }
        Ok(())
    }

    async fn run(self: Arc<Self>, job: Arc<JobRecord>) {
        let permit = tokio::select! {
            _ = job.cancel.cancelled() => None,
            permit = self.permits.clone().acquire_owned() => permit.ok(),
        };
        let Some(_permit) = permit else {
            job.set_status(JobStatus::Stopped);
            return;
        };
        if !job.wait_until_ready().await {
            job.set_status(JobStatus::Stopped);
            return;
        }
        job.set_status(JobStatus::Running);

        let mut iteration = 1_u32;
        loop {
            if !job.wait_until_ready().await {
                job.set_status(JobStatus::Stopped);
                return;
            }
            job.state.lock().unwrap().iteration = iteration;
            job.signal();

            let Some(child) = &job.child else {
                job.set_status(JobStatus::Failed);
                return;
            };
            let outcome = child_turn::run_child_turn(
                &job.parent,
                child,
                super_agent::AgentMessage::user(iteration_prompt(
                    job.kind, &job.goal, iteration, job.limit,
                )),
                Some(job.cancel.clone()),
                None,
            )
            .await;
            {
                let mut state = job.state.lock().unwrap();
                state.tokens = state.tokens.saturating_add(outcome.usage.total_tokens);
                state.latest_result = outcome.result.clone();
                state.error = outcome.error.clone();
            }
            job.signal();

            if job.cancel.is_cancelled() || outcome.status == AgentStatus::Interrupted {
                job.set_status(JobStatus::Stopped);
                return;
            }
            if outcome.status == AgentStatus::Failed {
                job.set_status(JobStatus::Failed);
                return;
            }
            if outcome
                .result
                .as_deref()
                .is_some_and(contains_completion_marker)
            {
                job.set_status(JobStatus::Completed);
                return;
            }
            if job.limit == Some(iteration) {
                job.set_status(JobStatus::Completed);
                return;
            }
            if let Some(interval) = job.interval {
                tokio::select! {
                    _ = job.cancel.cancelled() => {
                        job.set_status(JobStatus::Stopped);
                        return;
                    }
                    _ = tokio::time::sleep(interval) => {}
                }
            }
            iteration = iteration.saturating_add(1);
        }
    }

    pub fn get(&self, id: JobId) -> Option<Arc<JobRecord>> {
        self.jobs
            .lock()
            .ok()?
            .iter()
            .find(|job| job.id == id)
            .cloned()
    }

    pub fn summaries(&self) -> Vec<JobSummary> {
        self.jobs
            .lock()
            .map(|jobs| jobs.iter().map(|job| job.summary()).collect())
            .unwrap_or_default()
    }

    pub fn active_count(&self) -> usize {
        self.jobs
            .lock()
            .map(|jobs| {
                jobs.iter()
                    .filter(|job| !job.state.lock().unwrap().status.is_finished())
                    .count()
            })
            .unwrap_or(0)
    }

    pub fn latest(&self) -> Option<Arc<JobRecord>> {
        self.jobs.lock().ok()?.last().cloned()
    }

    pub(crate) fn stop_all(&self) {
        if let Ok(jobs) = self.jobs.lock() {
            for job in jobs.iter() {
                job.stop();
            }
        }
    }
}

/// How a round of a self-paced loop ended.
enum RoundEnd {
    /// The wake-up came due: the next round starts.
    Next,
    /// The model ended the loop, or the round left nothing to do and no
    /// wake-up pending.
    Over,
    /// The job was stopped, or the session went away.
    Stopped,
}

/// The rounds of a self-paced loop: see [`IterativeRuntime::start_self_paced`].
async fn run_self_paced(job: Arc<JobRecord>) {
    let Some(wakeups) = job.parent.upgrade().map(|session| session.wakeups()) else {
        job.set_status(JobStatus::Stopped);
        return;
    };
    let limit = job.limit.unwrap_or(MAX_ITERATIONS);
    let mut changes = wakeups.subscribe();
    let mut round = 0_u32;
    let status = loop {
        if !job.wait_until_ready().await {
            break JobStatus::Stopped;
        }
        round += 1;
        let woken_by = wakeups.begin_round(job.id, round);
        let Some(session) = job.parent.upgrade() else {
            break JobStatus::Stopped;
        };
        {
            let mut state = job.state.lock().unwrap();
            state.status = JobStatus::Running;
            state.iteration = round;
        }
        job.signal();
        let tokens_before = session.totals().total_tokens;
        let entries_before = session.manager.lock().unwrap().entries().len();
        session.queue_entry(
            crate::notifications::loop_round(job.id, &job.goal, round, limit, woken_by.as_ref()),
            Priority::Later,
        );
        drop(session);

        let end = round_end(&job, &wakeups, &mut changes).await;
        if let Some(session) = job.parent.upgrade() {
            let (result, error) = round_result(&session, entries_before);
            let mut state = job.state.lock().unwrap();
            let spent = session.totals().total_tokens.saturating_sub(tokens_before);
            state.tokens = state.tokens.saturating_add(spent);
            if result.is_some() {
                state.latest_result = result;
            }
            state.error = error;
        }
        job.signal();
        match end {
            RoundEnd::Next if round < limit => {}
            RoundEnd::Next | RoundEnd::Over => {
                let failed = job.state.lock().unwrap().error.is_some();
                break if failed {
                    JobStatus::Failed
                } else {
                    JobStatus::Completed
                };
            }
            RoundEnd::Stopped => break JobStatus::Stopped,
        }
    };
    wakeups.end_loop(job.id);
    job.set_status(status);
}

/// Wait until the current round of `job` ends: its wake-up came due, the
/// model stopped the loop, or the session has nothing left to do (no turn,
/// no queued entry, no running task) while no wake-up continues the loop.
/// Turns that happen meanwhile (the user's, a task's notice) belong to the
/// round: a wake-up they schedule continues the loop too.
async fn round_end(
    job: &JobRecord,
    wakeups: &crate::wakeup::Wakeups,
    changes: &mut watch::Receiver<u64>,
) -> RoundEnd {
    let mut settled = false;
    loop {
        changes.borrow_and_update();
        match wakeups.round_state(job.id) {
            None => return RoundEnd::Stopped,
            Some(state) if state.decision == Decision::Stop => return RoundEnd::Over,
            Some(state) if state.decision == Decision::Continue && state.due => {
                return RoundEnd::Next;
            }
            Some(state) if state.decision == Decision::Undecided && settled => {
                return RoundEnd::Over;
            }
            Some(_) => {}
        }
        let waited = if settled { None } else { job.parent.upgrade() };
        if !settled && waited.is_none() {
            return RoundEnd::Stopped;
        }
        let settle = async {
            match &waited {
                Some(session) => session.wait_for_background_work().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = job.cancel.cancelled() => return RoundEnd::Stopped,
            changed = changes.changed() => {
                if changed.is_err() {
                    return RoundEnd::Stopped;
                }
            }
            _ = settle, if !settled => settled = true,
        }
    }
}

/// The answer the round's turns ended with, and their error if the last one
/// failed: from the session's messages after the first `from` entries.
fn round_result(session: &AgentSession, from: usize) -> (Option<String>, Option<String>) {
    let manager = session.manager.lock().unwrap();
    let last = manager
        .entries()
        .iter()
        .skip(from)
        .rev()
        .find_map(|entry| match entry {
            crate::session::entry::SessionEntry::Message {
                message: super_agent::AgentMessage::Assistant(assistant),
                ..
            } => Some(assistant.clone()),
            _ => None,
        });
    match last {
        Some(assistant) if assistant.stop_reason == super_ai::StopReason::Error => (
            None,
            Some(
                assistant
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "the model request failed".into()),
            ),
        ),
        Some(assistant) => {
            let text = assistant.text();
            ((!text.trim().is_empty()).then_some(text), None)
        }
        None => (None, None),
    }
}

fn contains_completion_marker(result: &str) -> bool {
    result.to_ascii_lowercase().contains("[goal-complete]")
}

fn iteration_prompt(kind: JobKind, goal: &str, iteration: u32, limit: Option<u32>) -> String {
    let position = limit.map_or_else(
        || format!("iteration {iteration}"),
        |limit| format!("iteration {iteration} of {limit}"),
    );
    let common = format!(
        "You are in {position} for this goal:\n\n{goal}\n\nWork directly in the shared repository. Make one useful, verified unit of progress. Do not ask for permission. End the answer with exactly [goal-complete] if the goal is fully achieved and verified. Otherwise end with exactly [continue]."
    );
    match kind {
        JobKind::Loop => common,
        JobKind::Autoresearch => format!(
            "Autoresearch job. On the first iteration, establish a repeatable baseline and success metric. On each iteration, test one small idea, run the same verification, keep an improvement, and revert a regression. Protect existing behavior with focused tests. Record the measured result and the decision in the answer.\n\n{common}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use super_ai::{AssistantEvent, AssistantMessage, ContentBlock, EventStream, StopReason};

    #[test]
    fn unlimited_prompts_do_not_claim_a_limit() {
        let prompt = iteration_prompt(JobKind::Loop, "fix tests", 2, None);
        assert!(prompt.contains("iteration 2 for this goal"));
        assert!(!prompt.contains("iteration 2 of"));
    }

    #[test]
    fn interval_format_uses_compound_units() {
        assert_eq!(
            format_interval(Duration::from_secs(2 * 86_400 + 4 * 3_600)),
            "2d4h"
        );
        assert_eq!(format_interval(Duration::from_millis(1_500)), "1s500ms");
    }

    #[test]
    fn prompts_define_progress_and_completion() {
        let loop_prompt = iteration_prompt(JobKind::Loop, "fix tests", 2, Some(10));
        assert!(loop_prompt.contains("iteration 2 of 10"));
        assert!(loop_prompt.contains("[goal-complete]"));
        assert!(loop_prompt.contains("[continue]"));

        let research = iteration_prompt(JobKind::Autoresearch, "make it faster", 1, None);
        assert!(research.contains("baseline"));
        assert!(research.contains("success metric"));
        assert!(research.contains("revert a regression"));
    }

    #[test]
    fn completion_marker_is_case_insensitive() {
        assert!(contains_completion_marker("done\n[GOAL-COMPLETE]"));
        assert!(!contains_completion_marker("keep going\n[continue]"));
    }

    #[tokio::test]
    async fn one_job_reuses_its_forked_session_and_stops_early() {
        let registry = super_ai::Registry::from_builtin();
        let model = registry.all().first().unwrap().clone();
        let parent = AgentSession::new(
            crate::SessionManager::in_memory(std::path::Path::new("/test")),
            Vec::new(),
            registry,
            crate::Settings::default(),
            "test".into(),
            model,
            super_ai::ThinkingLevel::Off,
            None,
            Arc::new(|_| {}),
        );
        parent
            .manager
            .lock()
            .unwrap()
            .append_message(super_agent::AgentMessage::user("parent context"))
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        parent.set_stream_fn(Some(Arc::new({
            let calls = calls.clone();
            move |_, _, _| {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                let text = if call == 0 {
                    "first change\n[continue]"
                } else {
                    "verified\n[goal-complete]"
                };
                let mut message = AssistantMessage::empty("fake", "fake", "fake");
                message.content.push(ContentBlock::text(text));
                message.stop_reason = StopReason::Stop;
                let (sink, stream) = EventStream::channel();
                sink.send(AssistantEvent::Start {
                    partial: message.clone(),
                });
                sink.done(message);
                stream
            }
        })));

        let runtime = parent.iterative_jobs().unwrap();
        let job = runtime
            .start(JobKind::Loop, "finish it", None, None)
            .unwrap();
        let mut updates = job.subscribe();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !job.snapshot().status.is_finished() {
                updates.changed().await.unwrap();
            }
        })
        .await
        .unwrap();

        let snapshot = job.snapshot();
        assert_eq!(snapshot.status, JobStatus::Completed);
        assert_eq!(snapshot.iteration, 2);
        assert_eq!(snapshot.limit, None);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let messages = job
            .child
            .as_ref()
            .unwrap()
            .manager
            .lock()
            .unwrap()
            .build_session_context()
            .messages;
        assert!(matches!(
            messages.first(),
            Some(super_agent::AgentMessage::User(user)) if user.content.as_text() == "parent context"
        ));
        assert_eq!(
            messages
                .iter()
                .filter(|message| matches!(message, super_agent::AgentMessage::Assistant(_)))
                .count(),
            2
        );
        assert!(!snapshot.session_id.is_empty());
    }
}
