//! `/sessions` in the full-screen terminal: the picker from
//! [`crate::sessions`] in place of the editor. Picking a session leaves the
//! in-process session saved and continues in the attached front end on the
//! session picked ([`crate::attach`]), the way `/remote` does.

use super::remote_handover::Moved;
use super::{App, Cell, CommandEvent};
use std::sync::Arc;
use super_coding::session_runner::AgentSession;
use super_tui::KeyEvent;
use tokio::sync::mpsc;
#[cfg(unix)]
use {
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
    /// The switch is done: the main loop hands over.
    #[cfg(unix)]
    switched: Option<Moved>,
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
                        app.cells.push(Cell::Notice(format!(
                            "♦ switched to {} on {place}{}",
                            crate::sessions::title(&session),
                            if saved {
                                " · this session is saved; /sessions or `super -c` here brings it back"
                            } else {
                                ""
                            }
                        )));
                        app.sessions.switched = Some(crate::attach::Continuation {
                            link,
                            session,
                            intro: true,
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
pub(super) fn take_switch(state: &mut State) -> Option<Moved> {
    #[cfg(unix)]
    {
        state.switched.take()
    }
    #[cfg(not(unix))]
    {
        let _ = state;
        None
    }
}
