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

use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

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

    /// This read of the file with the key `key` as the tool's result
    /// carries it in the transcript (`details.fileRead`), so that a session
    /// loaded again can count it as read: `{"path", "at" (ms since the
    /// epoch), "modified" (ns since the epoch, as a string) or null, "len",
    /// "whole", "fingerprint" (16 hex digits) or null}`.
    pub fn to_json(&self, key: &Path) -> Value {
        let since = |time: SystemTime| time.duration_since(SystemTime::UNIX_EPOCH).ok();
        json!({
            "path": key,
            "at": since(self.at).map_or(0, |at| at.as_millis() as u64),
            "modified": self.modified.and_then(since).map(|modified| modified.as_nanos().to_string()),
            "len": self.len,
            "whole": self.whole,
            "fingerprint": self.fingerprint.map(|print| format!("{print:016x}")),
        })
    }

    /// The read [`FileRead::to_json`] wrote, with its key; `None` for
    /// anything else.
    pub fn from_json(value: &Value) -> Option<(PathBuf, FileRead)> {
        let key = PathBuf::from(value.get("path")?.as_str()?);
        let at = SystemTime::UNIX_EPOCH + Duration::from_millis(value.get("at")?.as_u64()?);
        let modified = match value.get("modified")? {
            Value::Null => None,
            Value::String(nanos) => {
                let nanos: u128 = nanos.parse().ok()?;
                let seconds = u64::try_from(nanos / 1_000_000_000).ok()?;
                Some(
                    SystemTime::UNIX_EPOCH + Duration::new(seconds, (nanos % 1_000_000_000) as u32),
                )
            }
            _ => return None,
        };
        let fingerprint = match value.get("fingerprint")? {
            Value::Null => None,
            Value::String(hex) => Some(u64::from_str_radix(hex, 16).ok()?),
            _ => return None,
        };
        Some((
            key,
            FileRead {
                at,
                modified,
                len: value.get("len")?.as_u64()?,
                whole: value.get("whole")?.as_bool()?,
                fingerprint,
            },
        ))
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

    #[test]
    fn a_read_written_into_a_transcript_reads_back_the_same() {
        let read = FileRead {
            at: SystemTime::UNIX_EPOCH + Duration::from_millis(1_764_000_000_123),
            modified: Some(SystemTime::UNIX_EPOCH + Duration::new(1_764_000_000, 987_654_321)),
            len: 42,
            whole: true,
            fingerprint: Some(0x00ab_cdef_0123_4567),
        };
        let key = Path::new("/work/src/main.rs");
        let written = read.to_json(key);
        assert_eq!(written["modified"], "1764000000987654321");
        assert_eq!(written["fingerprint"], "00abcdef01234567");
        assert_eq!(
            FileRead::from_json(&written),
            Some((key.to_path_buf(), read.clone()))
        );
        let partial = FileRead {
            modified: None,
            whole: false,
            fingerprint: None,
            ..read
        };
        assert_eq!(
            FileRead::from_json(&partial.to_json(key)),
            Some((key.to_path_buf(), partial))
        );
        assert_eq!(FileRead::from_json(&json!({"path": "/x"})), None);
        assert_eq!(FileRead::from_json(&Value::Null), None);
    }

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
