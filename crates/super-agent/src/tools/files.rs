//! The file tools' side of a session's record of reads: stamps taken from
//! the file system, content fingerprints, and the check Edit and Write run
//! before they change a file.

use crate::file_state::{FileRead, FileStates};
use crate::tools::path::file_key;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A hash of a file's bytes, kept for a whole read.
pub fn fingerprint(bytes: &[u8]) -> u64 {
    xxhash_rust::xxh64::xxh64(bytes, 0)
}

/// A hash built line by line while a file is read in order from its start.
pub type StreamingFingerprint = xxhash_rust::xxh64::Xxh64;

/// A new [`StreamingFingerprint`], equal to [`fingerprint`] once it has seen
/// every byte.
pub fn streaming_fingerprint() -> StreamingFingerprint {
    xxhash_rust::xxh64::Xxh64::new(0)
}

/// What a session saw of a file with this metadata, now.
pub fn record_of(meta: &Metadata, whole: bool, fingerprint: Option<u64>) -> FileRead {
    FileRead {
        at: SystemTime::now(),
        modified: meta.modified().ok(),
        len: meta.len(),
        whole,
        fingerprint,
    }
}

/// Whether the file, with this metadata and these bytes now, is still what
/// `read` saw: its stamp is the same, or the whole file was read and its
/// bytes are the same.
pub fn unchanged(read: &FileRead, meta: &Metadata, bytes: &[u8]) -> bool {
    read.same_stamp(meta.modified().ok(), meta.len())
        || (read.whole && read.fingerprint == Some(fingerprint(bytes)))
}

/// Record `read` of the file with the key `key`, and return it as the
/// tool's result carries it (`details.fileRead`, [`FileRead::to_json`]),
/// so that a session loaded again counts it as read.
pub fn remember(files: &FileStates, key: PathBuf, read: FileRead) -> serde_json::Value {
    let written = read.to_json(&key);
    files.record(key, read);
    written
}

/// Bring the record up to date after the session's own Edit or Write left
/// `bytes` in the file at `path`. The record then counts as a read of the
/// whole file; returned as [`remember`] returns it, or `null` when the file
/// cannot be looked at.
pub async fn record_own_change(files: &FileStates, path: &Path, bytes: &[u8]) -> serde_json::Value {
    let key = file_key(path).await;
    match tokio::fs::metadata(path).await {
        Ok(meta) => remember(files, key, record_of(&meta, true, Some(fingerprint(bytes)))),
        Err(_) => {
            files.forget(&key);
            serde_json::Value::Null
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_streaming_fingerprint_matches_the_whole_one() {
        let mut streaming = streaming_fingerprint();
        streaming.update(b"one\n");
        streaming.update(b"two");
        assert_eq!(streaming.digest(), fingerprint(b"one\ntwo"));
    }

    #[tokio::test]
    async fn a_whole_read_survives_a_new_time_but_not_new_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "abc").unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let mut read = record_of(&meta, true, Some(fingerprint(b"abc")));
        read.modified = None;
        assert!(unchanged(&read, &meta, b"abc"));
        assert!(!unchanged(&read, &meta, b"abd"));
        read.whole = false;
        assert!(!unchanged(&read, &meta, b"abc"));
        let partial = record_of(&meta, false, None);
        assert!(unchanged(&partial, &meta, b"anything"));
    }
}
