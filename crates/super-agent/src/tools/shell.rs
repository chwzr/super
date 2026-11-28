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
/// the command ends in to that file when the shell exits: a POSIX shell with
/// an `EXIT` trap, fish with a handler of its `fish_exit` event. PowerShell
/// and cmd start as [`command`] starts them.
///
/// On Unix what the profile writes is thrown away: `/bin/sh` starts the
/// login shell with its output and errors going nowhere and the real ones
/// kept as descriptors 3 and 4, and the command's script puts them back
/// before it runs anything ([`PROFILE_QUIET`]), so only the command's own
/// output reaches the result, the output file or a monitor's events.
pub fn script(
    shell_path: Option<&str>,
    source: &str,
    prefix: Option<&str>,
    directory: &Path,
    report_directory: Option<&Path>,
) -> Command {
    let shell = resolve(shell_path);
    let mut command = match kind(&shell) {
        ShellKind::Posix => {
            let mut text = format!(
                "{}cd -- {} || exit 1\n",
                if cfg!(unix) {
                    "exec 1>&3 2>&4 3>&- 4>&-\n"
                } else {
                    ""
                },
                posix_quote(&directory.to_string_lossy())
            );
            if let Some(file) = report_directory {
                let body = format!("pwd -P >| {}", posix_quote(&file.to_string_lossy()));
                text.push_str(&format!("trap {} EXIT\n", posix_quote(&body)));
            }
            if let Some(prefix) = prefix {
                text.push_str(prefix);
                text.push('\n');
            }
            text.push_str(source);
            login_shell(&shell, text)
        }
        ShellKind::Fish => {
            let mut text = format!(
                "builtin cd {}; or exit 1\n",
                fish_quote(&directory.to_string_lossy())
            );
            if let Some(file) = report_directory {
                text.push_str(&format!(
                    "function __super_report_folder --on-event fish_exit\n    builtin pwd -P > {}\nend\n",
                    fish_quote(&file.to_string_lossy())
                ));
            }
            if let Some(prefix) = prefix {
                text.push_str(prefix);
                text.push('\n');
            }
            text.push_str(source);
            if cfg!(unix) {
                // fish cannot move its own descriptors for good: the whole
                // script runs in a block that writes to the real ones.
                text = format!("begin\n{text}\nend 1>&3 2>&4");
            }
            login_shell(&shell, text)
        }
        _ => self::command(shell_path, source, prefix),
    };
    command.current_dir(directory);
    command
}

/// The script `/bin/sh` runs to start a login shell without its profile's
/// output: `$0` is the shell, `$1` the shell's script, which writes to
/// descriptors 3 and 4.
pub const PROFILE_QUIET: &str = r#"exec 3>&1 4>&2 >/dev/null 2>&1; exec "$0" -l -c "$1""#;

/// `shell` as a login shell running `text`; on Unix through `/bin/sh` with
/// [`PROFILE_QUIET`], unless the shell cannot be found, so that starting it
/// fails with an error that names it.
fn login_shell(shell: &Path, text: String) -> Command {
    if cfg!(unix) && findable(shell) {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", PROFILE_QUIET]).arg(shell).arg(text);
        command
    } else {
        let mut command = Command::new(shell);
        command.args(["-l", "-c"]).arg(text);
        command
    }
}

/// Whether `shell` is a file, or a bare name of a file in a folder of
/// `PATH`.
fn findable(shell: &Path) -> bool {
    if shell.components().count() > 1 {
        return shell.is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|folder| folder.join(shell).is_file()))
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

/// The shell `command` starts: the login shell `/bin/sh` hands over to
/// ([`PROFILE_QUIET`]), else its program.
fn shell_of(command: &Command) -> String {
    let command = command.as_std();
    let arguments: Vec<_> = command.get_args().collect();
    match arguments.as_slice() {
        [_, script, shell, ..] if *script == PROFILE_QUIET => shell.to_string_lossy().into_owned(),
        _ => command.get_program().to_string_lossy().into_owned(),
    }
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

    #[test]
    fn fish_enters_the_folder_and_reports_where_it_ended_like_a_posix_shell() {
        let directory = Path::new("/work/it's here");
        let report = Path::new("/tmp/super-cwd-1");
        // A file named fish stands in for one, which this machine may lack.
        let installed = tempfile::tempdir().unwrap();
        let fish_path = installed.path().join("fish");
        std::fs::write(&fish_path, "").unwrap();
        let fish_path = fish_path.display().to_string();
        let fish = script(
            Some(&fish_path),
            "cd sub\necho done",
            Some("set -x A 1"),
            directory,
            Some(report),
        );
        let expected = concat!(
            r"builtin cd '/work/it\'s here'; or exit 1",
            "\n",
            "function __super_report_folder --on-event fish_exit\n",
            "    builtin pwd -P > '/tmp/super-cwd-1'\n",
            "end\n",
            "set -x A 1\n",
            "cd sub\necho done",
        );
        assert_eq!(login(&fish, &fish_path), fish_block(expected));
        assert_eq!(fish.as_std().get_current_dir(), Some(directory));
        // Without a report file the command only starts in the folder.
        let quiet = script(Some(&fish_path), "ls", None, directory, None);
        assert_eq!(
            login(&quiet, &fish_path),
            fish_block(concat!(r"builtin cd '/work/it\'s here'; or exit 1", "\nls"))
        );
        // A shell that is not there is started as it is, so that the error
        // of its start names it.
        let missing = script(Some("/no/such/fish"), "ls", None, directory, None);
        assert_eq!(missing.as_std().get_program(), "/no/such/fish");
        assert_eq!(fish_quote(r"a\b'c"), r"'a\\b\'c'");
        // A POSIX shell does the same with a trap.
        let posix = script(Some("/bin/bash"), "ls", None, directory, Some(report));
        assert_eq!(
            login(&posix, "/bin/bash"),
            format!(
                "{}{}",
                if cfg!(unix) {
                    "exec 1>&3 2>&4 3>&- 4>&-\n"
                } else {
                    ""
                },
                concat!(
                    r"cd -- '/work/it'\''s here' || exit 1",
                    "\n",
                    r"trap 'pwd -P >| '\''/tmp/super-cwd-1'\''' EXIT",
                    "\nls"
                )
            )
        );
    }

    /// The script a login shell `shell` gets from `command`, checking how it
    /// is started.
    fn login(command: &Command, shell: &str) -> String {
        let arguments = arguments(command);
        if cfg!(unix) {
            assert_eq!(command.as_std().get_program(), "/bin/sh");
            assert_eq!(arguments[..3], ["-c", PROFILE_QUIET, shell]);
            arguments[3].clone()
        } else {
            assert_eq!(command.as_std().get_program(), shell);
            assert_eq!(arguments[..2], ["-l", "-c"]);
            arguments[2].clone()
        }
    }

    fn fish_block(script: &str) -> String {
        if cfg!(unix) {
            format!("begin\n{script}\nend 1>&3 2>&4")
        } else {
            script.to_string()
        }
    }

    #[tokio::test]
    async fn a_shell_that_cannot_start_names_the_missing_folder_or_shell() {
        let gone = tempfile::tempdir().unwrap().path().join("gone");
        let error = ShellProcess::start(script(Some("/bin/bash"), "pwd", None, &gone, None))
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
        let error = ShellProcess::start(script(Some(missing), "pwd", None, here.path(), None))
            .err()
            .expect("no shell to start");
        assert!(
            error
                .to_string()
                .starts_with(&format!("Could not start shell {missing}: ")),
            "{error}"
        );
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
            "echo PROFILE-OUT\necho PROFILE-ERR >&2\nexport FROM_PROFILE=yes\ncd /\n",
        )
        .unwrap();
        let mut command = script(
            Some("/bin/bash"),
            "echo out; echo err >&2; echo \"$FROM_PROFILE\"; pwd",
            None,
            &folder,
            None,
        );
        command
            .env("HOME", home.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = command.output().await.unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("out\nyes\n{}\n", folder.display())
        );
        assert_eq!(String::from_utf8_lossy(&output.stderr), "err\n");
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
