//! The terminal side of `/remote` (alias `/rc`): start the hand-over, show
//! its progress, and finish it once the turn is over. What to do when is
//! decided in [`crate::handover`]; after a successful hand-over the
//! terminal continues in the attached front end ([`crate::attach`]).

use super::{App, Cell, CommandEvent, InteractiveResources, terminal_host_state};
use crate::args::Args;
use crate::handover::{self, Facts, Step};
use std::sync::Arc;
use super_coding::session_runner::AgentSession;
use super_daemon::session_lock::SessionLockKeeper;
use super_tui::Component as _;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The hand-over in flight, if any.
#[derive(Default)]
pub(super) struct State {
    pending: Option<Pending>,
}

#[cfg_attr(not(unix), allow(dead_code))]
struct Pending {
    cancel: CancellationToken,
    /// The daemon connection, once one answered.
    #[cfg(unix)]
    ready: Option<handover::Ready>,
}

impl Pending {
    fn is_ready(&self) -> bool {
        #[cfg(unix)]
        {
            self.ready.is_some()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

/// What the hand-over task tells the terminal.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) enum Progress {
    /// The spinner line.
    Status(String),
    /// A line worth keeping: a daemon was installed or started.
    Note(String),
    #[cfg(unix)]
    Ready(Box<handover::Ready>),
    Failed(String),
    Cancelled,
}

/// The connection attached to the session the daemon now hosts.
#[cfg(unix)]
pub(super) type Moved = crate::attach::Continuation;

#[cfg(not(unix))]
pub(super) enum Moved {}

/// What the attached front end carries on with: the connection and the
/// terminal's input.
#[cfg(unix)]
pub(super) struct HandedOver {
    continuation: Moved,
    stdin: mpsc::UnboundedReceiver<Vec<u8>>,
}

#[cfg(not(unix))]
pub(super) enum HandedOver {}

/// Where the full-screen terminal leaves what it handed over.
pub(super) type Slot = Option<HandedOver>;

#[cfg(unix)]
pub(super) fn handed_over(moved: Moved, stdin: mpsc::UnboundedReceiver<Vec<u8>>) -> HandedOver {
    HandedOver {
        continuation: moved,
        stdin,
    }
}

#[cfg(not(unix))]
pub(super) fn handed_over(moved: Moved, _stdin: mpsc::UnboundedReceiver<Vec<u8>>) -> HandedOver {
    match moved {}
}

/// Add a line to the transcript without cutting a reply short: deltas only
/// land in a streaming reply while it is the last cell.
fn show(app: &mut App, cell: Cell) {
    match app.cells.last() {
        Some(Cell::AssistantStreaming(_)) => {
            let at = app.cells.len() - 1;
            app.cells.insert(at, cell);
        }
        _ => app.cells.push(cell),
    }
}

/// `/remote` or `/rc` without arguments.
pub(super) fn start(
    app: &mut App,
    session: &Arc<AgentSession>,
    args: &Args,
    resources: &InteractiveResources,
    command_tx: &mpsc::UnboundedSender<CommandEvent>,
) {
    if let Some(pending) = &app.handover.pending {
        show(
            app,
            Cell::Notice(if pending.is_ready() {
                "♦ this session moves to the SUPER daemon once the turn is over · /remote cancel keeps it here".into()
            } else {
                "♦ /remote is already preparing the SUPER daemon · /remote cancel stops it".into()
            }),
        );
        return;
    }
    let ephemeral = args.no_session || session.manager.lock().unwrap().session_file().is_none();
    let facts = Facts {
        attached: false,
        ephemeral,
        remote_enabled: resources.settings.remote.is_enabled(),
        signed_in: crate::remote::signed_in(),
        daemon_ready: false,
        busy: false,
    };
    let sign_in = match handover::next_step(facts) {
        Step::Refuse(reason) => {
            show(app, Cell::Error(format!("/remote: {reason}")));
            return;
        }
        Step::SignIn => true,
        _ => false,
    };
    spawn(app, sign_in, command_tx);
}

/// `/remote cancel`: keep the session here.
pub(super) fn cancel(app: &mut App) {
    match app.handover.pending.take() {
        Some(pending) => {
            pending.cancel.cancel();
            if !pending.is_ready() {
                app.command_status = None;
                app.command_cancel = None;
            }
            show(
                app,
                Cell::Notice("♦ /remote cancelled; the session stays here".into()),
            );
        }
        None => show(app, Cell::Notice("/remote: nothing to cancel".into())),
    }
}

#[cfg(not(unix))]
fn spawn(app: &mut App, _sign_in: bool, _command_tx: &mpsc::UnboundedSender<CommandEvent>) {
    show(
        app,
        Cell::Error(format!("/remote: {}", handover::UNSUPPORTED)),
    );
}

#[cfg(unix)]
fn spawn(app: &mut App, sign_in: bool, command_tx: &mpsc::UnboundedSender<CommandEvent>) {
    let cancel = CancellationToken::new();
    app.handover.pending = Some(Pending {
        cancel: cancel.clone(),
        ready: None,
    });
    app.command_cancel = Some(cancel.clone());
    app.command_status = Some(if sign_in {
        "signing in to SUPER for /remote".into()
    } else {
        "starting the SUPER daemon for /remote".into()
    });
    let tx = command_tx.clone();
    tokio::spawn(async move {
        let send = |progress: Progress| {
            let _ = tx.send(CommandEvent::Handover(progress));
        };
        if sign_in {
            let notice_tx = tx.clone();
            let result = crate::platform::sign_in(&cancel, move |device| {
                let opened = crate::auth_flow::open_browser(device.browser_url());
                let _ = notice_tx.send(CommandEvent::DeviceLoginNotice {
                    provider: crate::platform::PROVIDER.into(),
                    url: device.verification_uri.clone(),
                    code: device.display_code(),
                    opened,
                });
            })
            .await
            .map_err(|error| format!("{error:#}"));
            if cancel.is_cancelled() {
                send(Progress::Cancelled);
                return;
            }
            let signed_in = result.is_ok();
            // The terminal shows the sign-in and loads the models as `/login` does.
            let _ = tx.send(CommandEvent::PlatformSignIn(result));
            if !signed_in {
                send(Progress::Failed("not signed in to SUPER".into()));
                return;
            }
            send(Progress::Status(
                "starting the SUPER daemon for /remote".into(),
            ));
        }
        let note_tx = tx.clone();
        let note = move |text: String| {
            let _ = note_tx.send(CommandEvent::Handover(Progress::Note(text)));
        };
        let info = handover::client_info();
        let outcome = tokio::select! {
            _ = cancel.cancelled() => {
                send(Progress::Cancelled);
                return;
            }
            outcome = handover::provide_daemon(&info, &note) => outcome,
        };
        match outcome {
            Ok(ready) => send(Progress::Ready(Box::new(ready))),
            Err(error) => send(Progress::Failed(error)),
        }
    });
}

#[cfg_attr(not(unix), allow(unused_variables))]
pub(super) fn on_progress(app: &mut App, progress: Progress, session: &AgentSession) {
    let Some(pending) = app.handover.pending.as_mut() else {
        // Cancelled meanwhile.
        return;
    };
    match progress {
        Progress::Status(text) => {
            app.command_status = Some(text);
            app.command_cancel = Some(pending.cancel.clone());
        }
        Progress::Note(text) => show(app, Cell::Notice(format!("♦ {text}"))),
        #[cfg(unix)]
        Progress::Ready(ready) => {
            pending.ready = Some(*ready);
            app.command_status = None;
            app.command_cancel = None;
            if busy(app, session) {
                show(app, Cell::Notice(
                    "♦ the SUPER daemon is ready; this session moves there once the turn is over · /remote cancel keeps it here".into(),
                ));
            }
        }
        Progress::Failed(error) => {
            app.handover.pending = None;
            app.command_status = None;
            app.command_cancel = None;
            show(
                app,
                Cell::Error(format!("/remote: {error}. The session keeps running here.")),
            );
        }
        Progress::Cancelled => app.handover.pending = None,
    }
}

/// A turn, a queued message, a background request that writes the session,
/// or a question on screen: the hand-over waits for all of them.
fn busy(app: &App, session: &AgentSession) -> bool {
    !matches!(
        terminal_host_state(app, session),
        crate::terminal_hosts::State::Idle
    ) || app.picker.is_some()
        || app.secret_prompt.is_some()
        || app.btw_panel.is_some()
        || app.workflow_view.is_some()
        || app.job_view.is_some()
}

/// Whether the daemon is ready and nothing runs, so the hand-over can finish.
pub(super) fn due(app: &App, session: &AgentSession) -> bool {
    let ready = app.handover.pending.as_ref().is_some_and(Pending::is_ready);
    ready
        && handover::next_step(Facts {
            attached: false,
            ephemeral: false,
            remote_enabled: true,
            signed_in: true,
            daemon_ready: true,
            busy: busy(app, session),
        }) == Step::HandOver
}

/// How finishing the hand-over came out. Without a Unix socket it always
/// stays.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) enum Finished {
    /// The daemon runs the session; the terminal continues attached to it.
    Moved(Box<Moved>),
    /// The session keeps running here, the reason on screen.
    Stayed,
    /// The session file was let go and another process holds it now: the
    /// terminal must stop running the session. The reason.
    Stranded(String),
}

/// Move the session to the daemon.
#[cfg(not(unix))]
pub(super) async fn finish(
    _app: &mut App,
    _session: &Arc<AgentSession>,
    _lock: &mut Option<SessionLockKeeper>,
    _resources: &InteractiveResources,
    _command_tx: &mpsc::UnboundedSender<CommandEvent>,
) -> Finished {
    Finished::Stayed
}

/// Move the session to the daemon.
#[cfg(unix)]
pub(super) async fn finish(
    app: &mut App,
    session: &Arc<AgentSession>,
    lock: &mut Option<SessionLockKeeper>,
    resources: &InteractiveResources,
    command_tx: &mpsc::UnboundedSender<CommandEvent>,
) -> Finished {
    let Some(handover::Ready {
        mut client,
        mut events,
    }) = app
        .handover
        .pending
        .take()
        .and_then(|pending| pending.ready)
    else {
        return Finished::Stayed;
    };
    let failed = |app: &mut App, error: String| {
        show(
            app,
            Cell::Error(format!(
                "/remote: could not hand the session to the SUPER daemon: {error}. It keeps running here."
            )),
        );
    };
    // The daemon went away while the turn ran: try once more.
    if client.is_closed() {
        match handover::provide_daemon(&handover::client_info(), &|_| {}).await {
            Ok(ready) => (client, events) = (ready.client, ready.events),
            Err(error) => {
                failed(app, error);
                return Finished::Stayed;
            }
        }
    }
    let target = match handover::prepare(&client, session).await {
        Ok(target) => target,
        Err(error) => {
            failed(app, error);
            return Finished::Stayed;
        }
    };
    // The terminal's own mirror says goodbye first, so its "ended" record
    // cannot land after the daemon announced the session.
    let mirrored = match app.remote.take() {
        Some(remote) => {
            remote.stop().await;
            true
        }
        None => false,
    };
    app.remote_record_key = None;
    // Only daemon-level news queued up while waiting; none of it is ours.
    while events.try_recv().is_ok() {}
    match handover::hand_over(&client, &target, lock, session).await {
        Ok(attached) => {
            show(
                app,
                Cell::Notice(handover::confirmation(
                    &attached.session.session_id,
                    client.hello().pid,
                    crate::remote::signed_in(),
                )),
            );
            Finished::Moved(Box::new(crate::attach::Continuation {
                client,
                events,
                attached,
            }))
        }
        Err(handover::Failure::StaysHere(error)) => {
            if mirrored {
                super::start_remote(app, session, &resources.settings, command_tx);
            }
            failed(app, error);
            Finished::Stayed
        }
        Err(handover::Failure::Stranded(error)) => Finished::Stranded(format!(
            "/remote: could not hand the session to the SUPER daemon: {error}. `super` attaches to it if the daemon has it."
        )),
    }
}

/// The frame the full-screen terminal leaves behind: the transcript with the
/// confirmation, and blank rows where the editor and the footer were. Returns
/// the frame and how many rows at its end are blank.
pub(super) fn last_frame(
    app: &mut App,
    session: &Arc<AgentSession>,
    width: usize,
) -> (Vec<String>, usize) {
    let mut lines = app.render(width, session);
    let editor_top = app
        .editor
        .render(width)
        .first()
        .and_then(|first| lines.iter().rposition(|line| line == first));
    let blank = match editor_top {
        Some(top) => lines.len() - top,
        None => 0,
    };
    let len = lines.len();
    for line in &mut lines[len - blank..] {
        line.clear();
    }
    (lines, blank)
}

/// Put the cursor on the first blank row of the last frame, so the attached
/// front end writes where the editor was. The terminal is out of raw mode
/// and has moved to the row below the frame.
pub(super) fn reclaim_rows(blank: usize) {
    use std::io::Write as _;
    if blank > 0 {
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "\x1b[{blank}A\r");
        let _ = out.flush();
    }
}

/// After the full-screen terminal closed: the attached front end, reading
/// the same input.
#[cfg(unix)]
pub(super) async fn continue_attached(handed_over: HandedOver) -> anyhow::Result<i32> {
    let HandedOver {
        continuation,
        stdin,
    } = handed_over;
    let input: crate::attach::Input = Box::new(tokio::io::BufReader::new(
        handover::ChannelReader::new(stdin),
    ));
    crate::attach::run_handed_over(continuation, input).await
}

#[cfg(not(unix))]
pub(super) async fn continue_attached(handed_over: HandedOver) -> anyhow::Result<i32> {
    match handed_over {}
}
