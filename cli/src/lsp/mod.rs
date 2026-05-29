pub mod client;
pub mod config;
pub mod instance;
pub mod manager;
pub mod types;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use tokio::sync::RwLock;
use tracing::{debug, error};

use manager::LspServerManager;
use types::InitState;

static MANAGER: Mutex<Option<LspServerManager>> = Mutex::new(None);

static INIT_STATE: RwLock<InitState> = RwLock::const_new(InitState::NotStarted);

static INIT_GENERATION: AtomicU64 = AtomicU64::new(0);

static INIT_TASK: Mutex<Option<tokio::task::JoinHandle<()>>> = Mutex::new(None);

static IS_BARE_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_bare_mode() {
    IS_BARE_MODE.store(true, Ordering::Release);
}

fn is_bare_mode() -> bool {
    IS_BARE_MODE.load(Ordering::Acquire) || std::env::var("CLAUDE_CODE_SIMPLE").is_ok()
}

pub fn initialize_lsp_manager() {
    if is_bare_mode() {
        debug!("LSP: skipping init in bare mode");
        return;
    }

    // Use try_read/try_write so this is safe to call from within a tokio
    // runtime (e.g., bootstrap). If the lock is held for writing we
    // conservatively skip — a later call will handle it.
    let state = INIT_STATE
        .try_read()
        .map(|s| *s)
        .unwrap_or(InitState::NotStarted);
    match state {
        InitState::Pending | InitState::Success => {
            debug!("LSP: already initializing or initialized, skipping");
            return;
        }
        InitState::Failed => {
            debug!("LSP: retrying after previous failure");
        }
        InitState::NotStarted => {}
    }

    {
        let mut w = match INIT_STATE.try_write() {
            Ok(w) => w,
            Err(_) => {
                debug!("LSP: could not acquire write lock, skipping init for now");
                return;
            }
        };
        *w = InitState::Pending;
    }

    let gen = INIT_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    debug!("LSP: starting async initialization (generation {})", gen);

    let handle = tokio::spawn(async move {
        match LspServerManager::initialize().await {
            Ok(manager) => {
                if INIT_GENERATION.load(Ordering::SeqCst) == gen {
                    {
                        #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
                        let mut mg = MANAGER.lock().unwrap();
                        *mg = Some(manager);
                    }
                    *INIT_STATE.write().await = InitState::Success;
                    debug!("LSP manager initialized successfully");
                }
            }
            Err(e) => {
                if INIT_GENERATION.load(Ordering::SeqCst) == gen {
                    *INIT_STATE.write().await = InitState::Failed;
                    error!("LSP manager initialization failed: {}", e);
                }
            }
        }
    });

    #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
    let mut tg = INIT_TASK.lock().unwrap();
    *tg = Some(handle);
}

pub fn get_lsp_manager() -> Option<std::sync::MutexGuard<'static, Option<LspServerManager>>> {
    // Use try_read() so this is safe to call from within a tokio runtime
    // (e.g., tool call hooks). If the RwLock is held for writing we
    // conservatively return None rather than blocking.
    let state = INIT_STATE
        .try_read()
        .map(|s| *s)
        .unwrap_or(InitState::NotStarted);
    match state {
        InitState::Failed | InitState::NotStarted => None,
        #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
        _ => Some(MANAGER.lock().unwrap()),
    }
}

pub async fn get_initialization_status() -> InitState {
    *INIT_STATE.read().await
}

pub fn is_lsp_connected() -> bool {
    let state = INIT_STATE
        .try_read()
        .map(|s| *s)
        .unwrap_or(InitState::NotStarted);
    if matches!(state, InitState::Failed | InitState::NotStarted) {
        return false;
    }

    #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
    let guard = MANAGER.lock().unwrap();
    match guard.as_ref() {
        Some(manager) => {
            let servers = manager.get_all_servers();
            if servers.is_empty() {
                return false;
            }
            servers
                .values()
                .any(|s| !matches!(s.state, types::LspServerState::Error))
        }
        None => false,
    }
}

pub async fn wait_for_initialization() {
    let state = *INIT_STATE.read().await;
    match state {
        InitState::Success | InitState::Failed => (),
        InitState::Pending => {
            #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
            let handle = INIT_TASK.lock().unwrap().take();
            if let Some(h) = handle {
                let _ = h.await;
            }
        }
        InitState::NotStarted => {}
    }
}

pub async fn shutdown_lsp_manager() {
    #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
    let manager = MANAGER.lock().unwrap().take();
    if let Some(mut m) = manager {
        if let Err(e) = m.shutdown().await {
            error!("LSP shutdown error: {}", e);
        }
    }

    *INIT_STATE.write().await = InitState::NotStarted;
    INIT_GENERATION.fetch_add(1, Ordering::SeqCst);
    #[allow(clippy::unwrap_used)] // Mutex poisoning is irrecoverable
    let mut tg2 = INIT_TASK.lock().unwrap();
    *tg2 = None;

    debug!("LSP manager shutdown complete");
}
