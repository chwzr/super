//! The daemon log: one line per message to a rotated file and to stderr.
//! Never logs tokens, prompts or record bodies; command types and session
//! ids only.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
        }
    }

    /// `SUPER_LOG=daemon=debug` (or `debug`, or `daemon`) turns debug on; the
    /// same variable `super-remote` reads for `remote`.
    pub fn from_env() -> Self {
        match std::env::var("SUPER_LOG") {
            Ok(value)
                if value
                    .split(',')
                    .any(|part| matches!(part.trim(), "debug" | "daemon" | "daemon=debug")) =>
            {
                Level::Debug
            }
            _ => Level::Info,
        }
    }
}

/// Rotate at 5 MiB, keeping one previous file.
pub const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;

pub struct Logger {
    path: Option<PathBuf>,
    file: Mutex<Option<File>>,
    level: Level,
    stderr: bool,
    max_bytes: u64,
}

impl Logger {
    pub fn new(path: Option<PathBuf>, level: Level, stderr: bool, max_bytes: u64) -> Self {
        let file = path.as_deref().and_then(|path| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok()?;
            }
            open_append(path)
        });
        Self {
            path,
            file: Mutex::new(file),
            level,
            stderr,
            max_bytes,
        }
    }

    pub fn log(&self, level: Level, target: &str, message: &str) {
        if level > self.level {
            return;
        }
        let line = format!(
            "{} {} {}: {}",
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ"),
            level.label(),
            target,
            message
        );
        if self.stderr {
            eprintln!("{line}");
        }
        let Some(path) = &self.path else { return };
        let mut guard = self.file.lock().unwrap();
        let full = guard
            .as_ref()
            .and_then(|file| file.metadata().ok())
            .is_some_and(|metadata| metadata.len() >= self.max_bytes);
        if full {
            let _ = std::fs::rename(path, rotated(path));
            *guard = None;
        }
        if guard.is_none() {
            *guard = open_append(path);
        }
        if let Some(file) = guard.as_mut() {
            let _ = writeln!(file, "{line}");
        }
    }
}

fn rotated(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".1");
    path.with_file_name(name)
}

/// Open for appending. Does not create directories: a log directory that
/// went away (a test's temp dir) must not be resurrected.
fn open_append(path: &Path) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(path).ok()
}

static GLOBAL: OnceLock<Logger> = OnceLock::new();

/// Install the process-wide logger. A second call is ignored.
pub fn init(logger: Logger) {
    let _ = GLOBAL.set(logger);
}

fn emit(level: Level, target: &str, message: &str) {
    match GLOBAL.get() {
        Some(logger) => logger.log(level, target, message),
        None if level <= Level::Warn => eprintln!("{} {target}: {message}", level.label()),
        None => {}
    }
}

pub fn error(target: &str, message: impl AsRef<str>) {
    emit(Level::Error, target, message.as_ref());
}

pub fn warn(target: &str, message: impl AsRef<str>) {
    emit(Level::Warn, target, message.as_ref());
}

pub fn info(target: &str, message: impl AsRef<str>) {
    emit(Level::Info, target, message.as_ref());
}

pub fn debug(target: &str, message: impl AsRef<str>) {
    emit(Level::Debug, target, message.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_rotates_once_at_the_size_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("logs/daemon.log");
        let logger = Logger::new(Some(path.clone()), Level::Info, false, 120);
        for i in 0..6 {
            logger.log(
                Level::Info,
                "test",
                &format!("line number {i} with some padding text"),
            );
        }
        let rotated = dir.path().join("logs/daemon.log.1");
        assert!(rotated.exists(), "the full file was renamed");
        let current = std::fs::read_to_string(&path).expect("current file");
        // Each line is about 70 bytes, so the file rotates after two of them.
        assert!(current.lines().count() <= 2, "the current file started over: {current}");
        assert!(current.contains("INFO test: line number 5"));
        let previous = std::fs::read_to_string(&rotated).expect("rotated file");
        assert!(previous.contains("line number"), "{previous}");
    }

    #[test]
    fn debug_lines_are_dropped_at_info() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("daemon.log");
        let logger = Logger::new(Some(path.clone()), Level::Info, false, DEFAULT_MAX_BYTES);
        logger.log(Level::Debug, "test", "hidden");
        logger.log(Level::Warn, "test", "shown");
        let text = std::fs::read_to_string(&path).expect("file");
        assert!(!text.contains("hidden"));
        assert!(text.contains("WARN test: shown"));
    }
}
