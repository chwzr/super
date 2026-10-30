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
        None => DaemonPaths::discover().ok_or_else(|| anyhow::anyhow!("no home directory")),
    }
}

/// How `super daemon status|stop` introduce themselves.
#[cfg(unix)]
fn cli_client() -> super_daemon::client::ClientInfo {
    super_daemon::client::ClientInfo::new(super_sdk::daemon::ClientKind::Cli, "super")
}

#[cfg(not(unix))]
async fn run_daemon(_args: &Args, socket: Option<PathBuf>) -> Result<i32> {
    let _paths = paths(socket)?;
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
    let presence = settings
        .remote
        .is_enabled()
        .then(|| super_daemon::presence::PresenceConfig {
            platform_url: super_remote::credentials::resolve_platform_url(
                settings.platform.url.as_deref(),
            ),
            device_name: host.name.clone(),
            check_interval: Duration::from_secs(30),
            link: super_daemon::presence::LinkConfig {
                allow_shell: settings.remote.shell_allowed(),
                ..Default::default()
            },
        });
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
        presence,
        protocol_versions: advertised_protocol(std::env::var(PROTOCOL_ENV).ok().as_deref())?,
    };
    super_daemon::run(config, tokio_util::sync::CancellationToken::new()).await
}

/// Lets a test daemon advertise another protocol range (`2-3`), to see what
/// a client says to a newer or older daemon.
#[cfg(unix)]
const PROTOCOL_ENV: &str = "SUPER_DAEMON_PROTOCOL_VERSIONS";

#[cfg(unix)]
fn advertised_protocol(value: Option<&str>) -> Result<[u32; 2]> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(super_sdk::daemon::PROTOCOL_VERSIONS);
    };
    let parse = |part: &str| part.trim().parse::<u32>().ok();
    let range = match value.split_once(['-', ',']) {
        Some((low, high)) => parse(low).zip(parse(high)).map(|(low, high)| [low, high]),
        None => parse(value).map(|version| [version, version]),
    };
    match range {
        Some([low, high]) if low <= high => Ok([low, high]),
        _ => anyhow::bail!("{PROTOCOL_ENV} must be a version or a range like 2-3, not '{value}'"),
    }
}

#[cfg(not(unix))]
async fn status(socket: Option<PathBuf>) -> Result<i32> {
    let _paths = paths(socket)?;
    println!("no SUPER daemon is running");
    Ok(1)
}

#[cfg(unix)]
async fn status(socket: Option<PathBuf>) -> Result<i32> {
    use super_daemon::client::{ClientError, DaemonClient};

    let paths = paths(socket)?;
    let client = match DaemonClient::connect_to(&paths.socket, &cli_client()).await {
        Ok(client) => client,
        Err(ClientError::NotRunning { .. }) => {
            println!("no SUPER daemon is running");
            return Ok(1);
        }
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(2);
        }
    };
    let status = client.status().await?;
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
async fn stop(socket: Option<PathBuf>) -> Result<i32> {
    let _paths = paths(socket)?;
    println!("no SUPER daemon is running");
    Ok(0)
}

#[cfg(unix)]
async fn stop(socket: Option<PathBuf>) -> Result<i32> {
    use std::time::Duration;
    use super_daemon::client::{ClientError, DaemonClient};
    use super_daemon::lock::DaemonLock;

    let paths = paths(socket)?;
    match DaemonClient::connect_to(&paths.socket, &cli_client()).await {
        Ok(client) => {
            client.shutdown().await?;
            // The daemon closes every connection on its way out.
            let closed = tokio::time::timeout(Duration::from_secs(10), client.closed())
                .await
                .is_ok();
            if closed {
                println!("stopped");
            } else {
                println!("stop requested; the daemon is still shutting down");
            }
            Ok(0)
        }
        Err(ClientError::NotRunning { .. }) => match DaemonLock::holder_pid(&paths.lock) {
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
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(unix)]
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_test_daemon_can_advertise_another_protocol_range() {
        assert_eq!(
            advertised_protocol(None).unwrap(),
            super_sdk::daemon::PROTOCOL_VERSIONS
        );
        assert_eq!(advertised_protocol(Some("2-3")).unwrap(), [2, 3]);
        assert_eq!(advertised_protocol(Some("2,3")).unwrap(), [2, 3]);
        assert_eq!(advertised_protocol(Some(" 4 ")).unwrap(), [4, 4]);
        assert!(advertised_protocol(Some("3-2")).is_err());
        assert!(advertised_protocol(Some("two")).is_err());
    }

    #[test]
    fn uptime_reads_like_a_person_wrote_it() {
        assert_eq!(uptime(5), "5s");
        assert_eq!(uptime(125), "2m");
        assert_eq!(uptime(3 * 3600 + 12 * 60), "3h 12m");
    }
}
