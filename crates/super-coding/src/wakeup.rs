//! Wake-ups: a timer of the session that brings the agent back later, and
//! the self-paced loop built on it.
//!
//! `ScheduleWakeup` asks for one with [`Wakeups::schedule`]. A session has
//! at most one pending wake-up; a new one replaces it. Its delay is kept
//! between [`MIN_DELAY_SECONDS`] and [`MAX_DELAY_SECONDS`]. When it is due
//! it goes through the session's queue ([`AgentSession::queue_entry`]) at
//! [`Priority::Later`]: an idle session starts a turn with it, a busy one
//! takes it once its turn is over. It lives only as long as the session's
//! process; nothing of it is stored. While it waits it holds the session
//! ([`crate::BackgroundTasks::hold`]), so the daemon keeps the session
//! loaded.
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
}

/// The pending wake-up and the self-paced loop of one session: see
/// [`AgentSession::wakeups`].
pub struct Wakeups {
    session: Weak<AgentSession>,
    state: Mutex<State>,
    /// Bumped whenever the loop's state changes, for its driver.
    changes: watch::Sender<u64>,
}

impl Wakeups {
    pub(crate) fn new(session: Weak<AgentSession>) -> Arc<Self> {
        Arc::new(Wakeups {
            session,
            state: Mutex::new(State::default()),
            changes: watch::channel(0).0,
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
        let timer = CancellationToken::new();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let token = timer.clone();
            let wakeups = Arc::downgrade(self);
            let id = info.id;
            runtime.spawn(async move {
                tokio::select! {
                    _ = token.cancelled() => {}
                    _ = tokio::time::sleep(Duration::from_secs(delay)) => {
                        if let Some(wakeups) = wakeups.upgrade() {
                            wakeups.fire(id);
                        }
                    }
                }
            });
        }
        state.pending = Some(Pending {
            info: info.clone(),
            _timer: timer.drop_guard(),
            _hold: session.background_tasks().hold(),
        });
        drop(state);
        self.changed();
        Ok(Scheduled::Pending(info))
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
    pub(crate) fn reset(&self) {
        {
            let mut state = self.state.lock().unwrap();
            state.pending = None;
            state.running_loop = None;
        }
        self.changed();
    }

    /// A wake-up came due: queue it, or let the loop it continues go on.
    fn fire(&self, id: u64) {
        let mut state = self.state.lock().unwrap();
        if state.pending.as_ref().map(|pending| pending.info.id) != Some(id) {
            return;
        }
        let pending = state.pending.take().expect("checked above");
        match pending.info.job {
            Some(job) => {
                if let Some(running) = state.running_loop.as_mut()
                    && running.job == job
                    && running.decision == Decision::Continue
                {
                    running.due = true;
                    running.woken = Some(pending.info.clone());
                }
                drop(state);
                self.changed();
            }
            None => {
                drop(state);
                if let Some(session) = self.session.upgrade() {
                    session.queue_entry(wakeup_message(&pending.info), Priority::Later);
                }
            }
        }
        // Released only now: the queued entry keeps the session from here.
        drop(pending);
    }

    fn changed(&self) {
        self.changes
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    // ----------------------------------------------------------------
    // The loop's driver ([`crate::iterative`]).
    // ----------------------------------------------------------------

    /// Start the self-paced loop `job`; a session runs one at a time.
    pub(crate) fn begin_loop(&self, job: JobId, max_rounds: u32) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.running_loop.is_some() {
            return Err("a self-paced loop runs in this session already".into());
        }
        state.running_loop = Some(LoopState {
            job,
            round: 0,
            max_rounds,
            decision: Decision::Undecided,
            due: false,
            woken: None,
        });
        drop(state);
        self.changed();
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
        running.woken.take()
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
        if state
            .running_loop
            .as_ref()
            .is_some_and(|running| running.job == job)
        {
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
    }
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
