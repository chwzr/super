//! A run's journal on disk, so that a later run can resume it
//! (`resumeFromRunId`) after the run's record left the session's memory or
//! the process that ran it ended.
//!
//! The file holds the finished `agent()` calls of the run: position, prompt
//! and answer. A resumed run replays them up to the first position whose
//! prompt changed ([`super_workflow::Journal`]).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use super_workflow::Journal;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalFile {
    run_id: String,
    agents: Vec<JournalEntry>,
}

#[derive(Serialize, Deserialize)]
struct JournalEntry {
    index: u32,
    prompt: String,
    result: Value,
}

/// Where the journal of the run `run_id` is kept in `directory`.
pub(crate) fn path_in(directory: &Path, run_id: &str) -> PathBuf {
    directory.join(format!("{run_id}.journal.json"))
}

/// Write `journal` of the run `run_id` to `path`, through a temporary file
/// so that a reader never sees half of it.
pub(crate) fn save(path: &Path, run_id: &str, journal: &Journal) -> anyhow::Result<()> {
    let file = JournalFile {
        run_id: run_id.to_string(),
        agents: journal
            .completed()
            .into_iter()
            .map(|(index, prompt, result)| JournalEntry {
                index,
                prompt: prompt.to_string(),
                result: result.clone(),
            })
            .collect(),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec(&file)?)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Read the journal that [`save`] wrote.
pub(crate) fn load(path: &Path) -> anyhow::Result<Journal> {
    let text = std::fs::read(path)?;
    let file: JournalFile = serde_json::from_slice(&text)?;
    let mut journal = Journal::default();
    for entry in file.agents {
        journal.record_completed(entry.index, &entry.prompt, entry.result);
    }
    Ok(journal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_journal_reads_back_with_every_finished_call() {
        let directory = tempfile::tempdir().unwrap();
        let mut journal = Journal::default();
        journal.record_completed(0, "step one", Value::String("one".into()));
        journal.record_completed(3, "step four", serde_json::json!({"files": ["a.rs"]}));
        let path = path_in(&directory.path().join("workflows"), "wf_abcdefgh");
        save(&path, "wf_abcdefgh", &journal).unwrap();
        assert!(path.ends_with("workflows/wf_abcdefgh.journal.json"));

        let loaded = load(&path).unwrap();
        let entries: Vec<(u32, String, Value)> = loaded
            .completed()
            .into_iter()
            .map(|(index, prompt, value)| (index, prompt.to_string(), value.clone()))
            .collect();
        assert_eq!(
            entries,
            [
                (0, "step one".to_string(), Value::String("one".into())),
                (
                    3,
                    "step four".to_string(),
                    serde_json::json!({"files": ["a.rs"]})
                ),
            ]
        );
    }
}
