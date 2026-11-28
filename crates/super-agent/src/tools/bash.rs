//! Bash tool: run a shell command from the user's profile, capture merged
//! stdout and stderr, stream partial output (throttled), honor a timeout and
//! cancellation by killing the process group, or leave the command running
//! in the background as a task of the session.
//!
//! Rules:
//! - `timeout` is in milliseconds: [`DEFAULT_TIMEOUT_MS`] by default and
//!   [`MAX_TIMEOUT_MS`] at most; in the background [`DEFAULT_BACKGROUND_TIMEOUT_MS`]
//!   and [`MAX_BACKGROUND_TIMEOUT_MS`]. A longer one is cut to the most.
//! - A command that is nothing but a `sleep` is refused in the foreground.
//! - In the main session a `cd` carries over to the next command while it
//!   stays inside the session folder; a command that ends outside it puts the
//!   shell back in the session folder. Everywhere else every command starts
//!   in the session folder.
//! - Up to [`MAX_INLINE_CHARS`] characters of output come back with the
//!   result. Longer output is saved to a file, and the result names the file
//!   and shows its first [`PREVIEW_BYTES`] bytes.
//! - An exit code other than 0 is an error that starts with `Exit code N`,
//!   except 1 for a search or comparison command
//!   ([`command_rules::is_error_exit`]).
//! - `run_in_background` registers the command as a task of the session
//!   ([`crate::TaskHost`]); its output goes to the task's output file, and the
//!   agent gets a notice when it ends or reaches its time limit.
//! - `dangerouslyDisableSandbox` is accepted and has no effect: there is no
//!   sandbox.

use crate::background::{HostedTask, TaskEnd, TaskEndStatus};
use crate::placeholder;
use crate::tool::{AgentTool, DynTool, ExecutionMode, ToolResult, ToolSession, ToolUpdateSink};
use crate::tools::arguments;
use crate::tools::command_rules;
use crate::tools::shell::{self, ShellKind, ShellProcess};
use crate::tools::truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, truncate_tail};
use serde_json::{Value, json};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use super_ai::ContentBlock;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

/// The time limit of a command in the foreground when `timeout` is not set.
pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// The longest time limit a command in the foreground can have.
pub const MAX_TIMEOUT_MS: u64 = 600_000;
/// The time limit of a command in the background when `timeout` is not set.
pub const DEFAULT_BACKGROUND_TIMEOUT_MS: u64 = 600_000;
/// The longest time limit a command in the background can have.
pub const MAX_BACKGROUND_TIMEOUT_MS: u64 = 7_200_000;
/// The most characters of output that come back with the result.
pub const MAX_INLINE_CHARS: usize = 30_000;
/// How much of a longer output the result shows.
pub const PREVIEW_BYTES: usize = 2_000;

/// Output kept in memory before it goes to a file. More bytes than this are
/// more than [`MAX_INLINE_CHARS`] characters, whatever they encode.
const MEMORY_BYTES: usize = MAX_INLINE_CHARS * 4;
/// The end of the output kept for the live view.
const UPDATE_TAIL_BYTES: usize = 2 * DEFAULT_MAX_BYTES;
const UPDATE_THROTTLE: Duration = Duration::from_millis(100);

pub struct BashTool {
    /// The session folder: where commands start, and the folder a `cd`
    /// must stay inside to carry over.
    pub cwd: PathBuf,
    /// Shell binary, selected when the tool is constructed unless overridden.
    pub shell_path: Option<String>,
    /// Prefix prepended to every command (settings shellCommandPrefix).
    pub command_prefix: Option<String>,
    /// The session this tool runs for; [`AgentTool::for_session`] binds it.
    pub session: ToolSession,
}

impl BashTool {
    pub fn new(cwd: PathBuf) -> Self {
        BashTool {
            cwd,
            shell_path: Some(shell::resolve(None).to_string_lossy().into_owned()),
            command_prefix: None,
            session: ToolSession::new(),
        }
    }

    /// The folder the next command starts in, and whether the shell had to
    /// go back to the session folder because the folder it was in is gone.
    fn start_directory(&self) -> (PathBuf, bool) {
        if let Some(shell) = &self.session.shell
            && let Some(directory) = shell.get()
        {
            if directory.is_dir() {
                return (directory, false);
            }
            shell.reset();
            return (self.cwd.clone(), true);
        }
        (self.cwd.clone(), false)
    }

    /// Follow the folder a command ended in, which the shell wrote to
    /// `report`. Returns whether the shell went back to the session folder.
    fn follow_directory(&self, report: &Path) -> bool {
        let Some(shell) = &self.session.shell else {
            return false;
        };
        let Ok(text) = std::fs::read_to_string(report) else {
            return false;
        };
        let reported = text.trim_end_matches(['\n', '\r']);
        if reported.is_empty() {
            // The command replaced the exit trap or ended without it.
            return false;
        }
        let ended = PathBuf::from(reported);
        let root = std::fs::canonicalize(&self.cwd).unwrap_or_else(|_| self.cwd.clone());
        if ended == root {
            shell.reset();
            false
        } else if ended.starts_with(&root) {
            shell.set(Some(ended));
            false
        } else {
            shell.reset();
            true
        }
    }

    fn back_in_session_folder(&self) -> String {
        format!(
            "The shell is back in {}, because a cd carries over only inside that folder.",
            self.cwd.display()
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_in_foreground(
        &self,
        tool_call_id: &str,
        command: &str,
        timeout_ms: u64,
        directory: &Path,
        mut went_back: bool,
        cancel: CancellationToken,
        on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        // Only a shell whose `cd` carries over reports where it ended: a
        // POSIX shell or fish, on Unix.
        let carries_over = matches!(
            shell::kind(&self.shell()),
            ShellKind::Posix | ShellKind::Fish
        );
        let report = match &self.session.shell {
            Some(_) if cfg!(unix) && carries_over => Some(
                tempfile::Builder::new()
                    .prefix("super-cwd-")
                    .tempfile()
                    .map_err(|e| anyhow::anyhow!("Could not create a temporary file: {e}."))?,
            ),
            _ => None,
        };
        let mut cmd = shell::script(
            self.shell_path.as_deref(),
            command,
            self.command_prefix.as_deref(),
            directory,
            report.as_ref().map(|file| file.path()),
        );
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut process = ShellProcess::start(cmd)?;
        let mut stdout = process.child.stdout.take().expect("piped stdout");
        let mut stderr = process.child.stderr.take().expect("piped stderr");
        let mut capture = Capture::new(tool_call_id);
        let mut stdout_buf = [0u8; 8192];
        let mut stderr_buf = [0u8; 8192];
        let mut stdout_open = true;
        let mut stderr_open = true;
        let mut last_update = std::time::Instant::now() - UPDATE_THROTTLE;
        let mut cancelled = false;
        let mut timed_out = false;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);

        loop {
            tokio::select! {
                n = stdout.read(&mut stdout_buf), if stdout_open => {
                    match n {
                        Ok(0) | Err(_) => stdout_open = false,
                        Ok(n) => capture.push(&stdout_buf[..n]),
                    }
                }
                n = stderr.read(&mut stderr_buf), if stderr_open => {
                    match n {
                        Ok(0) | Err(_) => stderr_open = false,
                        Ok(n) => capture.push(&stderr_buf[..n]),
                    }
                }
                _ = cancel.cancelled() => {
                    cancelled = true;
                    process.kill();
                    break;
                }
                _ = tokio::time::sleep_until(deadline) => {
                    timed_out = true;
                    process.kill();
                    break;
                }
            }
            // The cancellation and deadline futures stay pending, so
            // `select!` never runs out of branches: stop once both output
            // pipes are closed.
            if !stdout_open && !stderr_open {
                break;
            }
            if let Some(update) = &on_update
                && last_update.elapsed() >= UPDATE_THROTTLE
            {
                last_update = std::time::Instant::now();
                update(ToolResult::text(capture.live_view()));
            }
        }

        let status = process.child.wait().await.ok();
        let exit_code = status.and_then(exit_code);
        if !cancelled
            && !timed_out
            && let Some(report) = &report
        {
            went_back |= self.follow_directory(report.path());
        }

        let saved = capture.finish();
        let mut output = saved.text;
        if went_back {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&self.back_in_session_folder());
        }
        let mut details = json!({"exitCode": exit_code});
        if let Some(path) = &saved.file {
            details["fullOutputPath"] = path.display().to_string().into();
        }
        let with_status = |status: &str| {
            if output.is_empty() {
                status.to_string()
            } else {
                format!("{output}\n\n{status}")
            }
        };
        if cancelled {
            anyhow::bail!("{}", with_status("Command aborted"));
        }
        if timed_out {
            anyhow::bail!(
                "{}",
                with_status(&format!(
                    "The command timed out after {timeout_ms} ms; give a longer timeout (at most {MAX_TIMEOUT_MS}) or set run_in_background."
                ))
            );
        }
        if let Some(code) = exit_code
            && command_rules::is_error_exit(command, code)
        {
            let text = if output.is_empty() {
                format!("Exit code {code}")
            } else {
                format!("Exit code {code}\n{output}")
            };
            return Ok(ToolResult {
                content: vec![ContentBlock::text(text)],
                details,
                is_error: true,
                ..Default::default()
            });
        }
        Ok(ToolResult {
            content: vec![ContentBlock::text(if output.is_empty() {
                "(no output)".to_string()
            } else {
                output
            })],
            details,
            ..Default::default()
        })
    }

    async fn start_in_background(
        &self,
        tool_call_id: &str,
        command: &str,
        description: Option<&str>,
        timeout_ms: u64,
        directory: &Path,
        went_back: bool,
    ) -> anyhow::Result<ToolResult> {
        let host = self.session.tasks.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "run_in_background needs a session that keeps background tasks; run the command without it."
            )
        })?;
        let label = description
            .map(str::to_string)
            .unwrap_or_else(|| command_label(command));
        // A task dropped before it finishes ends without a notice, so every
        // early return below leaves nothing behind.
        let task = host
            .start_shell(&label, tool_call_id)
            .map_err(|e| anyhow::anyhow!(e))?;
        let id = task.id().to_string();
        let output = task.output_file().to_path_buf();
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&output)
            .map_err(|e| anyhow::anyhow!("Could not open {}: {e}.", output.display()))?;
        let errors = file
            .try_clone()
            .map_err(|e| anyhow::anyhow!("Could not open {}: {e}.", output.display()))?;
        let mut cmd = shell::script(
            self.shell_path.as_deref(),
            command,
            self.command_prefix.as_deref(),
            directory,
            None,
        );
        cmd.stdout(Stdio::from(file)).stderr(Stdio::from(errors));
        let process = ShellProcess::start(cmd)?;
        tokio::spawn(watch_background(
            process,
            task,
            timeout_ms,
            command.to_string(),
            label,
        ));
        let mut text = format!(
            "The command runs in the background as task {id}. Its output is written to {}.",
            output.display()
        );
        if went_back {
            text.push('\n');
            text.push_str(&self.back_in_session_folder());
        }
        Ok(ToolResult {
            content: vec![ContentBlock::text(text)],
            details: json!({"backgroundTaskId": id, "outputFile": output.display().to_string()}),
            ..Default::default()
        })
    }

    fn shell(&self) -> PathBuf {
        shell::resolve(self.shell_path.as_deref())
    }
}

/// The time limit of one call in milliseconds: `timeout` when it is given,
/// cut to the most the call may have, else the default.
pub fn time_limit(timeout: Option<f64>, background: bool) -> anyhow::Result<u64> {
    let (default, most) = if background {
        (DEFAULT_BACKGROUND_TIMEOUT_MS, MAX_BACKGROUND_TIMEOUT_MS)
    } else {
        (DEFAULT_TIMEOUT_MS, MAX_TIMEOUT_MS)
    };
    match timeout {
        None => Ok(default),
        Some(timeout) if timeout.is_finite() && timeout > 0.0 => {
            Ok((timeout.ceil() as u64).min(most))
        }
        Some(_) => anyhow::bail!(
            "timeout must be a positive number of milliseconds, at most {most}; leave it out for {default}."
        ),
    }
}

/// The task description of a command started without one: its first line,
/// cut to 80 characters.
fn command_label(command: &str) -> String {
    let line = command.trim().lines().next().unwrap_or_default().trim();
    if line.chars().count() > 80 {
        format!("{}…", line.chars().take(79).collect::<String>())
    } else {
        line.to_string()
    }
}

/// The exit code of a shell, with a signal counted as the shell counts it,
/// 128 plus its number.
fn exit_code(status: std::process::ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return Some(128 + signal);
        }
    }
    status.code()
}

/// Wait for a command in the background and end its task: with a notice
/// when it exits or reaches its time limit, without one when the task was
/// stopped from outside.
async fn watch_background(
    mut process: ShellProcess,
    task: Box<dyn HostedTask>,
    timeout_ms: u64,
    command: String,
    label: String,
) {
    let stop = task.stop_signal();
    tokio::select! {
        status = process.child.wait() => {
            let code = status.ok().and_then(exit_code);
            let end = match code {
                Some(code) => TaskEnd {
                    status: if command_rules::is_error_exit(&command, code) {
                        TaskEndStatus::Failed
                    } else {
                        TaskEndStatus::Completed
                    },
                    summary: format!("{label} (exit code {code})"),
                },
                None => TaskEnd {
                    status: TaskEndStatus::Failed,
                    summary: format!("{label} (no exit code)"),
                },
            };
            task.finish(end);
        }
        _ = stop.cancelled() => {
            process.kill();
            let _ = process.child.wait().await;
        }
        _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => {
            process.kill();
            let _ = process.child.wait().await;
            task.finish(TaskEnd {
                status: TaskEndStatus::Stopped,
                summary: format!("{label} (stopped at its time limit of {timeout_ms} ms)"),
            });
        }
    }
}

/// The output of a command in the foreground: in memory while it is short,
/// in a file once it is longer than a result may be.
struct Capture {
    /// The tool call, for the file's name.
    tool_call_id: String,
    /// All output while it fits in memory; once it is in a file, its start.
    head: Vec<u8>,
    total: usize,
    file: Option<(PathBuf, std::fs::File)>,
    /// The end of the output, for the live view.
    recent: Vec<u8>,
}

/// What a result carries of the output.
struct Saved {
    text: String,
    file: Option<PathBuf>,
}

impl Capture {
    fn new(tool_call_id: &str) -> Self {
        Capture {
            tool_call_id: tool_call_id.to_string(),
            head: Vec::new(),
            total: 0,
            file: None,
            recent: Vec::new(),
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len();
        self.recent.extend_from_slice(bytes);
        if self.recent.len() > 2 * UPDATE_TAIL_BYTES {
            let cut = self.recent.len() - UPDATE_TAIL_BYTES;
            self.recent.drain(..cut);
        }
        match &mut self.file {
            Some((_, file)) => {
                let _ = file.write_all(bytes);
            }
            None => {
                self.head.extend_from_slice(bytes);
                if self.head.len() > MEMORY_BYTES
                    && let Ok(mut saved) = self.create_file()
                    && saved.1.write_all(&self.head).is_ok()
                {
                    self.head.truncate(PREVIEW_BYTES + 4);
                    self.file = Some(saved);
                }
            }
        }
    }

    /// The end of the output as the live view shows it.
    fn live_view(&self) -> String {
        truncate_tail(
            &String::from_utf8_lossy(&self.recent),
            DEFAULT_MAX_LINES,
            DEFAULT_MAX_BYTES,
        )
        .content
    }

    fn create_file(&self) -> std::io::Result<(PathBuf, std::fs::File)> {
        let directory = std::env::temp_dir().join("super-bash");
        std::fs::create_dir_all(&directory)?;
        let name: String = self
            .tool_call_id
            .chars()
            .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
            .take(64)
            .collect();
        let (file, path) = tempfile::Builder::new()
            .prefix(&format!("output-{}-{name}-", super_ai::now_ms()))
            .suffix(".txt")
            .tempfile_in(directory)?
            .keep()
            .map_err(|error| error.error)?;
        Ok((path, file))
    }

    fn finish(mut self) -> Saved {
        if self.file.is_none() {
            let text = String::from_utf8_lossy(&self.head);
            let text = text.trim_end_matches(['\n', '\r']);
            if text.chars().count() <= MAX_INLINE_CHARS {
                return Saved {
                    text: text.to_string(),
                    file: None,
                };
            }
            let saved = self.create_file().and_then(|mut saved| {
                saved.1.write_all(&self.head)?;
                Ok(saved)
            });
            match saved {
                Ok(saved) => self.file = Some(saved),
                Err(_) => {
                    // Nowhere to put it: return what fits.
                    let shown: String = text.chars().take(MAX_INLINE_CHARS).collect();
                    return Saved {
                        text: shown,
                        file: None,
                    };
                }
            }
        }
        let (path, mut file) = self.file.take().expect("the output is in a file");
        let _ = file.flush();
        let mut end = self.head.len().min(PREVIEW_BYTES);
        while end > 0 && end < self.head.len() && (self.head[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        let preview = String::from_utf8_lossy(&self.head[..end]);
        Saved {
            text: large_output_notice(&path, self.total, &preview),
            file: Some(path),
        }
    }
}

/// What a result says of output too long to return: the notice, whose fixed
/// text is a placeholder, and the file, size and preview.
fn large_output_notice(path: &Path, total_bytes: usize, preview: &str) -> String {
    format!(
        "{}\n<large-output>\n<output-file>{}</output-file>\n<total-bytes>{total_bytes}</total-bytes>\n<preview>\n{preview}\n</preview>\n</large-output>",
        placeholder::prompt_todo("Bash.notice.large_output"),
        path.display(),
    )
}

#[async_trait::async_trait]
impl AgentTool for BashTool {
    fn name(&self) -> &str {
        "Bash"
    }
    /// A command may change files and move the shell with `cd`, so it
    /// runs alone, in its place among the calls of one answer: the call
    /// before it has ended, and the call after it sees what it did.
    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Sequential
    }

    /// One placeholder per shell family, so the text can name the syntax of
    /// the shell the commands run in.
    fn description(&self) -> String {
        match shell::kind(&self.shell()) {
            ShellKind::Pwsh => placeholder::prompt_todo("Bash.description.pwsh"),
            ShellKind::PowerShell => placeholder::prompt_todo("Bash.description.powershell"),
            ShellKind::Cmd => placeholder::prompt_todo("Bash.description.cmd"),
            _ => placeholder::tool_description("Bash"),
        }
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": placeholder::tool_parameter("Bash", "command"),
                },
                "timeout": {
                    "type": "number",
                    "description": placeholder::tool_parameter("Bash", "timeout"),
                },
                "description": {
                    "type": "string",
                    "description": placeholder::tool_parameter("Bash", "description"),
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": placeholder::tool_parameter("Bash", "run_in_background"),
                },
                "dangerouslyDisableSandbox": {
                    "type": "boolean",
                    "description": placeholder::tool_parameter("Bash", "dangerouslyDisableSandbox"),
                },
            },
            "required": ["command"],
            "additionalProperties": false,
        })
    }

    fn prepare_arguments(&self, mut args: Value) -> Value {
        if let Some(object) = args.as_object_mut() {
            arguments::drop_nulls(
                object,
                &[
                    "timeout",
                    "description",
                    "run_in_background",
                    "dangerouslyDisableSandbox",
                ],
            );
            if let Some(Value::String(text)) = object.get("timeout")
                && let Ok(number) = text.trim().parse::<f64>()
                && let Some(number) = serde_json::Number::from_f64(number)
            {
                object.insert("timeout".into(), Value::Number(number));
            }
            arguments::boolean(object, "run_in_background");
            arguments::boolean(object, "dangerouslyDisableSandbox");
        }
        args
    }

    fn for_session(&self, session: &ToolSession) -> Option<DynTool> {
        Some(Arc::new(BashTool {
            cwd: self.cwd.clone(),
            shell_path: self.shell_path.clone(),
            command_prefix: self.command_prefix.clone(),
            session: session.clone(),
        }))
    }

    fn in_directory(&self, cwd: &Path) -> Option<DynTool> {
        Some(Arc::new(BashTool {
            cwd: cwd.to_path_buf(),
            shell_path: self.shell_path.clone(),
            command_prefix: self.command_prefix.clone(),
            session: self.session.clone(),
        }))
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        args: Value,
        cancel: CancellationToken,
        on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        let command = args["command"].as_str().unwrap_or_default().to_string();
        if command.trim().is_empty() {
            anyhow::bail!("command is empty; give the command to run.");
        }
        let background = args["run_in_background"].as_bool().unwrap_or(false);
        let timeout_ms = time_limit(args["timeout"].as_f64(), background)?;
        if !background && command_rules::is_lone_sleep(&command) {
            anyhow::bail!(
                "A command that only sleeps is not run in the foreground; set run_in_background to wait in the background."
            );
        }
        let description = args["description"]
            .as_str()
            .map(str::trim)
            .filter(|description| !description.is_empty());
        let (directory, went_back) = self.start_directory();
        if background {
            return self
                .start_in_background(
                    tool_call_id,
                    &command,
                    description,
                    timeout_ms,
                    &directory,
                    went_back,
                )
                .await;
        }
        self.run_in_foreground(
            tool_call_id,
            &command,
            timeout_ms,
            &directory,
            went_back,
            cancel,
            on_update,
        )
        .await
    }
}

#[cfg(test)]
#[path = "bash_tests.rs"]
mod tests;
