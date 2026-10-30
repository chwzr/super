//! Starting the daemon on demand through the service manager that owns it.
//!
//! On macOS the SUPER app registers the daemon as the launchd agent
//! [`LAUNCHD_LABEL`]; on Linux `super install` writes the systemd user unit
//! [`SYSTEMD_UNIT`]. A client whose socket is dead asks that manager to start
//! the daemon, so the daemon keeps one supervisor (and one pid) however it was
//! started. Nothing is registered, nothing is started: the caller runs
//! without a daemon.

use std::process::{Command, Stdio};

/// The launchd label of the agent the SUPER app registers.
pub const LAUNCHD_LABEL: &str = "io.flxkpe.super.daemon";
/// The systemd user unit `super install` writes.
pub const SYSTEMD_UNIT: &str = "super-daemon.service";

/// A service manager that may know how to start the daemon.
pub trait DaemonService: Send + Sync {
    /// For messages: "launchd agent io.flxkpe.super.daemon".
    fn name(&self) -> String;
    /// Whether the daemon is registered with this manager.
    fn is_registered(&self) -> bool;
    /// Ask the manager to start the daemon. Returns once the manager took
    /// the request; the socket appears a moment later.
    fn start(&self) -> std::io::Result<()>;
}

/// What to do about a dead socket.
pub enum StartDecision<'a> {
    Start(&'a dyn DaemonService),
    Unavailable(String),
}

/// Start the daemon only through a manager it is registered with.
pub fn start_decision(service: Option<&dyn DaemonService>) -> StartDecision<'_> {
    match service {
        None => StartDecision::Unavailable("no SUPER daemon is running".into()),
        Some(service) if !service.is_registered() => StartDecision::Unavailable(format!(
            "no SUPER daemon is running and the {} is not registered",
            service.name()
        )),
        Some(service) => StartDecision::Start(service),
    }
}

/// The manager of this platform: launchd on macOS, systemd on Linux.
pub fn platform_service() -> Option<Box<dyn DaemonService>> {
    #[cfg(target_os = "macos")]
    {
        Some(Box::new(Launchd::for_this_user()))
    }
    #[cfg(target_os = "linux")]
    {
        Some(Box::new(Systemd))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// The per-user launchd domain `gui/<uid>`.
pub struct Launchd {
    pub uid: u32,
}

impl Launchd {
    pub fn for_this_user() -> Self {
        // SAFETY: getuid has no preconditions and cannot fail.
        Self {
            uid: unsafe { libc::getuid() },
        }
    }

    /// `gui/<uid>/io.flxkpe.super.daemon`.
    pub fn target(&self) -> String {
        format!("gui/{}/{LAUNCHD_LABEL}", self.uid)
    }
}

impl DaemonService for Launchd {
    fn name(&self) -> String {
        format!("launchd agent {LAUNCHD_LABEL}")
    }

    fn is_registered(&self) -> bool {
        succeeds("launchctl", &["print", &self.target()])
    }

    fn start(&self) -> std::io::Result<()> {
        run("launchctl", &["kickstart", &self.target()])
    }
}

/// The systemd user manager.
pub struct Systemd;

impl DaemonService for Systemd {
    fn name(&self) -> String {
        format!("systemd user unit {SYSTEMD_UNIT}")
    }

    fn is_registered(&self) -> bool {
        succeeds("systemctl", &["--user", "cat", SYSTEMD_UNIT])
    }

    fn start(&self) -> std::io::Result<()> {
        run("systemctl", &["--user", "start", SYSTEMD_UNIT])
    }
}

fn succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn run(program: &str, args: &[&str]) -> std::io::Result<()> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(std::io::Error::other(if stderr.is_empty() {
        format!(
            "`{program} {}` failed with {}",
            args.join(" "),
            output.status
        )
    } else {
        stderr
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Probe(bool);

    impl DaemonService for Probe {
        fn name(&self) -> String {
            "probe".into()
        }
        fn is_registered(&self) -> bool {
            self.0
        }
        fn start(&self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn only_a_registered_daemon_is_started() {
        assert!(matches!(
            start_decision(Some(&Probe(true))),
            StartDecision::Start(_)
        ));
        let StartDecision::Unavailable(reason) = start_decision(Some(&Probe(false))) else {
            panic!("an unregistered daemon is not started");
        };
        assert_eq!(
            reason,
            "no SUPER daemon is running and the probe is not registered"
        );
        assert!(matches!(
            start_decision(None),
            StartDecision::Unavailable(_)
        ));
    }

    #[test]
    fn the_launchd_target_is_the_users_gui_domain() {
        let launchd = Launchd { uid: 501 };
        assert_eq!(launchd.target(), "gui/501/io.flxkpe.super.daemon");
        assert_eq!(launchd.name(), "launchd agent io.flxkpe.super.daemon");
    }
}
