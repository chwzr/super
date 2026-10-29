//! The SUPER daemon: one long-lived process per user that hosts coding-agent
//! sessions and serves them to local clients over a Unix socket.
//!
//! `super daemon run` calls [`run`]. The protocol a client speaks is defined in
//! `super_sdk::daemon` and described in `docs/daemon.md`; the design is in
//! `.super/specs/2026-10-29-daemon-design.md`.

pub mod env;
pub mod host;
pub mod lock;
pub mod log;
pub mod paths;

#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod server;

use crate::host::HostConfig;
use crate::paths::DaemonPaths;
use std::time::Duration;

/// Everything `run` needs. The binary fills it from settings and flags; tests
/// point every path at a temporary directory.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub paths: DaemonPaths,
    pub host: HostConfig,
    pub host_id: String,
    pub host_name: String,
    pub log_level: log::Level,
    pub log_to_stderr: bool,
    /// How often the idle sweep runs.
    pub sweep_interval: Duration,
}

/// Run the daemon until `shutdown` fires, SIGTERM or SIGINT arrives, or a
/// client sends `shutdown`. Returns the process exit code: 0 for a deliberate
/// stop, 1 when another daemon holds the lock.
#[cfg(unix)]
pub async fn run(
    config: DaemonConfig,
    shutdown: tokio_util::sync::CancellationToken,
) -> anyhow::Result<i32> {
    use crate::lock::{DaemonLock, LockError};
    use crate::server::{DaemonInfo, ServerContext};
    use anyhow::Context as _;
    use std::os::unix::fs::PermissionsExt as _;
    use super_sdk::daemon::DaemonEvent;

    config
        .paths
        .ensure_run_dir()
        .with_context(|| format!("create {}", config.paths.run_dir.display()))?;
    let _lock = match DaemonLock::acquire(&config.paths.lock) {
        Ok(lock) => lock,
        Err(error @ LockError::Held { .. }) => {
            eprintln!("{error}");
            return Ok(1);
        }
        Err(LockError::Io(error)) => {
            return Err(
                anyhow::Error::new(error).context(format!("lock {}", config.paths.lock.display()))
            );
        }
    };
    log::init(log::Logger::new(
        Some(config.paths.log.clone()),
        config.log_level,
        config.log_to_stderr,
        log::DEFAULT_MAX_BYTES,
    ));

    if config.paths.socket.exists() {
        std::fs::remove_file(&config.paths.socket)
            .with_context(|| format!("remove stale {}", config.paths.socket.display()))?;
    }
    let listener = tokio::net::UnixListener::bind(&config.paths.socket)
        .with_context(|| format!("listen on {}", config.paths.socket.display()))?;
    std::fs::set_permissions(&config.paths.socket, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod {}", config.paths.socket.display()))?;

    let host = host::SessionHost::new(config.host.clone());
    let info = DaemonInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        host_id: config.host_id.clone(),
        host_name: config.host_name.clone(),
        platform: std::env::consts::OS.to_string(),
        socket_path: config.paths.socket.clone(),
    };
    let ctx = ServerContext::new(host.clone(), info, shutdown.clone());
    log::info(
        "daemon",
        format!(
            "SUPER daemon {} listening on {} (pid {})",
            env!("CARGO_PKG_VERSION"),
            config.paths.socket.display(),
            std::process::id()
        ),
    );
    install_signal_handlers(shutdown.clone());

    let sweeper = {
        let host = host.clone();
        let cancel = shutdown.clone();
        let every = config.sweep_interval.max(Duration::from_millis(10));
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tick.tick() => {
                        let unloaded = host.sweep_idle().await;
                        if unloaded > 0 {
                            log::info("daemon", format!("idle sweep unloaded {unloaded} session(s)"));
                        }
                    }
                }
            }
        })
    };

    server::serve(listener, ctx.clone()).await;

    log::info("daemon", "shutting down");
    sweeper.abort();
    host.announce(DaemonEvent::DaemonShutdown {
        reason: "stopped".into(),
    });
    host.shutdown().await;
    // Let the connection writers flush the last lines before the sockets go.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = std::fs::remove_file(&config.paths.socket);
    log::info("daemon", "stopped");
    Ok(0)
}

#[cfg(unix)]
fn install_signal_handlers(shutdown: tokio_util::sync::CancellationToken) {
    use tokio::signal::unix::{SignalKind, signal};
    for (kind, name) in [
        (SignalKind::terminate(), "SIGTERM"),
        (SignalKind::interrupt(), "SIGINT"),
    ] {
        match signal(kind) {
            Ok(mut stream) => {
                let shutdown = shutdown.clone();
                tokio::spawn(async move {
                    if stream.recv().await.is_some() {
                        log::info("daemon", format!("{name} received"));
                        shutdown.cancel();
                    }
                });
            }
            Err(error) => log::warn("daemon", format!("cannot listen for {name}: {error}")),
        }
    }
}
