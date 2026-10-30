//! `super install` and `super uninstall`: the daemon as a systemd user
//! service on Linux, and the unit's line under `super daemon status`. The
//! decisions live in `super_daemon::service`; this module prints them.

use anyhow::{Context as _, Result};
use std::time::Duration;
use super_daemon::paths::DaemonPaths;
use super_daemon::service::{self, Platform, ServiceEnv, Uninstalled, UserSystemctl};

/// How long `super install` waits for the daemon it started to answer. The
/// daemon reads the login shell's PATH before it binds, which can take a few
/// seconds.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

pub fn install() -> Result<i32> {
    let env = ServiceEnv::current().context("no home directory")?;
    let paths = DaemonPaths::default_paths().context("no home directory")?;
    let exe = service::service_exe(
        &std::env::current_exe().context("locate the running super binary")?,
        std::env::var_os("PATH").as_deref(),
    );
    // A daemon that answers before the unit exists was started by hand.
    let by_hand = env.platform == Platform::Linux
        && !env.unit_path().exists()
        && service::daemon_answers(&paths.socket, Duration::ZERO);

    let installed = match service::install(&env, &exe, &mut UserSystemctl) {
        Ok(installed) => installed,
        Err(error) => {
            eprintln!("{error}");
            return Ok(1);
        }
    };
    for line in installed.summary() {
        println!("{line}");
    }
    let mut code = 0;
    if by_hand {
        println!(
            "daemon    a daemon you started yourself still holds {}; `super daemon stop` \
             stops it and systemd starts its own within five seconds",
            paths.socket.display()
        );
    } else if service::daemon_answers(&paths.socket, ANSWER_TIMEOUT) {
        println!(
            "daemon    answering on {}; `super daemon status` shows it",
            paths.socket.display()
        );
    } else {
        println!(
            "daemon    not answering after {} seconds; `journalctl --user -u super-daemon` \
             and {} show why",
            ANSWER_TIMEOUT.as_secs(),
            paths.log.display()
        );
        code = 1;
    }
    if !env.lingering {
        println!();
        println!(
            "The daemon runs while you are logged in. To keep it running after you log out \
             (an always-on machine), run: loginctl enable-linger {}",
            env.user_name()
        );
    }
    Ok(code)
}

pub fn uninstall() -> Result<i32> {
    let env = ServiceEnv::current().context("no home directory")?;
    match service::uninstall(&env, &mut UserSystemctl) {
        Ok(Uninstalled::NothingInstalled { unit_path }) => {
            println!(
                "Nothing to uninstall: {} does not exist.",
                unit_path.display()
            );
            Ok(0)
        }
        Ok(Uninstalled::Removed {
            unit_path,
            stopped,
            problems,
        }) => {
            if stopped {
                println!("Stopped and disabled {}.", service::UNIT_NAME);
            }
            println!("Removed {}.", unit_path.display());
            for problem in &problems {
                eprintln!("warning: {problem}");
            }
            Ok(if problems.is_empty() { 0 } else { 1 })
        }
        Err(error) => {
            eprintln!("{error}");
            Ok(1)
        }
    }
}

/// Print the systemd unit's line after `super daemon status` on Linux, and
/// pass the status command's result through.
pub fn with_unit_status(result: Result<i32>) -> Result<i32> {
    if let Some(status) =
        ServiceEnv::current().and_then(|env| service::unit_status(&env, &mut UserSystemctl))
    {
        println!("{}", status.line());
    }
    result
}
