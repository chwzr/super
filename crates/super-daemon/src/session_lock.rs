//! One writer per session file: `<file>.jsonl.lock` beside it.
//!
//! The daemon holds the lock while it hosts a session; a `super` running the
//! engine in-process holds it for the session it has open. Whoever comes
//! second is refused, so the two never append to one file.

use fs2::FileExt as _;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `<file>.jsonl.lock` for `<file>.jsonl`.
pub fn lock_path(session_file: &Path) -> PathBuf {
    session_file.with_extension("jsonl.lock")
}

/// Lock `<file>.jsonl.lock` beside a session file. `Ok(None)` means another
/// process (or another handle in this one) holds it.
pub fn try_lock(session_file: &Path) -> std::io::Result<Option<File>> {
    let path = lock_path(session_file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    match lock.try_lock_exclusive() {
        Ok(()) => Ok(Some(lock)),
        Err(error) if is_contention(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether another process holds the lock of `session_file` right now. A
/// session file that does not exist yet has no holder.
pub fn is_held(session_file: &Path) -> bool {
    if !lock_path(session_file).exists() {
        return false;
    }
    matches!(try_lock(session_file), Ok(None))
}

fn is_contention(error: &std::io::Error) -> bool {
    let contended = fs2::lock_contended_error();
    error.kind() == std::io::ErrorKind::WouldBlock
        || (contended.kind() == error.kind() && contended.raw_os_error() == error.raw_os_error())
}

/// Why [`SessionFileLock::follow`] could not lock the current file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("session {} is open in another SUPER process", short_name(.0))]
pub struct Held(pub PathBuf);

/// The `<stamp>_<id>` part of a session file name, for messages.
fn short_name(file: &Path) -> String {
    let stem = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| file.display().to_string());
    match stem.rsplit_once('_') {
        Some((_, id)) => id.chars().take(8).collect(),
        None => stem,
    }
}

/// The lock of whichever session file an in-process `super` has open. The
/// file changes when the user starts, resumes or forks a session, and a new
/// session's file appears only with its first entry, so the owner calls
/// [`follow`](Self::follow) with the current file whenever it may have
/// changed (or lets [`SessionLockKeeper`] poll).
#[derive(Debug, Default)]
pub struct SessionFileLock {
    held: Option<(PathBuf, File)>,
}

impl SessionFileLock {
    pub fn new() -> Self {
        Self::default()
    }

    /// The file whose lock is held.
    pub fn locked(&self) -> Option<&Path> {
        self.held.as_ref().map(|(path, _)| path.as_path())
    }

    /// Hold the lock of `current`, releasing the previous file's. A file that
    /// does not exist yet is left alone until it does. `Err(Held)` when
    /// another process holds it; the previous lock is released either way.
    pub fn follow(&mut self, current: Option<&Path>) -> Result<(), Held> {
        if self.locked() == current && current.is_some() {
            return Ok(());
        }
        self.held = None;
        let Some(current) = current else {
            return Ok(());
        };
        if !current.exists() {
            return Ok(());
        }
        match try_lock(current) {
            Ok(Some(file)) => {
                self.held = Some((current.to_path_buf(), file));
                Ok(())
            }
            Ok(None) => Err(Held(current.to_path_buf())),
            // A directory we cannot write is no reason to stop working.
            Err(_) => Ok(()),
        }
    }
}

/// Keeps a [`SessionFileLock`] on the current session file from a background
/// task, polling `current` every `interval`. Dropping it releases the lock.
pub struct SessionLockKeeper {
    task: Option<tokio::task::JoinHandle<()>>,
    conflict: Arc<Mutex<Option<Held>>>,
}

impl SessionLockKeeper {
    /// Lock `current()` now and keep following it. The first check runs
    /// before this returns, so a caller can refuse to start when the file is
    /// already held elsewhere.
    pub fn start(
        current: impl Fn() -> Option<PathBuf> + Send + 'static,
        interval: Duration,
    ) -> Result<Self, Held> {
        let mut lock = SessionFileLock::new();
        lock.follow(current().as_deref())?;
        let conflict: Arc<Mutex<Option<Held>>> = Arc::new(Mutex::new(None));
        let task = {
            let conflict = conflict.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(interval);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    let file = current();
                    let result = lock.follow(file.as_deref());
                    *conflict.lock().unwrap() = result.err();
                }
            })
        };
        Ok(Self {
            task: Some(task),
            conflict,
        })
    }

    /// The file the keeper could not lock at its last check, if any.
    pub fn conflict(&self) -> Option<Held> {
        self.conflict.lock().unwrap().clone()
    }
}

impl Drop for SessionLockKeeper {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_holder_is_refused_until_the_first_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("2026-10-30_a1b2c3d4-e5f6.jsonl");
        std::fs::write(&file, "{}\n").unwrap();
        let first = try_lock(&file).unwrap().expect("free");
        assert!(try_lock(&file).unwrap().is_none());
        assert!(is_held(&file));
        drop(first);
        assert!(!is_held(&file));
        assert!(try_lock(&file).unwrap().is_some());
    }

    #[test]
    fn follow_waits_for_the_file_and_moves_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("s_aaaa1111.jsonl");
        let second = dir.path().join("s_bbbb2222.jsonl");
        let mut lock = SessionFileLock::new();

        // A new session has no file yet: nothing to lock, no lock file left.
        lock.follow(Some(&first)).unwrap();
        assert_eq!(lock.locked(), None);
        assert!(!lock_path(&first).exists());

        std::fs::write(&first, "{}\n").unwrap();
        lock.follow(Some(&first)).unwrap();
        assert_eq!(lock.locked(), Some(first.as_path()));
        assert!(is_held(&first));

        std::fs::write(&second, "{}\n").unwrap();
        lock.follow(Some(&second)).unwrap();
        assert_eq!(lock.locked(), Some(second.as_path()));
        assert!(!is_held(&first), "the previous file is released");

        lock.follow(None).unwrap();
        assert!(!is_held(&second));
    }

    #[test]
    fn follow_reports_a_file_held_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("2026_a1b2c3d4-0000.jsonl");
        std::fs::write(&file, "{}\n").unwrap();
        let _daemon = try_lock(&file).unwrap().expect("free");
        let mut lock = SessionFileLock::new();
        let held = lock.follow(Some(&file)).expect_err("held");
        assert_eq!(
            held.to_string(),
            "session a1b2c3d4 is open in another SUPER process"
        );
        assert_eq!(lock.locked(), None);
    }

    #[tokio::test]
    async fn the_keeper_refuses_to_start_on_a_held_file_and_follows_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x_cccc3333.jsonl");
        std::fs::write(&file, "{}\n").unwrap();
        let other = try_lock(&file).unwrap().expect("free");
        let path = file.clone();
        assert!(
            SessionLockKeeper::start(move || Some(path.clone()), Duration::from_millis(10))
                .is_err()
        );
        drop(other);

        let current: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(Some(file.clone())));
        let keeper = {
            let current = current.clone();
            SessionLockKeeper::start(
                move || current.lock().unwrap().clone(),
                Duration::from_millis(10),
            )
            .expect("free")
        };
        assert!(is_held(&file));
        let next = dir.path().join("x_dddd4444.jsonl");
        std::fs::write(&next, "{}\n").unwrap();
        *current.lock().unwrap() = Some(next.clone());
        for _ in 0..100 {
            if is_held(&next) && !is_held(&file) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(is_held(&next) && !is_held(&file));
        assert_eq!(keeper.conflict(), None);
        drop(keeper);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!is_held(&next));
    }
}
