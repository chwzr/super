//! What a session keeps in its file besides the conversation, so that it
//! comes back when the session is loaded again (`super -c`, `--session`, the
//! daemon loading it after an idle unload or a restart, `/remote`,
//! `switch_session`).
//!
//! Each piece is a `custom` entry of its own type, appended whenever it
//! changes; the last entry of a type in the file is the state. Custom
//! entries are never part of the model's context. A fork of a session
//! (`--fork`, `fork`) does not copy them: it is a new session that has not
//! entered a worktree, scheduled a wake-up or started a workflow.
//!
//! * [`WORKTREE_ENTRY`]: the worktree the session works in
//!   (`{"worktree": EnteredWorktree | null, "missing": path?}`); `missing`
//!   names a worktree whose folder was gone when the session was loaded.
//! * [`WAKEUP_ENTRY`]: the pending wake-up and the self-paced loop
//!   (`{"wakeup": WakeupInfo | null, "loop": {"job", "goal", "round",
//!   "maxRounds"} | null}`).
//! * [`WORKFLOW_RUN_ENTRY`]: one entry per change of a workflow run
//!   (`{"runId", "name", "status", "cutOff"?}`), `status` being `running`
//!   while it runs and `completed`, `failed` or `stopped` once it ended. A
//!   run whose last entry says `running` when the session is loaded was cut
//!   off by the end of the process that ran it.
//!
//! What the session had read is not stored apart: it is in the transcript,
//! where every result of `Read`, `Edit` and `Write` carries the read in
//! `details.fileRead` ([`super_agent::FileRead::to_json`]), and
//! [`rebuild_reads`] takes it from there.

use crate::session::entry::SessionEntry;
use crate::session::manager::SessionManager;
use serde_json::Value;

/// The worktree the session works in.
pub const WORKTREE_ENTRY: &str = "session_worktree";

/// The session's pending wake-up and its self-paced loop.
pub const WAKEUP_ENTRY: &str = "session_wakeup";

/// A workflow run started or ended.
pub const WORKFLOW_RUN_ENTRY: &str = "workflow_run";

/// Every entry type of this module, which a fork leaves out.
pub const STATE_ENTRIES: [&str; 3] = [WORKTREE_ENTRY, WAKEUP_ENTRY, WORKFLOW_RUN_ENTRY];

/// Whether `entry` is one of the state entries of this module.
pub fn is_state_entry(entry: &SessionEntry) -> bool {
    matches!(entry, SessionEntry::Custom { custom_type, .. } if STATE_ENTRIES.contains(&custom_type.as_str()))
}

/// The data of the last entry of `custom_type` in the file, whatever branch
/// it is on: the state as the session last wrote it.
pub fn latest<'a>(manager: &'a SessionManager, custom_type: &str) -> Option<&'a Value> {
    manager
        .entries()
        .iter()
        .rev()
        .find_map(|entry| match entry {
            SessionEntry::Custom {
                custom_type: kind,
                data: Some(data),
                ..
            } if kind == custom_type => Some(data),
            _ => None,
        })
}

/// The data of every entry of `custom_type`, oldest first.
pub fn all<'a>(manager: &'a SessionManager, custom_type: &str) -> Vec<&'a Value> {
    manager
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            SessionEntry::Custom {
                custom_type: kind,
                data: Some(data),
                ..
            } if kind == custom_type => Some(data),
            _ => None,
        })
        .collect()
}

/// Write the state `data` of `custom_type`. A session that cannot write its
/// file keeps working; it only forgets the state when it is loaded again.
pub fn record(manager: &mut SessionManager, custom_type: &str, data: Value) {
    let _ = manager.append_custom(custom_type, Some(data));
}

/// Rebuild what a session loaded again had read, from its transcript, into
/// `files`: the last read of each file that a `Read`, `Edit` or `Write`
/// result of the session carries, with the modification time and size the
/// file had then. So the model need not read again a file nobody changed,
/// and `Edit` and `Write` refuse one that changed since as changed on disk,
/// as in a session that kept running. Returns how many files it counted.
pub fn rebuild_reads(manager: &SessionManager, files: &super_agent::FileStates) -> usize {
    let mut reads: Vec<(std::path::PathBuf, super_agent::FileRead)> = Vec::new();
    for entry in manager.entries() {
        let SessionEntry::Message {
            message: super_agent::AgentMessage::ToolResult(result),
            ..
        } = entry
        else {
            continue;
        };
        if result.is_error || !matches!(result.tool_name.as_str(), "Read" | "Edit" | "Write") {
            continue;
        }
        let Some((key, read)) = result
            .details
            .as_ref()
            .and_then(|details| details.get("fileRead"))
            .and_then(super_agent::FileRead::from_json)
        else {
            continue;
        };
        reads.retain(|(known, _)| *known != key);
        reads.push((key, read));
    }
    // The oldest go first when there are more than the record keeps.
    let skip = reads
        .len()
        .saturating_sub(super_agent::file_state::MAX_FILE_STATES);
    let mut counted = 0;
    for (key, read) in reads.into_iter().skip(skip) {
        files.record(key, read);
        counted += 1;
    }
    counted
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_last_entry_of_a_type_is_the_state_and_a_fork_leaves_them_out() {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = SessionManager::create(dir.path(), Some(dir.path().join("s"))).unwrap();
        manager
            .append_message(super_agent::AgentMessage::user("hi"))
            .unwrap();
        record(&mut manager, WORKTREE_ENTRY, json!({"worktree": "one"}));
        record(&mut manager, WAKEUP_ENTRY, json!({"wakeup": null}));
        record(&mut manager, WORKTREE_ENTRY, json!({"worktree": "two"}));
        assert_eq!(
            latest(&manager, WORKTREE_ENTRY),
            Some(&json!({"worktree": "two"}))
        );
        assert_eq!(all(&manager, WORKTREE_ENTRY).len(), 2);
        assert_eq!(latest(&manager, WORKFLOW_RUN_ENTRY), None);

        // Loaded again, the state is there; a fork has none of it.
        let file = manager.session_file().unwrap().to_path_buf();
        let loaded = SessionManager::open(&file).unwrap();
        assert_eq!(
            latest(&loaded, WORKTREE_ENTRY),
            Some(&json!({"worktree": "two"}))
        );
        let fork =
            SessionManager::fork_from(&file, dir.path(), Some(dir.path().join("s"))).unwrap();
        assert_eq!(latest(&fork, WORKTREE_ENTRY), None);
        assert_eq!(latest(&fork, WAKEUP_ENTRY), None);
        assert_eq!(fork.build_session_context().messages.len(), 1);
        let branch = loaded.fork_active_branch(None, true).unwrap();
        assert_eq!(latest(&branch, WORKTREE_ENTRY), None);
        assert_eq!(branch.build_session_context().messages.len(), 1);
    }
}
