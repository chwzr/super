//! What a command stream reader needs: where to start, when a command is
//! too old to run, and the cursor file that survives a restart. The session
//! transport and the daemon's host-command tailer share these.

use crate::s2::{ReadStart, S2Client, SequencedRecord};
use std::path::PathBuf;
use std::time::Duration;

/// The last applied sequence number of one command stream, kept on disk so a
/// restarted reader continues after it instead of re-running recent commands.
pub struct CommandCursor {
    path: Option<PathBuf>,
}

impl CommandCursor {
    /// `dir` defaults to `~/.super/agent/remote/cursors`; the file is named
    /// after the stream with every unsafe character replaced.
    pub fn new(dir: Option<PathBuf>, stream: &str) -> Self {
        let dir =
            dir.or_else(|| dirs::home_dir().map(|home| home.join(".super/agent/remote/cursors")));
        let name: String = stream
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Self {
            path: dir.map(|dir| dir.join(name)),
        }
    }

    pub fn load(&self) -> Option<u64> {
        let path = self.path.as_ref()?;
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    }

    pub fn store(&self, seq_num: u64) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(error) = std::fs::write(path, seq_num.to_string()) {
            crate::log::line(format!("command cursor not saved: {error}"));
        }
    }
}

/// A command past its `exp` header, or older than `default_expiry` when it
/// has none, must not run.
pub fn is_expired(record: &SequencedRecord, default_expiry: Duration) -> bool {
    let now = crate::now_ms();
    match record
        .header("exp")
        .and_then(|value| value.parse::<i64>().ok())
    {
        Some(exp) => exp < now,
        None => {
            let issued = i64::try_from(record.timestamp).unwrap_or(now);
            issued + default_expiry.as_millis() as i64 <= now
        }
    }
}

/// Where to start reading: after the last applied command when that is still
/// inside the stream; otherwise the recent commands (first start) or only new
/// ones (a cursor past the tail means the stream was recreated).
pub async fn start_position(
    client: &S2Client,
    stream: &str,
    cursor: &CommandCursor,
    lookback: u64,
) -> ReadStart {
    let Some(last) = cursor.load() else {
        return ReadStart::TailOffset(lookback);
    };
    match client.tail(stream).await {
        Ok(tail) if last < tail => ReadStart::SeqNum(last + 1),
        Ok(_) => ReadStart::TailOffset(0),
        Err(error) => {
            crate::log::line(format!("commands tail unknown, using lookback: {error}"));
            ReadStart::TailOffset(lookback)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(timestamp: u64, exp: Option<&str>) -> SequencedRecord {
        SequencedRecord {
            seq_num: 1,
            timestamp,
            headers: exp
                .map(|exp| vec![("exp".to_string(), exp.to_string())])
                .unwrap_or_default(),
            body: String::new(),
        }
    }

    #[test]
    fn a_command_expires_by_header_or_by_age() {
        let now = crate::now_ms() as u64;
        assert!(is_expired(
            &record(now, Some("1")),
            Duration::from_secs(300)
        ));
        assert!(!is_expired(
            &record(now, Some(&(now + 60_000).to_string())),
            Duration::from_secs(300)
        ));
        assert!(!is_expired(&record(now, None), Duration::from_secs(300)));
        assert!(is_expired(
            &record(now - 400_000, None),
            Duration::from_secs(300)
        ));
    }

    #[test]
    fn the_cursor_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let cursor = CommandCursor::new(Some(dir.path().to_path_buf()), "hosts/01H/commands");
        assert_eq!(cursor.load(), None);
        cursor.store(41);
        assert_eq!(cursor.load(), Some(41));
        assert!(dir.path().join("hosts_01H_commands").exists());
    }
}
