//! What a session has read of each file.
//!
//! `Read` records every file it returns. `Edit` and `Write` look the file up
//! before they change it: a file that was never read, or that changed on
//! disk since the read, is refused. After their own change they bring the
//! record up to date, so the model need not read a file again after it
//! changed it itself.
//!
//! A session owns one record, reached through [`crate::ToolSession`]. A child
//! agent gets a new, empty record; a fork starts with a copy of its parent's
//! ([`FileStates::copy_from`]).
//!
//! This module holds data only. The native file tools decide what the key of
//! a file is (its canonical path) and fill in the stamps.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// How many files one record remembers. When it is full, the file read
/// longest ago is forgotten first.
pub const MAX_FILE_STATES: usize = 1_000;

/// One file as a session last saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRead {
    /// When the read, or the session's own change, happened.
    pub at: SystemTime,
    /// The file's modification time at that moment, when the platform
    /// reports one.
    pub modified: Option<SystemTime>,
    /// The file's size in bytes at that moment.
    pub len: u64,
    /// Whether that one call returned the whole file. A read that stopped
    /// before the end, or started after the first line, is partial.
    pub whole: bool,
    /// A hash of the file's bytes, kept when the whole file was read, so a
    /// file whose time changed but whose content did not still counts as
    /// unchanged.
    pub fingerprint: Option<u64>,
}

impl FileRead {
    /// Whether a file with this modification time and size is the one that
    /// was read, judged by its stamp alone.
    pub fn same_stamp(&self, modified: Option<SystemTime>, len: u64) -> bool {
        self.modified == modified && self.len == len
    }
}

/// The record of one session: a file's key (its canonical path) to what the
/// session last saw of it.
#[derive(Debug, Default)]
pub struct FileStates {
    entries: Mutex<HashMap<PathBuf, FileRead>>,
}

impl FileStates {
    pub fn new() -> Self {
        Self::default()
    }

    /// What the session last saw of the file with this key.
    pub fn get(&self, key: &Path) -> Option<FileRead> {
        self.entries.lock().unwrap().get(key).cloned()
    }

    /// Remember `read` for the file with this key, in place of what was
    /// there. A full record forgets the file read longest ago.
    pub fn record(&self, key: PathBuf, read: FileRead) {
        let mut entries = self.entries.lock().unwrap();
        entries.insert(key, read);
        while entries.len() > MAX_FILE_STATES {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, read)| read.at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            entries.remove(&oldest);
        }
    }

    /// Forget one file.
    pub fn forget(&self, key: &Path) -> Option<FileRead> {
        self.entries.lock().unwrap().remove(key)
    }

    /// Forget every file, for a conversation that starts over.
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }

    /// Take over everything `other` remembers, in place of this record's
    /// own entries. A fork starts this way from its parent's record.
    pub fn copy_from(&self, other: &FileStates) {
        if std::ptr::eq(self, other) {
            return;
        }
        let copied = other.entries.lock().unwrap().clone();
        *self.entries.lock().unwrap() = copied;
    }

    /// Every file this record remembers, in no particular order.
    pub fn snapshot(&self) -> Vec<(PathBuf, FileRead)> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .map(|(key, read)| (key.clone(), read.clone()))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn read_at(seconds: u64, whole: bool) -> FileRead {
        FileRead {
            at: SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
            len: 3,
            whole,
            fingerprint: whole.then_some(7),
        }
    }

    #[test]
    fn a_record_keeps_the_last_read_of_each_file() {
        let files = FileStates::new();
        assert!(files.get(Path::new("/a")).is_none());
        files.record("/a".into(), read_at(1, false));
        files.record("/a".into(), read_at(2, true));
        assert_eq!(files.get(Path::new("/a")), Some(read_at(2, true)));
        assert_eq!(files.len(), 1);
        assert_eq!(files.forget(Path::new("/a")), Some(read_at(2, true)));
        assert!(files.is_empty());
    }

    #[test]
    fn a_full_record_forgets_the_oldest_read() {
        let files = FileStates::new();
        for index in 0..MAX_FILE_STATES as u64 + 1 {
            files.record(PathBuf::from(format!("/f{index}")), read_at(index, true));
        }
        assert_eq!(files.len(), MAX_FILE_STATES);
        assert!(files.get(Path::new("/f0")).is_none());
        assert!(files.get(Path::new("/f1")).is_some());
    }

    #[test]
    fn a_copy_is_independent_of_its_source() {
        let parent = FileStates::new();
        parent.record("/a".into(), read_at(1, true));
        let fork = FileStates::new();
        fork.record("/b".into(), read_at(1, true));
        fork.copy_from(&parent);
        assert!(fork.get(Path::new("/a")).is_some());
        assert!(fork.get(Path::new("/b")).is_none());
        fork.record("/c".into(), read_at(2, true));
        assert!(parent.get(Path::new("/c")).is_none());
        fork.copy_from(&fork);
        assert_eq!(fork.len(), 2);
        parent.clear();
        assert!(parent.is_empty());
        assert_eq!(fork.snapshot().len(), 2);
    }

    #[test]
    fn the_stamp_compares_time_and_size() {
        let read = read_at(5, true);
        assert!(read.same_stamp(read.modified, 3));
        assert!(!read.same_stamp(read.modified, 4));
        assert!(!read.same_stamp(None, 3));
    }
}
