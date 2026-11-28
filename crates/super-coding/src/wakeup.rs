//! Wake-ups: a timer of the session that brings the agent back later, and
//! the self-paced loop built on it.
//!
//! `ScheduleWakeup` asks for one with [`Wakeups::schedule`]. A session has
//! at most one pending wake-up; a new one replaces it. Its delay is kept
//! between [`MIN_DELAY_SECONDS`] and [`MAX_DELAY_SECONDS`]. When it is due
//! it goes through the session's queue ([`AgentSession::queue_entry`]) at
//! [`Priority::Later`]: an idle session starts a turn with it, a busy one
//! takes it once its turn is over. While it waits it holds the session
//! ([`crate::BackgroundTasks::hold`]), so the daemon keeps the session
//! loaded.
//!
//! The pending wake-up and the self-paced loop are stored in the session
//! ([`crate::stored_state::WAKEUP_ENTRY`]) whenever they change, and come
//! back when the session is loaded again ([`Wakeups::restore`], started by
//! [`crate::AgentSession::resume_stored_work`]): a wake-up whose time has
//! passed fires at once, a later one at its time, and the loop goes on
//! from the round it was in.
//!
//! A self-paced loop (`/loop` without an interval in the advanced mode, see
//! [`crate::iterative`]) runs in the session itself. Each round is a turn
//! whose message is the loop's prompt ([`crate::notifications::loop_round`]).
//! While a loop runs, a wake-up continues it: when it is due the next round
//! starts. `stop` ends the loop, and so does a session that has nothing
//! left to do (no turn, no queued entry, no running task) while no wake-up
//! is pending for the loop.

use crate::iterative::JobId;
use crate::notifications::{Priority, wakeup_message};
use crate::session_runner::AgentSession;
use crate::tasks::KeepAlive;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::watch;
use tokio_util::sync::{CancellationToken, DropGuard};

/// The shortest delay of a wake-up; a shorter one is made this long.
pub const MIN_DELAY_SECONDS: u64 = 60;

/// The longest delay of a wake-up; a longer one is made this long.
pub const MAX_DELAY_SECONDS: u64 = 3_600;

/// A pending wake-up, as clients and the tool's result see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeupInfo {
    pub id: u64,
    /// The delay it waits, after it was kept between the bounds.
    pub delay_seconds: u64,
    /// When it is due, in milliseconds since the epoch.
    pub due_at: i64,
    /// Why the model chose this delay; shown and recorded, nothing else.
    pub reason: Option<String>,
    /// The model marked this wake-up as a quiet one; shown and recorded,
    /// nothing else.
    pub noop: bool,
    /// What the model asked to be brought back. A wake-up outside a loop
    /// brings exactly this; a loop's next round brings the loop's prompt.
    pub prompt: Option<String>,
    /// The self-paced loop it continues.
    pub job: Option<JobId>,
    /// The loop's round it starts.
    pub round: Option<u32>,
}

/// What `ScheduleWakeup` asks for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WakeupRequest {
    pub delay_seconds: f64,
    pub reason: Option<String>,
    pub prompt: Option<String>,
    pub noop: bool,
}

/// What [`Wakeups::schedule`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum Scheduled {
    /// A wake-up is pending: the loop's next round, or a prompt outside a
    /// loop.
    Pending(WakeupInfo),
    /// The loop ran the most rounds it may, so it ends instead.
    LoopEnds { round: u32, max_rounds: u32 },
}

/// What [`Wakeups::stop`] stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum Stopped {
    /// The self-paced loop, after this round.
    Loop { round: u32 },
    /// The pending wake-up outside a loop.
    Wakeup(WakeupInfo),
    /// Nothing ran and nothing was pending.
    Nothing,
}

/// What the current round of a loop asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    /// No wake-up yet in this round.
    Undecided,
    /// A wake-up continues the loop.
    Continue,
    /// The loop is over.
    Stop,
}

/// Where a loop's round stands, for its driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RoundState {
    pub decision: Decision,
    /// The wake-up that continues the loop came due.
    pub due: bool,
}

struct Pending {
    info: WakeupInfo,
    /// Cancels the timer when the wake-up is replaced or dropped.
    _timer: DropGuard,
    _hold: KeepAlive,
}

struct LoopState {
    job: JobId,
    /// The loop's prompt, for the session's stored state.
    goal: String,
    round: u32,
    max_rounds: u32,
    decision: Decision,
    due: bool,
    /// The wake-up that came due, for the next round's message.
    woken: Option<WakeupInfo>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    pending: Option<Pending>,
    running_loop: Option<LoopState>,
    /// What the session stored, brought back when it was loaded and not
    /// started yet.
    restored: Option<StoredWakeups>,
}

/// A self-paced loop as the session stores it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredLoop {
    pub job: JobId,
    pub goal: String,
    /// The round it is in; 0 before its first.
    pub round: u32,
    pub max_rounds: u32,
}

/// The pending wake-up and the self-paced loop as the session stores them
/// ([`crate::stored_state::WAKEUP_ENTRY`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredWakeups {
    pub wakeup: Option<WakeupInfo>,
    #[serde(rename = "loop")]
    pub running_loop: Option<StoredLoop>,
}

/// What [`Wakeups::restore`] gives back to start: the loop that goes on, and
/// the wake-up that continues it (with the loop's new job id), if one was
/// pending.
#[derive(Debug, Clone, PartialEq)]
pub struct RestoredLoop {
    pub goal: String,
    pub round: u32,
    pub max_rounds: u32,
    pub wakeup: Option<WakeupInfo>,
}

/// The pending wake-up and the self-paced loop of one session: see
/// [`AgentSession::wakeups`].
pub struct Wakeups {
    session: Weak<AgentSession>,
    state: Mutex<State>,
    /// Bumped whenever the loop's state changes, for its driver.
    changes: watch::Sender<u64>,
    /// Held while the state is written down, so that writes come in the
    /// order of the changes.
    recording: Mutex<()>,
}

impl Wakeups {
    pub(crate) fn new(session: Weak<AgentSession>) -> Arc<Self> {
        Arc::new(Wakeups {
            session,
            state: Mutex::new(State::default()),
            changes: watch::channel(0).0,
            recording: Mutex::new(()),
        })
    }

    /// The delay a wake-up asked for `requested` seconds waits: rounded and
    /// kept between [`MIN_DELAY_SECONDS`] and [`MAX_DELAY_SECONDS`]; `None`
    /// for a value that is not a number.
    pub fn delay_for(requested: f64) -> Option<u64> {
        if !requested.is_finite() {
            return None;
        }
        Some(
            requested
                .round()
                .clamp(MIN_DELAY_SECONDS as f64, MAX_DELAY_SECONDS as f64) as u64,
        )
    }

    /// Schedule a wake-up; it replaces the pending one. While a self-paced
    /// loop runs it continues the loop with its next round, unless the loop
    /// ran its last round already; outside a loop it brings `prompt` back,
    /// which must not be empty then.
    pub fn schedule(self: &Arc<Self>, request: WakeupRequest) -> Result<Scheduled, String> {
        let delay = Self::delay_for(request.delay_seconds).ok_or_else(|| {
            format!(
                "delaySeconds must be a number of seconds between {MIN_DELAY_SECONDS} and {MAX_DELAY_SECONDS}."
            )
        })?;
        let session = self
            .session
            .upgrade()
            .ok_or("The session has ended, so no wake-up can be scheduled.")?;
        let prompt = request.prompt.filter(|prompt| !prompt.trim().is_empty());
        let reason = request.reason.filter(|reason| !reason.trim().is_empty());
        let mut state = self.state.lock().unwrap();
        let (job, round) = match state.running_loop.as_mut() {
            Some(running) if running.round >= running.max_rounds => {
                running.decision = Decision::Stop;
                running.due = false;
                let ended = Scheduled::LoopEnds {
                    round: running.round,
                    max_rounds: running.max_rounds,
                };
                state.pending = None;
                drop(state);
                self.changed();
                self.record();
                return Ok(ended);
            }
            Some(running) => {
                running.decision = Decision::Continue;
                running.due = false;
                (Some(running.job), Some(running.round + 1))
            }
            None if prompt.is_none() => {
                return Err(
                    "Give the prompt the wake-up brings back; no loop runs in this session.".into(),
                );
            }
            None => (None, None),
        };
        state.next_id += 1;
        let info = WakeupInfo {
            id: state.next_id,
            delay_seconds: delay,
            due_at: super_ai::now_ms().saturating_add((delay as i64).saturating_mul(1_000)),
            reason,
            noop: request.noop,
            prompt,
            job,
            round,
        };
        state.pending = Some(self.arm(&session, info.clone(), Duration::from_secs(delay)));
        drop(state);
        self.changed();
        self.record();
        Ok(Scheduled::Pending(info))
    }

    /// The pending wake-up `info`, whose timer fires after `delay`.
    fn arm(self: &Arc<Self>, session: &AgentSession, info: WakeupInfo, delay: Duration) -> Pending {
        let timer = CancellationToken::new();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let token = timer.clone();
            let wakeups = Arc::downgrade(self);
            let id = info.id;
            runtime.spawn(async move {
                tokio::select! {
                    _ = token.cancelled() => {}
                    _ = tokio::time::sleep(delay) => {
                        if let Some(wakeups) = wakeups.upgrade() {
                            wakeups.fire(id);
                        }
                    }
                }
            });
        }
        Pending {
            info,
            _timer: timer.drop_guard(),
            _hold: session.background_tasks().hold(),
        }
    }

    /// End the self-paced loop, or cancel the pending wake-up outside a
    /// loop.
    pub fn stop(&self) -> Stopped {
        let mut state = self.state.lock().unwrap();
        let stopped = if let Some(running) = state.running_loop.as_mut() {
            running.decision = Decision::Stop;
            running.due = false;
            let round = running.round;
            state.pending = None;
            Stopped::Loop { round }
        } else {
            match state.pending.take() {
                Some(pending) => Stopped::Wakeup(pending.info),
                None => Stopped::Nothing,
            }
        };
        drop(state);
        self.changed();
        if stopped != Stopped::Nothing {
            self.record();
        }
        stopped
    }

    /// The pending wake-up, if one waits.
    pub fn pending(&self) -> Option<WakeupInfo> {
        self.state
            .lock()
            .unwrap()
            .pending
            .as_ref()
            .map(|pending| pending.info.clone())
    }

    /// The self-paced loop that runs: its job, its round and the most rounds
    /// it may run.
    pub fn running_loop(&self) -> Option<(JobId, u32, u32)> {
        self.state
            .lock()
            .unwrap()
            .running_loop
            .as_ref()
            .map(|running| (running.job, running.round, running.max_rounds))
    }

    /// Forget the pending wake-up and the loop, for a session that moves to
    /// another conversation; the loop's job is stopped by its runtime.
    /// The stored state is not written: the conversation that is left keeps
    /// it for when it is loaded again.
    pub(crate) fn reset(&self) {
        {
            let mut state = self.state.lock().unwrap();
            state.pending = None;
            state.running_loop = None;
            state.restored = None;
        }
        self.changed();
    }

    /// A wake-up came due: queue it, or let the loop it continues go on.
    fn fire(&self, id: u64) {
        let mut state = self.state.lock().unwrap();
        let Some(info) = state
            .pending
            .as_ref()
            .filter(|pending| pending.info.id == id)
            .map(|pending| pending.info.clone())
        else {
            return;
        };
        match info.job {
            Some(job) => {
                let pending = state.pending.take();
                if let Some(running) = state.running_loop.as_mut()
                    && running.job == job
                    && running.decision == Decision::Continue
                {
                    running.due = true;
                    running.woken = Some(info);
                }
                drop(state);
                self.changed();
                self.record();
                drop(pending);
            }
            None => {
                drop(state);
                // Queued before it stops being pending, so that whoever
                // waits for the session's work always sees one of the two.
                if let Some(session) = self.session.upgrade() {
                    session.queue_entry(wakeup_message(&info), Priority::Later);
                }
                let mut state = self.state.lock().unwrap();
                let pending = state.pending.take_if(|pending| pending.info.id == id);
                drop(state);
                self.changed();
                self.record();
                // Released only now: the queued entry keeps the session.
                drop(pending);
            }
        }
    }

    /// Take the pending wake-up outside a loop away from this process
    /// without writing anything down, for a session another process takes
    /// over (the terminal's `/remote`): it comes back there from what the
    /// session stored. [`Wakeups::unpark`] gives it back when the session
    /// stays here after all.
    pub fn park(&self) -> Option<WakeupInfo> {
        let mut state = self.state.lock().unwrap();
        if state
            .pending
            .as_ref()
            .is_none_or(|pending| pending.info.job.is_some())
        {
            return None;
        }
        let parked = state.pending.take().map(|pending| pending.info.clone());
        drop(state);
        self.changed();
        parked
    }

    /// Make the wake-up [`Wakeups::park`] took pending again, due at its
    /// time, unless another one is pending meanwhile.
    pub fn unpark(self: &Arc<Self>, wakeup: WakeupInfo) {
        let Some(session) = self.session.upgrade() else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if state.pending.is_none() {
            let delay = due_in(wakeup.due_at);
            state.pending = Some(self.arm(&session, wakeup, delay));
        }
        drop(state);
        self.changed();
    }

    /// Whether a wake-up is pending or a self-paced loop runs: work that
    /// will reach the agent later, which `super -p` waits for.
    pub fn is_waiting(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.pending.is_some() || state.running_loop.is_some()
    }

    /// Changes of the pending wake-up and of the loop.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// The pending wake-up and the loop as the session stores them.
    pub fn stored(&self) -> StoredWakeups {
        let state = self.state.lock().unwrap();
        StoredWakeups {
            wakeup: state.pending.as_ref().map(|pending| pending.info.clone()),
            running_loop: state.running_loop.as_ref().map(|running| StoredLoop {
                job: running.job,
                goal: running.goal.clone(),
                round: running.round,
                max_rounds: running.max_rounds,
            }),
        }
    }

    /// Write the state down in the session ([`crate::stored_state`]).
    fn record(&self) {
        let Some(session) = self.session.upgrade() else {
            return;
        };
        if session.is_remote() {
            return;
        }
        let _order = self.recording.lock().unwrap();
        let stored = self.stored();
        let data = serde_json::to_value(&stored).unwrap_or_default();
        crate::stored_state::record(
            &mut session.manager.lock().unwrap(),
            crate::stored_state::WAKEUP_ENTRY,
            data,
        );
    }

    /// Take what the session stored, when it is loaded again; it waits
    /// until [`Wakeups::resume`].
    pub(crate) fn restore(&self, stored: StoredWakeups) {
        let empty = stored.wakeup.is_none() && stored.running_loop.is_none();
        self.state.lock().unwrap().restored = (!empty).then_some(stored);
    }

    /// Start what [`Wakeups::restore`] took: the pending wake-up outside a
    /// loop is pending again, due at its time (at once when that has
    /// passed). The loop is handed back for its runtime to start again
    /// with the new job id `job_id` ([`Wakeups::resume_loop`]); without a
    /// runtime it is gone.
    pub(crate) fn resume(self: &Arc<Self>) -> Option<RestoredLoop> {
        let session = self.session.upgrade()?;
        let restored = self.state.lock().unwrap().restored.take()?;
        let running_loop = restored.running_loop.map(|stored| RestoredLoop {
            goal: stored.goal,
            round: stored.round,
            max_rounds: stored.max_rounds,
            wakeup: restored
                .wakeup
                .clone()
                .filter(|wakeup| wakeup.job == Some(stored.job)),
        });
        if let Some(wakeup) = restored.wakeup.filter(|wakeup| wakeup.job.is_none()) {
            let mut state = self.state.lock().unwrap();
            if state.pending.is_none() {
                state.next_id = state.next_id.max(wakeup.id);
                let delay = due_in(wakeup.due_at);
                state.pending = Some(self.arm(&session, wakeup, delay));
            }
            drop(state);
            self.changed();
        }
        running_loop
    }

    /// Start the loop `restored` again as the job `job`: in its round, which
    /// the wake-up it had continues when it is due. Returns whether that
    /// wake-up is pending; without one the round runs again.
    pub(crate) fn resume_loop(
        self: &Arc<Self>,
        job: JobId,
        restored: &RestoredLoop,
    ) -> Result<bool, String> {
        let session = self
            .session
            .upgrade()
            .ok_or("The session has ended, so its loop cannot go on.")?;
        let mut state = self.state.lock().unwrap();
        if state.running_loop.is_some() {
            return Err("a self-paced loop runs in this session already".into());
        }
        let waiting = restored.wakeup.is_some();
        state.running_loop = Some(LoopState {
            job,
            goal: restored.goal.clone(),
            round: restored.round,
            max_rounds: restored.max_rounds,
            decision: if waiting {
                Decision::Continue
            } else {
                Decision::Undecided
            },
            due: false,
            woken: None,
        });
        if let Some(mut wakeup) = restored.wakeup.clone() {
            wakeup.job = Some(job);
            state.next_id = state.next_id.max(wakeup.id);
            let delay = due_in(wakeup.due_at);
            state.pending = Some(self.arm(&session, wakeup, delay));
        }
        drop(state);
        self.changed();
        self.record();
        Ok(waiting)
    }

    fn changed(&self) {
        self.changes
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    // ----------------------------------------------------------------
    // The loop's driver ([`crate::iterative`]).
    // ----------------------------------------------------------------

    /// Start the self-paced loop `job`; a session runs one at a time.
    pub(crate) fn begin_loop(&self, job: JobId, max_rounds: u32, goal: &str) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.running_loop.is_some() {
            return Err("a self-paced loop runs in this session already".into());
        }
        state.running_loop = Some(LoopState {
            job,
            goal: goal.to_string(),
            round: 0,
            max_rounds,
            decision: Decision::Undecided,
            due: false,
            woken: None,
        });
        drop(state);
        self.changed();
        self.record();
        Ok(())
    }

    /// Round `round` of `job` starts: nothing is decided for it yet. Returns
    /// the wake-up that started it.
    pub(crate) fn begin_round(&self, job: JobId, round: u32) -> Option<WakeupInfo> {
        let mut state = self.state.lock().unwrap();
        let running = state
            .running_loop
            .as_mut()
            .filter(|running| running.job == job)?;
        running.round = round;
        running.decision = Decision::Undecided;
        running.due = false;
        let woken = running.woken.take();
        drop(state);
        self.record();
        woken
    }

    /// Where the current round of `job` stands; `None` once the loop is gone.
    pub(crate) fn round_state(&self, job: JobId) -> Option<RoundState> {
        self.state
            .lock()
            .unwrap()
            .running_loop
            .as_ref()
            .filter(|running| running.job == job)
            .map(|running| RoundState {
                decision: running.decision,
                due: running.due,
            })
    }

    /// Changes of the loop's state, for its driver to wait on.
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// The loop `job` is over: forget it and the wake-up that would continue
    /// it.
    pub(crate) fn end_loop(&self, job: JobId) {
        let mut state = self.state.lock().unwrap();
        let ended = state
            .running_loop
            .as_ref()
            .is_some_and(|running| running.job == job);
        if ended {
            state.running_loop = None;
            if state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.info.job == Some(job))
            {
                state.pending = None;
            }
        }
        drop(state);
        self.changed();
        if ended {
            self.record();
        }
    }
}

/// How long until `due_at` (milliseconds since the epoch); nothing once it
/// has passed.
fn due_in(due_at: i64) -> Duration {
    Duration::from_millis(due_at.saturating_sub(super_ai::now_ms()).max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delay_is_kept_between_a_minute_and_an_hour() {
        assert_eq!(Wakeups::delay_for(0.0), Some(60));
        assert_eq!(Wakeups::delay_for(-5.0), Some(60));
        assert_eq!(Wakeups::delay_for(59.4), Some(60));
        assert_eq!(Wakeups::delay_for(60.0), Some(60));
        assert_eq!(Wakeups::delay_for(270.6), Some(271));
        assert_eq!(Wakeups::delay_for(3_600.0), Some(3_600));
        assert_eq!(Wakeups::delay_for(86_400.0), Some(3_600));
        assert_eq!(Wakeups::delay_for(f64::NAN), None);
        assert_eq!(Wakeups::delay_for(f64::INFINITY), None);
    }
}
