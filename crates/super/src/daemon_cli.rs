//! `super daemon run|status|stop`.

use crate::args::{Args, DaemonSubcommand};
use anyhow::Result;
use std::path::PathBuf;
use super_daemon::paths::DaemonPaths;

pub async fn run(args: &Args, command: &DaemonSubcommand) -> Result<i32> {
    match command {
        DaemonSubcommand::Run { socket } => run_daemon(args, socket.clone()).await,
        DaemonSubcommand::Status { socket } => status(socket.clone()).await,
        DaemonSubcommand::Stop { socket } => stop(socket.clone()).await,
    }
}

fn paths(socket: Option<PathBuf>) -> Result<DaemonPaths> {
    match socket {
        Some(socket) => Ok(DaemonPaths::beside_socket(socket)),
        None => DaemonPaths::default_paths().ok_or_else(|| anyhow::anyhow!("no home directory")),
    }
}

#[cfg(not(unix))]
async fn run_daemon(_args: &Args, _socket: Option<PathBuf>) -> Result<i32> {
    eprintln!("the SUPER daemon needs a Unix socket and is not available on Windows");
    Ok(1)
}

#[cfg(unix)]
async fn run_daemon(args: &Args, socket: Option<PathBuf>) -> Result<i32> {
    use anyhow::Context as _;
    use std::io::IsTerminal as _;
    use std::time::Duration;
    use super_daemon::host::HostConfig;
    use super_remote::HostIdentity;

    let paths = paths(socket)?;
    let home = dirs::home_dir().context("no home directory")?;
    let settings = super_coding::Settings::load(&home, false);
    let session_dir = crate::setup::session_dir(args, &settings);
    let host = match HostIdentity::default_path() {
        Some(path) => HostIdentity::load_or_create(&path)?,
        None => HostIdentity {
            host_id: "local".into(),
            name: "local".into(),
            platform: super_remote::host::platform_name().into(),
        },
    };
    let config = super_daemon::DaemonConfig {
        paths,
        host: HostConfig {
            session_dir,
            idle_unload: Duration::from_secs(60 * settings.daemon.idle_unload_minutes()),
            models_file: std::env::var_os("SUPER_MODELS_FILE").map(PathBuf::from),
            no_context_files: args.no_context_files,
        },
        host_id: host.host_id,
        host_name: host.name,
        log_level: super_daemon::log::Level::from_env(),
        log_to_stderr: std::io::stderr().is_terminal(),
        sweep_interval: Duration::from_secs(60),
    };
    super_daemon::run(config, tokio_util::sync::CancellationToken::new()).await
}

#[cfg(not(unix))]
async fn status(_socket: Option<PathBuf>) -> Result<i32> {
    println!("no SUPER daemon is running");
    Ok(1)
}

#[cfg(unix)]
async fn status(socket: Option<PathBuf>) -> Result<i32> {
    use anyhow::Context as _;
    use super_daemon::client::DaemonClient;
    use super_sdk::daemon::{ClientKind, DaemonCommand, DaemonStatus};

    let paths = paths(socket)?;
    let Ok(mut client) = DaemonClient::connect(&paths.socket).await else {
        println!("no SUPER daemon is running");
        return Ok(1);
    };
    if let Err(error) = client
        .hello(ClientKind::Cli, "super", env!("CARGO_PKG_VERSION"))
        .await
    {
        eprintln!("error: {error}");
        return Ok(2);
    }
    let response = client.call(DaemonCommand::Status {}).await?;
    if !response.success {
        anyhow::bail!(
            "{}",
            response.error.unwrap_or_else(|| "status failed".into())
        );
    }
    let status: DaemonStatus =
        serde_json::from_value(response.data.context("status without data")?)?;
    println!(
        "SUPER daemon {} running (pid {}, up {})",
        status.daemon_version,
        status.pid,
        uptime(status.uptime_seconds)
    );
    println!("socket   {}", status.socket_path);
    println!(
        "sessions {} hosted, {} on disk",
        status.hosted_sessions, status.stored_sessions
    );
    println!("clients  {}", status.clients);
    Ok(0)
}

#[cfg(not(unix))]
async fn stop(_socket: Option<PathBuf>) -> Result<i32> {
    println!("no SUPER daemon is running");
    Ok(0)
}

#[cfg(unix)]
async fn stop(socket: Option<PathBuf>) -> Result<i32> {
    use std::time::Duration;
    use super_daemon::client::DaemonClient;
    use super_daemon::lock::DaemonLock;
    use super_sdk::daemon::{ClientKind, DaemonCommand};

    let paths = paths(socket)?;
    match DaemonClient::connect(&paths.socket).await {
        Ok(mut client) => {
            client
                .hello(ClientKind::Cli, "super", env!("CARGO_PKG_VERSION"))
                .await?;
            let response = client.call(DaemonCommand::Shutdown {}).await?;
            if !response.success {
                anyhow::bail!(
                    "{}",
                    response.error.unwrap_or_else(|| "shutdown refused".into())
                );
            }
            // The daemon closes every connection on its way out.
            let closed = tokio::time::timeout(Duration::from_secs(10), async {
                while let Ok(Some(_)) = client.next_line().await {}
            })
            .await
            .is_ok();
            if closed {
                println!("stopped");
            } else {
                println!("stop requested; the daemon is still shutting down");
            }
            Ok(0)
        }
        Err(_) => match DaemonLock::holder_pid(&paths.lock) {
            Some(pid) if process_alive(pid) => {
                // SAFETY: kill with SIGTERM has no memory-safety preconditions.
                let sent = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } == 0;
                if sent {
                    println!("stopped (pid {pid})");
                } else {
                    println!("no SUPER daemon is running");
                }
                Ok(0)
            }
            _ => {
                println!("no SUPER daemon is running");
                Ok(0)
            }
        },
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn uptime(seconds: u64) -> String {
    let (hours, minutes) = (seconds / 3600, (seconds % 3600) / 60);
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_reads_like_a_person_wrote_it() {
        assert_eq!(uptime(5), "5s");
        assert_eq!(uptime(125), "2m");
        assert_eq!(uptime(3 * 3600 + 12 * 60), "3h 12m");
    }
}
