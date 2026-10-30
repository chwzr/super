//! Where the daemon keeps its socket, lock and log.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Points clients, and `super daemon run`, at another socket.
pub const SOCKET_ENV: &str = "SUPER_DAEMON_SOCKET";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonPaths {
    /// Holds the socket and the lock; created with mode 0700.
    pub run_dir: PathBuf,
    pub socket: PathBuf,
    pub lock: PathBuf,
    pub log: PathBuf,
}

impl DaemonPaths {
    /// `~/.super/run/daemon.sock`, `~/.super/run/daemon.lock` and
    /// `~/.super/agent/logs/daemon.log`.
    pub fn default_paths() -> Option<Self> {
        let home = dirs::home_dir()?;
        let run_dir = home.join(".super/run");
        Some(Self {
            socket: run_dir.join("daemon.sock"),
            lock: run_dir.join("daemon.lock"),
            log: home.join(".super/agent/logs/daemon.log"),
            run_dir,
        })
    }

    /// The socket clients look for: `SUPER_DAEMON_SOCKET` when set, else
    /// `~/.super/run/daemon.sock`.
    pub fn discover() -> Option<Self> {
        Self::discover_from(std::env::var_os(SOCKET_ENV), Self::default_paths())
    }

    fn discover_from(env: Option<OsString>, defaults: Option<Self>) -> Option<Self> {
        match env.filter(|value| !value.is_empty()) {
            Some(socket) => Some(Self::beside_socket(PathBuf::from(socket))),
            None => defaults,
        }
    }

    /// Whether this is the socket launchd and systemd start the daemon on.
    /// Only then does starting the registered service bring it up.
    pub fn is_default(&self) -> bool {
        Self::default_paths().is_some_and(|defaults| defaults.socket == self.socket)
    }

    /// A socket somewhere else (`--socket`, tests). The lock sits beside the
    /// socket so two daemons on different sockets do not fight; the log stays
    /// at its default place when there is a home directory.
    pub fn beside_socket(socket: PathBuf) -> Self {
        let run_dir = socket
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let lock = socket.with_extension("lock");
        let log = Self::default_paths()
            .map(|paths| paths.log)
            .unwrap_or_else(|| run_dir.join("daemon.log"));
        Self {
            run_dir,
            socket,
            lock,
            log,
        }
    }

    /// Create the run directory. Under the user's home it is made readable
    /// by this user only; a socket placed elsewhere (`--socket /tmp/x.sock`)
    /// leaves that shared directory's permissions alone.
    pub fn ensure_run_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.run_dir)?;
        if restrict_permissions(&self.run_dir, dirs::home_dir().as_deref()) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&self.run_dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(())
    }
}

/// Only a run directory inside the home directory is ours to lock down.
fn restrict_permissions(run_dir: &Path, home: Option<&Path>) -> bool {
    home.is_some_and(|home| run_dir.starts_with(home))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_live_under_the_super_directory() {
        let paths = DaemonPaths::default_paths().expect("a home directory");
        assert!(paths.socket.ends_with(".super/run/daemon.sock"));
        assert!(paths.lock.ends_with(".super/run/daemon.lock"));
        assert!(paths.log.ends_with(".super/agent/logs/daemon.log"));
        assert_eq!(paths.run_dir, paths.socket.parent().unwrap());
    }

    #[test]
    fn only_a_run_dir_under_home_is_made_private() {
        let home = Path::new("/Users/felix");
        assert!(restrict_permissions(
            Path::new("/Users/felix/.super/run"),
            Some(home)
        ));
        assert!(!restrict_permissions(Path::new("/tmp"), Some(home)));
        assert!(!restrict_permissions(
            Path::new("/Users/felix/.super/run"),
            None
        ));
    }

    #[cfg(unix)]
    #[test]
    fn ensure_run_dir_leaves_a_shared_directory_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let paths = DaemonPaths::beside_socket(dir.path().join("d.sock"));
        paths
            .ensure_run_dir()
            .expect("no chmod on a shared directory");
        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn discovery_prefers_the_environment() {
        let defaults =
            DaemonPaths::beside_socket(PathBuf::from("/home/felix/.super/run/daemon.sock"));
        let found = DaemonPaths::discover_from(
            Some(OsString::from("/tmp/eng38.sock")),
            Some(defaults.clone()),
        )
        .unwrap();
        assert_eq!(found.socket, PathBuf::from("/tmp/eng38.sock"));
        assert_eq!(found.lock, PathBuf::from("/tmp/eng38.lock"));
        assert_eq!(
            DaemonPaths::discover_from(Some(OsString::new()), Some(defaults.clone())),
            Some(defaults.clone()),
            "an empty variable counts as unset"
        );
        assert_eq!(
            DaemonPaths::discover_from(None, Some(defaults.clone())),
            Some(defaults)
        );
        assert_eq!(DaemonPaths::discover_from(None, None), None);
    }

    #[test]
    fn only_the_default_socket_is_the_default() {
        let defaults = DaemonPaths::default_paths().expect("a home directory");
        assert!(defaults.is_default());
        assert!(!DaemonPaths::beside_socket(PathBuf::from("/tmp/other.sock")).is_default());
    }

    #[test]
    fn a_custom_socket_keeps_its_lock_beside_it() {
        let paths = DaemonPaths::beside_socket(PathBuf::from("/tmp/x/d.sock"));
        assert_eq!(paths.lock, PathBuf::from("/tmp/x/d.lock"));
        assert_eq!(paths.run_dir, PathBuf::from("/tmp/x"));
    }
}
