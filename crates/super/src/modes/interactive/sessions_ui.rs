//! `/sessions` in the full-screen TUI, in-process or attached: the picker
//! from [`crate::sessions`] in place of the editor. A session on this
//! device turns the TUI into the attached TUI on it, in place, as after
//! `/remote`; the session the TUI ran in-process stays saved, one it was
//! attached to keeps running in the daemon. A session on another device is
//! shown by the line-based front end ([`crate::attach`]), which takes over
//! below the TUI's last frame.

use super::{App, Cell, CommandEvent};
use std::sync::Arc;
use super_coding::session_runner::AgentSession;
use super_tui::KeyEvent;
use tokio::sync::mpsc;
#[cfg(unix)]
use {
    crate::attach::Continuation,
    crate::session_link::Link,
    crate::sessions::{
        BrowseOutcome, Choice, DeviceRef, DirectoryPicker, Listing, Outcome, SessionsPicker,
    },
    super_sdk::daemon::{DirectoryEntry, SessionSummary},
    tokio_util::sync::CancellationToken,
};

/// The picker while it is open.
#[derive(Default)]
pub(super) struct State {
    #[cfg(unix)]
    open: Option<Open>,
    /// A request in flight; its answer is dropped once this is cancelled.
    #[cfg(unix)]
    pending: Option<CancellationToken>,
    /// The switch is done: the main loop shows the session picked.
    #[cfg(unix)]
    switched: Option<Switch>,
}

/// A switch the daemon or the platform accepted.
#[cfg(unix)]
pub(super) struct Switch {
    next: Continuation,
    /// The line saying where the TUI went and what became of the session
    /// it showed.
    note: String,
}

/// Where the TUI goes once a switch went through.
#[cfg(unix)]
pub(super) enum Next {
    /// It shows the session picked, attached to it in place.
    Here,
    /// A session on another device: the line-based front end takes over.
    Leave(Box<Continuation>),
}

#[cfg(unix)]
struct Open {
    /// Away while a request uses it.
    link: Option<Link>,
    sessions: SessionsPicker,
    browser: Option<DirectoryPicker>,
    note: Option<String>,
}

/// Answers of the requests the picker makes.
pub(super) enum Event {
    #[cfg(unix)]
    Loaded(Box<(Link, Listing)>),
    #[cfg(unix)]
    Directories(Box<Directories>),
    #[cfg(unix)]
    Switched(Box<(Link, Choice, Result<SessionSummary, String>)>),
}

/// The answer to a directory listing, with the link it borrowed.
#[cfg(unix)]
pub(super) struct Directories {
    link: Link,
    device: DeviceRef,
    path: String,
    entries: Result<Vec<DirectoryEntry>, String>,
}

/// Whether the picker owns the keyboard and the editor's place.
pub(super) fn is_open(state: &State) -> bool {
    #[cfg(unix)]
    {
        state.open.is_some()
    }
    #[cfg(not(unix))]
    {
        let _ = state;
        false
    }
}

/// `/sessions`.
pub(super) fn open(
    app: &mut App,
    session: &Arc<AgentSession>,
    command_tx: &mpsc::UnboundedSender<CommandEvent>,
) {
    #[cfg(not(unix))]
    {
        let _ = (session, command_tx);
        app.cells.push(Cell::Error(
            "/sessions needs the SUPER daemon, which needs a Unix socket and is not available on this system".into(),
        ));
    }
    #[cfg(unix)]
    {
        let current = session.manager.lock().unwrap().session_id().to_string();
        let cancel = CancellationToken::new();
        app.sessions.pending = Some(cancel.clone());
        app.command_cancel = Some(cancel.clone());
        app.command_status = Some("listing sessions".into());
        let tx = command_tx.clone();
        tokio::spawn(async move {
            let info = crate::handover::client_info();
            let (daemon, reason) =
                match super_daemon::client::DaemonClient::connect_or_start(&info).await {
                    Ok(mut client) => match client.take_events() {
                        Some(events) => (Some((client, events)), None),
                        None => (
                            None,
                            Some("the daemon connection has no events".to_string()),
                        ),
                    },
                    Err(error) => (None, Some(error.to_string())),
                };
            let mut link = Link::new(daemon, None);
            let mut listing = crate::sessions::load(&mut link, Some(&current)).await;
            if let (Some(reason), Err(_)) = (reason, &listing.local) {
                listing.local = Err(reason);
            }
            if !cancel.is_cancelled() {
                let _ = tx.send(CommandEvent::Sessions(Event::Loaded(Box::new((
                    link, listing,
                )))));
            }
        });
    }
}

pub(super) fn on_event(app: &mut App, event: Event, session: &AgentSession) {
    #[cfg(not(unix))]
    {
        let _ = (app, session);
        match event {}
    }
    #[cfg(unix)]
    {
        if app
            .sessions
            .pending
            .take()
            .is_none_or(|cancel| cancel.is_cancelled())
        {
            // Closed or cancelled meanwhile.
            return;
        }
        app.command_status = None;
        app.command_cancel = None;
        match event {
            Event::Loaded(loaded) => {
                let (link, listing) = *loaded;
                let now = chrono::Utc::now().timestamp_millis();
                let sessions = SessionsPicker::new(
                    crate::sessions::group(&listing, now),
                    now,
                    app.theme.clone(),
                );
                app.sessions.open = Some(Open {
                    link: Some(link),
                    sessions,
                    browser: None,
                    note: None,
                });
            }
            Event::Directories(answer) => {
                let Directories {
                    link,
                    device,
                    path,
                    entries,
                } = *answer;
                let theme = app.theme.clone();
                let Some(open) = app.sessions.open.as_mut() else {
                    return;
                };
                open.link = Some(link);
                match entries {
                    Ok(entries) => {
                        open.browser = Some(DirectoryPicker::new(device, path, entries, theme));
                    }
                    Err(error) => open.note = Some(error),
                }
            }
            Event::Switched(answer) => {
                let this = session;
                let (link, choice, result) = *answer;
                match result {
                    Ok(session) => {
                        let place = match &choice {
                            Choice::Session { device, .. } | Choice::NewSession { device, .. } => {
                                device.name.clone()
                            }
                        };
                        app.sessions.open = None;
                        let saved = this
                            .manager
                            .lock()
                            .unwrap()
                            .session_file()
                            .is_some_and(std::path::Path::exists);
                        let left = if super::is_attached(app) {
                            " · the session shown before keeps running in the SUPER daemon"
                        } else if saved {
                            " · this session is saved; /sessions or `super -c` here brings it back"
                        } else {
                            ""
                        };
                        let note = format!(
                            "♦ switched to {} on {place}{left}",
                            crate::sessions::title(&session),
                        );
                        app.sessions.switched = Some(Switch {
                            next: Continuation { link, session },
                            note,
                        });
                    }
                    Err(error) => {
                        if let Some(open) = app.sessions.open.as_mut() {
                            open.link = Some(link);
                            open.note = Some(error);
                        }
                    }
                }
            }
        }
    }
}

/// Keys while the picker is open.
pub(super) fn handle_key(
    app: &mut App,
    session: &AgentSession,
    key: &KeyEvent,
    command_tx: &mpsc::UnboundedSender<CommandEvent>,
) {
    #[cfg(not(unix))]
    {
        let _ = (app, session, key, command_tx);
    }
    #[cfg(unix)]
    {
        let busy = super::remote_handover::busy(app, session);
        let action = {
            let Some(open) = app.sessions.open.as_mut() else {
                return;
            };
            if open.link.is_none() {
                // A request is on its way; Esc closes regardless.
                if key.key == super_tui::Key::Escape || *key == KeyEvent::ctrl('c') {
                    Action::Close
                } else {
                    Action::Nothing
                }
            } else {
                open.note = None;
                match &mut open.browser {
                    Some(browser) => match browser.handle_key(key) {
                        BrowseOutcome::Stay => Action::Nothing,
                        BrowseOutcome::Back => {
                            open.browser = None;
                            Action::Nothing
                        }
                        BrowseOutcome::Open(path) => {
                            Action::Request(Request::Browse(browser.device.clone(), path))
                        }
                        BrowseOutcome::Use(cwd) => {
                            Action::Request(Request::Switch(Choice::NewSession {
                                device: browser.device.clone(),
                                cwd,
                            }))
                        }
                    },
                    None => match open.sessions.handle_key(key) {
                        Outcome::Stay => Action::Nothing,
                        Outcome::Close => Action::Close,
                        Outcome::Say(text) => {
                            open.note = Some(text);
                            Action::Nothing
                        }
                        Outcome::Browse { device, path } => {
                            Action::Request(Request::Browse(device, path))
                        }
                        Outcome::Pick(choice) => Action::Request(Request::Switch(choice)),
                    },
                }
            }
        };
        match action {
            Action::Nothing => {}
            Action::Close => close(app),
            Action::Request(Request::Switch(_)) if busy => {
                if let Some(open) = app.sessions.open.as_mut() {
                    open.note = Some(
                        "this session is busy; switch once the turn is over (esc aborts it)".into(),
                    );
                }
            }
            Action::Request(request) => send(app, request, command_tx),
        }
    }
}

#[cfg(unix)]
enum Action {
    Nothing,
    Close,
    Request(Request),
}

#[cfg(unix)]
enum Request {
    Browse(DeviceRef, String),
    Switch(Choice),
}

#[cfg(unix)]
fn send(app: &mut App, request: Request, command_tx: &mpsc::UnboundedSender<CommandEvent>) {
    let Some(open) = app.sessions.open.as_mut() else {
        return;
    };
    let Some(mut link) = open.link.take() else {
        return;
    };
    let cancel = CancellationToken::new();
    app.sessions.pending = Some(cancel.clone());
    app.command_cancel = Some(cancel);
    let tx = command_tx.clone();
    match request {
        Request::Browse(device, path) => {
            app.command_status = Some(format!("listing {path} on {}", device.name));
            tokio::spawn(async move {
                let entries = crate::sessions::list_directories(&mut link, &device, &path).await;
                let _ = tx.send(CommandEvent::Sessions(Event::Directories(Box::new(
                    Directories {
                        link,
                        device,
                        path,
                        entries,
                    },
                ))));
            });
        }
        Request::Switch(choice) => {
            app.command_status = Some("switching sessions".into());
            tokio::spawn(async move {
                let result = crate::sessions::switch(&mut link, choice.clone()).await;
                let _ = tx.send(CommandEvent::Sessions(Event::Switched(Box::new((
                    link, choice, result,
                )))));
            });
        }
    }
}

#[cfg(unix)]
fn close(app: &mut App) {
    app.sessions.open = None;
    if let Some(cancel) = app.sessions.pending.take() {
        cancel.cancel();
        app.command_status = None;
        app.command_cancel = None;
    }
}

/// The picker's lines, in place of the editor.
pub(super) fn render(state: &mut State, width: usize, theme: &super_tui::Theme) -> Vec<String> {
    #[cfg(unix)]
    {
        let Some(open) = state.open.as_mut() else {
            return Vec::new();
        };
        let (_, height) = super_tui::Terminal::size();
        let rows = height.saturating_sub(10).clamp(3, 18);
        let mut lines = match &mut open.browser {
            Some(browser) => {
                browser.max_visible = rows;
                browser.render(width)
            }
            None => {
                open.sessions.max_visible = rows;
                open.sessions.render(width)
            }
        };
        if let Some(note) = &open.note {
            lines.push(theme.fg("warning", note));
        }
        lines
    }
    #[cfg(not(unix))]
    {
        let _ = (state, width, theme);
        Vec::new()
    }
}

/// The session picked, once the switch went through.
#[cfg(unix)]
pub(super) fn take_switch(state: &mut State) -> Option<Switch> {
    state.switched.take()
}

/// Show the session a switch picked: one on this device in this TUI, in
/// place; one on another device through the line-based front end.
#[cfg(unix)]
pub(super) async fn carry_on(
    app: &mut App,
    session: &Arc<AgentSession>,
    switch: Switch,
    sink: super_coding::SessionEventSink,
    command_tx: &mpsc::UnboundedSender<CommandEvent>,
) -> Next {
    let Switch { next, note } = switch;
    if next.link.remote().is_some() {
        app.cells.push(Cell::Notice(note));
        return Next::Leave(Box::new(next));
    }
    let Continuation {
        link,
        session: picked,
    } = next;
    match super::attached::switch(app, session, link, picked, sink, command_tx).await {
        Ok(()) => app.cells.push(Cell::Notice(note)),
        Err(error) => app.cells.push(Cell::Error(format!("/sessions: {error:#}"))),
    }
    Next::Here
}

/// What the line-based front end carries on with after a switch to a
/// session on another device: the link and the terminal's input.
#[cfg(unix)]
pub(super) struct HandedOver {
    continuation: Continuation,
    stdin: mpsc::UnboundedReceiver<Vec<u8>>,
}

#[cfg(not(unix))]
pub(super) enum HandedOver {}

/// Where the full-screen TUI leaves what it handed over.
pub(super) type Slot = Option<HandedOver>;

#[cfg(unix)]
pub(super) fn handed_over(
    continuation: Continuation,
    stdin: mpsc::UnboundedReceiver<Vec<u8>>,
) -> HandedOver {
    HandedOver {
        continuation,
        stdin,
    }
}

/// The frame the full-screen TUI leaves behind: the transcript with the
/// switch, and blank rows where the editor and the footer were. Returns the
/// frame and how many rows at its end are blank.
#[cfg(unix)]
pub(super) fn last_frame(
    app: &mut App,
    session: &Arc<AgentSession>,
    width: usize,
) -> (Vec<String>, usize) {
    use super_tui::Component as _;
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

/// Put the cursor on the first blank row of the last frame, so the
/// line-based front end writes where the editor was. The terminal is out of
/// raw mode and has moved to the row below the frame.
#[cfg(unix)]
pub(super) fn reclaim_rows(blank: usize) {
    use std::io::Write as _;
    if blank > 0 {
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "\x1b[{blank}A\r");
        let _ = out.flush();
    }
}

/// After the full-screen TUI closed: the line-based front end on the
/// session picked, reading the same input.
#[cfg(unix)]
pub(super) async fn continue_line_based(handed_over: HandedOver) -> anyhow::Result<i32> {
    let HandedOver {
        continuation,
        stdin,
    } = handed_over;
    let input: crate::attach::Input =
        Box::new(tokio::io::BufReader::new(ChannelReader::new(stdin)));
    crate::attach::run_continued(continuation, input).await
}

#[cfg(not(unix))]
pub(super) async fn continue_line_based(handed_over: HandedOver) -> anyhow::Result<i32> {
    match handed_over {}
}

/// Reads what the TUI's stdin thread delivers, so the line-based front end
/// continues on the same input the full-screen TUI used. A second reader of
/// stdin would swallow the first line typed.
#[cfg(unix)]
struct ChannelReader {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    pending: Vec<u8>,
    offset: usize,
}

#[cfg(unix)]
impl ChannelReader {
    fn new(rx: mpsc::UnboundedReceiver<Vec<u8>>) -> Self {
        Self {
            rx,
            pending: Vec::new(),
            offset: 0,
        }
    }
}

#[cfg(unix)]
impl tokio::io::AsyncRead for ChannelReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;
        while self.offset >= self.pending.len() {
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some(bytes)) => {
                    self.pending = bytes;
                    self.offset = 0;
                }
                // The sender is gone: end of input.
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
        let available = &self.pending[self.offset..];
        let count = available.len().min(buf.remaining());
        buf.put_slice(&available[..count]);
        self.offset += count;
        Poll::Ready(Ok(()))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::tests::{test_app, test_resources, test_session};
    use super::*;
    use std::time::Duration;
    use super_daemon::DaemonConfig;
    use super_daemon::attached::Connector;
    use super_daemon::client::{DaemonClient, NewSession};
    use super_daemon::host::HostConfig;
    use super_daemon::paths::DaemonPaths;
    use super_sdk::mock::{MockProvider, MockScript};
    use super_tui::Key;

    /// The attached TUI (ENG-13) runs `/sessions` (ENG-15): the attached
    /// module leaves the command to the picker, the picker opens in place of
    /// the editor on its own link, typing filters, Enter switches through
    /// `sessions::switch` on that link, and the TUI then shows the session
    /// picked on its own connection while the one before keeps running.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_attached_tui_switches_sessions_with_the_picker() {
        let dir = tempfile::Builder::new().prefix("su").tempdir().unwrap();
        let provider = MockProvider::start(dir.path().join("mock"), MockScript::text("Hello."))
            .await
            .unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let socket = dir.path().join("d.sock");
        let shutdown = CancellationToken::new();
        let daemon = tokio::spawn(super_daemon::run(
            DaemonConfig {
                paths: DaemonPaths {
                    run_dir: dir.path().to_path_buf(),
                    socket: socket.clone(),
                    lock: dir.path().join("d.lock"),
                    log: dir.path().join("d.log"),
                },
                host: HostConfig {
                    session_dir: dir.path().join("sessions"),
                    idle_unload: Duration::ZERO,
                    models_file: Some(provider.catalog_path()),
                    no_context_files: true,
                },
                host_id: "01M".into(),
                host_name: "mbp".into(),
                log_level: super_daemon::log::Level::Debug,
                log_to_stderr: false,
                sweep_interval: Duration::from_secs(3600),
                presence: None,
                protocol_versions: super_sdk::daemon::PROTOCOL_VERSIONS,
            },
            shutdown.clone(),
        ));
        for _ in 0..200 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let info = crate::handover::client_info();
        let connect = || async {
            let mut client = DaemonClient::connect_to(&socket, &info).await.unwrap();
            let events = client.take_events().unwrap();
            (client, events)
        };
        let new = |name: Option<&str>| NewSession {
            cwd: work.clone(),
            model: Some("mock/mock-1".into()),
            name: name.map(str::to_string),
            ..NewSession::default()
        };

        // The attached TUI on one session, as `run_attached` builds it.
        let (client, events) = connect().await;
        let first = client.create_session(new(None)).await.unwrap();
        let second = client
            .create_session(new(Some("the other one")))
            .await
            .unwrap();
        let attached = client.attach(&first.session_id).await.unwrap();
        let mut app = test_app();
        let resources = test_resources();
        let view = test_session(super_coding::SessionManager::in_memory(&work));
        let sink: super_coding::SessionEventSink = Arc::new(|_| {});
        let (command_tx, mut command_rx) = mpsc::unbounded_channel();
        let (shown, _notices) = super::super::attached::attach_with(
            crate::attach::Target {
                client,
                events,
                attached,
                info: info.clone(),
            },
            &view,
            sink.clone(),
            Connector::Socket(socket.clone()),
        )
        .await
        .unwrap();
        app.attached = Some(shown);
        assert_eq!(view.manager.lock().unwrap().session_id(), first.session_id);
        assert!(
            super::super::attached::slash_command(
                &mut app,
                &view,
                "sessions",
                "",
                &resources,
                &command_tx
            )
            .is_none(),
            "/sessions is the picker's while attached too"
        );

        // `/sessions` opens on its own link, listing this device's daemon
        // (signed out here: no other devices).
        let (picker_client, picker_events) = connect().await;
        let hello = picker_client.hello().clone();
        let sessions = picker_client.list_sessions(None).await.unwrap();
        let listing = Listing {
            local: Ok(crate::sessions::LocalListing {
                host_id: hello.host_id.clone(),
                host_name: hello.host_name.clone(),
                sessions,
            }),
            registry: None,
            local_host_id: Some(hello.host_id),
            current: Some(first.session_id.clone()),
        };
        let link = Link::local(picker_client, picker_events);
        app.sessions.pending = Some(CancellationToken::new());
        on_event(&mut app, Event::Loaded(Box::new((link, listing))), &view);
        assert!(is_open(&app.sessions));
        let theme = app.theme.clone();
        let lines = render(&mut app.sessions, 120, &theme).join("\n");
        assert!(lines.contains("the other one"), "{lines}");

        // Type to filter, Enter to switch.
        for c in "the other".chars() {
            handle_key(&mut app, &view, &KeyEvent::char(c), &command_tx);
        }
        let enter = KeyEvent {
            key: Key::Enter,
            ..KeyEvent::default()
        };
        handle_key(&mut app, &view, &enter, &command_tx);
        let event = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(CommandEvent::Sessions(event)) = command_rx.recv().await {
                    break event;
                }
            }
        })
        .await
        .expect("the switch answered");
        on_event(&mut app, event, &view);
        assert!(!is_open(&app.sessions));
        let switch = take_switch(&mut app.sessions).expect("the switch went through");
        assert!(
            switch.note.contains("keeps running in the SUPER daemon"),
            "{}",
            switch.note
        );
        let next = carry_on(&mut app, &view, switch, sink, &command_tx).await;
        assert!(
            matches!(next, Next::Here),
            "a session here is shown in place"
        );

        // The TUI shows the session picked, on its own connection.
        assert!(super::super::is_attached(&app));
        assert_eq!(view.manager.lock().unwrap().session_id(), second.session_id);
        assert!(
            app.cells
                .iter()
                .any(|cell| matches!(cell, Cell::Notice(text) if text.starts_with("♦ switched to the other one on mbp"))),
        );
        let check = connect().await.0;
        let mut attachments = std::collections::BTreeMap::new();
        for _ in 0..100 {
            attachments = check
                .list_sessions(None)
                .await
                .unwrap()
                .into_iter()
                .map(|session| {
                    (
                        session.session_id,
                        (session.hosted, session.attached_clients),
                    )
                })
                .collect();
            if attachments[&second.session_id] == (true, 1) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(
            attachments[&second.session_id],
            (true, 1),
            "the TUI is attached to it, the picker's link let go"
        );
        assert_eq!(
            attachments[&first.session_id],
            (true, 0),
            "the session shown before keeps running"
        );

        // Detach before the daemon stops, so nothing reconnects.
        let shown = app.attached.take().unwrap();
        super::super::attached::detach(shown).await;
        drop(check);
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), daemon)
            .await
            .expect("the daemon stopped in time")
            .unwrap()
            .unwrap();
    }
}
