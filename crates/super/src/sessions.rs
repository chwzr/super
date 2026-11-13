//! `/sessions`: every session this terminal can reach, grouped by machine and
//! workspace with the apps' status model, and the switch to the one picked.
//!
//! Sessions on this machine come from the local daemon over its socket;
//! signed in, sessions on other machines come from the platform registry.
//! The picker is keyboard only and looks like the other pickers of the
//! terminal. Picking a session on this machine attaches the daemon connection
//! to it (the previous session keeps running); picking one on another machine
//! drives it through the platform with the same commands and rendering
//! ([`crate::session_link`]). The picker also starts a new session in a
//! workspace, or in a directory of a machine ("new workspace on machine…").

use std::collections::BTreeMap;
use super_remote::registry::{Registry, RegistrySession};
use super_sdk::daemon::{DirectoryEntry, SessionState, SessionSummary};
use super_tui::text::{display_width, fit_to_width, truncate_to_width};
use super_tui::{Key, KeyEvent, Theme};

/// This machine's daemon and what it lists.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LocalListing {
    pub machine_id: String,
    pub machine_name: String,
    pub sessions: Vec<SessionSummary>,
}

/// Everything `/sessions` knows when it opens.
#[derive(Debug, Clone)]
pub(crate) struct Listing {
    /// This machine's daemon, or why there is none.
    pub local: Result<LocalListing, String>,
    /// Other machines from the platform registry; `None` when signed out.
    pub registry: Option<Result<Registry, String>>,
    /// This machine's machine id, so the registry's copy of it is not
    /// listed twice.
    pub local_machine_id: Option<String>,
    /// The session this terminal shows.
    pub current: Option<String>,
}

/// A machine with its workspaces, as the picker lists it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Machine {
    pub machine_id: String,
    pub name: String,
    pub platform: String,
    pub local: bool,
    pub online: bool,
    /// A SUPER daemon answers there: new sessions can start on it.
    pub can_start: bool,
    /// Why the machine shows nothing, when that needs saying.
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

/// A session's status as the apps show it: an ended session stays ended,
/// any other state is only current while its machine is online.
pub(crate) fn session_status(state: SessionState, machine_online: bool) -> Status {
    if state != SessionState::Ended && !machine_online {
        return Status {
            label: "offline",
            mark: "○",
            token: "dim",
        };
    }
    status(state)
}

/// A machine's presence dot, label and colour.
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

/// A session of the registry in the shape the daemon lists sessions. Its
/// time is the time of its last activity.
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
        modified_at: record.last_activity_at.clone().unwrap_or_default(),
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

/// Machines → workspaces → sessions. This machine comes first; other machines
/// follow, online ones first, then by name.
pub(crate) fn group(listing: &Listing) -> Vec<Machine> {
    let current = listing.current.as_deref();
    let mut machines = Vec::new();
    let local_machine = match &listing.local {
        Ok(local) => {
            machines.push(Machine {
                machine_id: local.machine_id.clone(),
                name: local.machine_name.clone(),
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
            Some(local.machine_id.clone())
        }
        Err(reason) => {
            machines.push(Machine {
                machine_id: listing.local_machine_id.clone().unwrap_or_default(),
                name: "this machine".into(),
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
        Some(Err(reason)) => machines.push(Machine {
            machine_id: String::new(),
            name: "other machines".into(),
            platform: String::new(),
            local: false,
            online: false,
            can_start: false,
            note: Some(reason.clone()),
            workspaces: Vec::new(),
        }),
        Some(Ok(registry)) => {
            let mut others: Vec<Machine> = registry
                .machines
                .iter()
                .filter(|machine| {
                    Some(&machine.id) != local_machine.as_ref()
                        && Some(&machine.id) != listing.local_machine_id.as_ref()
                })
                .filter_map(|machine| {
                    let entries: Vec<Entry> = registry
                        .sessions_on(&machine.id)
                        .map(|session| entry(summary_from_record(session), current))
                        .collect();
                    // The platform says which machines are online and which
                    // of them run a daemon.
                    let can_start = machine.online && machine.daemon;
                    // A machine with nothing to show and nothing to start is noise.
                    (!entries.is_empty() || can_start).then(|| Machine {
                        machine_id: machine.id.clone(),
                        name: if machine.name.is_empty() {
                            machine.id.clone()
                        } else {
                            machine.name.clone()
                        },
                        platform: machine.platform.clone(),
                        local: false,
                        online: machine.online,
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
                    .then(a.machine_id.cmp(&b.machine_id))
            });
            machines.extend(others);
        }
    }
    machines
}

/// Where a machine is reached: over the local socket or through the platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MachineRef {
    pub machine_id: String,
    pub name: String,
    pub local: bool,
    pub online: bool,
    pub can_start: bool,
}

impl MachineRef {
    fn of(machine: &Machine) -> Self {
        Self {
            machine_id: machine.machine_id.clone(),
            name: machine.name.clone(),
            local: machine.local,
            online: machine.online,
            can_start: machine.can_start,
        }
    }
}

/// What the user picked.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Choice {
    Session {
        machine: MachineRef,
        session: Box<SessionSummary>,
    },
    NewSession {
        machine: MachineRef,
        cwd: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Machine(usize),
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
    /// Show the directories of `machine` under `path`.
    Browse {
        machine: MachineRef,
        path: String,
    },
    /// Tell the user why nothing happened.
    Say(String),
}

/// The session picker: machine and workspace headers, sessions and the
/// "new session" and "new workspace" actions, filtered by what is typed.
pub(crate) struct SessionsPicker {
    machines: Vec<Machine>,
    now: i64,
    filter: String,
    rows: Vec<Row>,
    selected: usize,
    pub max_visible: usize,
    theme: Theme,
}

impl SessionsPicker {
    pub fn new(machines: Vec<Machine>, now: i64, theme: Theme) -> Self {
        let mut picker = Self {
            machines,
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
            Row::Session(d, w, s) => picker.machines[*d].workspaces[*w].sessions[*s].current,
            _ => false,
        });
        picker.selected = current
            .or_else(|| picker.rows.iter().position(|row| row.selectable()))
            .unwrap_or(0);
        picker
    }

    fn label(&self, row: Row) -> String {
        match row {
            Row::Machine(d) => self.machines[d].name.clone(),
            Row::Workspace(d, w) => self.machines[d].workspaces[w].name.clone(),
            Row::Session(d, w, s) => {
                let workspace = &self.machines[d].workspaces[w];
                format!(
                    "{} {} {}",
                    workspace.sessions[s].title, workspace.name, self.machines[d].name
                )
            }
            Row::NewSession(d, w) => format!(
                "new session in {} {}",
                self.machines[d].workspaces[w].name, self.machines[d].name
            ),
            Row::NewWorkspace(d) => format!("new workspace on {}", self.machines[d].name),
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
        for (d, machine) in self.machines.iter().enumerate() {
            let mut machine_rows = Vec::new();
            for (w, workspace) in machine.workspaces.iter().enumerate() {
                let mut workspace_rows: Vec<Row> = (0..workspace.sessions.len())
                    .map(|s| Row::Session(d, w, s))
                    .collect();
                if machine.can_start {
                    workspace_rows.push(Row::NewSession(d, w));
                }
                workspace_rows.retain(|row| self.matches(*row));
                if !workspace_rows.is_empty() {
                    machine_rows.push(Row::Workspace(d, w));
                    machine_rows.extend(workspace_rows);
                }
            }
            if machine.can_start && self.matches(Row::NewWorkspace(d)) {
                machine_rows.push(Row::NewWorkspace(d));
            }
            // A machine stays listed without a filter, so its state shows.
            if !machine_rows.is_empty() || self.filter.is_empty() {
                rows.push(Row::Machine(d));
                rows.extend(machine_rows);
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
                let machine = &self.machines[d];
                let entry = &machine.workspaces[w].sessions[s];
                if entry.current {
                    return Outcome::Close;
                }
                if !machine.local && !machine.online {
                    return Outcome::Say(format!("{} is offline", machine.name));
                }
                Outcome::Pick(Choice::Session {
                    machine: MachineRef::of(machine),
                    session: Box::new(entry.summary.clone()),
                })
            }
            Row::NewSession(d, w) => Outcome::Pick(Choice::NewSession {
                machine: MachineRef::of(&self.machines[d]),
                cwd: self.machines[d].workspaces[w].path.clone(),
            }),
            Row::NewWorkspace(d) => {
                let machine = &self.machines[d];
                // Start where the machine's newest workspace lives.
                let path = machine
                    .workspaces
                    .first()
                    .and_then(|workspace| {
                        std::path::Path::new(&workspace.path)
                            .parent()
                            .map(|parent| parent.display().to_string())
                    })
                    .or_else(|| {
                        machine
                            .local
                            .then(|| dirs::home_dir().map(|home| home.display().to_string()))
                            .flatten()
                    })
                    .unwrap_or_else(|| "/".into());
                Outcome::Browse {
                    machine: MachineRef::of(machine),
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
            Row::Machine(d) => {
                let machine = &self.machines[d];
                let dot = presence(machine.online || machine.local && machine.note.is_none());
                let mut detail = Vec::new();
                if machine.local {
                    detail.push("this machine".to_string());
                } else {
                    detail.push(dot.label.to_string());
                }
                if let Some(note) = &machine.note {
                    detail.push(note.clone());
                }
                format!(
                    "{} {}  {}",
                    theme.fg(dot.token, dot.mark),
                    theme.bold(&machine.name),
                    theme.fg("muted", &detail.join(" · "))
                )
            }
            Row::Workspace(d, w) => {
                let workspace = &self.machines[d].workspaces[w];
                format!(
                    "  {}  {}",
                    theme.bold(&workspace.name),
                    theme.fg("dim", &workspace.path)
                )
            }
            Row::Session(d, w, s) => {
                let machine = &self.machines[d];
                let entry = &machine.workspaces[w].sessions[s];
                let state = session_status(entry.summary.state, machine.online || machine.local);
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
                    &format!("+ new session in {}", self.machines[d].workspaces[w].name)
                )
            ),
            Row::NewWorkspace(d) => format!(
                "{marker}{}",
                theme.fg(
                    "accent",
                    &format!("+ new workspace on {}…", self.machines[d].name)
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

/// "New workspace on machine…": a directory of the machine, its
/// subdirectories, and "start a session here".
pub(crate) struct DirectoryPicker {
    pub machine: MachineRef,
    path: String,
    entries: Vec<DirectoryEntry>,
    filter: String,
    selected: usize,
    pub max_visible: usize,
    theme: Theme,
}

impl DirectoryPicker {
    pub fn new(
        machine: MachineRef,
        path: String,
        entries: Vec<DirectoryEntry>,
        theme: Theme,
    ) -> Self {
        Self {
            machine,
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
                &theme.bold(&format!("New workspace on {}", self.machine.name)),
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
    /// shows this machine alone.
    const REGISTRY_WAIT: Duration = Duration::from_secs(8);

    /// Everything the picker lists: this machine's daemon over the link, other
    /// machines from the registry when signed in. Connects to the platform on
    /// first use and keeps that connection in the link.
    pub(crate) async fn load(link: &mut Link, current: Option<&str>) -> Listing {
        let local = match link.daemon() {
            Some(client) => local_listing(client).await,
            None => Err("no SUPER daemon is running on this machine".into()),
        };
        let registry = if crate::remote::signed_in() {
            // A platform out of reach must not keep the picker from opening.
            let read =
                tokio::time::timeout(REGISTRY_WAIT, async {
                    match link.platform_or_connect().await {
                        Ok(platform) => platform.registry.read().await.map_err(|error| {
                            format!("could not read the platform registry: {error}")
                        }),
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
        let local_machine_id = match &local {
            Ok(listing) => Some(listing.machine_id.clone()),
            Err(_) => link.platform().map(|platform| platform.machine_id.clone()),
        };
        Listing {
            local,
            registry,
            local_machine_id,
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
            machine_id: hello.machine_id.clone(),
            machine_name: hello.machine_name.clone(),
            sessions,
        })
    }

    /// The subdirectories of `path` on `machine`.
    pub(crate) async fn list_directories(
        link: &mut Link,
        machine: &MachineRef,
        path: &str,
    ) -> Result<Vec<DirectoryEntry>, String> {
        if machine.local {
            let client = link
                .daemon()
                .ok_or("no SUPER daemon is running on this machine")?;
            return client
                .list_directories(std::path::Path::new(path))
                .await
                .map_err(|error| error.to_string());
        }
        let platform = link.platform_or_connect().await?;
        let data = machine_call(
            &platform,
            machine,
            &DaemonCommand::ListDirectories {
                path: path.to_string(),
            },
        )
        .await?;
        serde_json::from_value(data["directories"].clone()).map_err(|error| error.to_string())
    }

    async fn machine_call(
        platform: &Arc<Platform>,
        machine: &MachineRef,
        command: &DaemonCommand,
    ) -> Result<serde_json::Value, String> {
        let response = platform
            .machine_commands(&machine.machine_id)
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
            Choice::Session { machine, session } if machine.local => {
                attach_local(link, &session.session_id).await
            }
            Choice::NewSession { machine, cwd } if machine.local => {
                let client = link
                    .daemon()
                    .ok_or("no SUPER daemon is running on this machine")?;
                let created = client
                    .create_session(NewSession {
                        cwd: cwd.into(),
                        ..NewSession::default()
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                attach_local(link, &created.session_id).await
            }
            Choice::Session { machine, session } => {
                if !machine.online {
                    return Err(format!("{} is offline", machine.name));
                }
                let platform = link.platform_or_connect().await?;
                if session.state == SessionState::Ended {
                    if !machine.can_start {
                        return Err(format!(
                            "the session ended on {}, which runs no SUPER daemon to resume it",
                            machine.name
                        ));
                    }
                    machine_call(
                        &platform,
                        &machine,
                        &DaemonCommand::ResumeSession {
                            session_id: session.session_id.clone(),
                        },
                    )
                    .await?;
                }
                attach_remote(link, platform, &machine, *session).await
            }
            Choice::NewSession { machine, cwd } => {
                if !machine.can_start {
                    return Err(format!(
                        "{} runs no SUPER daemon that could start a session",
                        machine.name
                    ));
                }
                let platform = link.platform_or_connect().await?;
                let data = machine_call(
                    &platform,
                    &machine,
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
                attach_remote(link, platform, &machine, session).await
            }
        }
    }

    /// Attach the daemon connection to a session on this machine. Detaching
    /// first, and dropping what queued up, keeps the previous session's last
    /// events from showing under the new one. When the daemon refuses the
    /// new one, the connection goes back to the session it showed.
    async fn attach_local(link: &mut Link, session_id: &str) -> Result<SessionSummary, String> {
        const NO_DAEMON: &str = "no SUPER daemon is running on this machine";
        let previous = link
            .daemon()
            .ok_or(NO_DAEMON)?
            .detach()
            .await
            .ok()
            .flatten();
        link.drain();
        let client = link.daemon().ok_or(NO_DAEMON)?;
        let attached = match client.attach(session_id).await {
            Ok(attached) => attached,
            Err(error) => {
                let mut message = error.to_string();
                if let Some(previous) = previous
                    && let Err(back) = client.attach(&previous).await
                {
                    message.push_str(&format!(
                        "; this terminal could not go back to {}: {back}",
                        short_id(&previous)
                    ));
                }
                return Err(message);
            }
        };
        link.set_remote(None);
        Ok(attached.session)
    }

    async fn attach_remote(
        link: &mut Link,
        platform: Arc<Platform>,
        machine: &MachineRef,
        session: SessionSummary,
    ) -> Result<SessionSummary, String> {
        let remote = RemoteSession::open(
            platform,
            &machine.machine_id,
            &machine.name,
            &session.session_id,
        )
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
        let mut sessions = SessionsPicker::new(group(&listing), now, theme.clone());
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
                                machine: open.machine.clone(),
                                cwd: path,
                            });
                            None
                        }
                        BrowseOutcome::Open(path) => Some((open.machine.clone(), path)),
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
                        Outcome::Browse { machine, path } => Some((machine, path)),
                    },
                };
                if let Some((machine, path)) = browse {
                    match list_directories(link, &machine, &path).await {
                        Ok(entries) => {
                            browser =
                                Some(DirectoryPicker::new(machine, path, entries, theme.clone()));
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

    /// A session as `GET /api/registry` lists it.
    fn listed(
        machine: &str,
        id: &str,
        workspace: &str,
        state: &str,
        last_activity: i64,
    ) -> serde_json::Value {
        serde_json::json!({
            "machineId": machine, "id": id, "workspace": workspace, "cwd": workspace,
            "state": state, "blockedReason": null, "model": "super/gpt-5",
            "thinkingLevel": "low", "name": null, "firstMessage": null,
            "resumeArgv": ["super", "--session", format!("/h/{id}.jsonl")],
            "daemon": true, "headless": false, "lastActivityAt": time(last_activity),
            "createdAt": time(last_activity), "updatedAt": time(NOW - 1_000),
        })
    }

    fn registry_with(extra: Vec<serde_json::Value>) -> Registry {
        let machine = |id: &str, name: &str, online: bool, daemon: bool| {
            serde_json::json!({
                "id": id, "name": name, "platform": "linux", "daemon": daemon,
                "online": online, "lastSeenAt": time(NOW - 5_000), "createdAt": time(0),
            })
        };
        let mut sessions = vec![
            listed("01M", "s1", "/w/super", "idle", NOW - 5_000),
            listed("02L", "r1", "/srv/app", "working", NOW - 10_000),
            listed("03O", "o1", "/home/o", "ended", NOW - 3_600_000),
        ];
        sessions.extend(extra);
        Registry::from_value(&serde_json::json!({
            "version": 7,
            "reset": true,
            "machines": [
                // This machine is in the registry too; the daemon's listing wins.
                machine("01M", "mbp", true, true),
                machine("02L", "linuxbox", true, true),
                machine("03O", "old-laptop", false, false),
                // Nothing to show and nothing to start: left out.
                machine("04E", "empty", true, false),
                // A daemon with no session yet: listed, for "new workspace".
                machine("05Z", "zeta", true, true),
            ],
            "sessions": sessions,
            "projects": [],
            "chats": [],
            "deleted": [],
        }))
    }

    fn registry() -> Registry {
        registry_with(Vec::new())
    }

    fn listing() -> Listing {
        Listing {
            local: Ok(LocalListing {
                machine_id: "01M".into(),
                machine_name: "mbp".into(),
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
            local_machine_id: Some("01M".into()),
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
    fn a_session_on_another_machine_shows_when_it_was_last_active() {
        // The registry heard of the session a second ago (its daemon signed
        // in again); the session itself was last used three days ago.
        let three_days = NOW - 3 * 86_400_000;
        let registry = registry_with(vec![listed("02L", "r2", "/srv/app", "ended", three_days)]);
        let machines = group(&Listing {
            registry: Some(Ok(registry)),
            ..listing()
        });
        let linuxbox = machines
            .iter()
            .find(|machine| machine.name == "linuxbox")
            .unwrap();
        let entries = &linuxbox.workspaces[0].sessions;
        let ids: Vec<&str> = entries
            .iter()
            .map(|entry| entry.summary.session_id.as_str())
            .collect();
        assert_eq!(ids, ["r1", "r2"], "the session used last comes first");
        assert_eq!(entries[1].modified, three_days);
        assert_eq!(ago(entries[1].modified, NOW), "3d");

        let mut picker = SessionsPicker::new(machines, NOW, Theme::dark());
        picker.max_visible = 40;
        let text = strip_ansi(&picker.render(120).join("\n"));
        assert!(text.contains("✓ ended           r2  3d"), "{text}");
    }

    #[test]
    fn sessions_group_by_machine_then_workspace_this_machine_first() {
        let machines = group(&listing());
        let names: Vec<&str> = machines
            .iter()
            .map(|machine| machine.name.as_str())
            .collect();
        assert_eq!(names, ["mbp", "linuxbox", "zeta", "old-laptop"]);

        let mbp = &machines[0];
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

        let linuxbox = &machines[1];
        assert!(!linuxbox.local && linuxbox.online && linuxbox.can_start);
        let remote = &linuxbox.workspaces[0].sessions[0];
        assert_eq!(remote.summary.state, SessionState::Working);
        assert_eq!(remote.summary.session_file, "/h/r1.jsonl");
        assert_eq!(remote.summary.model.as_deref(), Some("super/gpt-5"));
        assert!(remote.summary.hosted);

        assert!(machines[2].workspaces.is_empty() && machines[2].can_start);
        let laptop = &machines[3];
        assert!(!laptop.online && !laptop.can_start);
        assert_eq!(
            laptop.workspaces[0].sessions[0].summary.state,
            SessionState::Ended
        );
    }

    #[test]
    fn a_session_on_an_offline_machine_reads_offline_unless_it_ended() {
        assert_eq!(
            session_status(SessionState::Working, false).label,
            "offline"
        );
        assert_eq!(
            session_status(SessionState::Blocked, false).label,
            "offline"
        );
        assert_eq!(session_status(SessionState::Idle, false).label, "offline");
        assert_eq!(session_status(SessionState::Ended, false).label, "ended");
        assert_eq!(session_status(SessionState::Working, true).label, "working");

        let mut left_running = listed("03O", "o2", "/home/o", "working", NOW - 3_600_000);
        left_running["firstMessage"] = "left running".into();
        let listing = Listing {
            registry: Some(Ok(registry_with(vec![left_running]))),
            ..listing()
        };
        let mut picker = SessionsPicker::new(group(&listing), NOW, Theme::dark());
        picker.max_visible = 40;
        let text = strip_ansi(&picker.render(120).join("\n"));
        assert!(text.contains("○ offline         left running"), "{text}");
        assert!(!text.contains("working         left running"), "{text}");
    }

    #[test]
    fn a_missing_daemon_or_registry_says_why() {
        let listing = Listing {
            local: Err("no SUPER daemon is running on this machine".into()),
            registry: Some(Err("could not reach the platform: offline".into())),
            local_machine_id: None,
            current: None,
        };
        let machines = group(&listing);
        assert_eq!(machines.len(), 2);
        assert_eq!(machines[0].name, "this machine");
        assert!(!machines[0].can_start);
        assert_eq!(
            machines[0].note.as_deref(),
            Some("no SUPER daemon is running on this machine")
        );
        assert_eq!(machines[1].name, "other machines");
        assert!(machines[1].note.as_deref().unwrap().contains("offline"));

        let signed_out = Listing {
            registry: None,
            ..listing
        };
        assert_eq!(group(&signed_out).len(), 1);
    }

    fn key(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            ..KeyEvent::default()
        }
    }

    fn picker() -> SessionsPicker {
        SessionsPicker::new(group(&listing()), NOW, Theme::dark())
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
                machine: MachineRef {
                    machine_id: "01M".into(),
                    name: "mbp".into(),
                    local: true,
                    online: true,
                    can_start: true,
                },
                cwd: "/w/super".into(),
            })
        );
        picker.handle_key(&key(Key::Up));
        let Outcome::Pick(Choice::Session { machine, session }) =
            picker.handle_key(&key(Key::Enter))
        else {
            panic!("a session row");
        };
        assert!(machine.local);
        assert_eq!(session.session_id, "s1");
    }

    #[test]
    fn typing_filters_and_offline_machines_are_refused() {
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
        let Outcome::Pick(Choice::Session { machine, session }) =
            picker.handle_key(&key(Key::Enter))
        else {
            panic!("the remote session");
        };
        assert_eq!(machine.name, "linuxbox");
        assert!(!machine.local && machine.online);
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
        let Outcome::Browse { machine, path } = picker.handle_key(&key(Key::Enter)) else {
            panic!("a browse");
        };
        assert_eq!(machine.machine_id, "02L");
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
        let mut browser = DirectoryPicker::new(machine, path, entries, Theme::dark());
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
    fn the_picker_renders_machines_workspaces_states_and_actions() {
        let mut picker = picker();
        picker.max_visible = 40;
        let text: Vec<String> = picker
            .render(120)
            .iter()
            .map(|line| strip_ansi(line))
            .collect();
        let joined = text.join("\n");
        assert_eq!(text[0], "Sessions");
        assert!(joined.contains("● mbp  this machine"), "{joined}");
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
            "no actions on an offline machine"
        );
        assert!(
            text.last().unwrap().contains("esc close"),
            "the hint comes last"
        );
        assert!(text.iter().all(|line| display_width(line) <= 120));
    }
}

/// Two sessions in a daemon on a temporary socket, and a third on "another
/// machine": a daemon whose platform link runs against the fake S2. The link
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
    use super_remote::fake::{FakePlatform, TestCredentials, registry_client, serve};
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

    /// A stored session of this machine's daemon that nothing hosts.
    async fn client_create_and_unload(link: &Link, cwd: &std::path::Path) -> SessionSummary {
        let client = link.daemon().unwrap();
        let created = client
            .create_session(NewSession {
                cwd: cwd.to_path_buf(),
                model: Some("mock/mock-1".into()),
                ..NewSession::default()
            })
            .await
            .unwrap();
        client.stop_session(&created.session_id).await.unwrap();
        created
    }

    /// How many clients each session of this machine's daemon has attached.
    async fn attachments(link: &Link) -> std::collections::BTreeMap<String, usize> {
        link.daemon()
            .unwrap()
            .list_sessions(None)
            .await
            .unwrap()
            .into_iter()
            .map(|session| (session.session_id, session.attached_clients))
            .collect()
    }

    async fn shown_session(link: &Link) -> String {
        let state = link
            .execute_ok(Command::GetState {})
            .await
            .unwrap()
            .unwrap();
        state["sessionId"].as_str().unwrap().to_string()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_link_switches_between_sessions_here_and_on_another_machine() {
        let dir = tempfile::Builder::new().prefix("sw").tempdir().unwrap();
        let provider = MockProvider::start(dir.path().join("mock"), MockScript::text("Hello."))
            .await
            .unwrap();
        let work: PathBuf = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();

        // This machine's daemon, on a temporary socket.
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
                machine_id: "01M".into(),
                machine_name: "mbp".into(),
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

        // Another machine: a daemon whose link to the fake platform announces
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
        // It was last used two days ago.
        std::fs::File::options()
            .write(true)
            .open(&remote_file)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(2 * 86_400))
            .unwrap();
        let remote_activity = super_remote::registry::modified_at(&remote_file).unwrap();
        let ctx = ServerContext::new(
            remote_host,
            DaemonInfo {
                version: "test".into(),
                machine_id: "01L".into(),
                machine_name: "linuxbox".into(),
                platform: "linux".into(),
                socket_path: dir.path().join("remote.sock"),
                protocol_versions: super_sdk::daemon::PROTOCOL_VERSIONS,
            },
            CancellationToken::new(),
        );
        let fake = Arc::new(FakePlatform::default());
        let s2 = Arc::new(super_remote::s2::S2Client::new(
            reqwest::Client::new(),
            serve(fake.clone()).await,
            Arc::new(TestCredentials),
        ));
        let registry = registry_client(&fake);
        let presence = PresenceLink::start(
            s2.clone(),
            registry.clone(),
            ctx.clone(),
            super_remote::MachineIdentity {
                machine_id: "01L".into(),
                name: "linuxbox".into(),
                platform: "linux".into(),
            },
            LinkConfig {
                cursor_dir: Some(dir.path().join("cursors")),
                ..LinkConfig::default()
            },
        );
        let platform = Arc::new(Platform {
            client: s2,
            registry,
            sender: Sender {
                device_id: "device-mac".into(),
                name: "Terminal on mbp".into(),
            },
            machine_id: "01M".into(),
        });
        let mut link = Link::new(Some((client, events)), Some(platform));

        // `/sessions` lists both machines once the other one announced itself.
        let mut machines = Vec::new();
        for _ in 0..200 {
            let mut listing = load(&mut link, Some(&first.session_id)).await;
            // The test is not signed in; read the registry the way a
            // signed-in terminal does.
            listing.registry = Some(
                link.platform()
                    .unwrap()
                    .registry
                    .read()
                    .await
                    .map_err(|error| error.to_string()),
            );
            machines = group(&listing);
            if machines.len() == 2 && !machines[1].workspaces.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(machines.len(), 2, "{machines:?}");
        let here = MachineRef::of(&machines[0]);
        let there = MachineRef::of(&machines[1]);
        assert!(here.local && there.online && there.can_start);
        assert_eq!(there.name, "linuxbox");
        let local_ids: Vec<&str> = machines[0].workspaces[0]
            .sessions
            .iter()
            .map(|entry| entry.summary.session_id.as_str())
            .collect();
        assert_eq!(local_ids.len(), 2);
        assert!(
            machines[0].workspaces[0]
                .sessions
                .iter()
                .any(|entry| entry.current)
        );
        let remote_entry = machines[1].workspaces[0].sessions[0].summary.clone();
        assert_eq!(remote_entry.session_id, remote_id);
        // Its time is when it was last used, as its daemon told the
        // registry, not when the registry heard of it.
        assert_eq!(remote_entry.modified_at, remote_activity);
        assert_eq!(
            ago(
                machines[1].workspaces[0].sessions[0].modified,
                chrono::Utc::now().timestamp_millis()
            ),
            "2d"
        );

        // Over to the second session here, a prompt, and back.
        let shown = switch(
            &mut link,
            Choice::Session {
                machine: here.clone(),
                session: Box::new(second.clone()),
            },
        )
        .await
        .unwrap();
        assert_eq!(shown.session_id, second.session_id);
        let attached = attachments(&link).await;
        assert_eq!(attached[&first.session_id], 0, "never attached twice");
        assert_eq!(attached[&second.session_id], 1);
        assert_eq!(
            prompt_and_settle(&mut link, "for the second").await,
            "Hello."
        );
        let shown = switch(
            &mut link,
            Choice::Session {
                machine: here.clone(),
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

        // A session another process holds is refused, and the terminal keeps
        // showing the one it showed.
        let held = client_create_and_unload(&link, &work).await;
        let other_process =
            super_daemon::session_lock::try_lock(std::path::Path::new(&held.session_file))
                .unwrap()
                .expect("free once unloaded");
        let refused = switch(
            &mut link,
            Choice::Session {
                machine: here.clone(),
                session: Box::new(held.clone()),
            },
        )
        .await
        .unwrap_err();
        assert!(refused.contains("open in another process"), "{refused}");
        assert_eq!(shown_session(&link).await, first.session_id);
        let attached = attachments(&link).await;
        assert_eq!(attached[&first.session_id], 1, "still attached");
        assert_eq!(attached[&second.session_id], 0);
        drop(other_process);

        // Over to the session on the other machine: the same commands.
        switch(
            &mut link,
            Choice::Session {
                machine: there.clone(),
                session: Box::new(remote_entry),
            },
        )
        .await
        .unwrap();
        assert!(link.remote().is_some());
        assert!(
            attachments(&link).await.values().all(|count| *count == 0),
            "the terminal follows the other machine only"
        );
        assert_eq!(link.describe(), "linuxbox through the SUPER platform");
        assert_eq!(prompt_and_settle(&mut link, "for linuxbox").await, "Hello.");
        assert_eq!(user_messages(&link).await, ["for linuxbox"]);
        assert!(
            std::fs::read_to_string(&remote_file)
                .unwrap()
                .contains("for linuxbox")
        );

        // A new session on the other machine, in a directory it lists.
        let directories = list_directories(&mut link, &there, &dir.path().display().to_string())
            .await
            .unwrap();
        assert!(directories.iter().any(|entry| entry.name == "work"));
        let created = switch(
            &mut link,
            Choice::NewSession {
                machine: there.clone(),
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
                machine: here.clone(),
                session: Box::new(first.clone()),
            },
        )
        .await
        .unwrap();
        assert!(link.remote().is_none());
        assert_eq!(attachments(&link).await[&first.session_id], 1);
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
