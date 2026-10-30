//! `/sessions`: every session this terminal can reach, grouped by device and
//! workspace with the apps' status model, and the switch to the one picked.
//!
//! Sessions on this device come from the local daemon over its socket;
//! signed in, sessions on other devices come from the platform registry.
//! The picker is keyboard only and looks like the other pickers of the
//! terminal. Picking a session on this device attaches the daemon connection
//! to it (the previous session keeps running); picking one on another device
//! drives it through the platform with the same commands and rendering
//! ([`crate::session_link`]). The picker also starts a new session in a
//! workspace, or in a directory of a device ("new workspace on device…").

use std::collections::BTreeMap;
use super_remote::registry::{RegistrySession, RegistryView};
use super_sdk::daemon::{DirectoryEntry, SessionState, SessionSummary};
use super_tui::text::{display_width, fit_to_width, truncate_to_width};
use super_tui::{Key, KeyEvent, Theme};

/// This device's daemon and what it lists.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LocalListing {
    pub host_id: String,
    pub host_name: String,
    pub sessions: Vec<SessionSummary>,
}

/// Everything `/sessions` knows when it opens.
#[derive(Debug, Clone)]
pub(crate) struct Listing {
    /// This device's daemon, or why there is none.
    pub local: Result<LocalListing, String>,
    /// Other devices from the platform registry; `None` when signed out.
    pub registry: Option<Result<RegistryView, String>>,
    /// This device's host id, so its registry records are not listed twice.
    pub local_host_id: Option<String>,
    /// The session this terminal shows.
    pub current: Option<String>,
}

/// A device with its workspaces, as the picker lists it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Device {
    pub host_id: String,
    pub name: String,
    pub platform: String,
    pub local: bool,
    pub online: bool,
    /// A SUPER daemon answers there: new sessions can start on it.
    pub can_start: bool,
    /// Why the device shows nothing, when that needs saying.
    pub note: Option<String>,
    pub workspaces: Vec<Workspace>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Workspace {
    pub path: String,
    pub name: String,
    pub sessions: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub summary: SessionSummary,
    pub title: String,
    pub current: bool,
    /// Unix milliseconds.
    pub modified: i64,
}

/// A session state as the apps show it: a label, a mark and a colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Status {
    pub label: &'static str,
    pub mark: &'static str,
    pub token: &'static str,
}

pub(crate) fn status(state: SessionState) -> Status {
    match state {
        SessionState::Working => Status {
            label: "working",
            mark: "◐",
            token: "accent",
        },
        SessionState::Blocked => Status {
            label: "needs an answer",
            mark: "?",
            token: "warning",
        },
        SessionState::Idle => Status {
            label: "idle",
            mark: "○",
            token: "muted",
        },
        SessionState::Ended => Status {
            label: "ended",
            mark: "✓",
            token: "dim",
        },
    }
}

/// A device's presence dot, label and colour.
pub(crate) fn presence(online: bool) -> Status {
    if online {
        Status {
            label: "online",
            mark: "●",
            token: "success",
        }
    } else {
        Status {
            label: "offline",
            mark: "○",
            token: "dim",
        }
    }
}

/// What a session is called: its name, else its first message, else its id.
pub(crate) fn title(summary: &SessionSummary) -> String {
    let first_line = |text: &str| text.lines().next().unwrap_or_default().trim().to_string();
    summary
        .name
        .as_deref()
        .map(first_line)
        .filter(|name| !name.is_empty())
        .or_else(|| {
            summary
                .first_message
                .as_deref()
                .map(first_line)
                .filter(|text| !text.is_empty())
        })
        .unwrap_or_else(|| short_id(&summary.session_id).to_string())
}

pub(crate) fn short_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}

fn parse_time(rfc3339: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|time| time.timestamp_millis())
        .unwrap_or(0)
}

/// `now`, `5m`, `3h`, `2d`: how long ago, short.
pub(crate) fn ago(then: i64, now: i64) -> String {
    let seconds = (now - then).max(0) / 1000;
    match seconds {
        _ if then <= 0 => String::new(),
        0..60 => "now".into(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// A registry record in the shape the daemon lists sessions.
pub(crate) fn summary_from_record(session: &RegistrySession) -> SessionSummary {
    let record = &session.record;
    let session_file = record
        .resume_argv
        .iter()
        .position(|arg| arg == "--session")
        .and_then(|index| record.resume_argv.get(index + 1))
        .cloned()
        .unwrap_or_default();
    let non_empty = |text: &str| (!text.is_empty()).then(|| text.to_string());
    SessionSummary {
        session_id: record.session_id.clone(),
        cwd: record.cwd.clone(),
        workspace: record.workspace.clone(),
        session_file,
        name: record.name.clone(),
        first_message: record.first_message.clone(),
        state: record.state,
        blocked_reason: record.blocked_reason.clone(),
        hosted: record.state != SessionState::Ended,
        attached_clients: 0,
        model: non_empty(&record.model),
        thinking_level: non_empty(&record.thinking_level),
        modified_at: chrono::DateTime::from_timestamp_millis(session.at)
            .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_default(),
    }
}

fn entry(summary: SessionSummary, current: Option<&str>) -> Entry {
    Entry {
        title: title(&summary),
        current: current == Some(summary.session_id.as_str()),
        modified: parse_time(&summary.modified_at),
        summary,
    }
}

fn workspace_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// Group sessions by workspace: newest session first, the workspace with the
/// newest session first.
fn workspaces(entries: Vec<Entry>) -> Vec<Workspace> {
    let mut by_path: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for entry in entries {
        let path = if entry.summary.workspace.is_empty() {
            entry.summary.cwd.clone()
        } else {
            entry.summary.workspace.clone()
        };
        by_path.entry(path).or_default().push(entry);
    }
    let mut workspaces: Vec<Workspace> = by_path
        .into_iter()
        .map(|(path, mut sessions)| {
            sessions.sort_by(|a, b| {
                b.modified
                    .cmp(&a.modified)
                    .then(a.summary.session_id.cmp(&b.summary.session_id))
            });
            Workspace {
                name: workspace_name(&path),
                path,
                sessions,
            }
        })
        .collect();
    let newest = |workspace: &Workspace| {
        workspace
            .sessions
            .iter()
            .map(|entry| entry.modified)
            .max()
            .unwrap_or(0)
    };
    workspaces.sort_by(|a, b| newest(b).cmp(&newest(a)).then(a.path.cmp(&b.path)));
    workspaces
}

/// Devices → workspaces → sessions. This device comes first; other devices
/// follow, online ones first, then by name.
pub(crate) fn group(listing: &Listing, now: i64) -> Vec<Device> {
    let current = listing.current.as_deref();
    let mut devices = Vec::new();
    let local_host = match &listing.local {
        Ok(local) => {
            devices.push(Device {
                host_id: local.host_id.clone(),
                name: local.host_name.clone(),
                platform: String::new(),
                local: true,
                online: true,
                can_start: true,
                note: None,
                workspaces: workspaces(
                    local
                        .sessions
                        .iter()
                        .map(|summary| entry(summary.clone(), current))
                        .collect(),
                ),
            });
            Some(local.host_id.clone())
        }
        Err(reason) => {
            devices.push(Device {
                host_id: listing.local_host_id.clone().unwrap_or_default(),
                name: "this device".into(),
                platform: String::new(),
                local: true,
                online: false,
                can_start: false,
                note: Some(reason.clone()),
                workspaces: Vec::new(),
            });
            None
        }
    };
    match &listing.registry {
        None => {}
        Some(Err(reason)) => devices.push(Device {
            host_id: String::new(),
            name: "other devices".into(),
            platform: String::new(),
            local: false,
            online: false,
            can_start: false,
            note: Some(reason.clone()),
            workspaces: Vec::new(),
        }),
        Some(Ok(view)) => {
            let mut others: Vec<Device> = view
                .hosts()
                .filter(|host| {
                    Some(&host.host_id) != local_host.as_ref()
                        && Some(&host.host_id) != listing.local_host_id.as_ref()
                })
                .filter_map(|host| {
                    let online = view.is_online(&host.host_id, now);
                    let entries: Vec<Entry> = view
                        .sessions()
                        .filter(|session| session.host_id == host.host_id)
                        .map(|session| entry(summary_from_record(session), current))
                        .collect();
                    let can_start = online && host.daemon;
                    // A device with nothing to show and nothing to start is noise.
                    (!entries.is_empty() || can_start).then(|| Device {
                        host_id: host.host_id.clone(),
                        name: host.name.clone(),
                        platform: host.platform.clone(),
                        local: false,
                        online,
                        can_start,
                        note: None,
                        workspaces: workspaces(entries),
                    })
                })
                .collect();
            others.sort_by(|a, b| {
                b.online
                    .cmp(&a.online)
                    .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                    .then(a.host_id.cmp(&b.host_id))
            });
            devices.extend(others);
        }
    }
    devices
}

/// Where a device is reached: over the local socket or through the platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceRef {
    pub host_id: String,
    pub name: String,
    pub local: bool,
    pub online: bool,
    pub can_start: bool,
}

impl DeviceRef {
    fn of(device: &Device) -> Self {
        Self {
            host_id: device.host_id.clone(),
            name: device.name.clone(),
            local: device.local,
            online: device.online,
            can_start: device.can_start,
        }
    }
}

/// What the user picked.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Choice {
    Session {
        device: DeviceRef,
        session: Box<SessionSummary>,
    },
    NewSession {
        device: DeviceRef,
        cwd: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Device(usize),
    Workspace(usize, usize),
    Session(usize, usize, usize),
    NewSession(usize, usize),
    NewWorkspace(usize),
}

impl Row {
    fn selectable(self) -> bool {
        matches!(
            self,
            Row::Session(..) | Row::NewSession(..) | Row::NewWorkspace(_)
        )
    }
}

/// What a key did to the picker.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    Stay,
    Close,
    Pick(Choice),
    /// Show the directories of `device` under `path`.
    Browse {
        device: DeviceRef,
        path: String,
    },
    /// Tell the user why nothing happened.
    Say(String),
}

/// The session picker: device and workspace headers, sessions and the
/// "new session" and "new workspace" actions, filtered by what is typed.
pub(crate) struct SessionsPicker {
    devices: Vec<Device>,
    now: i64,
    filter: String,
    rows: Vec<Row>,
    selected: usize,
    pub max_visible: usize,
    theme: Theme,
}

impl SessionsPicker {
    pub fn new(devices: Vec<Device>, now: i64, theme: Theme) -> Self {
        let mut picker = Self {
            devices,
            now,
            filter: String::new(),
            rows: Vec::new(),
            selected: 0,
            max_visible: 18,
            theme,
        };
        picker.rebuild();
        // Start on the current session, else on the first row to pick.
        let current = picker.rows.iter().position(|row| match row {
            Row::Session(d, w, s) => picker.devices[*d].workspaces[*w].sessions[*s].current,
            _ => false,
        });
        picker.selected = current
            .or_else(|| picker.rows.iter().position(|row| row.selectable()))
            .unwrap_or(0);
        picker
    }

    fn label(&self, row: Row) -> String {
        match row {
            Row::Device(d) => self.devices[d].name.clone(),
            Row::Workspace(d, w) => self.devices[d].workspaces[w].name.clone(),
            Row::Session(d, w, s) => {
                let workspace = &self.devices[d].workspaces[w];
                format!(
                    "{} {} {}",
                    workspace.sessions[s].title, workspace.name, self.devices[d].name
                )
            }
            Row::NewSession(d, w) => format!(
                "new session in {} {}",
                self.devices[d].workspaces[w].name, self.devices[d].name
            ),
            Row::NewWorkspace(d) => format!("new workspace on {}", self.devices[d].name),
        }
    }

    fn matches(&self, row: Row) -> bool {
        self.filter.is_empty()
            || self
                .label(row)
                .to_lowercase()
                .contains(&self.filter.to_lowercase())
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        for (d, device) in self.devices.iter().enumerate() {
            let mut device_rows = Vec::new();
            for (w, workspace) in device.workspaces.iter().enumerate() {
                let mut workspace_rows: Vec<Row> = (0..workspace.sessions.len())
                    .map(|s| Row::Session(d, w, s))
                    .collect();
                if device.can_start {
                    workspace_rows.push(Row::NewSession(d, w));
                }
                workspace_rows.retain(|row| self.matches(*row));
                if !workspace_rows.is_empty() {
                    device_rows.push(Row::Workspace(d, w));
                    device_rows.extend(workspace_rows);
                }
            }
            if device.can_start && self.matches(Row::NewWorkspace(d)) {
                device_rows.push(Row::NewWorkspace(d));
            }
            // A device stays listed without a filter, so its state shows.
            if !device_rows.is_empty() || self.filter.is_empty() {
                rows.push(Row::Device(d));
                rows.extend(device_rows);
            }
        }
        self.rows = rows;
        self.selected = self
            .rows
            .iter()
            .position(|row| row.selectable())
            .unwrap_or(0);
    }

    fn move_selection(&mut self, delta: isize) {
        let selectable: Vec<usize> = (0..self.rows.len())
            .filter(|index| self.rows[*index].selectable())
            .collect();
        if selectable.is_empty() {
            return;
        }
        let at = selectable
            .iter()
            .position(|index| *index == self.selected)
            .unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(selectable.len() as isize) as usize;
        self.selected = selectable[next];
    }

    /// The row under the cursor, if it can be picked.
    fn current(&self) -> Option<Row> {
        self.rows
            .get(self.selected)
            .copied()
            .filter(|row| row.selectable())
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> Outcome {
        if *key == KeyEvent::ctrl('c') {
            return Outcome::Close;
        }
        match key.key {
            Key::Escape if !self.filter.is_empty() => {
                self.filter.clear();
                self.rebuild();
            }
            Key::Escape => return Outcome::Close,
            Key::Up => self.move_selection(-1),
            Key::Down | Key::Tab => self.move_selection(1),
            Key::PageUp => self.move_selection(-5),
            Key::PageDown => self.move_selection(5),
            Key::Enter => return self.activate(),
            Key::Backspace => {
                self.filter.pop();
                self.rebuild();
            }
            Key::Char(c) if !key.ctrl && !key.alt => {
                self.filter.push(c);
                self.rebuild();
            }
            _ => {}
        }
        Outcome::Stay
    }

    fn activate(&self) -> Outcome {
        let Some(row) = self.current() else {
            return Outcome::Stay;
        };
        match row {
            Row::Session(d, w, s) => {
                let device = &self.devices[d];
                let entry = &device.workspaces[w].sessions[s];
                if entry.current {
                    return Outcome::Close;
                }
                if !device.local && !device.online {
                    return Outcome::Say(format!("{} is offline", device.name));
                }
                Outcome::Pick(Choice::Session {
                    device: DeviceRef::of(device),
                    session: Box::new(entry.summary.clone()),
                })
            }
            Row::NewSession(d, w) => Outcome::Pick(Choice::NewSession {
                device: DeviceRef::of(&self.devices[d]),
                cwd: self.devices[d].workspaces[w].path.clone(),
            }),
            Row::NewWorkspace(d) => {
                let device = &self.devices[d];
                // Start where the device's newest workspace lives.
                let path = device
                    .workspaces
                    .first()
                    .and_then(|workspace| {
                        std::path::Path::new(&workspace.path)
                            .parent()
                            .map(|parent| parent.display().to_string())
                    })
                    .or_else(|| {
                        device
                            .local
                            .then(|| dirs::home_dir().map(|home| home.display().to_string()))
                            .flatten()
                    })
                    .unwrap_or_else(|| "/".into());
                Outcome::Browse {
                    device: DeviceRef::of(device),
                    path,
                }
            }
            _ => Outcome::Stay,
        }
    }

    pub fn render(&mut self, width: usize) -> Vec<String> {
        let theme = &self.theme;
        let mut lines = vec![theme.fg("accent", &theme.bold("Sessions"))];
        if !self.filter.is_empty() {
            lines.push(theme.fg("muted", &format!("filter: {}", self.filter)));
        }
        if self.rows.is_empty() {
            lines.push(theme.fg("dim", "(no matches)"));
        }
        let visible = self.max_visible.max(3).min(self.rows.len());
        let start = self
            .selected
            .saturating_sub(visible / 2)
            .min(self.rows.len().saturating_sub(visible));
        for index in start..start + visible {
            lines.push(self.render_row(index, width));
        }
        if self.rows.len() > visible {
            lines.push(theme.fg(
                "dim",
                &format!("({}/{})", self.selected + 1, self.rows.len()),
            ));
        }
        lines.push(theme.fg("dim", "↑/↓ move · enter open · type to filter · esc close"));
        lines
    }

    fn render_row(&self, index: usize, width: usize) -> String {
        let theme = &self.theme;
        let row = self.rows[index];
        let selected = index == self.selected && row.selectable();
        let marker = if selected { "→ " } else { "  " };
        let line = match row {
            Row::Device(d) => {
                let device = &self.devices[d];
                let dot = presence(device.online || device.local && device.note.is_none());
                let mut detail = Vec::new();
                if device.local {
                    detail.push("this device".to_string());
                } else {
                    detail.push(dot.label.to_string());
                }
                if let Some(note) = &device.note {
                    detail.push(note.clone());
                }
                format!(
                    "{} {}  {}",
                    theme.fg(dot.token, dot.mark),
                    theme.bold(&device.name),
                    theme.fg("muted", &detail.join(" · "))
                )
            }
            Row::Workspace(d, w) => {
                let workspace = &self.devices[d].workspaces[w];
                format!(
                    "  {}  {}",
                    theme.bold(&workspace.name),
                    theme.fg("dim", &workspace.path)
                )
            }
            Row::Session(d, w, s) => {
                let entry = &self.devices[d].workspaces[w].sessions[s];
                let state = status(entry.summary.state);
                let mut detail = Vec::new();
                if entry.current {
                    detail.push("current".to_string());
                }
                if let Some(reason) = &entry.summary.blocked_reason {
                    detail.push(reason.clone());
                }
                let when = ago(entry.modified, self.now);
                if !when.is_empty() {
                    detail.push(when);
                }
                format!(
                    "{marker}  {} {} {}  {}",
                    theme.fg(state.token, state.mark),
                    theme.fg(state.token, &format!("{:<15}", state.label)),
                    entry.title,
                    theme.fg("muted", &detail.join(" · "))
                )
            }
            Row::NewSession(d, w) => format!(
                "{marker}  {}",
                theme.fg(
                    "accent",
                    &format!("+ new session in {}", self.devices[d].workspaces[w].name)
                )
            ),
            Row::NewWorkspace(d) => format!(
                "{marker}{}",
                theme.fg(
                    "accent",
                    &format!("+ new workspace on {}…", self.devices[d].name)
                )
            ),
        };
        let line = if display_width(&line) > width {
            truncate_to_width(&line, width)
        } else {
            line
        };
        if selected {
            format!(
                "{}{}\x1b[49m",
                theme.color("selectedBg").bg_code(),
                fit_to_width(&line, width)
            )
        } else {
            line
        }
    }
}

/// What a key did to the directory browser.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BrowseOutcome {
    Stay,
    /// Back to the session list.
    Back,
    /// List this directory.
    Open(String),
    /// Start a session here.
    Use(String),
}

/// "New workspace on device…": a directory of the device, its
/// subdirectories, and "start a session here".
pub(crate) struct DirectoryPicker {
    pub device: DeviceRef,
    path: String,
    entries: Vec<DirectoryEntry>,
    filter: String,
    selected: usize,
    pub max_visible: usize,
    theme: Theme,
}

impl DirectoryPicker {
    pub fn new(
        device: DeviceRef,
        path: String,
        entries: Vec<DirectoryEntry>,
        theme: Theme,
    ) -> Self {
        Self {
            device,
            path,
            entries,
            filter: String::new(),
            selected: 0,
            max_visible: 16,
            theme,
        }
    }

    fn parent(&self) -> Option<String> {
        std::path::Path::new(&self.path)
            .parent()
            .map(|parent| parent.display().to_string())
    }

    /// The rows: "start a session here", "..", then the subdirectories
    /// matching the filter.
    fn rows(&self) -> Vec<BrowseOutcome> {
        let mut rows = Vec::new();
        if self.filter.is_empty() {
            rows.push(BrowseOutcome::Use(self.path.clone()));
            if let Some(parent) = self.parent() {
                rows.push(BrowseOutcome::Open(parent));
            }
        }
        let filter = self.filter.to_lowercase();
        rows.extend(
            self.entries
                .iter()
                .filter(|entry| entry.name.to_lowercase().contains(&filter))
                .map(|entry| BrowseOutcome::Open(entry.path.clone())),
        );
        rows
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> BrowseOutcome {
        if *key == KeyEvent::ctrl('c') {
            return BrowseOutcome::Back;
        }
        let count = self.rows().len();
        match key.key {
            Key::Escape if !self.filter.is_empty() => {
                self.filter.clear();
                self.selected = 0;
            }
            Key::Escape => return BrowseOutcome::Back,
            Key::Up if count > 0 => {
                self.selected = (self.selected + count - 1) % count;
            }
            Key::Down | Key::Tab if count > 0 => self.selected = (self.selected + 1) % count,
            Key::Enter => {
                return self
                    .rows()
                    .get(self.selected)
                    .cloned()
                    .unwrap_or(BrowseOutcome::Stay);
            }
            Key::Backspace => {
                self.filter.pop();
                self.selected = 0;
            }
            Key::Char(c) if !key.ctrl && !key.alt => {
                self.filter.push(c);
                self.selected = 0;
            }
            _ => {}
        }
        BrowseOutcome::Stay
    }

    pub fn render(&mut self, width: usize) -> Vec<String> {
        let theme = &self.theme;
        let mut lines = vec![
            theme.fg(
                "accent",
                &theme.bold(&format!("New workspace on {}", self.device.name)),
            ),
            theme.fg("muted", &self.path),
        ];
        if !self.filter.is_empty() {
            lines.push(theme.fg("muted", &format!("filter: {}", self.filter)));
        }
        let rows = self.rows();
        if rows.is_empty() {
            lines.push(theme.fg("dim", "(no subdirectories)"));
        }
        let visible = self.max_visible.max(3).min(rows.len());
        let start = self
            .selected
            .saturating_sub(visible / 2)
            .min(rows.len().saturating_sub(visible));
        let parent = self.parent();
        for (index, row) in rows.iter().enumerate().skip(start).take(visible) {
            let text = match row {
                BrowseOutcome::Use(_) => theme.fg("accent", "✓ start a session here"),
                BrowseOutcome::Open(path) if Some(path) == parent.as_ref() => "..".to_string(),
                BrowseOutcome::Open(path) => format!(
                    "{}/",
                    std::path::Path::new(path)
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.clone())
                ),
                _ => String::new(),
            };
            if index == self.selected {
                lines.push(format!(
                    "{}{}\x1b[49m",
                    theme.color("selectedBg").bg_code(),
                    fit_to_width(&truncate_to_width(&format!("→ {text}"), width), width)
                ));
            } else {
                lines.push(truncate_to_width(&format!("  {text}"), width));
            }
        }
        lines.push(theme.fg("dim", "enter opens a directory · type to filter · esc back"));
        lines
    }
}

#[cfg(unix)]
pub(crate) use daemon::*;

#[cfg(unix)]
mod daemon {
    use super::*;
    use crate::session_link::{Link, Platform, RemoteSession};
    use std::sync::Arc;
    use std::time::Duration;
    use super_daemon::client::{DaemonClient, NewSession};
    use super_sdk::daemon::DaemonCommand;

    /// How long reading the platform registry may take before `/sessions`
    /// shows this device alone.
    const REGISTRY_WAIT: Duration = Duration::from_secs(8);

    /// Everything the picker lists: this device's daemon over the link, other
    /// devices from the registry when signed in. Connects to the platform on
    /// first use and keeps that connection in the link.
    pub(crate) async fn load(link: &mut Link, current: Option<&str>) -> Listing {
        let local = match link.daemon() {
            Some(client) => local_listing(client).await,
            None => Err("no SUPER daemon is running on this device".into()),
        };
        let registry = if crate::remote::signed_in() {
            // A platform out of reach must not keep the picker from opening.
            let read = tokio::time::timeout(REGISTRY_WAIT, async {
                match link.platform_or_connect().await {
                    Ok(platform) => {
                        super_remote::registry::read_registry(&platform.client, REGISTRY_WAIT)
                            .await
                            .map_err(|error| {
                                format!("could not read the platform registry: {error}")
                            })
                    }
                    Err(error) => Err(format!("could not reach the platform: {error}")),
                }
            })
            .await;
            Some(read.unwrap_or_else(|_| {
                Err(format!(
                    "the platform did not answer within {}s",
                    REGISTRY_WAIT.as_secs()
                ))
            }))
        } else {
            None
        };
        let local_host_id = match &local {
            Ok(listing) => Some(listing.host_id.clone()),
            Err(_) => link.platform().map(|platform| platform.host_id.clone()),
        };
        Listing {
            local,
            registry,
            local_host_id,
            current: current.map(str::to_string),
        }
    }

    async fn local_listing(client: &DaemonClient) -> Result<LocalListing, String> {
        let sessions = client
            .list_sessions(None)
            .await
            .map_err(|error| error.to_string())?;
        let hello = client.hello();
        Ok(LocalListing {
            host_id: hello.host_id.clone(),
            host_name: hello.host_name.clone(),
            sessions,
        })
    }

    /// The subdirectories of `path` on `device`.
    pub(crate) async fn list_directories(
        link: &mut Link,
        device: &DeviceRef,
        path: &str,
    ) -> Result<Vec<DirectoryEntry>, String> {
        if device.local {
            let client = link
                .daemon()
                .ok_or("no SUPER daemon is running on this device")?;
            return client
                .list_directories(std::path::Path::new(path))
                .await
                .map_err(|error| error.to_string());
        }
        let platform = link.platform_or_connect().await?;
        let data = host_call(
            &platform,
            device,
            &DaemonCommand::ListDirectories {
                path: path.to_string(),
            },
        )
        .await?;
        serde_json::from_value(data["directories"].clone()).map_err(|error| error.to_string())
    }

    async fn host_call(
        platform: &Arc<Platform>,
        device: &DeviceRef,
        command: &DaemonCommand,
    ) -> Result<serde_json::Value, String> {
        let response = platform
            .host_commands(&device.host_id)
            .call(command)
            .await
            .map_err(|error| format!("{error:#}"))?;
        if response.success {
            Ok(response.data.unwrap_or_default())
        } else {
            Err(format!(
                "{}: {}",
                response.command,
                response.error.unwrap_or_else(|| "failed".into())
            ))
        }
    }

    /// Make the link show the picked session. The previous one keeps running
    /// where it runs; on failure the link stays where it was.
    pub(crate) async fn switch(link: &mut Link, choice: Choice) -> Result<SessionSummary, String> {
        match choice {
            Choice::Session { device, session } if device.local => {
                attach_local(link, &session.session_id).await
            }
            Choice::NewSession { device, cwd } if device.local => {
                let client = link
                    .daemon()
                    .ok_or("no SUPER daemon is running on this device")?;
                let created = client
                    .create_session(NewSession {
                        cwd: cwd.into(),
                        ..NewSession::default()
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                attach_local(link, &created.session_id).await
            }
            Choice::Session { device, session } => {
                if !device.online {
                    return Err(format!("{} is offline", device.name));
                }
                let platform = link.platform_or_connect().await?;
                if session.state == SessionState::Ended {
                    if !device.can_start {
                        return Err(format!(
                            "the session ended on {}, which runs no SUPER daemon to resume it",
                            device.name
                        ));
                    }
                    host_call(
                        &platform,
                        &device,
                        &DaemonCommand::ResumeSession {
                            session_id: session.session_id.clone(),
                        },
                    )
                    .await?;
                }
                attach_remote(link, platform, &device, *session).await
            }
            Choice::NewSession { device, cwd } => {
                if !device.can_start {
                    return Err(format!(
                        "{} runs no SUPER daemon that could start a session",
                        device.name
                    ));
                }
                let platform = link.platform_or_connect().await?;
                let data = host_call(
                    &platform,
                    &device,
                    &DaemonCommand::CreateSession {
                        cwd,
                        model: None,
                        provider: None,
                        thinking_level: None,
                        name: None,
                    },
                )
                .await?;
                let session: SessionSummary = serde_json::from_value(data["session"].clone())
                    .map_err(|error| format!("create_session: {error}"))?;
                attach_remote(link, platform, &device, session).await
            }
        }
    }

    /// Attach the daemon connection to a session on this device. Detaching
    /// first, and dropping what queued up, keeps the previous session's last
    /// events from showing under the new one.
    async fn attach_local(link: &mut Link, session_id: &str) -> Result<SessionSummary, String> {
        const NO_DAEMON: &str = "no SUPER daemon is running on this device";
        let _ = link.daemon().ok_or(NO_DAEMON)?.detach().await;
        link.drain();
        let attached = link
            .daemon()
            .ok_or(NO_DAEMON)?
            .attach(session_id)
            .await
            .map_err(|error| error.to_string())?;
        link.set_remote(None);
        Ok(attached.session)
    }

    async fn attach_remote(
        link: &mut Link,
        platform: Arc<Platform>,
        device: &DeviceRef,
        session: SessionSummary,
    ) -> Result<SessionSummary, String> {
        let remote =
            RemoteSession::open(platform, &device.host_id, &device.name, &session.session_id)
                .await?;
        if let Some(client) = link.daemon() {
            let _ = client.detach().await;
        }
        link.drain();
        link.set_remote(Some(remote));
        Ok(session)
    }

    /// `/sessions` in the attached terminal: the picker in raw mode for a
    /// moment, on the same input. Returns the session switched to, `None`
    /// when the picker closed without a switch.
    pub(crate) async fn pick_inline(
        link: &mut Link,
        current: &str,
        input: &mut crate::attach::Input,
    ) -> Result<Option<SessionSummary>, String> {
        use std::io::Write as _;
        use super_tui::{DiffRenderer, InputDecoder, InputEvent, Terminal};
        use tokio::io::AsyncReadExt as _;

        enum Next {
            Stay,
            Close,
            Pick(Choice),
        }

        // The picker's first frame starts on this line and replaces it.
        {
            let mut out = std::io::stdout().lock();
            let _ = write!(out, "\r\x1b[2mlisting sessions…\x1b[0m");
            let _ = out.flush();
        }
        let listing = load(link, Some(current)).await;
        let now = chrono::Utc::now().timestamp_millis();
        let theme = picker_theme();
        let mut sessions = SessionsPicker::new(group(&listing, now), now, theme.clone());
        let mut browser: Option<DirectoryPicker> = None;
        let mut note: Option<String> = None;

        let mut terminal = Terminal::new().map_err(|error| error.to_string())?;
        let mut renderer = DiffRenderer::new();
        let mut decoder = InputDecoder::default();
        let mut buffer = [0_u8; 1024];
        let mut drawn = 0;
        let choice = loop {
            let (width, height) = Terminal::size();
            let rows = height.saturating_sub(6).max(3);
            let mut lines = match &mut browser {
                Some(browser) => {
                    browser.max_visible = rows;
                    browser.render(width)
                }
                None => {
                    sessions.max_visible = rows;
                    sessions.render(width)
                }
            };
            if let Some(note) = &note {
                lines.push(theme.fg("warning", note));
            }
            // Never shrink the frame: a shorter one makes the renderer clear
            // the whole screen.
            lines.resize(lines.len().max(drawn), String::new());
            drawn = lines.len();
            {
                let mut out = std::io::stdout().lock();
                let _ = renderer.render_frame(&lines, width, height, &mut out);
                let _ = out.flush();
            }
            let count = match input.read(&mut buffer).await {
                Ok(0) | Err(_) => break None,
                Ok(count) => count,
            };
            let mut events = Vec::new();
            decoder.feed(&buffer[..count], &mut events);
            let mut next = Next::Stay;
            for event in events {
                let InputEvent::Key(key) = event else {
                    continue;
                };
                note = None;
                let browse = match &mut browser {
                    Some(open) => match open.handle_key(&key) {
                        BrowseOutcome::Stay => None,
                        BrowseOutcome::Back => {
                            browser = None;
                            None
                        }
                        BrowseOutcome::Use(path) => {
                            next = Next::Pick(Choice::NewSession {
                                device: open.device.clone(),
                                cwd: path,
                            });
                            None
                        }
                        BrowseOutcome::Open(path) => Some((open.device.clone(), path)),
                    },
                    None => match sessions.handle_key(&key) {
                        Outcome::Stay => None,
                        Outcome::Close => {
                            next = Next::Close;
                            None
                        }
                        Outcome::Say(text) => {
                            note = Some(text);
                            None
                        }
                        Outcome::Pick(choice) => {
                            next = Next::Pick(choice);
                            None
                        }
                        Outcome::Browse { device, path } => Some((device, path)),
                    },
                };
                if let Some((device, path)) = browse {
                    match list_directories(link, &device, &path).await {
                        Ok(entries) => {
                            browser =
                                Some(DirectoryPicker::new(device, path, entries, theme.clone()));
                        }
                        Err(error) => note = Some(error),
                    }
                }
                if !matches!(next, Next::Stay) {
                    break;
                }
            }
            match next {
                Next::Stay => {}
                Next::Close => break None,
                Next::Pick(choice) => break Some(choice),
            }
        };
        // Leave nothing behind: blank the frame, put the cursor where the
        // picker started and hand the terminal back.
        let (width, height) = Terminal::size();
        let mut blank = vec![String::new(); drawn];
        // The last row must change, so the cursor ends there.
        if let Some(last) = blank.last_mut() {
            last.push(' ');
        }
        {
            let mut out = std::io::stdout().lock();
            let _ = renderer.render_frame(&blank, width, height, &mut out);
            let _ = out.flush();
        }
        terminal.restore();
        if drawn > 0 {
            let mut out = std::io::stdout().lock();
            let _ = write!(out, "\x1b[{drawn}A\r");
            let _ = out.flush();
        }
        match choice {
            Some(choice) => switch(link, choice).await.map(Some),
            None => Ok(None),
        }
    }

    /// The theme the settings name, for the picker of the attached terminal.
    fn picker_theme() -> Theme {
        let cwd = std::env::current_dir().unwrap_or_default();
        let settings = super_coding::Settings::load(&cwd, false);
        settings
            .theme
            .as_deref()
            .and_then(Theme::builtin)
            .unwrap_or_else(Theme::dark)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super_tui::text::strip_ansi;

    const NOW: i64 = 1_793_350_000_000;

    fn time(ms: i64) -> String {
        chrono::DateTime::from_timestamp_millis(ms)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    fn summary(id: &str, workspace: &str, state: SessionState, modified: i64) -> SessionSummary {
        SessionSummary {
            session_id: id.into(),
            cwd: workspace.into(),
            workspace: workspace.into(),
            session_file: format!("/s/{id}.jsonl"),
            name: None,
            first_message: Some(format!("question {id}")),
            state,
            blocked_reason: None,
            hosted: state != SessionState::Ended,
            attached_clients: 0,
            model: None,
            thinking_level: None,
            modified_at: time(modified),
        }
    }

    fn registry() -> RegistryView {
        let mut view = RegistryView::default();
        let host = |id: &str, name: &str, daemon: bool, at: i64| {
            format!(
                r#"{{"kind":"host","hostId":"{id}","name":"{name}","platform":"linux","daemon":{daemon},"at":{at}}}"#
            )
        };
        let session = |host: &str, id: &str, workspace: &str, state: &str, at: i64| {
            format!(
                r#"{{"kind":"session","hostId":"{host}","sessionId":"{id}","workspace":"{workspace}","cwd":"{workspace}","state":"{state}","model":"super/gpt-5","thinkingLevel":"low","resumeArgv":["super","--session","/h/{id}.jsonl"],"at":{at}}}"#
            )
        };
        // This device announces itself too; the daemon's listing wins.
        view.apply(&host("01M", "mbp", true, NOW - 5_000));
        view.apply(&session("01M", "s1", "/w/super", "idle", NOW - 5_000));
        view.apply(&host("02L", "linuxbox", true, NOW - 10_000));
        view.apply(&session("02L", "r1", "/srv/app", "working", NOW - 10_000));
        view.apply(&host("03O", "old-laptop", false, NOW - 3_600_000));
        view.apply(&session("03O", "o1", "/home/o", "ended", NOW - 3_600_000));
        // Nothing to show and nothing to start: left out.
        view.apply(&host("04E", "empty", false, NOW - 1_000));
        // A daemon with no session yet: listed, for "new workspace".
        view.apply(&host("05Z", "zeta", true, NOW - 1_000));
        view
    }

    fn listing() -> Listing {
        Listing {
            local: Ok(LocalListing {
                host_id: "01M".into(),
                host_name: "mbp".into(),
                sessions: vec![
                    summary("s1", "/w/super", SessionState::Idle, NOW - 7_200_000),
                    summary("s2", "/w/super", SessionState::Working, NOW - 60_000),
                    summary(
                        "s3",
                        "/w/harness",
                        SessionState::Ended,
                        NOW - 86_400_000 * 2,
                    ),
                ],
            }),
            registry: Some(Ok(registry())),
            local_host_id: Some("01M".into()),
            current: Some("s3".into()),
        }
    }

    #[test]
    fn states_and_presence_read_as_in_the_apps() {
        assert_eq!(status(SessionState::Working).label, "working");
        assert_eq!(status(SessionState::Blocked).label, "needs an answer");
        assert_eq!(status(SessionState::Blocked).token, "warning");
        assert_eq!(status(SessionState::Idle).label, "idle");
        assert_eq!(status(SessionState::Ended).label, "ended");
        assert_eq!(status(SessionState::Ended).token, "dim");
        assert_eq!(presence(true).label, "online");
        assert_eq!(presence(true).mark, "●");
        assert_eq!(presence(false).label, "offline");
        assert_eq!(ago(NOW - 30_000, NOW), "now");
        assert_eq!(ago(NOW - 5 * 60_000, NOW), "5m");
        assert_eq!(ago(NOW - 3 * 3_600_000, NOW), "3h");
        assert_eq!(ago(NOW - 2 * 86_400_000, NOW), "2d");
        assert_eq!(ago(0, NOW), "");
    }

    #[test]
    fn sessions_group_by_device_then_workspace_this_device_first() {
        let devices = group(&listing(), NOW);
        let names: Vec<&str> = devices.iter().map(|device| device.name.as_str()).collect();
        assert_eq!(names, ["mbp", "linuxbox", "zeta", "old-laptop"]);

        let mbp = &devices[0];
        assert!(mbp.local && mbp.online && mbp.can_start);
        let workspaces: Vec<&str> = mbp.workspaces.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(workspaces, ["super", "harness"], "newest workspace first");
        let super_ids: Vec<&str> = mbp.workspaces[0]
            .sessions
            .iter()
            .map(|entry| entry.summary.session_id.as_str())
            .collect();
        assert_eq!(super_ids, ["s2", "s1"], "newest session first");
        assert!(mbp.workspaces[1].sessions[0].current);
        assert_eq!(mbp.workspaces[1].sessions[0].title, "question s3");

        let linuxbox = &devices[1];
        assert!(!linuxbox.local && linuxbox.online && linuxbox.can_start);
        let remote = &linuxbox.workspaces[0].sessions[0];
        assert_eq!(remote.summary.state, SessionState::Working);
        assert_eq!(remote.summary.session_file, "/h/r1.jsonl");
        assert_eq!(remote.summary.model.as_deref(), Some("super/gpt-5"));
        assert!(remote.summary.hosted);

        assert!(devices[2].workspaces.is_empty() && devices[2].can_start);
        let laptop = &devices[3];
        assert!(!laptop.online && !laptop.can_start);
        assert_eq!(
            laptop.workspaces[0].sessions[0].summary.state,
            SessionState::Ended
        );
    }

    #[test]
    fn a_missing_daemon_or_registry_says_why() {
        let listing = Listing {
            local: Err("no SUPER daemon is running on this device".into()),
            registry: Some(Err("could not reach the platform: offline".into())),
            local_host_id: None,
            current: None,
        };
        let devices = group(&listing, NOW);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].name, "this device");
        assert!(!devices[0].can_start);
        assert_eq!(
            devices[0].note.as_deref(),
            Some("no SUPER daemon is running on this device")
        );
        assert_eq!(devices[1].name, "other devices");
        assert!(devices[1].note.as_deref().unwrap().contains("offline"));

        let signed_out = Listing {
            registry: None,
            ..listing
        };
        assert_eq!(group(&signed_out, NOW).len(), 1);
    }

    fn key(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            ..KeyEvent::default()
        }
    }

    fn picker() -> SessionsPicker {
        SessionsPicker::new(group(&listing(), NOW), NOW, Theme::dark())
    }

    #[test]
    fn the_picker_starts_on_the_current_session_and_skips_headers() {
        let mut picker = picker();
        // Enter on the current session closes: nothing to switch to.
        assert_eq!(picker.handle_key(&key(Key::Enter)), Outcome::Close);
        picker.handle_key(&key(Key::Up));
        // Above "harness / s3" is the "new session in super" row.
        assert_eq!(
            picker.handle_key(&key(Key::Enter)),
            Outcome::Pick(Choice::NewSession {
                device: DeviceRef {
                    host_id: "01M".into(),
                    name: "mbp".into(),
                    local: true,
                    online: true,
                    can_start: true,
                },
                cwd: "/w/super".into(),
            })
        );
        picker.handle_key(&key(Key::Up));
        let Outcome::Pick(Choice::Session { device, session }) =
            picker.handle_key(&key(Key::Enter))
        else {
            panic!("a session row");
        };
        assert!(device.local);
        assert_eq!(session.session_id, "s1");
    }

    #[test]
    fn typing_filters_and_offline_devices_are_refused() {
        let mut picker = picker();
        for c in "o1".chars() {
            picker.handle_key(&key(Key::Char(c)));
        }
        assert_eq!(
            picker.handle_key(&key(Key::Enter)),
            Outcome::Say("old-laptop is offline".into())
        );
        picker.handle_key(&key(Key::Backspace));
        picker.handle_key(&key(Key::Backspace));
        for c in "r1 app".chars() {
            picker.handle_key(&key(Key::Char(c)));
        }
        let Outcome::Pick(Choice::Session { device, session }) =
            picker.handle_key(&key(Key::Enter))
        else {
            panic!("the remote session");
        };
        assert_eq!(device.name, "linuxbox");
        assert!(!device.local && device.online);
        assert_eq!(session.session_id, "r1");
        // Esc clears the filter first, then closes.
        assert_eq!(picker.handle_key(&key(Key::Escape)), Outcome::Stay);
        assert_eq!(picker.handle_key(&key(Key::Escape)), Outcome::Close);
    }

    #[test]
    fn new_workspace_browses_from_the_newest_workspace_parent() {
        let mut picker = picker();
        for c in "new workspace on linuxbox".chars() {
            picker.handle_key(&key(Key::Char(c)));
        }
        let Outcome::Browse { device, path } = picker.handle_key(&key(Key::Enter)) else {
            panic!("a browse");
        };
        assert_eq!(device.host_id, "02L");
        assert_eq!(path, "/srv");

        let entries = vec![
            DirectoryEntry {
                name: "app".into(),
                path: "/srv/app".into(),
            },
            DirectoryEntry {
                name: "web".into(),
                path: "/srv/web".into(),
            },
        ];
        let mut browser = DirectoryPicker::new(device, path, entries, Theme::dark());
        assert_eq!(
            browser.handle_key(&key(Key::Enter)),
            BrowseOutcome::Use("/srv".into())
        );
        browser.handle_key(&key(Key::Down));
        assert_eq!(
            browser.handle_key(&key(Key::Enter)),
            BrowseOutcome::Open("/".into()),
            "the parent"
        );
        browser.handle_key(&key(Key::Char('w')));
        assert_eq!(
            browser.handle_key(&key(Key::Enter)),
            BrowseOutcome::Open("/srv/web".into())
        );
        assert_eq!(browser.handle_key(&key(Key::Escape)), BrowseOutcome::Stay);
        assert_eq!(browser.handle_key(&key(Key::Escape)), BrowseOutcome::Back);
    }

    #[test]
    fn the_picker_renders_devices_workspaces_states_and_actions() {
        let mut picker = picker();
        picker.max_visible = 40;
        let text: Vec<String> = picker
            .render(120)
            .iter()
            .map(|line| strip_ansi(line))
            .collect();
        let joined = text.join("\n");
        assert_eq!(text[0], "Sessions");
        assert!(joined.contains("● mbp  this device"), "{joined}");
        assert!(joined.contains("  super  /w/super"), "{joined}");
        assert!(joined.contains("◐ working         question s2"), "{joined}");
        assert!(
            joined.contains("✓ ended           question s3  current · 2d"),
            "{joined}"
        );
        assert!(joined.contains("+ new session in super"), "{joined}");
        assert!(joined.contains("+ new workspace on mbp…"), "{joined}");
        assert!(joined.contains("○ old-laptop  offline"), "{joined}");
        assert!(
            !joined.contains("new session in o"),
            "no actions on an offline device"
        );
        assert!(
            text.last().unwrap().contains("esc close"),
            "the hint comes last"
        );
        assert!(text.iter().all(|line| display_width(line) <= 120));
    }
}

/// Two sessions in a daemon on a temporary socket, and a third on "another
/// device": a daemon whose platform link runs against the fake S2. The link
/// switches between all three and drives each with the same commands.
#[cfg(all(test, unix))]
mod end_to_end {
    use super::*;
    use crate::session_link::{Link, Platform};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use super_daemon::DaemonConfig;
    use super_daemon::client::{ClientEvent, DaemonClient, NewSession};
    use super_daemon::host::{CreateRequest, HostConfig, SessionHost};
    use super_daemon::paths::DaemonPaths;
    use super_daemon::presence::{Link as PresenceLink, LinkConfig};
    use super_daemon::server::{DaemonInfo, ServerContext};
    use super_remote::controller::Sender;
    use super_remote::fake::{FakeS2, TestCredentials, serve};
    use super_sdk::mock::{MockProvider, MockScript};
    use super_sdk::protocol::Command;
    use tokio_util::sync::CancellationToken;

    async fn prompt_and_settle(link: &mut Link, message: &str) -> String {
        link.execute_ok(Command::Prompt {
            message: message.into(),
            images: Vec::new(),
            streaming_behavior: None,
        })
        .await
        .unwrap();
        let mut text = String::new();
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match link.recv().await {
                    Some(ClientEvent::Session(event)) => {
                        let value = &event.0;
                        if value["type"] == "message_update"
                            && let Some(delta) = value["assistantMessageEvent"]["delta"].as_str()
                        {
                            text.push_str(delta);
                        }
                        if value["type"] == "agent_settled" {
                            break;
                        }
                    }
                    Some(ClientEvent::Daemon(_)) => {}
                    None => panic!("the link closed"),
                }
            }
        })
        .await
        .expect("the turn settled");
        text
    }

    async fn user_messages(link: &Link) -> Vec<String> {
        let data = link
            .execute_ok(Command::GetMessages {})
            .await
            .unwrap()
            .unwrap();
        data["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "user")
            .map(|message| match &message["content"] {
                serde_json::Value::String(text) => text.clone(),
                blocks => blocks
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|block| block["text"].as_str())
                    .collect::<String>(),
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_link_switches_between_sessions_here_and_on_another_device() {
        let dir = tempfile::Builder::new().prefix("sw").tempdir().unwrap();
        let provider = MockProvider::start(dir.path().join("mock"), MockScript::text("Hello."))
            .await
            .unwrap();
        let work: PathBuf = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();

        // This device's daemon, on a temporary socket.
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
        let mut client = DaemonClient::connect_to(&socket, &crate::handover::client_info())
            .await
            .unwrap();
        let events = client.take_events().unwrap();
        let new = || NewSession {
            cwd: work.clone(),
            model: Some("mock/mock-1".into()),
            ..NewSession::default()
        };
        let first = client.create_session(new()).await.unwrap();
        let second = client.create_session(new()).await.unwrap();
        client.attach(&first.session_id).await.unwrap();

        // Another device: a daemon whose link to the fake platform announces
        // it and its session.
        let remote_host = SessionHost::new(HostConfig {
            session_dir: dir.path().join("remote-sessions"),
            idle_unload: Duration::ZERO,
            models_file: Some(provider.catalog_path()),
            no_context_files: true,
        });
        let remote_session = remote_host
            .create(CreateRequest {
                cwd: work.clone(),
                model: Some("mock/mock-1".into()),
                ..CreateRequest::default()
            })
            .await
            .unwrap();
        let remote_id = remote_session.session_id.clone();
        let remote_file = remote_session.file.clone();
        drop(remote_session);
        let ctx = ServerContext::new(
            remote_host,
            DaemonInfo {
                version: "test".into(),
                host_id: "01L".into(),
                host_name: "linuxbox".into(),
                platform: "linux".into(),
                socket_path: dir.path().join("remote.sock"),
                protocol_versions: super_sdk::daemon::PROTOCOL_VERSIONS,
            },
            CancellationToken::new(),
        );
        let fake = Arc::new(FakeS2::default());
        let s2 = Arc::new(super_remote::s2::S2Client::new(
            reqwest::Client::new(),
            serve(fake).await,
            Arc::new(TestCredentials),
        ));
        let presence = PresenceLink::start(
            s2.clone(),
            ctx.clone(),
            super_remote::HostIdentity {
                host_id: "01L".into(),
                name: "linuxbox".into(),
                platform: "linux".into(),
            },
            "device-linux".into(),
            LinkConfig {
                cursor_dir: Some(dir.path().join("cursors")),
                republish_interval: Duration::from_secs(3600),
                ..LinkConfig::default()
            },
        );
        let platform = Arc::new(Platform {
            client: s2,
            sender: Sender {
                device_id: "device-mac".into(),
                name: "Terminal on mbp".into(),
            },
            host_id: "01M".into(),
        });
        let mut link = Link::new(Some((client, events)), Some(platform));

        // `/sessions` lists both devices once the other one announced itself.
        let mut devices = Vec::new();
        for _ in 0..200 {
            let mut listing = load(&mut link, Some(&first.session_id)).await;
            // The test is not signed in; read the registry the way a
            // signed-in terminal does.
            listing.registry = Some(
                super_remote::registry::read_registry(
                    &link.platform().unwrap().client,
                    Duration::from_secs(5),
                )
                .await
                .map_err(|error| error.to_string()),
            );
            devices = group(&listing, chrono::Utc::now().timestamp_millis());
            if devices.len() == 2 && !devices[1].workspaces.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(devices.len(), 2, "{devices:?}");
        let here = DeviceRef::of(&devices[0]);
        let there = DeviceRef::of(&devices[1]);
        assert!(here.local && there.online && there.can_start);
        assert_eq!(there.name, "linuxbox");
        let local_ids: Vec<&str> = devices[0].workspaces[0]
            .sessions
            .iter()
            .map(|entry| entry.summary.session_id.as_str())
            .collect();
        assert_eq!(local_ids.len(), 2);
        assert!(
            devices[0].workspaces[0]
                .sessions
                .iter()
                .any(|entry| entry.current)
        );
        let remote_entry = devices[1].workspaces[0].sessions[0].summary.clone();
        assert_eq!(remote_entry.session_id, remote_id);

        // Over to the second session here, a prompt, and back.
        let shown = switch(
            &mut link,
            Choice::Session {
                device: here.clone(),
                session: Box::new(second.clone()),
            },
        )
        .await
        .unwrap();
        assert_eq!(shown.session_id, second.session_id);
        assert_eq!(
            prompt_and_settle(&mut link, "for the second").await,
            "Hello."
        );
        let shown = switch(
            &mut link,
            Choice::Session {
                device: here.clone(),
                session: Box::new(first.clone()),
            },
        )
        .await
        .unwrap();
        assert_eq!(shown.session_id, first.session_id);
        assert!(
            user_messages(&link).await.is_empty(),
            "the first is untouched"
        );

        // Over to the session on the other device: the same commands.
        switch(
            &mut link,
            Choice::Session {
                device: there.clone(),
                session: Box::new(remote_entry),
            },
        )
        .await
        .unwrap();
        assert!(link.remote().is_some());
        assert_eq!(link.describe(), "linuxbox through the SUPER platform");
        assert_eq!(prompt_and_settle(&mut link, "for linuxbox").await, "Hello.");
        assert_eq!(user_messages(&link).await, ["for linuxbox"]);
        assert!(
            std::fs::read_to_string(&remote_file)
                .unwrap()
                .contains("for linuxbox")
        );

        // A new session on the other device, in a directory it lists.
        let directories = list_directories(&mut link, &there, &dir.path().display().to_string())
            .await
            .unwrap();
        assert!(directories.iter().any(|entry| entry.name == "work"));
        let created = switch(
            &mut link,
            Choice::NewSession {
                device: there.clone(),
                cwd: work.display().to_string(),
            },
        )
        .await
        .unwrap();
        assert!(ctx.host.get(&created.session_id).is_some());

        // And home again: the first session answers, both here keep running.
        switch(
            &mut link,
            Choice::Session {
                device: here.clone(),
                session: Box::new(first.clone()),
            },
        )
        .await
        .unwrap();
        assert!(link.remote().is_none());
        assert_eq!(prompt_and_settle(&mut link, "back home").await, "Hello.");
        let listed = link.daemon().unwrap().list_sessions(None).await.unwrap();
        let hosted: Vec<&str> = listed
            .iter()
            .filter(|session| session.hosted)
            .map(|session| session.session_id.as_str())
            .collect();
        assert!(hosted.contains(&first.session_id.as_str()));
        assert!(hosted.contains(&second.session_id.as_str()));

        presence.stop().await;
        drop(link);
        shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(10), daemon).await;
    }
}
