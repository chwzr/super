//! Read tool: the numbered lines of a text file within a line, size and
//! token limit, and images as attachments. Every read is recorded in the
//! session's file record, which Edit and Write consult.
//!
//! Limits:
//! - a read returns [`DEFAULT_LINES`] lines unless it sets `limit`;
//! - a read of the whole file (no `offset`, no `limit`) is refused above
//!   [`MAX_WHOLE_FILE_BYTES`];
//! - a read returns at most [`MAX_READ_TOKENS`] tokens: a read of the whole
//!   file above that returns the first part that fits and a notice, a read
//!   with `offset` or `limit` above it is refused;
//! - `allow_large` raises both limits to [`LARGE_READ_PERCENT`] percent of the
//!   context's free room, when that is more.
//!
//! Tokens are counted with [`crate::tokens::estimate_tokens_text`], about four
//! bytes of text to the token.

use crate::placeholder;
use crate::tokens::{BYTES_PER_TOKEN, estimate_tokens_text};
use crate::tool::{AgentTool, DynTool, ToolResult, ToolSession, ToolUpdateSink};
use crate::tools::arguments;
use crate::tools::files::{fingerprint, record_of, streaming_fingerprint};
use crate::tools::mutation_queue::lock_path;
use crate::tools::path::{absolute, file_key};
use crate::tools::truncate::format_size;
use base64::Engine;
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use super_ai::ContentBlock;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};
use tokio_util::sync::CancellationToken;

/// Lines a read returns when it does not set `limit`.
pub const DEFAULT_LINES: usize = 2000;
/// The largest file a read of the whole file (no `offset`, no `limit`) takes.
pub const MAX_WHOLE_FILE_BYTES: u64 = 256 * 1024;
/// The most tokens one read returns.
pub const MAX_READ_TOKENS: u64 = 25_000;
/// The share of the context's free room, in percent, that a read with
/// `allow_large` may fill.
pub const LARGE_READ_PERCENT: u64 = 70;

pub struct ReadTool {
    /// The session's folder, used only to suggest an absolute path.
    pub cwd: PathBuf,
    /// The session this tool reads for; [`AgentTool::for_session`] binds it.
    pub session: ToolSession,
}

impl ReadTool {
    /// A Read tool with a file record of its own.
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            session: ToolSession::new(),
        }
    }

    fn limits(&self, allow_large: bool) -> Limits {
        let mut tokens = MAX_READ_TOKENS;
        if allow_large && let Some(room) = self.session.context_room() {
            tokens = tokens.max(room.saturating_mul(LARGE_READ_PERCENT) / 100);
        }
        Limits {
            tokens,
            bytes: MAX_WHOLE_FILE_BYTES.max(tokens.saturating_mul(BYTES_PER_TOKEN)),
        }
    }
}

pub fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The limits that apply to one read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Limits {
    tokens: u64,
    bytes: u64,
}

/// What one call asked for.
struct Request {
    /// Lines skipped before the first line returned.
    start: usize,
    offset: Option<u64>,
    limit: Option<u64>,
}

impl Request {
    /// No `offset` and no `limit`: the whole file.
    fn whole_file(&self) -> bool {
        self.offset.is_none() && self.limit.is_none()
    }
}

/// Why a read stopped before the end of the file without being asked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cut {
    /// After [`DEFAULT_LINES`] lines.
    Lines,
    /// At the token limit.
    Tokens,
}

impl Cut {
    fn name(self) -> &'static str {
        match self {
            Cut::Lines => "lines",
            Cut::Tokens => "tokens",
        }
    }
}

/// The lines one read returns.
struct Part {
    text: String,
    /// The first and last line returned; `last < first` when none was.
    first: usize,
    last: usize,
    /// Lines in the file, when the read saw its end.
    total: Option<usize>,
    /// Whether the read returned every line of the file.
    whole: bool,
    fingerprint: Option<u64>,
    cut: Option<Cut>,
}

/// The data that follows the notice of a read that returned only the first
/// part of what it asked for. The fixed text is a placeholder.
fn part_notice(first: usize, last: usize, total: Option<usize>, next_offset: usize) -> String {
    let mut notice = placeholder::prompt_todo("Read.notice.partial");
    notice.push_str("\n<partial-read>\n");
    let _ = writeln!(notice, "<lines-shown>{first}-{last}</lines-shown>");
    if let Some(total) = total {
        let _ = writeln!(notice, "<total-lines>{total}</total-lines>");
    }
    let _ = write!(
        notice,
        "<next-offset>{next_offset}</next-offset>\n</partial-read>"
    );
    notice
}

/// Tokens kept free for the notice, so a read with one stays within its limit.
fn notice_reserve() -> u64 {
    estimate_tokens_text(&part_notice(
        usize::MAX,
        usize::MAX,
        Some(usize::MAX),
        usize::MAX,
    )) + 1
}

/// One line as the model sees it: without its line ending, a byte-order
/// mark dropped from the first line, invalid UTF-8 replaced.
fn decode_line(raw: &[u8], first: bool) -> String {
    let mut line = raw;
    if let Some(rest) = line.strip_suffix(b"\n") {
        line = rest;
        if let Some(rest) = line.strip_suffix(b"\r") {
            line = rest;
        }
    }
    if first && let Some(rest) = line.strip_prefix("\u{feff}".as_bytes()) {
        line = rest;
    }
    String::from_utf8_lossy(line).into_owned()
}

fn not_readable(path: &Path, error: std::io::Error) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        anyhow::anyhow!("{} does not exist; check the path.", path.display())
    } else {
        anyhow::anyhow!("Could not read {}: {error}.", path.display())
    }
}

async fn read_lines(
    file: tokio::fs::File,
    path: &Path,
    request: &Request,
    limits: Limits,
    cancel: &CancellationToken,
) -> anyhow::Result<Part> {
    let wanted = request.limit.map_or(DEFAULT_LINES, |limit| {
        usize::try_from(limit).unwrap_or(usize::MAX)
    });
    let budget = if request.limit.is_none() {
        limits.tokens.saturating_sub(notice_reserve())
    } else {
        limits.tokens
    };
    let first = request.start + 1;
    let mut reader = BufReader::new(file);
    let mut raw = Vec::new();
    let mut hasher = (request.start == 0).then(streaming_fingerprint);
    let mut line_number = 0usize;
    let mut text = String::new();
    let mut shown = 0usize;
    let mut more = false;
    let mut cut = None;
    loop {
        raw.clear();
        if reader.read_until(b'\n', &mut raw).await? == 0 {
            break;
        }
        if cancel.is_cancelled() {
            anyhow::bail!("Read cancelled");
        }
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(&raw);
        }
        line_number += 1;
        if line_number <= request.start {
            continue;
        }
        if shown >= wanted {
            more = true;
            if request.limit.is_none() {
                cut = Some(Cut::Lines);
            }
            break;
        }
        let mark = text.len();
        if !text.is_empty() {
            text.push('\n');
        }
        let _ = write!(
            text,
            "{line_number:>6}\t{}",
            decode_line(&raw, line_number == 1)
        );
        if estimate_tokens_text(&text) > budget {
            text.truncate(mark);
            if !request.whole_file() {
                anyhow::bail!(
                    "Reading {wanted} lines of {} from line {first} returns more than {} tokens, the most one read may return; read fewer lines with a smaller limit.",
                    path.display(),
                    limits.tokens
                );
            }
            if shown == 0 {
                anyhow::bail!(
                    "Line {line_number} of {} alone is more than {} tokens, the most one read may return; look at it with Bash, for example with head -c.",
                    path.display(),
                    limits.tokens
                );
            }
            more = true;
            cut = Some(Cut::Tokens);
            break;
        }
        shown += 1;
    }
    if shown == 0 && !more {
        anyhow::bail!(
            "{} has {line_number} lines, fewer than the offset {}; read from a smaller offset.",
            path.display(),
            request.offset.unwrap_or(1)
        );
    }
    // A read of the whole file is small enough to count the rest of its
    // lines for the notice.
    let mut total = (!more).then_some(line_number);
    if more && request.whole_file() {
        let mut rest = line_number;
        loop {
            raw.clear();
            if reader.read_until(b'\n', &mut raw).await? == 0 {
                break;
            }
            rest += 1;
        }
        total = Some(rest);
    }
    let whole = request.start == 0 && !more;
    Ok(Part {
        text,
        first,
        last: request.start + shown,
        total,
        whole,
        fingerprint: hasher.filter(|_| whole).map(|hasher| hasher.digest()),
        cut,
    })
}

#[async_trait::async_trait]
impl AgentTool for ReadTool {
    fn name(&self) -> &str {
        "Read"
    }

    fn description(&self) -> String {
        placeholder::tool_description("Read")
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": placeholder::tool_parameter("Read", "file_path"),
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 9_007_199_254_740_991_u64,
                    "description": placeholder::tool_parameter("Read", "offset"),
                },
                "limit": {
                    "type": "integer",
                    "exclusiveMinimum": 0,
                    "maximum": 9_007_199_254_740_991_u64,
                    "description": placeholder::tool_parameter("Read", "limit"),
                },
                "allow_large": {
                    "type": "boolean",
                    "description": placeholder::tool_parameter("Read", "allow_large"),
                },
            },
            "required": ["file_path"],
            "additionalProperties": false,
        })
    }

    fn prepare_arguments(&self, mut args: Value) -> Value {
        if let Some(object) = args.as_object_mut() {
            arguments::drop_nulls(object, &["offset", "limit", "allow_large"]);
            arguments::integer(object, "offset");
            arguments::integer(object, "limit");
            arguments::boolean(object, "allow_large");
        }
        args
    }

    fn for_session(&self, session: &ToolSession) -> Option<DynTool> {
        Some(Arc::new(ReadTool {
            cwd: self.cwd.clone(),
            session: session.clone(),
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
        let request = {
            let offset = args["offset"].as_u64();
            Request {
                start: offset
                    .map(|offset| usize::try_from(offset.saturating_sub(1)).unwrap_or(usize::MAX))
                    .unwrap_or(0),
                offset,
                limit: args["limit"].as_u64(),
            }
        };
        let limits = self.limits(args["allow_large"].as_bool().unwrap_or(false));
        let key = file_key(&path).await;
        let _guard = lock_path(&key).await;

        let meta = tokio::fs::metadata(&path)
            .await
            .map_err(|error| not_readable(&path, error))?;
        if meta.is_dir() {
            anyhow::bail!(
                "{} is a folder, not a file; list it with Bash, for example with ls.",
                path.display()
            );
        }
        if !meta.is_file() {
            anyhow::bail!(
                "{} is not a regular file; Read reads regular files only.",
                path.display()
            );
        }
        let mut file = tokio::fs::File::open(&path)
            .await
            .map_err(|error| not_readable(&path, error))?;
        let mut header = [0u8; 16];
        let header_len = file
            .read(&mut header)
            .await
            .map_err(|error| not_readable(&path, error))?;

        if header_len == 0 {
            if request.offset.is_some_and(|offset| offset > 1) {
                anyhow::bail!(
                    "{} is empty, so it has no line {}; read it without an offset.",
                    path.display(),
                    request.offset.unwrap_or_default()
                );
            }
            self.session
                .files
                .record(key, record_of(&meta, true, Some(fingerprint(b""))));
            return Ok(ToolResult {
                content: vec![ContentBlock::text(placeholder::prompt_todo(
                    "Read.empty_file",
                ))],
                details: json!({"wholeFile": true, "lines": {"total": 0}}),
                ..Default::default()
            });
        }

        if let Some(mime_type) = detect_image_mime(&header[..header_len]) {
            file.seek(std::io::SeekFrom::Start(0)).await?;
            let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
            file.read_to_end(&mut bytes)
                .await
                .map_err(|error| not_readable(&path, error))?;
            self.session
                .files
                .record(key, record_of(&meta, true, Some(fingerprint(&bytes))));
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            return Ok(ToolResult {
                content: vec![
                    ContentBlock::text(format!("Read image file [{mime_type}]")),
                    ContentBlock::Image {
                        data,
                        mime_type: mime_type.to_string(),
                    },
                ],
                details: json!({"wholeFile": true}),
                ..Default::default()
            });
        }

        if request.whole_file() && meta.len() > limits.bytes {
            anyhow::bail!(
                "{} is {}, more than the {} a read of the whole file may take; read a part of it with offset and limit.",
                path.display(),
                format_size(usize::try_from(meta.len()).unwrap_or(usize::MAX)),
                format_size(usize::try_from(limits.bytes).unwrap_or(usize::MAX))
            );
        }
        file.seek(std::io::SeekFrom::Start(0)).await?;
        let part = read_lines(file, &path, &request, limits, &cancel).await?;
        self.session
            .files
            .record(key, record_of(&meta, part.whole, part.fingerprint));

        let mut text = part.text;
        if part.cut.is_some() {
            text.push_str("\n\n");
            text.push_str(&part_notice(
                part.first,
                part.last,
                part.total,
                part.last + 1,
            ));
        }
        Ok(ToolResult {
            content: vec![ContentBlock::text(text)],
            details: json!({
                "wholeFile": part.whole,
                "lines": {"first": part.first, "last": part.last, "total": part.total},
                "truncatedBy": part.cut.map(Cut::name),
            }),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(dir: &Path) -> ReadTool {
        ReadTool::new(dir.to_path_buf())
    }

    fn file(dir: &Path, name: &str, content: impl AsRef<[u8]>) -> String {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path.display().to_string()
    }

    async fn read(tool: &ReadTool, args: Value) -> anyhow::Result<ToolResult> {
        let args = tool.prepare_arguments(args);
        crate::validate::validate_arguments(&tool.parameters(), &args)
            .map_err(anyhow::Error::msg)?;
        tool.execute("1", args, CancellationToken::new(), None)
            .await
    }

    fn numbered(lines: std::ops::RangeInclusive<usize>, text: impl Fn(usize) -> String) -> String {
        lines
            .map(|number| format!("{number:>6}\t{}", text(number)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn record(tool: &ReadTool, path: &str) -> Option<crate::FileRead> {
        tool.session.files.get(&file_key(Path::new(path)).await)
    }

    #[tokio::test]
    async fn lines_are_numbered_and_offset_and_limit_select_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "f.txt", "l1\nl2\r\nl3\nl4\nl5");
        let t = tool(dir.path());
        let result = read(&t, json!({"file_path": path, "offset": 2, "limit": 2}))
            .await
            .unwrap();
        assert_eq!(result.output_text(), "     2\tl2\n     3\tl3");
        assert!(!record(&t, &path).await.unwrap().whole);

        let result = read(
            &t,
            json!({"file_path": path, "offset": null, "limit": null}),
        )
        .await
        .unwrap();
        assert_eq!(
            result.output_text(),
            numbered(1..=5, |number| format!("l{number}"))
        );
        let whole = record(&t, &path).await.unwrap();
        assert!(whole.whole);
        assert_eq!(
            whole.fingerprint,
            Some(fingerprint(b"l1\nl2\r\nl3\nl4\nl5"))
        );

        // Offset 0 and 1 both start at the first line, and lenient shapes
        // are accepted.
        let result = read(&t, json!({"file_path": path, "offset": 0, "limit": "1"}))
            .await
            .unwrap();
        assert_eq!(result.output_text(), "     1\tl1");
        let result = read(&t, json!({"file_path": path, "offset": "5"}))
            .await
            .unwrap();
        assert_eq!(result.output_text(), "     5\tl5");
        assert!(!record(&t, &path).await.unwrap().whole);

        // A limited read that covers every line is a read of the whole file.
        read(&t, json!({"file_path": path, "offset": 1, "limit": 10}))
            .await
            .unwrap();
        assert!(record(&t, &path).await.unwrap().whole);
    }

    #[tokio::test]
    async fn the_path_must_be_absolute_and_named_file_path() {
        let dir = tempfile::tempdir().unwrap();
        file(dir.path(), "f.txt", "x");
        let t = tool(dir.path());
        let error = read(&t, json!({"file_path": "f.txt"})).await.unwrap_err();
        assert!(error.to_string().contains("absolute"), "{error}");
        let error = read(&t, json!({"path": dir.path().join("f.txt")}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("file_path"), "{error}");
        assert!(t.parameters()["properties"].get("pages").is_none());
        assert!(t.parameters()["properties"].get("path").is_none());
    }

    #[tokio::test]
    async fn description_and_parameters_are_placeholders() {
        let t = tool(Path::new("/"));
        assert_eq!(t.description(), placeholder::tool_description("Read"));
        let parameters = t.parameters();
        for name in ["file_path", "offset", "limit", "allow_large"] {
            assert_eq!(
                parameters["properties"][name]["description"],
                placeholder::tool_parameter("Read", name)
            );
        }
        assert_eq!(parameters["required"], json!(["file_path"]));
        assert_eq!(parameters["additionalProperties"], false);
    }

    #[tokio::test]
    async fn missing_files_folders_and_offsets_past_the_end_fail() {
        let dir = tempfile::tempdir().unwrap();
        let t = tool(dir.path());
        let missing = dir.path().join("missing.txt").display().to_string();
        let error = read(&t, json!({"file_path": missing})).await.unwrap_err();
        assert!(error.to_string().contains("does not exist"), "{error}");
        let folder = dir.path().display().to_string();
        let error = read(&t, json!({"file_path": folder})).await.unwrap_err();
        assert!(error.to_string().contains("folder"), "{error}");
        let path = file(dir.path(), "f.txt", "one\n");
        let error = read(&t, json!({"file_path": path, "offset": 10}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("has 1 lines"), "{error}");
        assert!(record(&t, &missing).await.is_none());
    }

    #[tokio::test]
    async fn an_empty_file_answers_with_a_notice() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "empty.txt", "");
        let t = tool(dir.path());
        let result = read(&t, json!({"file_path": path})).await.unwrap();
        assert_eq!(
            result.output_text(),
            placeholder::prompt_todo("Read.empty_file")
        );
        assert!(record(&t, &path).await.unwrap().whole);
    }

    #[tokio::test]
    async fn text_read_is_lossy_for_invalid_utf8_and_drops_the_byte_order_mark() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "invalid.txt", [b'a', 0xff, b'b', b'\n']);
        let result = read(&tool(dir.path()), json!({"file_path": path}))
            .await
            .unwrap();
        assert_eq!(result.output_text(), "     1\ta�b");
        let path = file(dir.path(), "bom.txt", "\u{feff}first\nsecond\n");
        let result = read(&tool(dir.path()), json!({"file_path": path}))
            .await
            .unwrap();
        assert_eq!(result.output_text(), "     1\tfirst\n     2\tsecond");
    }

    #[tokio::test]
    async fn a_whole_file_read_stops_after_two_thousand_lines_with_a_notice() {
        let dir = tempfile::tempdir().unwrap();
        let content = (1..=2500).map(|n| format!("x{n}\n")).collect::<String>();
        let path = file(dir.path(), "long.txt", &content);
        let t = tool(dir.path());
        let result = read(&t, json!({"file_path": path})).await.unwrap();
        let text = result.output_text();
        let expected = format!(
            "{}\n\n{}",
            numbered(1..=2000, |n| format!("x{n}")),
            part_notice(1, 2000, Some(2500), 2001)
        );
        assert_eq!(text, expected);
        assert!(text.contains(&placeholder::prompt_todo("Read.notice.partial")));
        assert_eq!(result.details["truncatedBy"], "lines");
        assert!(!record(&t, &path).await.unwrap().whole);

        // A read with a limit returns what it asked for, without a notice.
        let result = read(&t, json!({"file_path": path, "offset": 2001, "limit": 3}))
            .await
            .unwrap();
        assert_eq!(
            result.output_text(),
            numbered(2001..=2003, |n| format!("x{n}"))
        );
        // A limit above two thousand lines is honoured.
        let result = read(&t, json!({"file_path": path, "limit": 2500}))
            .await
            .unwrap();
        assert!(result.output_text().ends_with("  2500\tx2500"));
        assert!(record(&t, &path).await.unwrap().whole);
    }

    #[tokio::test]
    async fn a_whole_file_above_256_kb_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let line = format!("{}\n", "a".repeat(99));
        let path = file(dir.path(), "big.txt", line.repeat(2700));
        let t = tool(dir.path());
        let error = read(&t, json!({"file_path": path})).await.unwrap_err();
        let error = error.to_string();
        assert!(error.contains("256.0KB"), "{error}");
        assert!(error.contains("offset and limit"), "{error}");
        assert!(record(&t, &path).await.is_none());
        // A part of the same file can be read.
        let result = read(&t, json!({"file_path": path, "offset": 10, "limit": 2}))
            .await
            .unwrap();
        assert_eq!(result.output_text().lines().count(), 2);
    }

    #[tokio::test]
    async fn a_whole_file_above_the_token_limit_returns_its_first_part() {
        let dir = tempfile::tempdir().unwrap();
        // 1,000 lines of 200 bytes: about 52,000 tokens with the numbers,
        // under 256 KB.
        let content = (1..=1000)
            .map(|n| format!("{n:0>200}\n"))
            .collect::<String>();
        let path = file(dir.path(), "wide.txt", &content);
        let t = tool(dir.path());
        let result = read(&t, json!({"file_path": path})).await.unwrap();
        let text = result.output_text();
        assert!(
            estimate_tokens_text(&text) <= MAX_READ_TOKENS,
            "{}",
            text.len()
        );
        let last = result.details["lines"]["last"].as_u64().unwrap() as usize;
        assert!((400..1000).contains(&last), "{last}");
        assert!(text.ends_with(&part_notice(1, last, Some(1000), last + 1)));
        assert_eq!(result.details["truncatedBy"], "tokens");
        assert!(!record(&t, &path).await.unwrap().whole);

        // A read with offset or limit above the token limit is refused.
        let error = read(&t, json!({"file_path": path, "limit": 1000}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("25000 tokens"), "{error}");
        let error = read(&t, json!({"file_path": path, "offset": 2}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("smaller limit"), "{error}");
    }

    #[tokio::test]
    async fn a_single_line_above_the_token_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "minified.js", "x".repeat(120_000));
        let error = read(&tool(dir.path()), json!({"file_path": path}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Line 1"), "{error}");
    }

    #[tokio::test]
    async fn allow_large_raises_both_limits_to_seventy_percent_of_the_free_room() {
        let dir = tempfile::tempdir().unwrap();
        let line = format!("{}\n", "b".repeat(99));
        // 300 KB: over 256 KB and over 25,000 tokens.
        let path = file(dir.path(), "large.txt", line.repeat(3000));
        let mut t = tool(dir.path());
        t.session.context_room = Some(Arc::new(|| Some(200_000)));
        assert_eq!(
            t.limits(true),
            Limits {
                tokens: 140_000,
                bytes: 560_000
            }
        );
        assert_eq!(
            t.limits(false),
            Limits {
                tokens: MAX_READ_TOKENS,
                bytes: MAX_WHOLE_FILE_BYTES
            }
        );
        let result = read(
            &t,
            json!({"file_path": path, "allow_large": true, "limit": 3000}),
        )
        .await
        .unwrap();
        let last_line = format!("  3000\t{}", "b".repeat(99));
        assert!(result.output_text().ends_with(&last_line));
        assert!(record(&t, &path).await.unwrap().whole);
        let error = read(&t, json!({"file_path": path, "limit": 3000}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("tokens"), "{error}");

        // A whole-file read over 256 KB now passes, and its first 2,000
        // lines fit.
        let result = read(&t, json!({"file_path": path, "allow_large": "true"}))
            .await
            .unwrap();
        assert_eq!(result.details["truncatedBy"], "lines");

        // allow_large never lowers a limit, and without a session it keeps
        // the defaults.
        t.session.context_room = Some(Arc::new(|| Some(1_000)));
        assert_eq!(t.limits(true).tokens, MAX_READ_TOKENS);
        t.session.context_room = None;
        assert_eq!(t.limits(true).tokens, MAX_READ_TOKENS);
    }

    #[tokio::test]
    async fn png_detected_as_image() {
        let dir = tempfile::tempdir().unwrap();
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
        let path = file(dir.path(), "i.png", png);
        let t = tool(dir.path());
        let r = read(&t, json!({"file_path": path})).await.unwrap();
        assert!(
            matches!(&r.content[1], ContentBlock::Image { mime_type, .. } if mime_type == "image/png")
        );
        assert!(record(&t, &path).await.unwrap().whole);
    }

    #[tokio::test]
    async fn binary_files_and_notebooks_are_read_as_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "notes.ipynb", "{\n \"cells\": []\n}\n");
        let result = read(&tool(dir.path()), json!({"file_path": path}))
            .await
            .unwrap();
        assert_eq!(
            result.output_text(),
            "     1\t{\n     2\t \"cells\": []\n     3\t}"
        );
        let path = file(dir.path(), "data.bin", [0u8, 1, 2, b'\n', 3]);
        let result = read(&tool(dir.path()), json!({"file_path": path}))
            .await
            .unwrap();
        assert!(result.output_text().starts_with("     1\t\u{0}\u{1}\u{2}"));
    }

    #[tokio::test]
    async fn a_bound_copy_records_into_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "f.txt", "x\n");
        let unbound = tool(dir.path());
        let session = ToolSession::new();
        let bound = unbound.for_session(&session).unwrap();
        bound
            .execute(
                "1",
                json!({"file_path": path}),
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(session.files.len(), 1);
        assert!(unbound.session.files.is_empty());
    }

    #[tokio::test]
    #[ignore = "release-mode performance benchmark"]
    async fn benchmark_performance_bounded_text_read() {
        let dir = tempfile::tempdir().unwrap();
        let line = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n";
        let path = file(dir.path(), "large.txt", line.repeat(200_000));
        let tool = tool(dir.path());
        let args = json!({"file_path": path, "offset": 190_000, "limit": 50});
        let head_args = json!({"file_path": path, "offset": 1, "limit": 50});

        for _ in 0..2 {
            tool.execute("warmup", args.clone(), CancellationToken::new(), None)
                .await
                .unwrap();
        }
        let mut samples = Vec::with_capacity(11);
        for _ in 0..11 {
            let started = std::time::Instant::now();
            let result = tool
                .execute("bench", args.clone(), CancellationToken::new(), None)
                .await
                .unwrap();
            std::hint::black_box(result.output_text().len());
            samples.push(started.elapsed().as_nanos());
        }
        super_bench::report(
            "read_offset_13mb",
            &mut samples,
            1,
            "13mb_file_offset_190000_limit_50",
        );

        let mut head_samples = Vec::with_capacity(11);
        for _ in 0..11 {
            let started = std::time::Instant::now();
            let result = tool
                .execute("bench", head_args.clone(), CancellationToken::new(), None)
                .await
                .unwrap();
            std::hint::black_box(result.output_text().len());
            head_samples.push(started.elapsed().as_nanos());
        }
        super_bench::report(
            "read_head_13mb",
            &mut head_samples,
            1,
            "13mb_file_offset_1_limit_50",
        );
    }
}
