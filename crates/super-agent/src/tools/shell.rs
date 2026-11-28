//! Command arguments shared by the agent, SDK, and terminal shell runners.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::{Child, Command};

/// The family a shell belongs to, which decides how a command is handed to
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// bash, zsh, sh, ksh, dash and their relatives.
    Posix,
    Fish,
    /// PowerShell 7.
    Pwsh,
    /// Windows PowerShell 5.1.
    PowerShell,
    Cmd,
    Other,
}

/// The family of the shell at `shell`, judged by its file name.
pub fn kind(shell: &Path) -> ShellKind {
    let name = shell
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    match name.as_str() {
        "bash" | "sh" | "zsh" | "ksh" | "mksh" | "dash" | "ash" | "yash" => ShellKind::Posix,
        "fish" => ShellKind::Fish,
        "pwsh" => ShellKind::Pwsh,
        "powershell" => ShellKind::PowerShell,
        "cmd" => ShellKind::Cmd,
        _ => ShellKind::Other,
    }
}

/// `text` in single quotes for a POSIX shell.
pub fn posix_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `text` in single quotes for fish, where a backslash and a single quote
/// are escaped with a backslash inside them.
pub fn fish_quote(text: &str) -> String {
    format!("'{}'", text.replace('\\', r"\\").replace('\'', r"\'"))
}

/// How `Bash` and `Monitor` start a command: in `directory`, and for a POSIX
/// shell or fish as a login shell, so the shell starts from the user's
/// profile. Both change to `directory` again after the profile ran, in case
/// the profile changed folder, and with `report_directory` write the folder
/// the command ends in to that file: a POSIX shell when the command is done
/// and with an `EXIT` trap for a command that exits, fish with a handler of
/// its `fish_exit` event. A POSIX shell runs the command with `eval`, so
/// that a syntax error in it is reported when it runs, with the command's
/// own line numbers. PowerShell and cmd start as [`command`] starts them.
///
/// On Unix what the profile writes is left out: the script first writes
/// [`OUTPUT_MARK`] to the output and to the errors, and [`ProfileOutput`]
/// drops everything up to it. `output_file`, the file a background command
/// writes both to, is emptied after the mark instead. The shell keeps its
/// real output and errors while the profile runs, so a job the profile
/// starts with its own output sent elsewhere holds nothing of the command's
/// open.
pub fn script(
    shell_path: Option<&str>,
    source: &str,
    prefix: Option<&str>,
    directory: &Path,
    report_directory: Option<&Path>,
    output_file: Option<&Path>,
) -> Command {
    let shell = resolve(shell_path);
    let marks = cfg!(unix);
    let mut command = match kind(&shell) {
        ShellKind::Posix => {
            // Everything before the command is on its first line, so that
            // bash, which counts the lines of an `eval` from the line it is
            // on, counts the command's own.
            let mut text = String::new();
            if marks {
                text.push_str(&format!(
                    "printf '{MARK_FORMAT}'; printf '{MARK_FORMAT}' >&2; "
                ));
                if let Some(file) = output_file {
                    text.push_str(&format!(": >| {}; ", posix_quote(&file.to_string_lossy())));
                }
            }
            text.push_str(&format!(
                "cd -- {} || exit 1; ",
                posix_quote(&directory.to_string_lossy())
            ));
            let report = report_directory.map(|file| {
                format!(
                    "pwd -P 2>/dev/null >| {} || :",
                    posix_quote(&file.to_string_lossy())
                )
            });
            if let Some(report) = &report {
                text.push_str(&format!("trap {} EXIT; ", posix_quote(report)));
            }
            if let Some(prefix) = prefix {
                text.push_str(&format!("eval {}; ", posix_quote(prefix)));
            }
            text.push_str(&format!("eval {}", posix_quote(source)));
            if let Some(report) = &report {
                // The command may have replaced the trap: report here too,
                // and end with the command's status.
                text.push_str(&format!("\nset -- \"$?\"\n{report}\n(exit \"$1\")"));
            }
            login_shell(&shell, text)
        }
        ShellKind::Fish => {
            let mut text = String::new();
            if marks {
                text.push_str(&format!(
                    "printf '{MARK_FORMAT}'; printf '{MARK_FORMAT}' >&2\n"
                ));
                if let Some(file) = output_file {
                    text.push_str(&format!("true > {}\n", fish_quote(&file.to_string_lossy())));
                }
            }
            text.push_str(&format!(
                "builtin cd {}; or exit 1\n",
                fish_quote(&directory.to_string_lossy())
            ));
            if let Some(file) = report_directory {
                text.push_str(&format!(
                    "function __super_report_folder --on-event fish_exit\n    builtin pwd -P 2>/dev/null > {}\nend\n",
                    fish_quote(&file.to_string_lossy())
                ));
            }
            if let Some(prefix) = prefix {
                text.push_str(prefix);
                text.push('\n');
            }
            text.push_str(source);
            login_shell(&shell, text)
        }
        _ => self::command(shell_path, source, prefix),
    };
    command.current_dir(directory);
    command
}

/// What a shell started by [`script`] writes on Unix, to its output and to
/// its errors, before it runs anything of the command: what comes before it
/// is the profile's.
pub const OUTPUT_MARK: &[u8] = b"\x1esuper-output\x1e";

/// [`OUTPUT_MARK`] as a `printf` format, so that the script's text does not
/// hold the mark itself (a profile that traces commands prints the text).
const MARK_FORMAT: &str = r"\036super-output\036";

/// Whether the output and errors of a command [`script`] starts with the
/// shell at `shell_path` begin with what the profile wrote and
/// [`OUTPUT_MARK`].
pub fn marks_output(shell_path: Option<&str>) -> bool {
    cfg!(unix)
        && matches!(
            kind(&resolve(shell_path)),
            ShellKind::Posix | ShellKind::Fish
        )
}

/// One output stream of a command [`script`] started, with what the
/// profile wrote before [`OUTPUT_MARK`] left out.
#[derive(Debug, Default)]
pub struct ProfileOutput {
    /// What came before the mark, while it has not come.
    held: Option<Vec<u8>>,
}

impl ProfileOutput {
    /// A stream that starts with the mark when `marked`
    /// ([`marks_output`]), else one that passes everything.
    pub fn new(marked: bool) -> Self {
        ProfileOutput {
            held: marked.then(Vec::new),
        }
    }

    /// The part of `bytes` that is the command's.
    pub fn feed<'a>(&mut self, bytes: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
        let Some(held) = &mut self.held else {
            return std::borrow::Cow::Borrowed(bytes);
        };
        let from = held.len().saturating_sub(OUTPUT_MARK.len() - 1);
        held.extend_from_slice(bytes);
        match held[from..]
            .windows(OUTPUT_MARK.len())
            .position(|window| window == OUTPUT_MARK)
        {
            Some(at) => {
                let rest = held[from + at + OUTPUT_MARK.len()..].to_vec();
                self.held = None;
                std::borrow::Cow::Owned(rest)
            }
            None => std::borrow::Cow::Borrowed(&[]),
        }
    }

    /// At the end of the stream: what was held back because the mark never
    /// came, which is what the shell wrote before it could run the script,
    /// such as an error about its profile.
    pub fn finish(&mut self) -> Vec<u8> {
        self.held.take().unwrap_or_default()
    }
}

/// `shell` as a login shell running `text`.
fn login_shell(shell: &Path, text: String) -> Command {
    let mut command = Command::new(shell);
    command.args(["-l", "-c"]).arg(text);
    command
}

/// The error for a shell that could not start in `folder`: the folder is
/// gone, or `shell` is not installed.
fn start_error(folder: Option<&Path>, shell: &str, error: std::io::Error) -> anyhow::Error {
    if let Some(folder) = folder
        && !folder.is_dir()
    {
        return anyhow::anyhow!(
            "The folder {} does not exist, so the command cannot start there.",
            folder.display()
        );
    }
    anyhow::anyhow!(
        "Could not start shell {shell}: {error}. Install Bash or set shellPath to an installed Bash, PowerShell, or cmd.exe executable."
    )
}

/// The shell `command` starts.
fn shell_of(command: &Command) -> String {
    command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned()
}

/// A shell process started in a process group of its own, so that stopping
/// it also stops everything it started.
pub struct ShellProcess {
    pub child: Child,
    group: Option<i32>,
}

impl ShellProcess {
    /// Start `command` with no input. Dropping the process kills it.
    pub fn spawn(mut command: Command) -> std::io::Result<Self> {
        command.stdin(Stdio::null()).kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn()?;
        #[cfg(unix)]
        let group = child.id().and_then(|pid| i32::try_from(pid).ok());
        #[cfg(not(unix))]
        let group = None;
        Ok(ShellProcess { child, group })
    }

    /// [`spawn`](Self::spawn), with an error that says what is missing:
    /// the folder the command starts in, or the shell.
    pub fn start(command: Command) -> anyhow::Result<Self> {
        let folder = command.as_std().get_current_dir().map(Path::to_path_buf);
        let shell = shell_of(&command);
        Self::spawn(command).map_err(|error| start_error(folder.as_deref(), &shell, error))
    }

    /// Kill the shell and everything in its process group.
    pub fn kill(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.group {
            // SAFETY: killpg only sends a signal to the group the shell leads.
            unsafe {
                libc::killpg(group, libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        let _ = self.group;
        let _ = self.child.start_kill();
    }
}

impl Drop for ShellProcess {
    /// A shell still running when its owner goes away is killed with its
    /// group. One that has ended is left alone: its group id may be reused.
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.kill();
        }
    }
}

pub fn resolve(shell_path: Option<&str>) -> PathBuf {
    shell_path
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            #[cfg(windows)]
            {
                windows_shell()
            }
            #[cfg(not(windows))]
            {
                PathBuf::from("bash")
            }
        })
}

#[cfg(any(windows, test))]
fn windows_shell() -> PathBuf {
    for (variable, directory) in [
        ("ProgramFiles", "Git"),
        ("ProgramFiles(x86)", "Git"),
        ("LOCALAPPDATA", "Programs/Git"),
    ] {
        if let Some(root) = std::env::var_os(variable) {
            let bash = PathBuf::from(root).join(directory).join("bin/bash.exe");
            if bash.is_file() {
                return bash;
            }
        }
    }
    let paths = std::env::var_os("PATH").unwrap_or_default();
    let on_path = |name: &str| {
        std::env::split_paths(&paths)
            .filter(|directory| directory.is_absolute())
            .map(|directory| directory.join(name))
            .find(|path| path.is_file())
    };
    if let Some(bash) = on_path("bash.exe") {
        return bash;
    }
    for (name, variable, relative) in [
        ("pwsh.exe", "ProgramFiles", "PowerShell/7/pwsh.exe"),
        (
            "powershell.exe",
            "SystemRoot",
            "System32/WindowsPowerShell/v1.0/powershell.exe",
        ),
    ] {
        if let Some(shell) = on_path(name) {
            return shell;
        }
        if let Some(root) = std::env::var_os(variable) {
            let shell = PathBuf::from(root).join(relative);
            if shell.is_file() {
                return shell;
            }
        }
    }
    if let Some(cmd) = on_path("cmd.exe") {
        return cmd;
    }
    if let Some(cmd) = std::env::var_os("COMSPEC").map(PathBuf::from)
        && cmd.is_file()
        && cmd
            .file_stem()
            .is_some_and(|name| name.eq_ignore_ascii_case("cmd"))
    {
        return cmd;
    }
    std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32/cmd.exe"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("cmd.exe"))
}

pub fn command(shell_path: Option<&str>, source: &str, prefix: Option<&str>) -> Command {
    let shell = resolve(shell_path);
    let name = shell
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let source = match prefix {
        Some(prefix) if name == "cmd" => format!("{prefix} & {source}"),
        Some(prefix) => format!("{prefix}\n{source}"),
        None => source.to_string(),
    };
    let mut command = Command::new(shell);
    #[cfg(windows)]
    if name == "cmd" {
        // cmd.exe parses command text itself; C runtime argument escaping is unsuitable.
        command.args(["/D", "/S", "/C"]);
        command.raw_arg(format!("\"{source}\""));
        return command;
    }
    if matches!(name.as_str(), "powershell" | "pwsh") {
        command.args(["-NoProfile", "-NonInteractive", "-Command"]);
    } else {
        command.arg("-c");
    }
    command.arg(source);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The arguments `command` runs its shell with.
    fn arguments(command: &Command) -> Vec<String> {
        command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    /// The start of every script on Unix: the mark, on the output and on
    /// the errors.
    fn marked(script: &str, separator: &str) -> String {
        if cfg!(unix) {
            format!("printf '{MARK_FORMAT}'; printf '{MARK_FORMAT}' >&2{separator}{script}")
        } else {
            script.to_string()
        }
    }

    #[test]
    fn fish_enters_the_folder_and_reports_where_it_ended_like_a_posix_shell() {
        let directory = Path::new("/work/it's here");
        let report = Path::new("/tmp/super-cwd-1");
        let fish = script(
            Some("/usr/bin/fish"),
            "cd sub\necho done",
            Some("set -x A 1"),
            directory,
            Some(report),
            None,
        );
        let expected = concat!(
            r"builtin cd '/work/it\'s here'; or exit 1",
            "\n",
            "function __super_report_folder --on-event fish_exit\n",
            "    builtin pwd -P 2>/dev/null > '/tmp/super-cwd-1'\n",
            "end\n",
            "set -x A 1\n",
            "cd sub\necho done",
        );
        assert_eq!(login(&fish, "/usr/bin/fish"), marked(expected, "\n"));
        assert_eq!(fish.as_std().get_current_dir(), Some(directory));
        // Without a report file the command only starts in the folder; a
        // background command's file is emptied of what the profile wrote.
        let quiet = script(
            Some("/usr/bin/fish"),
            "ls",
            None,
            directory,
            None,
            Some(Path::new("/tmp/b1.output")),
        );
        assert_eq!(
            login(&quiet, "/usr/bin/fish"),
            marked(
                &format!(
                    "{}{}",
                    if cfg!(unix) {
                        "true > '/tmp/b1.output'\n"
                    } else {
                        ""
                    },
                    concat!(r"builtin cd '/work/it\'s here'; or exit 1", "\nls")
                ),
                "\n"
            )
        );
        assert_eq!(fish_quote(r"a\b'c"), r"'a\\b\'c'");
        // A POSIX shell runs the command with eval and reports its folder
        // after it, and with a trap for a command that exits.
        let posix = script(
            Some("/bin/bash"),
            "echo 'hi'",
            Some("set -u"),
            directory,
            Some(report),
            None,
        );
        let report = r"pwd -P 2>/dev/null >| '/tmp/super-cwd-1' || :";
        assert_eq!(
            login(&posix, "/bin/bash"),
            marked(
                &format!(
                    "{}; trap {} EXIT; eval 'set -u'; eval {}\nset -- \"$?\"\n{report}\n(exit \"$1\")",
                    r"cd -- '/work/it'\''s here' || exit 1",
                    posix_quote(report),
                    posix_quote("echo 'hi'"),
                ),
                "; "
            )
        );
    }

    /// The script a login shell `shell` gets from `command`, checking how it
    /// is started.
    fn login(command: &Command, shell: &str) -> String {
        let arguments = arguments(command);
        assert_eq!(command.as_std().get_program(), shell);
        assert_eq!(arguments[..2], ["-l", "-c"]);
        arguments[2].clone()
    }

    /// What a command `script` started wrote to one stream, as the tools
    /// read it.
    #[cfg(unix)]
    fn read_as_the_tools_do(bytes: &[u8]) -> String {
        let mut stream = ProfileOutput::new(true);
        let mut text = stream.feed(bytes).into_owned();
        text.extend(stream.finish());
        String::from_utf8(text).unwrap()
    }

    #[test]
    fn only_what_comes_after_the_mark_is_the_commands() {
        let mut stream = ProfileOutput::new(true);
        // The mark may come in pieces, and the command's output with it.
        assert!(stream.feed(b"profile says hi\n\x1esuper-").is_empty());
        assert_eq!(&*stream.feed(b"output\x1ehello"), b"hello");
        assert_eq!(
            &*stream.feed(b" world\x1esuper-output\x1e"),
            b" world\x1esuper-output\x1e"
        );
        assert!(stream.finish().is_empty());
        // Without a mark everything counts, at the end: the shell's own
        // error from before the script ran.
        let mut stream = ProfileOutput::new(true);
        assert!(stream.feed(b"zsh: bad option").is_empty());
        assert_eq!(stream.finish(), b"zsh: bad option");
        // A shell that prints no mark passes everything at once.
        let mut stream = ProfileOutput::new(false);
        assert_eq!(&*stream.feed(b"as it is"), b"as it is");
    }

    #[tokio::test]
    async fn a_shell_that_cannot_start_names_the_missing_folder_or_shell() {
        let gone = tempfile::tempdir().unwrap().path().join("gone");
        let error = ShellProcess::start(script(Some("/bin/bash"), "pwd", None, &gone, None, None))
            .err()
            .expect("no folder to start in");
        assert_eq!(
            error.to_string(),
            format!(
                "The folder {} does not exist, so the command cannot start there.",
                gone.display()
            )
        );
        let here = tempfile::tempdir().unwrap();
        let missing = "/no/such/shell/bash";
        let error =
            ShellProcess::start(script(Some(missing), "pwd", None, here.path(), None, None))
                .err()
                .expect("no shell to start");
        assert!(
            error
                .to_string()
                .starts_with(&format!("Could not start shell {missing}: ")),
            "{error}"
        );
    }

    /// A shell file that is there but cannot run is named as one that is
    /// missing is: one that may not be run, and one whose interpreter is
    /// gone.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_shell_that_is_there_but_cannot_run_is_named() {
        use std::os::unix::fs::PermissionsExt as _;
        let here = tempfile::tempdir().unwrap();
        let not_runnable = here.path().join("noexec/bash");
        let broken = here.path().join("shebang/bash");
        for (shell, text, mode) in [
            (&not_runnable, "#!/bin/sh\necho hi\n", 0o644),
            (&broken, "#!/no/such/interpreter\necho hi\n", 0o755),
        ] {
            std::fs::create_dir_all(shell.parent().unwrap()).unwrap();
            std::fs::write(shell, text).unwrap();
            std::fs::set_permissions(shell, std::fs::Permissions::from_mode(mode)).unwrap();
            let shell = shell.display().to_string();
            let error =
                ShellProcess::start(script(Some(&shell), "pwd", None, here.path(), None, None))
                    .err()
                    .expect("the shell cannot run");
            assert!(
                error
                    .to_string()
                    .starts_with(&format!("Could not start shell {shell}: ")),
                "{error}"
            );
        }
    }

    /// What the profile writes stays out of the command's output and errors;
    /// its environment and its `cd` reach the command, and the command still
    /// starts in its folder.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_login_shell_keeps_its_profiles_output_out_of_the_commands() {
        let home = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let folder = folder.path().canonicalize().unwrap();
        std::fs::write(
            home.path().join(".bash_profile"),
            "echo PROFILE-OUT\necho PROFILE-ERR >&2\nexport FROM_PROFILE=yes\ncd /\nprintf no-newline\n",
        )
        .unwrap();
        let mut command = script(
            Some("/bin/bash"),
            "echo out; echo err >&2; echo \"$FROM_PROFILE\"; pwd",
            None,
            &folder,
            None,
            None,
        );
        command
            .env("HOME", home.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = command.output().await.unwrap();
        assert!(output.status.success());
        assert_eq!(
            read_as_the_tools_do(&output.stdout),
            format!("out\nyes\n{}\n", folder.display())
        );
        assert_eq!(read_as_the_tools_do(&output.stderr), "err\n");

        // A background command's file is emptied of the profile's output.
        let file = folder.join("b1.output");
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .unwrap();
        let mut command = script(
            Some("/bin/bash"),
            "echo out; echo err >&2",
            None,
            &folder,
            None,
            Some(&file),
        );
        command
            .env("HOME", home.path())
            .stdout(Stdio::from(written.try_clone().unwrap()))
            .stderr(Stdio::from(written));
        assert!(command.status().await.unwrap().success());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "out\nerr\n");
    }

    /// A job the profile starts with its output sent elsewhere, the usual
    /// way to start an agent from a profile, does not keep the command's
    /// output open: the command's output ends when the command does.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_job_the_profile_starts_does_not_hold_the_commands_output_open() {
        let home = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(".bash_profile"),
            "(exec sleep 6) >/dev/null 2>&1 &\n",
        )
        .unwrap();
        let mut command = script(
            Some("/bin/bash"),
            "echo hi",
            None,
            folder.path(),
            None,
            None,
        );
        command
            .env("HOME", home.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = tokio::time::timeout(std::time::Duration::from_secs(4), command.output())
            .await
            .expect("the output ends with the command, not with the profile's job")
            .unwrap();
        assert_eq!(read_as_the_tools_do(&output.stdout), "hi\n");
    }

    /// A syntax error in the command is reported, also by zsh, which parses
    /// a whole script before it runs any of it, and bash counts the lines
    /// of the command, not of the script around it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_syntax_error_in_the_command_is_reported_with_its_own_line() {
        let folder = tempfile::tempdir().unwrap();
        let errors = |shell: &'static str, source: &'static str| {
            let mut command = script(Some(shell), source, None, folder.path(), None, None);
            command
                .env("HOME", folder.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            async move {
                let output = command.output().await.unwrap();
                assert!(!output.status.success(), "{shell} {source}");
                read_as_the_tools_do(&output.stderr)
            }
        };
        let bash = errors("/bin/bash", "if then").await;
        assert!(bash.contains("syntax error"), "{bash}");
        let line: u32 = bash
            .split("line ")
            .nth(1)
            .and_then(|rest| rest.split(':').next())
            .and_then(|number| number.parse().ok())
            .expect("a line number");
        assert!(line <= 1, "{bash}");
        if Path::new("/bin/zsh").is_file() {
            let zsh = errors("/bin/zsh", "echo \"unterminated").await;
            assert!(zsh.contains("unmatched \""), "{zsh}");
            let zsh = errors("/bin/zsh", "echo ok; (").await;
            assert!(zsh.contains("parse error"), "{zsh}");
        }
    }

    #[test]
    fn windows_shell_selection() {
        if std::env::var_os("SUPER_TEST_SHELL_PROBE").is_some() {
            #[cfg(windows)]
            for name in [
                "ProgramFiles",
                "ProgramFiles(x86)",
                "LOCALAPPDATA",
                "SystemRoot",
                "COMSPEC",
                "PATH",
            ] {
                let value = std::env::var_os(format!("SUPER_TEST_{name}")).unwrap();
                // SAFETY: Windows environment mutation is thread-safe. Keep this
                // inside the probe process so the parent test is not affected.
                unsafe { std::env::set_var(name, value) };
            }
            #[cfg(windows)]
            let shell = command(None, "echo probe", None)
                .as_std()
                .get_program()
                .to_owned();
            #[cfg(not(windows))]
            let shell = windows_shell().into_os_string();
            println!("SUPER_SELECTED_SHELL={}", shell.to_string_lossy());
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let program_files = root.join("Program Files");
        let windows = root.join("Windows");
        let path = root.join("path");
        let candidates = [
            program_files.join("Git/bin/bash.exe"),
            root.join("Program Files x86/Git/bin/bash.exe"),
            root.join("LocalAppData/Programs/Git/bin/bash.exe"),
            path.join("bash.exe"),
            path.join("pwsh.exe"),
            program_files.join("PowerShell/7/pwsh.exe"),
            path.join("powershell.exe"),
            windows.join("System32/WindowsPowerShell/v1.0/powershell.exe"),
            path.join("cmd.exe"),
            root.join("comspec/cmd.exe"),
            windows.join("System32/cmd.exe"),
        ];
        let probe = |expected: &std::path::Path| {
            let mut process = std::process::Command::new(std::env::current_exe().unwrap());
            process
                .env_clear()
                .args([
                    "--exact",
                    "tools::shell::tests::windows_shell_selection",
                    "--nocapture",
                ])
                .env("SUPER_TEST_SHELL_PROBE", "1");
            for (name, value) in [
                ("ProgramFiles", program_files.clone()),
                ("ProgramFiles(x86)", root.join("Program Files x86")),
                ("LOCALAPPDATA", root.join("LocalAppData")),
                ("SystemRoot", windows.clone()),
                ("COMSPEC", root.join("comspec/cmd.exe")),
                ("PATH", path.clone()),
            ] {
                process
                    .env(name, &value)
                    .env(format!("SUPER_TEST_{name}"), value);
            }
            let output = process.output().unwrap();
            assert!(output.status.success(), "{output:?}");
            let stdout = String::from_utf8(output.stdout).unwrap();
            let selected = stdout
                .lines()
                .find_map(|line| line.strip_prefix("SUPER_SELECTED_SHELL="))
                .unwrap();
            assert_eq!(std::path::Path::new(selected), expected, "{stdout}");
        };

        // No Bash: choose an installed native Windows shell before command submission.
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("pwsh.exe"), "fixture").unwrap();
        probe(&path.join("pwsh.exe"));
        std::fs::remove_file(path.join("pwsh.exe")).unwrap();

        for candidate in &candidates {
            std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
            std::fs::write(candidate, "fixture").unwrap();
        }
        for candidate in &candidates {
            probe(candidate);
            std::fs::remove_file(candidate).unwrap();
        }
        probe(std::path::Path::new("cmd.exe"));
        let explicit = root.join("custom shell.exe");
        assert_eq!(
            command(explicit.to_str(), "echo test", None)
                .as_std()
                .get_program(),
            explicit.as_os_str()
        );
    }

    #[tokio::test]
    async fn preserves_quoted_url_prefix_and_exit_status() {
        let url = "https://cursor.com/loginDeepControl?challenge=test&uuid=test&mode=login";
        let mut cases = vec![(
            None,
            format!("printf '%s\\n' '{url}' \"$SUPER_TEST_PREFIX\"; exit 7"),
            "SUPER_TEST_PREFIX='prefix value'",
            format!("{url}\nprefix value\n"),
        )];
        if cfg!(windows) {
            cases.extend([
                (
                    Some("cmd.exe"),
                    format!("echo \"{url}\"& exit /b 7"),
                    "echo prefix value",
                    format!("prefix value\n\"{url}\"\n"),
                ),
                (
                    Some("powershell.exe"),
                    format!("Write-Output '{url}'; Write-Output $env:SUPER_TEST_PREFIX; exit 7"),
                    "$env:SUPER_TEST_PREFIX='prefix value'",
                    format!("{url}\nprefix value\n"),
                ),
            ]);
        }
        for (shell, source, prefix, expected) in cases {
            let output = command(shell, &source, Some(prefix))
                .output()
                .await
                .unwrap();
            assert_eq!(output.status.code(), Some(7), "{shell:?}: {output:?}");
            assert_eq!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .lines()
                    .map(str::trim_end)
                    .collect::<Vec<_>>(),
                expected.lines().collect::<Vec<_>>(),
                "{shell:?}"
            );
        }
    }
}
