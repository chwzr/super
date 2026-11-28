//! The sessions the daemon hosts: loading, listing, state, unloading.
//!
//! A session is its JSONL file; it is *hosted* while an `Arc<Session>` for it
//! lives here and *stored* otherwise. Clients see `idle`, `working` and
//! `blocked` for a hosted session and `ended` for a stored one.

use crate::log;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};
use super_coding::SessionManager;
use super_coding::session::manager::SessionListing;
use super_sdk::daemon::{DaemonEvent, EndReason, SessionState, SessionSummary, WorkspaceSummary};
use super_sdk::{
    AgentMessage, Session, SessionEntry, SessionOptions, SessionSource, ThinkingLevel,
};
use tokio::sync::broadcast;

/// How long a session a client stops waits for an answer to the question
/// whether to keep or remove the worktree it works in.
pub const WORKTREE_QUESTION_WAIT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub session_dir: PathBuf,
    /// Unload a hosted session with no client after this much quiet.
    /// `Duration::ZERO` disables the sweep.
    pub idle_unload: Duration,
    /// An alternative model catalog; tests point this at the mock provider.
    pub models_file: Option<PathBuf>,
    pub no_context_files: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CreateRequest {
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub thinking_level: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("no session {0}")]
    Unknown(String),
    #[error("session {0} is open in another process")]
    Locked(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0:#}")]
    Startup(anyhow::Error),
}

/// One hosted session and what the daemon knows about it.
pub struct Hosted {
    pub session: Arc<Session>,
    pub session_id: String,
    pub cwd: PathBuf,
    pub file: PathBuf,
    /// Released by `stop`, before the `Arc` necessarily goes away.
    lock: Mutex<Option<File>>,
    attached: AtomicUsize,
    last_activity: Mutex<Instant>,
    state: Mutex<(SessionState, Option<String>)>,
    tracker: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Hosted {
    /// Note activity, which postpones the idle unload.
    pub fn touch(&self) {
        *self.last_activity.lock().unwrap() = Instant::now();
    }

    pub fn state(&self) -> (SessionState, Option<String>) {
        self.state.lock().unwrap().clone()
    }

    pub fn attached(&self) -> usize {
        self.attached.load(Ordering::SeqCst)
    }

    fn idle_for(&self) -> Duration {
        self.last_activity.lock().unwrap().elapsed()
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        if let Some(tracker) = self.tracker.lock().unwrap().take() {
            tracker.abort();
        }
        self.session.close();
    }
}

pub struct SessionHost {
    config: HostConfig,
    hosted: Mutex<BTreeMap<String, Arc<Hosted>>>,
    events: broadcast::Sender<DaemonEvent>,
}

impl SessionHost {
    pub fn new(config: HostConfig) -> Arc<Self> {
        let (events, _) = broadcast::channel(256);
        Arc::new(Self {
            config,
            hosted: Mutex::new(BTreeMap::new()),
            events,
        })
    }

    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    /// Daemon-level events: every connection subscribes.
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.events.subscribe()
    }

    pub fn announce(&self, event: DaemonEvent) {
        let _ = self.events.send(event);
    }

    pub fn get(&self, session_id: &str) -> Option<Arc<Hosted>> {
        self.hosted.lock().unwrap().get(session_id).cloned()
    }

    pub fn hosted_count(&self) -> usize {
        self.hosted.lock().unwrap().len()
    }

    /// Build a new session in `cwd`, which must exist.
    pub async fn create(
        self: &Arc<Self>,
        request: CreateRequest,
    ) -> Result<Arc<Hosted>, HostError> {
        if !request.cwd.is_absolute() {
            return Err(HostError::Invalid(format!(
                "cwd must be an absolute path: {}",
                request.cwd.display()
            )));
        }
        if !request.cwd.is_dir() {
            return Err(HostError::Invalid(format!(
                "no such directory: {}",
                request.cwd.display()
            )));
        }
        let thinking_level =
            match request.thinking_level.as_deref() {
                None => None,
                Some(level) => Some(ThinkingLevel::parse(level).ok_or_else(|| {
                    HostError::Invalid(format!("unknown thinking level '{level}'"))
                })?),
            };
        let options = SessionOptions {
            cwd: request.cwd.clone(),
            model: request.model,
            provider: request.provider,
            thinking_level,
            models_file: self.config.models_file.clone(),
            session: SessionSource::Create,
            session_dir: Some(self.config.session_dir.clone()),
            session_name: request.name,
            no_context_files: self.config.no_context_files,
            ..Default::default()
        };
        let session = Session::create(options)
            .await
            .map_err(|error| HostError::Startup(anyhow::Error::new(error)))?;
        let file = session
            .state()
            .session_file
            .ok_or_else(|| HostError::Invalid("the session has no file".into()))?;
        let session_id = session.state().session_id;
        let lock = crate::session_lock::try_lock(&file)
            .map_err(|error| HostError::Startup(error.into()))?
            .ok_or_else(|| HostError::Locked(session_id.clone()))?;
        // Clients get the session id now and may stop it before anything is
        // said in it; `resume_session` and `attach` then need the file.
        let persisted = session
            .agent_session()
            .manager
            .lock()
            .unwrap()
            .persist_header();
        if let Err(error) = persisted {
            session.close();
            return Err(HostError::Startup(
                error.context(format!("write session file {}", file.display())),
            ));
        }
        let hosted = self.install(session, request.cwd, file, lock);
        log::info("host", format!("created session {session_id}"));
        self.announce(DaemonEvent::SessionCreated {
            session: self.summary(&hosted),
        });
        Ok(hosted)
    }

    /// Host a stored session; a no-op when it is hosted already.
    pub async fn load(self: &Arc<Self>, session_id: &str) -> Result<Arc<Hosted>, HostError> {
        if let Some(hosted) = self.get(session_id) {
            return Ok(hosted);
        }
        let dir = self.config.session_dir.clone();
        let wanted = session_id.to_string();
        let found = tokio::task::spawn_blocking(move || {
            let path = SessionManager::find_by_id(&dir, &wanted)?;
            path.map(|path| {
                SessionManager::open(&path).map(|manager| (path, manager.cwd().to_path_buf()))
            })
            .transpose()
        })
        .await
        .map_err(|error| HostError::Invalid(error.to_string()))?
        .map_err(HostError::Startup)?;
        let Some((path, cwd)) = found else {
            return Err(HostError::Unknown(session_id.to_string()));
        };
        let Some(lock) = crate::session_lock::try_lock(&path)
            .map_err(|error| HostError::Startup(error.into()))?
        else {
            // Either another process, or a concurrent `load` of ours.
            return match self.get(session_id) {
                Some(hosted) => Ok(hosted),
                None => Err(HostError::Locked(session_id.to_string())),
            };
        };
        let options = SessionOptions {
            cwd: cwd.clone(),
            models_file: self.config.models_file.clone(),
            session: SessionSource::Open(path.clone()),
            session_dir: Some(self.config.session_dir.clone()),
            no_context_files: self.config.no_context_files,
            ..Default::default()
        };
        let session = Session::create(options)
            .await
            .map_err(|error| HostError::Startup(anyhow::Error::new(error)))?;
        let opened_id = session.state().session_id;
        if !opened_id.starts_with(session_id) {
            return Err(HostError::Invalid(format!(
                "session {session_id} resolved to {opened_id}"
            )));
        }
        let hosted = self.install(session, cwd, path, lock);
        log::info("host", format!("loaded session {opened_id}"));
        self.announce(DaemonEvent::SessionUpdated {
            session: self.summary(&hosted),
        });
        Ok(hosted)
    }

    fn install(
        self: &Arc<Self>,
        session: Arc<Session>,
        cwd: PathBuf,
        file: PathBuf,
        lock: File,
    ) -> Arc<Hosted> {
        super_sdk::install_workflow_approval_gate(&session);
        let session_id = session.state().session_id;
        let events = session.events();
        let hosted = Arc::new(Hosted {
            session,
            session_id: session_id.clone(),
            cwd,
            file,
            lock: Mutex::new(Some(lock)),
            attached: AtomicUsize::new(0),
            last_activity: Mutex::new(Instant::now()),
            state: Mutex::new((SessionState::Idle, None)),
            tracker: Mutex::new(None),
        });
        let tracker = tokio::spawn(track_state(
            Arc::downgrade(self),
            Arc::downgrade(&hosted),
            events,
        ));
        *hosted.tracker.lock().unwrap() = Some(tracker);
        self.hosted
            .lock()
            .unwrap()
            .insert(session_id, hosted.clone());
        hosted
    }

    /// A client attached; the session will not be swept while any is.
    pub fn attach(&self, hosted: &Arc<Hosted>) {
        hosted.attached.fetch_add(1, Ordering::SeqCst);
        hosted.touch();
        self.announce(DaemonEvent::SessionUpdated {
            session: self.summary(hosted),
        });
    }

    pub fn detach(&self, hosted: &Arc<Hosted>) {
        // A saturating decrement: a client never detaches twice, but a
        // counter must not wrap if one ever did.
        let mut current = hosted.attached.load(Ordering::SeqCst);
        while current > 0 {
            match hosted.attached.compare_exchange(
                current,
                current - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        hosted.touch();
        self.announce(DaemonEvent::SessionUpdated {
            session: self.summary(hosted),
        });
    }

    /// Abort the running turn, unload the session, keep its file. A session
    /// a client stops while it works in a worktree it made asks its clients
    /// first whether to keep or remove the worktree, for at most
    /// [`WORKTREE_QUESTION_WAIT`]; without an answer, and when the session
    /// is unloaded while idle or with the daemon, the worktree is kept.
    pub async fn stop(&self, session_id: &str, reason: EndReason) -> Result<(), HostError> {
        if reason == EndReason::Stopped {
            let asking = self.hosted.lock().unwrap().get(session_id).cloned();
            if let Some(hosted) = asking
                && let Some(done) = hosted.session.settle_worktree(WORKTREE_QUESTION_WAIT).await
            {
                log::info("host", format!("session {session_id}: {done}"));
            }
        }
        let hosted = self
            .hosted
            .lock()
            .unwrap()
            .remove(session_id)
            .ok_or_else(|| HostError::Unknown(session_id.to_string()))?;
        hosted.session.abort();
        let _ = tokio::time::timeout(Duration::from_secs(5), hosted.session.wait_idle()).await;
        *hosted.lock.lock().unwrap() = None;
        drop(hosted);
        log::info(
            "host",
            format!("unloaded session {session_id} ({})", reason_label(reason)),
        );
        self.announce(DaemonEvent::SessionEnded {
            session_id: session_id.to_string(),
            reason,
        });
        Ok(())
    }

    pub async fn shutdown(&self) {
        let ids: Vec<String> = self.hosted.lock().unwrap().keys().cloned().collect();
        for id in ids {
            let _ = self.stop(&id, EndReason::Shutdown).await;
        }
    }

    /// Unload hosted sessions that are idle, unattached and quiet for longer
    /// than the configured timeout. A session with a running background
    /// task, a pending wake-up or a queued notice stays. Returns how many
    /// were unloaded.
    pub async fn sweep_idle(&self) -> usize {
        if self.config.idle_unload.is_zero() {
            return 0;
        }
        let candidates: Vec<String> = self
            .hosted
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, hosted)| {
                hosted.attached() == 0
                    && hosted.state().0 == SessionState::Idle
                    && hosted.idle_for() >= self.config.idle_unload
                    && !hosted.session.agent_session().has_background_work()
            })
            .map(|(id, _)| id.clone())
            .collect();
        let mut unloaded = 0;
        for id in candidates {
            if self.stop(&id, EndReason::Idle).await.is_ok() {
                unloaded += 1;
            }
        }
        unloaded
    }

    /// Hosted and stored sessions, newest first. `cwd` limits the listing.
    pub async fn list(&self, cwd: Option<&Path>) -> Vec<SessionSummary> {
        let dir = self.config.session_dir.clone();
        let filter = cwd.map(Path::to_path_buf);
        let scan_cwd = filter.clone();
        let stored: Vec<SessionListing> = tokio::task::spawn_blocking(move || match scan_cwd {
            Some(cwd) => SessionManager::list(&cwd, &dir),
            None => SessionManager::list_all(&dir),
        })
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
        let hosted: BTreeMap<String, Arc<Hosted>> = self.hosted.lock().unwrap().clone();
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<SessionSummary> = Vec::new();
        for listing in stored {
            seen.insert(listing.id.clone());
            match hosted.get(&listing.id) {
                Some(live) => out.push(self.summary(live)),
                None => out.push(stored_summary(&listing)),
            }
        }
        for (id, live) in &hosted {
            let in_scope = filter.as_deref().is_none_or(|cwd| cwd == live.cwd);
            if in_scope && !seen.contains(id) {
                out.push(self.summary(live));
            }
        }
        out.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
        out
    }

    /// The sessions grouped by working directory, newest first.
    pub async fn workspaces(&self) -> Vec<WorkspaceSummary> {
        let mut groups: BTreeMap<String, WorkspaceSummary> = BTreeMap::new();
        for session in self.list(None).await {
            let entry = groups
                .entry(session.cwd.clone())
                .or_insert_with(|| WorkspaceSummary {
                    name: Path::new(&session.cwd)
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| session.cwd.clone()),
                    path: session.cwd.clone(),
                    session_count: 0,
                    latest_state: session.state,
                    latest_modified_at: session.modified_at.clone(),
                });
            entry.session_count += 1;
        }
        let mut out: Vec<WorkspaceSummary> = groups.into_values().collect();
        out.sort_by(|a, b| b.latest_modified_at.cmp(&a.latest_modified_at));
        out
    }

    pub fn summary(&self, hosted: &Arc<Hosted>) -> SessionSummary {
        let agent = hosted.session.agent_session();
        let model = agent.model();
        let thinking = agent.thinking_level();
        let (name, first_message) = {
            let manager = agent.manager.lock().unwrap();
            (
                manager.session_name(),
                first_user_message(manager.entries()),
            )
        };
        let (state, blocked_reason) = hosted.state();
        SessionSummary {
            session_id: hosted.session_id.clone(),
            cwd: hosted.cwd.display().to_string(),
            workspace: hosted.cwd.display().to_string(),
            session_file: hosted.file.display().to_string(),
            name,
            first_message,
            state,
            blocked_reason,
            hosted: true,
            attached_clients: hosted.attached(),
            model: Some(format!("{}/{}", model.provider, model.id)),
            thinking_level: Some(thinking.as_str().to_string()),
            modified_at: file_modified_at(&hosted.file)
                .unwrap_or_else(|| rfc3339(SystemTime::now())),
        }
    }
}

fn stored_summary(listing: &SessionListing) -> SessionSummary {
    SessionSummary {
        session_id: listing.id.clone(),
        cwd: listing.cwd.clone(),
        workspace: listing.cwd.clone(),
        session_file: listing.path.display().to_string(),
        name: listing.name.clone(),
        first_message: listing.first_message.clone(),
        state: SessionState::Ended,
        blocked_reason: None,
        hosted: false,
        attached_clients: 0,
        model: None,
        thinking_level: None,
        modified_at: rfc3339(listing.modified),
    }
}

fn first_user_message(entries: &[SessionEntry]) -> Option<String> {
    entries.iter().find_map(|entry| match entry {
        SessionEntry::Message {
            message: AgentMessage::User(user),
            ..
        } => {
            let text = user.content.as_text();
            let text = text.trim();
            (!text.is_empty()).then(|| text.chars().take(200).collect::<String>())
        }
        _ => None,
    })
}

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// When a session's file was last written, as a summary's `modified_at`
/// names it: the time of the session's newest entry.
pub(crate) fn file_modified_at(file: &Path) -> Option<String> {
    std::fs::metadata(file)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(rfc3339)
}

fn reason_label(reason: EndReason) -> &'static str {
    match reason {
        EndReason::Stopped => "stopped",
        EndReason::Idle => "idle",
        EndReason::Shutdown => "shutdown",
    }
}

/// What one session event does to the client-visible state. `None` leaves
/// it alone. `Blocked` is sticky until the approval is resolved.
pub fn next_state(
    current: &(SessionState, Option<String>),
    event: &Value,
    streaming: bool,
) -> Option<(SessionState, Option<String>)> {
    let kind = event.get("type").and_then(Value::as_str)?;
    match kind {
        "approval_request" => Some((
            SessionState::Blocked,
            event
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string),
        )),
        "approval_resolved" => Some((
            if streaming {
                SessionState::Working
            } else {
                SessionState::Idle
            },
            None,
        )),
        _ if current.0 == SessionState::Blocked => None,
        "agent_start" | "turn_start" => Some((SessionState::Working, None)),
        "agent_settled" => Some((SessionState::Idle, None)),
        _ => None,
    }
}

/// Whether `event` reports a change to what a session's summary shows
/// besides its state: a successful command that renamed it or changed its
/// model or thinking level.
fn changes_summary(event: &Value) -> bool {
    match event.get("type").and_then(Value::as_str) {
        Some("model_changed") => true,
        Some("command_applied") => {
            event.get("success").and_then(Value::as_bool) == Some(true)
                && matches!(
                    event.pointer("/command/type").and_then(Value::as_str),
                    Some("set_session_name" | "set_model" | "set_thinking_level")
                )
        }
        _ => false,
    }
}

/// Follow one hosted session's events, keep its state, announce changes.
/// Holds only weak references, so a dropped session ends the task.
async fn track_state(
    host: Weak<SessionHost>,
    hosted: Weak<Hosted>,
    mut events: super_sdk::EventStream,
) {
    while let Some(event) = events.recv().await {
        let (Some(host), Some(hosted)) = (host.upgrade(), hosted.upgrade()) else {
            break;
        };
        hosted.touch();
        let streaming = hosted.session.agent_session().is_running();
        let current = hosted.state();
        if let Some(next) = next_state(&current, &event.0, streaming)
            && next != current
        {
            *hosted.state.lock().unwrap() = next;
            host.announce(DaemonEvent::SessionUpdated {
                session: host.summary(&hosted),
            });
        } else if changes_summary(&event.0) {
            // A client renamed the session or changed its model or thinking
            // level: the listing shows it.
            host.announce(DaemonEvent::SessionUpdated {
                session: host.summary(&hosted),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use super_sdk::mock::{MockProvider, MockScript};

    fn state(kind: SessionState) -> (SessionState, Option<String>) {
        (kind, None)
    }

    #[test]
    fn renames_and_model_or_thinking_changes_update_the_listing() {
        let applied = |command: &str, success: bool| {
            json!({"type": "command_applied", "origin": "Terminal",
                   "command": {"type": command}, "success": success})
        };
        assert!(changes_summary(&applied("set_thinking_level", true)));
        assert!(changes_summary(&applied("set_session_name", true)));
        assert!(changes_summary(&applied("set_model", true)));
        assert!(changes_summary(
            &json!({"type": "model_changed", "provider": "mock", "modelId": "mock-1"})
        ));
        assert!(!changes_summary(&applied("set_thinking_level", false)));
        assert!(!changes_summary(&applied("abort", true)));
        assert!(!changes_summary(&json!({"type": "message_end"})));
    }

    #[test]
    fn events_drive_the_state_machine() {
        let idle = state(SessionState::Idle);
        assert_eq!(
            next_state(&idle, &json!({"type": "agent_start"}), true),
            Some(state(SessionState::Working))
        );
        let working = state(SessionState::Working);
        assert_eq!(
            next_state(&working, &json!({"type": "agent_settled"}), false),
            Some(state(SessionState::Idle))
        );
        assert_eq!(
            next_state(
                &working,
                &json!({"type": "approval_request", "title": "Run workflow audit?"}),
                true
            ),
            Some((SessionState::Blocked, Some("Run workflow audit?".into())))
        );
        let blocked = (SessionState::Blocked, Some("x".to_string()));
        assert_eq!(
            next_state(&blocked, &json!({"type": "agent_settled"}), false),
            None
        );
        assert_eq!(
            next_state(&blocked, &json!({"type": "approval_resolved"}), true),
            Some(state(SessionState::Working))
        );
        assert_eq!(
            next_state(&blocked, &json!({"type": "approval_resolved"}), false),
            Some(state(SessionState::Idle))
        );
        assert_eq!(
            next_state(&working, &json!({"type": "message_update"}), true),
            None
        );
    }

    struct Fixture {
        _provider: MockProvider,
        dir: tempfile::TempDir,
        host: Arc<SessionHost>,
    }

    async fn fixture(idle_unload: Duration) -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let provider = MockProvider::start(dir.path().join("mock"), MockScript::text("Hello."))
            .await
            .expect("mock provider");
        std::fs::create_dir_all(dir.path().join("work")).unwrap();
        let host = SessionHost::new(HostConfig {
            session_dir: dir.path().join("sessions"),
            idle_unload,
            models_file: Some(provider.catalog_path()),
            no_context_files: true,
        });
        Fixture {
            _provider: provider,
            dir,
            host,
        }
    }

    fn request(fixture: &Fixture) -> CreateRequest {
        CreateRequest {
            cwd: fixture.dir.path().join("work"),
            model: Some("mock/mock-1".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_created_session_is_listed_hosted_and_stored_after_stop() {
        let fixture = fixture(Duration::ZERO).await;
        let mut events = fixture.host.subscribe();
        let hosted = fixture
            .host
            .create(request(&fixture))
            .await
            .expect("create");
        assert_eq!(hosted.state().0, SessionState::Idle);
        assert!(matches!(
            events.recv().await,
            Ok(DaemonEvent::SessionCreated { .. })
        ));

        hosted.session.prompt("say hi").await.expect("prompt");
        let listed = fixture.host.list(None).await;
        assert_eq!(listed.len(), 1);
        assert!(listed[0].hosted);
        assert_eq!(listed[0].first_message.as_deref(), Some("say hi"));
        assert_eq!(listed[0].model.as_deref(), Some("mock/mock-1"));

        let id = hosted.session_id.clone();
        drop(hosted);
        fixture
            .host
            .stop(&id, EndReason::Stopped)
            .await
            .expect("stop");
        let listed = fixture.host.list(None).await;
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].hosted);
        assert_eq!(listed[0].state, SessionState::Ended);
        assert_eq!(fixture.host.hosted_count(), 0);

        let workspaces = fixture.host.workspaces().await;
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].name, "work");
        assert_eq!(workspaces[0].session_count, 1);
    }

    #[tokio::test]
    async fn a_stored_session_loads_again_unless_another_process_holds_it() {
        let fixture = fixture(Duration::ZERO).await;
        let hosted = fixture
            .host
            .create(request(&fixture))
            .await
            .expect("create");
        hosted
            .session
            .prompt("remember this")
            .await
            .expect("prompt");
        let id = hosted.session_id.clone();
        let file = hosted.file.clone();
        drop(hosted);
        fixture
            .host
            .stop(&id, EndReason::Stopped)
            .await
            .expect("stop");

        let lock_path = file.with_extension("jsonl.lock");
        let foreign = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&foreign).unwrap();
        assert!(matches!(
            fixture.host.load(&id).await,
            Err(HostError::Locked(_))
        ));
        drop(foreign);

        let resumed = fixture.host.load(&id).await.expect("load");
        assert_eq!(resumed.session_id, id);
        assert_eq!(resumed.session.messages().len(), 2);
        assert!(matches!(
            fixture.host.load("nope").await,
            Err(HostError::Unknown(_))
        ));
    }

    #[tokio::test]
    async fn a_sessions_agents_are_not_listed_and_a_loaded_session_knows_them() {
        let fixture = fixture(Duration::ZERO).await;
        let hosted = fixture
            .host
            .create(request(&fixture))
            .await
            .expect("create");
        hosted.session.prompt("start").await.expect("prompt");
        let id = hosted.session_id.clone();
        let file = hosted.file.clone();
        drop(hosted);
        fixture
            .host
            .stop(&id, EndReason::Stopped)
            .await
            .expect("stop");
        // An agent of the session, stored as the session stores its agents.
        let parent = SessionManager::open(&file).unwrap();
        let mut agent = parent
            .create_child_named(Some("a1b2c3d4e"), &fixture.dir.path().join("work"))
            .unwrap();
        agent
            .append_message(super_sdk::AgentMessage::user("check the tests"))
            .unwrap();
        agent
            .append_custom(
                super_coding::session::manager::SUBAGENT_ENTRY,
                Some(json!({"agentId": "a1b2c3d4e", "taskName": "Check", "agentType": "Explore"})),
            )
            .unwrap();
        agent
            .append_custom(
                super_coding::agents::AGENT_RUN_ENTRY,
                Some(json!({"status": "completed", "background": true})),
            )
            .unwrap();
        drop((parent, agent));

        let listed = fixture.host.list(None).await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, id);
        assert!(matches!(
            fixture.host.load("a1b2c3d4e").await,
            Err(HostError::Unknown(_))
        ));
        let loaded = fixture.host.load(&id).await.expect("load");
        let agents = loaded
            .session
            .execute(super_sdk::protocol::Command::GetAgents { agent: None })
            .await;
        assert!(agents.success, "{:?}", agents.error);
        let agents = agents.data.unwrap()["agents"].clone();
        assert_eq!(agents[0]["id"], "a1b2c3d4e");
        assert_eq!(agents[0]["name"], "Check");
        assert_eq!(agents[0]["type"], "Explore");
        assert_eq!(agents[0]["status"], "completed");
        assert_eq!(fixture.host.list(None).await.len(), 1);
    }

    #[tokio::test]
    async fn the_sweep_unloads_idle_sessions_nobody_watches() {
        let fixture = fixture(Duration::from_millis(10)).await;
        let hosted = fixture
            .host
            .create(request(&fixture))
            .await
            .expect("create");
        hosted.session.prompt("hi").await.expect("prompt");
        fixture.host.attach(&hosted);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            fixture.host.sweep_idle().await,
            0,
            "an attached session stays"
        );
        fixture.host.detach(&hosted);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(fixture.host.sweep_idle().await, 1);
        assert_eq!(fixture.host.hosted_count(), 0);
    }

    #[tokio::test]
    async fn the_sweep_keeps_a_session_with_background_work() {
        let fixture = fixture(Duration::from_millis(10)).await;
        let hosted = fixture
            .host
            .create(request(&fixture))
            .await
            .expect("create");
        let tasks = hosted.session.agent_session().background_tasks();
        let task = tasks
            .register(super_coding::TaskSpec::new(
                super_coding::TaskKind::Shell,
                "sleep 60",
            ))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            fixture.host.sweep_idle().await,
            0,
            "a running task keeps the session"
        );
        let wake_up = tasks.hold();
        // Ends without a notice, so no turn starts.
        drop(task);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            fixture.host.sweep_idle().await,
            0,
            "a pending wake-up keeps it too"
        );
        drop(wake_up);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(fixture.host.sweep_idle().await, 1);
    }

    #[tokio::test]
    async fn the_sweep_keeps_a_session_whose_wake_up_is_pending() {
        let fixture = fixture(Duration::from_millis(10)).await;
        let hosted = fixture
            .host
            .create(request(&fixture))
            .await
            .expect("create");
        let wakeups = hosted.session.agent_session().wakeups();
        wakeups
            .schedule(super_coding::wakeup::WakeupRequest {
                delay_seconds: 600.0,
                prompt: Some("look again".into()),
                ..Default::default()
            })
            .expect("scheduled");
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            fixture.host.sweep_idle().await,
            0,
            "a pending wake-up keeps the session"
        );
        wakeups.stop();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(fixture.host.sweep_idle().await, 1);
    }

    fn git(directory: &Path, arguments: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(arguments)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .expect("git runs")
            .status;
        assert!(status.success(), "git {arguments:?}");
    }

    /// Make `root` a repository with one commit, and move `hosted` into a
    /// new worktree `name` of it, as `EnterWorktree` does.
    fn enter_worktree(hosted: &Hosted, root: &Path, name: &str) -> PathBuf {
        if !root.join(".git").exists() {
            for arguments in [
                vec!["init", "--quiet", "-b", "main"],
                vec!["config", "user.name", "Test"],
                vec!["config", "user.email", "test@example.com"],
                vec!["config", "commit.gpgsign", "false"],
            ] {
                git(root, &arguments);
            }
            std::fs::write(root.join("README"), "hello\n").unwrap();
            git(root, &["add", "README"]);
            git(root, &["commit", "--quiet", "-m", "first"]);
        }
        let worktree = super_coding::worktree::create(root, name, None).unwrap();
        let path = worktree.path.clone();
        hosted
            .session
            .agent_session()
            .enter_worktree(super_coding::EnteredWorktree {
                worktree,
                created: true,
                original: root.to_path_buf(),
                entered_at: 0,
            });
        path
    }

    #[tokio::test]
    async fn a_session_stopped_in_its_worktree_asks_its_clients_and_an_idle_one_keeps_it() {
        let fixture = fixture(Duration::from_millis(10)).await;
        let root = fixture.dir.path().join("work").canonicalize().unwrap();
        let hosted = fixture
            .host
            .create(request(&fixture))
            .await
            .expect("create");
        let path = enter_worktree(&hosted, &root, "stopped");
        let mut events = hosted.session.events();
        let id = hosted.session_id.clone();
        let host = fixture.host.clone();
        let stopping = tokio::spawn(async move { host.stop(&id, EndReason::Stopped).await });
        let question = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = events.recv().await.expect("open");
                if event.0["type"] == "approval_request" {
                    return event.0;
                }
            }
        })
        .await
        .expect("the question is asked");
        assert_eq!(question["kind"], "worktree");
        assert_eq!(fixture.host.hosted_count(), 1, "still loaded while it asks");
        hosted
            .session
            .answer_approval(question["approvalId"].as_str().unwrap(), "remove", "Mac")
            .unwrap();
        stopping.await.unwrap().expect("stopped");
        assert!(!path.exists());
        assert_eq!(fixture.host.hosted_count(), 0);
        drop(hosted);

        // Unloaded while idle, nobody is asked and the worktree stays.
        let idle = fixture
            .host
            .create(request(&fixture))
            .await
            .expect("create");
        let path = enter_worktree(&idle, &root, "idle");
        let mut events = idle.session.events();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(fixture.host.sweep_idle().await, 1);
        assert!(path.exists());
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_millis(50), events.recv()).await
        {
            assert_ne!(event.0["type"], "approval_request");
        }
    }

    #[tokio::test]
    async fn create_rejects_missing_or_relative_directories() {
        let fixture = fixture(Duration::ZERO).await;
        let mut missing = request(&fixture);
        missing.cwd = fixture.dir.path().join("absent");
        assert!(matches!(
            fixture.host.create(missing).await,
            Err(HostError::Invalid(_))
        ));
        let mut relative = request(&fixture);
        relative.cwd = PathBuf::from("work");
        assert!(matches!(
            fixture.host.create(relative).await,
            Err(HostError::Invalid(_))
        ));
    }
}
