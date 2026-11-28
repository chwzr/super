//! Edit tool: one exact-text replacement per call, in a file the session has
//! read. The byte-order mark and Windows line endings are kept, and the
//! result details carry a diff for the interface.
//!
//! Rules:
//! - `file_path` is absolute;
//! - the file was read first in this session (a partial read counts), and has
//!   not changed on disk since; the session's own Edit and Write keep that
//!   record current;
//! - `old_string` occurs exactly once, or `replace_all` replaces every
//!   occurrence; when it does not occur as given, curly quotes count as
//!   their straight forms, in the file and in `old_string`, and what is
//!   replaced is the text that is in the file, with the straight quotes of
//!   `new_string` written in the file's curly style;
//! - an empty `old_string` creates a file that does not exist yet, or fills
//!   an empty one.

use crate::placeholder;
use crate::tool::{AgentTool, DynTool, ToolResult, ToolSession, ToolUpdateSink};
use crate::tools::arguments;
use crate::tools::files::{record_own_change, unchanged};
use crate::tools::mutation_queue::lock_path;
use crate::tools::path::{absolute, display, file_key};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use super_ai::ContentBlock;
use tokio_util::sync::CancellationToken;

pub struct EditTool {
    /// The session's folder, used to suggest an absolute path and to shorten
    /// the diff's file names.
    pub cwd: PathBuf,
    /// The session this tool edits for; [`AgentTool::for_session`] binds it.
    pub session: ToolSession,
}

impl EditTool {
    /// An Edit tool with a file record of its own.
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            session: ToolSession::new(),
        }
    }
}

const BOM: &str = "\u{feff}";

#[derive(Debug, Clone, Copy, PartialEq)]
enum LineEnding {
    Lf,
    CrLf,
}

fn detect_line_ending(text: &str) -> LineEnding {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    if crlf > lf {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    }
}

/// The straight form of a curly quote; any other character as it is.
fn straight(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' => '\'',
        '\u{201C}' | '\u{201D}' => '"',
        other => other,
    }
}

/// Where `old` occurs in `content` with curly quotes counted as straight
/// ones, on both sides: the byte ranges of `content` it covers, without
/// overlaps, from the start.
fn quote_blind_matches(content: &str, old: &str) -> Vec<std::ops::Range<usize>> {
    let straight_content: String = content.chars().map(straight).collect();
    let straight_old: String = old.chars().map(straight).collect();
    // Each character keeps its place: the n-th character of one is the
    // n-th of the other, so a match is mapped back by character counts.
    let mut original: Vec<usize> = content.char_indices().map(|(at, _)| at).collect();
    original.push(content.len());
    let straightened: Vec<usize> = straight_content.char_indices().map(|(at, _)| at).collect();
    let length = straight_old.chars().count();
    straight_content
        .match_indices(&straight_old)
        .filter_map(|(at, _)| {
            let first = straightened.binary_search(&at).ok()?;
            Some(original[first]..original[first + length])
        })
        .collect()
}

/// `new` written in the quote style of `found`, the text it replaces: where
/// `found` has curly double or single quotes, the straight ones of that kind
/// in `new` become curly. An apostrophe inside a word becomes a right single
/// quote; a quote at the start or after a space or an opening bracket
/// becomes an opening one, any other a closing one.
fn in_quote_style(found: &str, new: &str) -> String {
    let double = found.contains(['\u{201C}', '\u{201D}']);
    let single = found.contains(['\u{2018}', '\u{2019}']);
    if !double && !single {
        return new.to_string();
    }
    let chars: Vec<char> = new.chars().collect();
    let opens = |index: usize| {
        index == 0 || {
            let before = chars[index - 1];
            before.is_whitespace() || matches!(before, '(' | '[' | '{')
        }
    };
    chars
        .iter()
        .enumerate()
        .map(|(index, &c)| match c {
            '"' if double => {
                if opens(index) {
                    '\u{201C}'
                } else {
                    '\u{201D}'
                }
            }
            '\'' if single => {
                let in_word = index > 0
                    && chars[index - 1].is_alphanumeric()
                    && chars
                        .get(index + 1)
                        .is_some_and(|after| after.is_alphanumeric());
                if !in_word && opens(index) {
                    '\u{2018}'
                } else {
                    '\u{2019}'
                }
            }
            other => other,
        })
        .collect()
}

/// Replace `old` in `content`: its one occurrence, or every one with
/// `replace_all`. When `old` does not occur as given, it is looked for with
/// curly quotes counted as straight ones, and each place found is replaced
/// by `new` in that place's quote style. Returns the new content and how
/// many were replaced.
fn replace(
    path: &Path,
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> anyhow::Result<(String, usize)> {
    let count = content.matches(old).count();
    if count == 0 {
        let found = quote_blind_matches(content, old);
        if !found.is_empty() {
            return replace_ranges(path, content, old, new, replace_all, &found);
        }
        anyhow::bail!(
            "old_string was not found in {}; give the text exactly as it is in the file, without the line numbers Read adds.\nold_string:\n{old}",
            path.display()
        );
    }
    if count > 1 && !replace_all {
        anyhow::bail!(
            "old_string occurs {count} times in {}; give more of the surrounding text so it occurs once, or set replace_all to replace every occurrence.\nold_string:\n{old}",
            path.display()
        );
    }
    let replaced = if replace_all {
        content.replace(old, new)
    } else {
        content.replacen(old, new, 1)
    };
    Ok((replaced, count))
}

/// Replace the places `found` of `content`, which `old` matched with its
/// quotes counted as straight ones, by `new` in each place's quote style.
fn replace_ranges(
    path: &Path,
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
    found: &[std::ops::Range<usize>],
) -> anyhow::Result<(String, usize)> {
    let count = found.len();
    if count > 1 && !replace_all {
        anyhow::bail!(
            "old_string occurs {count} times in {}; give more of the surrounding text so it occurs once, or set replace_all to replace every occurrence.\nold_string:\n{old}",
            path.display()
        );
    }
    let mut replaced = String::with_capacity(content.len() + new.len() * count);
    let mut end = 0;
    for range in found {
        replaced.push_str(&content[end..range.start]);
        replaced.push_str(&in_quote_style(&content[range.clone()], new));
        end = range.end;
    }
    replaced.push_str(&content[end..]);
    Ok((replaced, count))
}

/// The diff's file name: relative to the session's folder when inside it.
fn diff_name(prefix: &str, cwd: &Path, path: &Path) -> String {
    let shown = display(cwd, path);
    if shown.starts_with('/') {
        format!("{prefix}{shown}")
    } else {
        format!("{prefix}/{shown}")
    }
}

fn unified_diff(cwd: &Path, path: &Path, old: &str, new: &str) -> (String, Option<usize>) {
    let diff = similar::TextDiff::from_lines(old, new);
    let first_changed = diff
        .ops()
        .iter()
        .find(|op| !matches!(op.tag(), similar::DiffTag::Equal))
        .map(|op| op.new_range().start + 1);
    let patch = diff
        .unified_diff()
        .context_radius(3)
        .header(&diff_name("a", cwd, path), &diff_name("b", cwd, path))
        .to_string();
    (patch, first_changed)
}

fn not_editable(path: &Path, error: std::io::Error) -> anyhow::Error {
    anyhow::anyhow!("Could not edit {}: {error}.", path.display())
}

impl EditTool {
    /// The result of an edit; `read` is the record of the file after it
    /// ([`record_own_change`]).
    fn result(
        &self,
        path: &Path,
        summary: String,
        old: &str,
        new: &str,
        read: Value,
    ) -> ToolResult {
        let (patch, first_changed_line) = unified_diff(&self.cwd, path, old, new);
        ToolResult {
            content: vec![ContentBlock::text(summary)],
            details: json!({
                "diff": patch,
                "patch": patch,
                "firstChangedLine": first_changed_line,
                "fileRead": read,
            }),
            ..Default::default()
        }
    }

    async fn write(&self, path: &Path, bytes: &[u8]) -> anyhow::Result<Value> {
        tokio::fs::write(path, bytes)
            .await
            .map_err(|error| not_editable(path, error))?;
        Ok(record_own_change(&self.session.files, path, bytes).await)
    }
}

#[async_trait::async_trait]
impl AgentTool for EditTool {
    fn name(&self) -> &str {
        "Edit"
    }

    fn description(&self) -> String {
        placeholder::tool_description("Edit")
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": placeholder::tool_parameter("Edit", "file_path"),
                },
                "old_string": {
                    "type": "string",
                    "description": placeholder::tool_parameter("Edit", "old_string"),
                },
                "new_string": {
                    "type": "string",
                    "description": placeholder::tool_parameter("Edit", "new_string"),
                },
                "replace_all": {
                    "type": "boolean",
                    "default": false,
                    "description": placeholder::tool_parameter("Edit", "replace_all"),
                },
            },
            "required": ["file_path", "old_string", "new_string"],
            "additionalProperties": false,
        })
    }

    fn prepare_arguments(&self, mut args: Value) -> Value {
        if let Some(object) = args.as_object_mut() {
            arguments::drop_nulls(object, &["replace_all"]);
            arguments::boolean(object, "replace_all");
        }
        args
    }

    fn for_session(&self, session: &ToolSession) -> Option<DynTool> {
        Some(Arc::new(EditTool {
            cwd: self.cwd.clone(),
            session: session.clone(),
        }))
    }

    fn in_directory(&self, cwd: &std::path::Path) -> Option<DynTool> {
        Some(Arc::new(EditTool {
            cwd: cwd.to_path_buf(),
            session: self.session.clone(),
        }))
    }

    async fn execute(
        &self,
        _id: &str,
        args: Value,
        cancel: CancellationToken,
        _on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        let path = absolute(&self.cwd, args["file_path"].as_str().unwrap_or_default())?;
        let old = args["old_string"]
            .as_str()
            .unwrap_or_default()
            .replace("\r\n", "\n");
        let new = args["new_string"]
            .as_str()
            .unwrap_or_default()
            .replace("\r\n", "\n");
        let replace_all = args["replace_all"].as_bool().unwrap_or(false);
        if old == new {
            anyhow::bail!(
                "old_string and new_string are the same, so there is nothing to change; put the changed text in new_string."
            );
        }
        let key = file_key(&path).await;
        let _guard = lock_path(&key).await;
        if cancel.is_cancelled() {
            anyhow::bail!("Operation aborted");
        }

        let meta = match tokio::fs::metadata(&path).await {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !old.is_empty() {
                    anyhow::bail!(
                        "{} does not exist; check the path, or create the file with Write.",
                        path.display()
                    );
                }
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|error| not_editable(&path, error))?;
                }
                let read = self.write(&path, new.as_bytes()).await?;
                return Ok(self.result(
                    &path,
                    format!("Created {}.", path.display()),
                    "",
                    &new,
                    read,
                ));
            }
            Err(error) => return Err(not_editable(&path, error)),
        };
        if meta.is_dir() {
            anyhow::bail!(
                "{} is a folder, not a file; give the path of a file to edit.",
                path.display()
            );
        }
        if !meta.is_file() {
            anyhow::bail!(
                "{} is not a regular file; Edit changes regular files only.",
                path.display()
            );
        }

        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|error| not_editable(&path, error))?;
        let raw = std::str::from_utf8(&bytes).map_err(|_| {
            anyhow::anyhow!(
                "{} is not UTF-8 text; Edit changes text files only.",
                path.display()
            )
        })?;
        let (bom, content) = match raw.strip_prefix(BOM) {
            Some(rest) => (BOM, rest),
            None => ("", raw),
        };
        let ending = detect_line_ending(content);
        let normalized = content.replace("\r\n", "\n");

        // An empty old_string fills an empty file, which needs no read.
        if old.is_empty() {
            if !normalized.trim().is_empty() {
                anyhow::bail!(
                    "old_string is empty, but {} is not; give the text to replace in old_string.",
                    path.display()
                );
            }
        } else {
            match self.session.files.get(&key) {
                None => anyhow::bail!(
                    "{} has not been read yet; read it with Read before you edit it.",
                    path.display()
                ),
                Some(read) if !unchanged(&read, &meta, &bytes) => anyhow::bail!(
                    "{} has changed on disk since it was read; read it again before you edit it.",
                    path.display()
                ),
                Some(_) => {}
            }
        }

        let (replaced, count) = if old.is_empty() {
            (new.clone(), 1)
        } else {
            replace(&path, &normalized, &old, &new, replace_all)?
        };
        if cancel.is_cancelled() {
            anyhow::bail!("Operation aborted");
        }
        let restored = match ending {
            LineEnding::Lf => replaced.clone(),
            LineEnding::CrLf => replaced.replace('\n', "\r\n"),
        };
        let read = self
            .write(&path, format!("{bom}{restored}").as_bytes())
            .await?;

        let summary = if count == 1 {
            format!("Replaced 1 occurrence in {}.", path.display())
        } else {
            format!("Replaced {count} occurrences in {}.", path.display())
        };
        Ok(self.result(&path, summary, &normalized, &replaced, read))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::read::ReadTool;

    struct Files {
        dir: tempfile::TempDir,
        session: ToolSession,
    }

    impl Files {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                session: ToolSession::new(),
            }
        }

        fn path(&self, name: &str) -> String {
            self.dir.path().join(name).display().to_string()
        }

        fn create(&self, name: &str, content: &str) -> String {
            std::fs::write(self.dir.path().join(name), content).unwrap();
            self.path(name)
        }

        fn content(&self, name: &str) -> String {
            std::fs::read_to_string(self.dir.path().join(name)).unwrap()
        }

        fn tool<T: AgentTool>(&self, tool: T) -> DynTool {
            tool.for_session(&self.session).unwrap()
        }

        async fn read(&self, args: Value) {
            let tool = self.tool(ReadTool::new(self.dir.path().to_path_buf()));
            tool.execute("r", args, CancellationToken::new(), None)
                .await
                .unwrap();
        }

        async fn edit(&self, args: Value) -> anyhow::Result<ToolResult> {
            let tool = self.tool(EditTool::new(self.dir.path().to_path_buf()));
            let args = tool.prepare_arguments(args);
            crate::validate::validate_arguments(&tool.parameters(), &args)
                .map_err(anyhow::Error::msg)?;
            tool.execute("e", args, CancellationToken::new(), None)
                .await
        }
    }

    #[tokio::test]
    async fn replaces_one_occurrence_after_a_read() {
        let files = Files::new();
        let path = files.create("f.txt", "alpha\nbeta\ngamma\n");
        files.read(json!({"file_path": path})).await;
        let result = files
            .edit(json!({"file_path": path, "old_string": "beta", "new_string": "BETA"}))
            .await
            .unwrap();
        assert_eq!(files.content("f.txt"), "alpha\nBETA\ngamma\n");
        assert!(result.output_text().contains("Replaced 1 occurrence"));
        let diff = result.details["diff"].as_str().unwrap();
        assert!(diff.contains("-beta\n+BETA"), "{diff}");
        assert!(diff.contains("a/f.txt"), "{diff}");
        assert_eq!(result.details["firstChangedLine"], 2);
    }

    #[tokio::test]
    async fn straight_quotes_find_curly_ones_and_new_text_takes_the_files_style() {
        let files = Files::new();
        // Curly double quotes in the file, straight ones in old_string: the
        // text in the file is replaced, and the new quotes are curled.
        let path = files.create(
            "story.md",
            "She said \u{201C}hello\u{201D} and left.\nThe end.\n",
        );
        files.read(json!({"file_path": path})).await;
        let result = files
            .edit(json!({
                "file_path": path,
                "old_string": "said \"hello\" and",
                "new_string": "said \"goodbye\" and",
            }))
            .await
            .unwrap();
        assert!(result.output_text().contains("Replaced 1 occurrence"));
        assert_eq!(
            files.content("story.md"),
            "She said \u{201C}goodbye\u{201D} and left.\nThe end.\n"
        );

        // Curly single quotes: an apostrophe in a word is a right single
        // quote, a quote after a space an opening one, else a closing one.
        let path = files.create("note.md", "It\u{2019}s \u{2018}fine\u{2019} here.\n");
        files.read(json!({"file_path": path})).await;
        files
            .edit(json!({
                "file_path": path,
                "old_string": "It's 'fine' here.",
                "new_string": "'Tis what it's, 'really' it.",
            }))
            .await
            .unwrap();
        assert_eq!(
            files.content("note.md"),
            "\u{2018}Tis what it\u{2019}s, \u{2018}really\u{2019} it.\n"
        );

        // Curly quotes in old_string find straight ones in the file, and a
        // file without curly quotes keeps new_string as it is.
        let path = files.create("code.rs", "let s = \"it's\";\n");
        files.read(json!({"file_path": path})).await;
        files
            .edit(json!({
                "file_path": path,
                "old_string": "\u{201C}it\u{2019}s\u{201D}",
                "new_string": "\"it's done\"",
            }))
            .await
            .unwrap();
        assert_eq!(files.content("code.rs"), "let s = \"it's done\";\n");
    }

    #[tokio::test]
    async fn quote_blind_matches_count_like_exact_ones_and_exact_matches_win() {
        let files = Files::new();
        let path = files.create("twice.md", "\u{201C}a\u{201D} then \u{201C}a\u{201D}\n");
        files.read(json!({"file_path": path})).await;
        let error = files
            .edit(json!({"file_path": path, "old_string": "\"a\"", "new_string": "\"b\""}))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("occurs 2 times"), "{error}");
        let result = files
            .edit(json!({"file_path": path, "old_string": "\"a\"", "new_string": "\"b\"", "replace_all": true}))
            .await
            .unwrap();
        assert!(result.output_text().contains("Replaced 2 occurrences"));
        assert_eq!(
            files.content("twice.md"),
            "\u{201C}b\u{201D} then \u{201C}b\u{201D}\n"
        );

        // A text that is in the file as given is replaced as given, even
        // where the file also has a curly form of it, and nothing is curled.
        let path = files.create("both.md", "\"x\" and \u{201C}x\u{201D}\n");
        files.read(json!({"file_path": path})).await;
        files
            .edit(json!({"file_path": path, "old_string": "\"x\"", "new_string": "\"y\""}))
            .await
            .unwrap();
        assert_eq!(files.content("both.md"), "\"y\" and \u{201C}x\u{201D}\n");

        // Text that differs in more than its quotes is still not found.
        let error = files
            .edit(json!({"file_path": path, "old_string": "\"z\"", "new_string": "\"w\""}))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("old_string was not found"), "{error}");
    }

    #[test]
    fn the_quote_style_follows_the_replaced_text() {
        assert_eq!(in_quote_style("plain", "say \"hi\""), "say \"hi\"");
        // Only the kind of quote the file curls is curled.
        assert_eq!(
            in_quote_style("\u{201C}x\u{201D}", "\"it's\" (\"no\")"),
            "\u{201C}it's\u{201D} (\u{201C}no\u{201D})"
        );
        assert_eq!(
            in_quote_style("don\u{2019}t", "won't \"x\" 'y'"),
            "won\u{2019}t \"x\" \u{2018}y\u{2019}"
        );
        // Ranges map back over characters of different widths.
        let content = "\u{00e9}\u{201C}a\u{201D} \u{201C}a\u{201D}";
        let found = quote_blind_matches(content, "\"a\"");
        assert_eq!(found.len(), 2);
        for range in found {
            assert_eq!(&content[range], "\u{201C}a\u{201D}");
        }
    }

    #[tokio::test]
    async fn an_unread_file_is_refused() {
        let files = Files::new();
        let path = files.create("f.txt", "old\n");
        let error = files
            .edit(json!({"file_path": path, "old_string": "old", "new_string": "new"}))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("has not been read yet"),
            "{error}"
        );
        assert_eq!(files.content("f.txt"), "old\n");
    }

    #[tokio::test]
    async fn a_partial_read_is_enough() {
        let files = Files::new();
        let path = files.create("f.txt", "one\ntwo\nthree\n");
        files
            .read(json!({"file_path": path, "offset": 3, "limit": 1}))
            .await;
        files
            .edit(json!({"file_path": path, "old_string": "one", "new_string": "ONE"}))
            .await
            .unwrap();
        assert_eq!(files.content("f.txt"), "ONE\ntwo\nthree\n");
    }

    #[tokio::test]
    async fn a_file_changed_on_disk_since_the_read_is_refused() {
        let files = Files::new();
        let path = files.create("f.txt", "one\ntwo\n");
        files.read(json!({"file_path": path, "limit": 1})).await;
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let error = files
            .edit(json!({"file_path": path, "old_string": "two", "new_string": "2"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("changed on disk"), "{error}");
        // Reading it again lets the edit through.
        files.read(json!({"file_path": path})).await;
        files
            .edit(json!({"file_path": path, "old_string": "two", "new_string": "2"}))
            .await
            .unwrap();
        assert_eq!(files.content("f.txt"), "one\n2\nthree\n");
    }

    #[tokio::test]
    async fn a_new_time_with_the_same_content_passes_after_a_whole_read() {
        let files = Files::new();
        let path = files.create("f.txt", "same\n");
        files.read(json!({"file_path": path})).await;
        let key = file_key(Path::new(&path)).await;
        let mut read = files.session.files.get(&key).unwrap();
        read.modified = None;
        files.session.files.record(key.clone(), read.clone());
        files
            .edit(json!({"file_path": path, "old_string": "same", "new_string": "new"}))
            .await
            .unwrap();
        // After a partial read the same change of time is refused.
        read.whole = false;
        read.modified = None;
        files.session.files.record(key, read);
        let error = files
            .edit(json!({"file_path": path, "old_string": "new", "new_string": "newer"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("changed on disk"), "{error}");
    }

    #[tokio::test]
    async fn the_sessions_own_edits_keep_the_record_current() {
        let files = Files::new();
        let path = files.create("f.txt", "a\nb\n");
        files.read(json!({"file_path": path})).await;
        for (old, new) in [("a", "A"), ("b", "B"), ("A", "a")] {
            files
                .edit(json!({"file_path": path, "old_string": old, "new_string": new}))
                .await
                .unwrap();
        }
        assert_eq!(files.content("f.txt"), "a\nB\n");
        let read = files
            .session
            .files
            .get(&file_key(Path::new(&path)).await)
            .unwrap();
        assert!(read.whole);
    }

    #[tokio::test]
    async fn a_repeated_string_needs_replace_all() {
        let files = Files::new();
        let path = files.create("f.txt", "dup\ndup\n");
        files.read(json!({"file_path": path})).await;
        let error = files
            .edit(json!({"file_path": path, "old_string": "dup", "new_string": "x"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("occurs 2 times"), "{error}");
        let result = files
            .edit(json!({"file_path": path, "old_string": "dup", "new_string": "x", "replace_all": "true"}))
            .await
            .unwrap();
        assert!(result.output_text().contains("Replaced 2 occurrences"));
        assert_eq!(files.content("f.txt"), "x\nx\n");
    }

    #[tokio::test]
    async fn missing_text_and_identical_strings_fail() {
        let files = Files::new();
        let path = files.create("f.txt", "content\n");
        files.read(json!({"file_path": path})).await;
        let error = files
            .edit(json!({"file_path": path, "old_string": "absent", "new_string": "x"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not found"), "{error}");
        let error = files
            .edit(json!({"file_path": path, "old_string": "content", "new_string": "content"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("the same"), "{error}");
    }

    #[tokio::test]
    async fn crlf_and_bom_preserved() {
        let files = Files::new();
        let path = files.create("f.txt", "\u{feff}a\r\nb\r\n");
        files.read(json!({"file_path": path})).await;
        files
            .edit(json!({"file_path": path, "old_string": "a\r\nb", "new_string": "a\nB"}))
            .await
            .unwrap();
        assert_eq!(files.content("f.txt"), "\u{feff}a\r\nB\r\n");
    }

    #[tokio::test]
    async fn an_empty_old_string_creates_or_fills_a_file() {
        let files = Files::new();
        let path = files.path("new/dir/f.txt");
        let result = files
            .edit(json!({"file_path": path, "old_string": "", "new_string": "hello\n"}))
            .await
            .unwrap();
        assert!(result.output_text().starts_with("Created"));
        assert_eq!(files.content("new/dir/f.txt"), "hello\n");
        // The session made it, so it may edit it without reading it.
        files
            .edit(json!({"file_path": path, "old_string": "hello", "new_string": "bye"}))
            .await
            .unwrap();

        let empty = files.create("empty.txt", "");
        files
            .edit(json!({"file_path": empty, "old_string": "", "new_string": "filled"}))
            .await
            .unwrap();
        assert_eq!(files.content("empty.txt"), "filled");

        let error = files
            .edit(json!({"file_path": empty, "old_string": "", "new_string": "again"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("is not"), "{error}");
    }

    #[tokio::test]
    async fn the_old_argument_shapes_are_refused() {
        let files = Files::new();
        let path = files.create("f.txt", "old\n");
        files.read(json!({"file_path": path})).await;
        for args in [
            json!({"path": path, "oldText": "old", "newText": "new"}),
            json!({"path": path, "edits": [{"oldText": "old", "newText": "new"}]}),
            json!({"file_path": path, "edits": [{"oldText": "old", "newText": "new"}]}),
        ] {
            assert!(files.edit(args).await.is_err());
        }
        let error = files
            .edit(json!({"file_path": "f.txt", "old_string": "old", "new_string": "new"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("absolute"), "{error}");
        assert_eq!(files.content("f.txt"), "old\n");
    }

    #[tokio::test]
    async fn folders_and_missing_files_are_refused() {
        let files = Files::new();
        let folder = files.dir.path().display().to_string();
        let error = files
            .edit(json!({"file_path": folder, "old_string": "a", "new_string": "b"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("folder"), "{error}");
        let error = files
            .edit(
                json!({"file_path": files.path("none.txt"), "old_string": "a", "new_string": "b"}),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("does not exist"), "{error}");
    }

    #[tokio::test]
    async fn description_and_parameters_are_placeholders() {
        let tool = EditTool::new("/".into());
        assert_eq!(tool.description(), placeholder::tool_description("Edit"));
        let parameters = tool.parameters();
        for name in ["file_path", "old_string", "new_string", "replace_all"] {
            assert_eq!(
                parameters["properties"][name]["description"],
                placeholder::tool_parameter("Edit", name)
            );
        }
        assert!(parameters["properties"].get("edits").is_none());
    }
}
