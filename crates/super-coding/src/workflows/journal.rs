//! A run's journal on disk, so that a later run can resume it
//! (`resumeFromRunId`) after the run's record left the session's memory or
//! the process that ran it ended, also when that process ended in the
//! middle of the run.
//!
//! The file holds the finished `agent()` calls of the run: position, prompt
//! and answer. A resumed run replays them up to the first position whose
//! prompt changed ([`super_workflow::Journal`]). It is JSON Lines, written
//! as the run goes: a first line `{"runId": …}` with the calls the run
//! started from, then one line per change of the journal: `{"index",
//! "prompt", "result"}` when a call finished and `{"invalidateFrom": n}`
//! when the calls from a position on were forgotten. When the run ends the
//! file is written again with its finished calls only. Reading it replays
//! the lines in order; a line cut off half-way, as the end of a process can
//! leave it, is left out.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use super_workflow::{Journal, JournalChange};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    run_id: String,
}

#[derive(Serialize, Deserialize)]
struct Completed {
    index: u32,
    prompt: String,
    result: Value,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Invalidated {
    invalidate_from: u32,
}

/// Where the journal of the run `run_id` is kept in `directory`.
pub(crate) fn path_in(directory: &Path, run_id: &str) -> PathBuf {
    directory.join(format!("{run_id}.journal.jsonl"))
}

/// Write `journal` of the run `run_id` to `path` as a whole, through a
/// temporary file so that a reader never sees half of it: at the start of
/// a run, with the calls it resumes, and at its end.
pub(crate) fn save(path: &Path, run_id: &str, journal: &Journal) -> anyhow::Result<()> {
    let mut text = serde_json::to_string(&Header {
        run_id: run_id.to_string(),
    })?;
    text.push('\n');
    for (index, prompt, result) in journal.completed() {
        text.push_str(&serde_json::to_string(&Completed {
            index,
            prompt: prompt.to_string(),
            result: result.clone(),
        })?);
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("jsonl.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Add one change of a running run's journal to the file [`save`] began.
pub(crate) fn append(path: &Path, change: JournalChange<'_>) -> anyhow::Result<()> {
    let mut line = match change {
        JournalChange::Completed {
            index,
            prompt,
            value,
        } => serde_json::to_string(&Completed {
            index,
            prompt: prompt.to_string(),
            result: value.clone(),
        })?,
        JournalChange::Invalidated { from } => serde_json::to_string(&Invalidated {
            invalidate_from: from,
        })?,
    };
    line.push('\n');
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// Read the journal that [`save`] and [`append`] wrote.
pub(crate) fn load(path: &Path) -> anyhow::Result<Journal> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header = lines
        .next()
        .ok_or_else(|| anyhow::anyhow!("the file is empty"))?;
    serde_json::from_str::<Header>(header)?;
    let mut journal = Journal::default();
    for line in lines {
        if let Ok(completed) = serde_json::from_str::<Completed>(line) {
            journal.record_completed(completed.index, &completed.prompt, completed.result);
        } else if let Ok(invalidated) = serde_json::from_str::<Invalidated>(line) {
            journal.invalidate_from(invalidated.invalidate_from);
        }
    }
    Ok(journal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(journal: &Journal) -> Vec<(u32, String, Value)> {
        journal
            .completed()
            .into_iter()
            .map(|(index, prompt, value)| (index, prompt.to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn a_saved_journal_reads_back_with_every_finished_call() {
        let directory = tempfile::tempdir().unwrap();
        let mut journal = Journal::default();
        journal.record_completed(0, "step one", Value::String("one".into()));
        journal.record_completed(3, "step four", serde_json::json!({"files": ["a.rs"]}));
        let path = path_in(&directory.path().join("workflows"), "wf_abcdefgh");
        save(&path, "wf_abcdefgh", &journal).unwrap();
        assert!(path.ends_with("workflows/wf_abcdefgh.journal.jsonl"));

        let loaded = load(&path).unwrap();
        assert_eq!(
            entries(&loaded),
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

    #[test]
    fn changes_written_as_the_run_goes_read_back_in_order_and_a_cut_line_is_left_out() {
        let directory = tempfile::tempdir().unwrap();
        let path = path_in(directory.path(), "wf_running1");
        let mut resumed = Journal::default();
        resumed.record_completed(0, "one", Value::String("1".into()));
        resumed.record_completed(1, "two", Value::String("2".into()));
        save(&path, "wf_running1", &resumed).unwrap();
        append(&path, JournalChange::Invalidated { from: 1 }).unwrap();
        let value = Value::String("2 again".into());
        append(
            &path,
            JournalChange::Completed {
                index: 1,
                prompt: "two, revised",
                value: &value,
            },
        )
        .unwrap();
        // The process ended in the middle of a line.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"{\"index\":2,\"prom").unwrap();
        drop(file);

        assert_eq!(
            entries(&load(&path).unwrap()),
            [
                (0, "one".to_string(), Value::String("1".into())),
                (1, "two, revised".to_string(), value),
            ]
        );
    }
}
