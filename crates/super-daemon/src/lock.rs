//! One daemon per user: an exclusive lock file with the holder's pid inside.

use fs2::FileExt as _;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

/// Held for the daemon's lifetime; dropping it releases the lock.
#[derive(Debug)]
pub struct DaemonLock {
    _file: File,
    path: PathBuf,
}

#[derive(Debug)]
pub enum LockError {
    /// Another process holds the lock. `pid` is what it wrote, when readable.
    Held {
        pid: Option<u32>,
    },
    Io(std::io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held { pid: Some(pid) } => {
                write!(f, "another SUPER daemon is running (pid {pid})")
            }
            Self::Held { pid: None } => write!(f, "another SUPER daemon is running"),
            Self::Io(error) => write!(f, "daemon lock: {error}"),
        }
    }
}

impl std::error::Error for LockError {}

impl DaemonLock {
    /// Take the lock, writing this process's pid into the file.
    pub fn acquire(path: &Path) -> Result<Self, LockError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(LockError::Io)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(LockError::Io)?;
        if let Err(error) = file.try_lock_exclusive() {
            if is_contention(&error) {
                return Err(LockError::Held {
                    pid: Self::holder_pid(path),
                });
            }
            return Err(LockError::Io(error));
        }
        file.set_len(0).map_err(LockError::Io)?;
        file.seek(SeekFrom::Start(0)).map_err(LockError::Io)?;
        writeln!(file, "{}", std::process::id()).map_err(LockError::Io)?;
        file.flush().map_err(LockError::Io)?;
        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The pid written by whoever holds, or last held, the lock.
    pub fn holder_pid(path: &Path) -> Option<u32> {
        let mut text = String::new();
        File::open(path).ok()?.read_to_string(&mut text).ok()?;
        text.trim().parse().ok()
    }
}

/// fs2 reports contention through its own error, not always `WouldBlock`.
fn is_contention(error: &std::io::Error) -> bool {
    let contended = fs2::lock_contended_error();
    error.kind() == std::io::ErrorKind::WouldBlock
        || (contended.kind() == error.kind() && contended.raw_os_error() == error.raw_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_acquire_sees_the_holder_pid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("run/daemon.lock");
        let first = DaemonLock::acquire(&path).expect("first lock");
        assert_eq!(DaemonLock::holder_pid(&path), Some(std::process::id()));
        match DaemonLock::acquire(&path) {
            Err(LockError::Held { pid }) => assert_eq!(pid, Some(std::process::id())),
            other => panic!("expected Held, got {other:?}"),
        }
        drop(first);
        assert!(DaemonLock::acquire(&path).is_ok());
    }

    #[test]
    fn the_error_text_names_the_pid() {
        let held = LockError::Held { pid: Some(4242) };
        assert_eq!(
            held.to_string(),
            "another SUPER daemon is running (pid 4242)"
        );
        assert_eq!(
            LockError::Held { pid: None }.to_string(),
            "another SUPER daemon is running"
        );
    }
}
