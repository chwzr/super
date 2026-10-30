//! The daemon as a Linux service: a systemd user unit that runs
//! `super daemon run` from login to logout and brings it back after a crash.
//!
//! `super install` and `super uninstall` drive [`install`] and [`uninstall`];
//! `super daemon status` adds the line from [`unit_status`]. macOS has no
//! install path: the daemon ships with the SUPER app, which registers it with
//! launchd. Every function checks the platform first and refuses anything
//! but Linux before it touches a file or runs systemctl. Every
//! `systemctl --user` call goes through [`Systemctl`], so the decisions here
//! are tested without a systemd.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The unit, as systemctl names it.
pub const UNIT_NAME: &str = "super-daemon.service";

/// The operating system, as far as installing a service cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// `super install` or `super uninstall`, for the refusal on other platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Install,
    Uninstall,
}

impl Action {
    fn command(self) -> &'static str {
        match self {
            Self::Install => "super install",
            Self::Uninstall => "super uninstall",
        }
    }
}

/// Where the unit lives and whose it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceEnv {
    pub platform: Platform,
    /// `~/.config/systemd/user`, or `$XDG_CONFIG_HOME/systemd/user`.
    pub unit_dir: PathBuf,
    /// The login name, for the `loginctl enable-linger` hints.
    pub user: Option<String>,
    /// Whether logind keeps this user's manager, and so the daemon, running
    /// without a login.
    pub lingering: bool,
}

impl ServiceEnv {
    /// This process's platform, home directory and user. `None` without a
    /// home directory.
    pub fn current() -> Option<Self> {
        let home = dirs::home_dir()?;
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .ok()
            .filter(|user| !user.is_empty());
        // logind marks a lingering user with a file named after them.
        let lingering = user
            .as_deref()
            .is_some_and(|user| Path::new("/var/lib/systemd/linger").join(user).exists());
        Some(Self {
            platform: Platform::current(),
            unit_dir: unit_dir(&home, std::env::var_os("XDG_CONFIG_HOME").as_deref()),
            user,
            lingering,
        })
    }

    pub fn unit_path(&self) -> PathBuf {
        self.unit_dir.join(UNIT_NAME)
    }

    /// The link `systemctl enable` makes for `WantedBy=default.target`.
    fn wants_link(&self) -> PathBuf {
        self.unit_dir.join("default.target.wants").join(UNIT_NAME)
    }

    /// The user's name for a command line to copy, or `<user>`.
    pub fn user_name(&self) -> &str {
        self.user.as_deref().unwrap_or("<user>")
    }
}

/// systemd's directory for a user's own units: `$XDG_CONFIG_HOME/systemd/user`
/// when that is an absolute path, else `~/.config/systemd/user`.
pub fn unit_dir(home: &Path, xdg_config_home: Option<&OsStr>) -> PathBuf {
    xdg_config_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
        .join("systemd")
        .join("user")
}

/// The binary the unit runs: the running one, unless the `super` first on
/// PATH is a symlink to it. Then the symlink, so a package manager that keeps
/// each version in its own directory (Homebrew's cellar) does not leave the
/// unit pointing at a removed version after an upgrade.
pub fn service_exe(current_exe: &Path, path_var: Option<&OsStr>) -> PathBuf {
    let fallback = current_exe.to_path_buf();
    let (Some(name), Some(path_var)) = (current_exe.file_name(), path_var) else {
        return fallback;
    };
    let Ok(real) = std::fs::canonicalize(current_exe) else {
        return fallback;
    };
    for dir in std::env::split_paths(path_var) {
        if !dir.is_absolute() {
            continue;
        }
        let candidate = dir.join(name);
        let Ok(metadata) = std::fs::symlink_metadata(&candidate) else {
            continue;
        };
        let same_binary = std::fs::canonicalize(&candidate).is_ok_and(|target| target == real);
        return if metadata.file_type().is_symlink() && same_binary {
            candidate
        } else {
            fallback
        };
    }
    fallback
}

/// `path` as the program word of `ExecStart=`: `%` doubled so systemd does
/// not read a specifier, and the whole path in double quotes with `\` and `"`
/// escaped when it holds whitespace, a quote or a backslash.
pub fn exec_path(path: &Path) -> Result<String, ServiceError> {
    let unusable = |reason| ServiceError::ExePath {
        path: path.to_path_buf(),
        reason,
    };
    if !path.is_absolute() {
        return Err(unusable("the path is not absolute"));
    }
    let text = path
        .to_str()
        .ok_or_else(|| unusable("the path is not valid UTF-8"))?;
    if text.chars().any(char::is_control) {
        return Err(unusable("the path contains a control character"));
    }
    // `$$` is a literal dollar in arguments, but systemd does not expand the
    // program path, so no spelling of `$` survives there in every version.
    if text.contains('$') {
        return Err(unusable("systemd cannot run a path that contains `$`"));
    }
    let quote = text
        .chars()
        .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\'' | '\\'));
    let mut word = String::with_capacity(text.len() + 2);
    if quote {
        word.push('"');
    }
    for ch in text.chars() {
        match ch {
            '%' => word.push_str("%%"),
            '"' | '\\' if quote => {
                word.push('\\');
                word.push(ch);
            }
            _ => word.push(ch),
        }
    }
    if quote {
        word.push('"');
    }
    Ok(word)
}

/// The unit file `super install` writes for `exe`.
pub fn render_unit(exe: &Path) -> Result<String, ServiceError> {
    let program = exec_path(exe)?;
    Ok(format!(
        "# Written by `super install`; `super uninstall` removes it.\n\
         [Unit]\n\
         Description=SUPER daemon: hosts this user's SUPER sessions\n\
         Documentation=https://github.com/chwzr/super/blob/main/docs/daemon.md\n\
         \n\
         [Service]\n\
         ExecStart={program} daemon run\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    ))
}

/// What one `systemctl --user` call printed and whether it succeeded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemctlOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `systemctl --user <args>`. [`UserSystemctl`] runs the real command;
/// the tests answer from a script and record the calls.
pub trait Systemctl {
    fn run(&mut self, args: &[&str]) -> io::Result<SystemctlOutput>;
}

/// The real `systemctl --user`.
#[derive(Debug, Clone, Copy, Default)]
pub struct UserSystemctl;

impl Systemctl for UserSystemctl {
    fn run(&mut self, args: &[&str]) -> io::Result<SystemctlOutput> {
        let output = std::process::Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()?;
        Ok(SystemctlOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("{}", unsupported(*.platform, *.action))]
    Unsupported { platform: Platform, action: Action },
    #[error(
        "systemctl was not found, so this system does not seem to run systemd. \
         `super install` needs a systemd user manager; without one, run \
         `super daemon run` under the service manager you use."
    )]
    NoSystemctl,
    #[error(
        "systemctl --user did not reach a systemd user manager ({detail}). \
         Log in directly rather than through su or sudo so that the manager \
         starts, or have root run `loginctl enable-linger {user}`, then try again."
    )]
    NoUserManager { detail: String, user: String },
    #[error("{} cannot be the unit's ExecStart: {reason}", .path.display())]
    ExePath { path: PathBuf, reason: &'static str },
    #[error(
        "the running super binary is gone ({}); run `super install` from the installed binary",
        .path.display()
    )]
    ExeMissing { path: PathBuf },
    #[error("{action} {}: {source}", .path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    #[error("`systemctl --user {command}` failed: {detail}")]
    Systemctl { command: String, detail: String },
}

fn unsupported(platform: Platform, action: Action) -> String {
    let command = action.command();
    match (platform, action) {
        (Platform::MacOs, Action::Install) => format!(
            "{command} is for Linux. On macOS the SUPER daemon ships with the SUPER app, \
             which starts it at login: install and open SUPER.app to run it."
        ),
        (Platform::MacOs, Action::Uninstall) => format!(
            "{command} is for Linux. On macOS the SUPER daemon belongs to the SUPER app: \
             turn it off under Login Items in System Settings, or remove the app."
        ),
        (Platform::Windows, _) => format!(
            "{command} is for Linux; the SUPER daemon needs a Unix socket and is not \
             available on Windows."
        ),
        (Platform::Linux | Platform::Other, _) => format!(
            "{command} sets up a systemd user unit on Linux; on this system run \
             `super daemon run` under the service manager you use."
        ),
    }
}

/// Refuse every platform but Linux.
pub fn supported(platform: Platform, action: Action) -> Result<(), ServiceError> {
    if platform == Platform::Linux {
        Ok(())
    } else {
        Err(ServiceError::Unsupported { platform, action })
    }
}

/// How the unit file on disk compared with the one `super install` renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitChange {
    Created,
    /// A different unit was there (super moved, or an older release wrote
    /// it); it was replaced and the daemon restarted.
    Updated,
    /// The same unit was there; it was enabled and started again.
    Unchanged,
}

/// What `super install` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub unit_path: PathBuf,
    pub exe: PathBuf,
    pub change: UnitChange,
    /// `systemctl --user is-enabled`, normally `enabled`.
    pub enabled: String,
    /// `systemctl --user is-active`, normally `active`.
    pub active: String,
}

impl Installed {
    /// The lines `super install` prints.
    pub fn summary(&self) -> Vec<String> {
        let path = self.unit_path.display();
        let first = match self.change {
            UnitChange::Created => format!("installed {path}"),
            UnitChange::Updated => format!("updated   {path} (daemon restarted)"),
            UnitChange::Unchanged => format!("unchanged {path}"),
        };
        vec![
            first,
            format!("runs      {} daemon run", self.exe.display()),
            format!("service   {}, {}", self.enabled, self.active),
        ]
    }
}

/// Write the unit for `exe`, then `systemctl --user daemon-reload` and
/// `enable --now`. A unit that differs from the one on disk replaces it and
/// restarts the daemon; the same unit is enabled and started again.
pub fn install(
    env: &ServiceEnv,
    exe: &Path,
    systemctl: &mut dyn Systemctl,
) -> Result<Installed, ServiceError> {
    supported(env.platform, Action::Install)?;
    let unit = render_unit(exe)?;
    if !exe.is_file() {
        return Err(ServiceError::ExeMissing {
            path: exe.to_path_buf(),
        });
    }
    probe(env, systemctl)?;

    let unit_path = env.unit_path();
    let change = match std::fs::read(&unit_path) {
        Ok(existing) if existing == unit.as_bytes() => UnitChange::Unchanged,
        Ok(_) => UnitChange::Updated,
        Err(error) if error.kind() == io::ErrorKind::NotFound => UnitChange::Created,
        Err(source) => return Err(io_error("read", &unit_path, source)),
    };
    if change != UnitChange::Unchanged {
        std::fs::create_dir_all(&env.unit_dir)
            .map_err(|source| io_error("create", &env.unit_dir, source))?;
        std::fs::write(&unit_path, &unit)
            .map_err(|source| io_error("write", &unit_path, source))?;
    }
    call(systemctl, &["daemon-reload"])?;
    if change == UnitChange::Updated {
        call(systemctl, &["enable", UNIT_NAME])?;
        call(systemctl, &["restart", UNIT_NAME])?;
    } else {
        call(systemctl, &["enable", "--now", UNIT_NAME])?;
    }
    Ok(Installed {
        unit_path,
        exe: exe.to_path_buf(),
        change,
        enabled: query(systemctl, "is-enabled"),
        active: query(systemctl, "is-active"),
    })
}

/// What `super uninstall` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uninstalled {
    /// There was no unit file.
    NothingInstalled { unit_path: PathBuf },
    Removed {
        unit_path: PathBuf,
        /// systemd stopped and disabled the unit. False when no user manager
        /// answered or `disable` failed; the files are removed either way.
        stopped: bool,
        /// What went wrong along the way, for the user to look at.
        problems: Vec<String>,
    },
}

/// `systemctl --user disable --now`, remove the unit file, `daemon-reload`
/// and clear a failed state. Without a user manager only the files go.
pub fn uninstall(
    env: &ServiceEnv,
    systemctl: &mut dyn Systemctl,
) -> Result<Uninstalled, ServiceError> {
    supported(env.platform, Action::Uninstall)?;
    let unit_path = env.unit_path();
    if std::fs::symlink_metadata(&unit_path).is_err() {
        return Ok(Uninstalled::NothingInstalled { unit_path });
    }

    let mut problems = Vec::new();
    let manager = probe(env, systemctl);
    let stopped = match &manager {
        Ok(()) => match call(systemctl, &["disable", "--now", UNIT_NAME]) {
            Ok(_) => true,
            Err(error) => {
                problems.push(error.to_string());
                false
            }
        },
        Err(error) => {
            problems.push(format!("nothing was stopped: {error}"));
            false
        }
    };
    if !stopped {
        // `disable` removes this link; when it did not run, remove it here.
        let link = env.wants_link();
        if std::fs::symlink_metadata(&link).is_ok() {
            std::fs::remove_file(&link).map_err(|source| io_error("remove", &link, source))?;
        }
    }
    std::fs::remove_file(&unit_path).map_err(|source| io_error("remove", &unit_path, source))?;
    if manager.is_ok() {
        if let Err(error) = call(systemctl, &["daemon-reload"]) {
            problems.push(error.to_string());
        }
        // A unit that had failed stays listed until reset; one that had not
        // makes this call fail, which is fine.
        let _ = systemctl.run(&["reset-failed", UNIT_NAME]);
    }
    Ok(Uninstalled::Removed {
        unit_path,
        stopped,
        problems,
    })
}

/// The unit as `super daemon status` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitStatus {
    pub unit_path: PathBuf,
    pub state: UnitState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitState {
    NotInstalled,
    /// systemctl's `is-enabled` and `is-active` answers.
    Installed {
        enabled: String,
        active: String,
    },
    /// The file is there but systemctl could not say more.
    Unknown {
        reason: String,
    },
}

impl UnitStatus {
    /// One line in the `label    value` shape of `super daemon status`.
    pub fn line(&self) -> String {
        let path = self.unit_path.display();
        match &self.state {
            UnitState::NotInstalled => {
                "service  not installed; `super install` starts the daemon at login".into()
            }
            UnitState::Installed { enabled, active } => {
                let hint = if active == "failed" {
                    "; journalctl --user -u super-daemon shows why"
                } else {
                    ""
                };
                format!("service  {enabled}, {active} ({path}){hint}")
            }
            UnitState::Unknown { reason } => {
                format!("service  installed ({path}), state unknown: {reason}")
            }
        }
    }
}

/// The unit's state on Linux; `None` elsewhere, where there is no unit.
pub fn unit_status(env: &ServiceEnv, systemctl: &mut dyn Systemctl) -> Option<UnitStatus> {
    if env.platform != Platform::Linux {
        return None;
    }
    let unit_path = env.unit_path();
    let state = if std::fs::symlink_metadata(&unit_path).is_err() {
        UnitState::NotInstalled
    } else {
        match probe(env, systemctl) {
            Ok(()) => UnitState::Installed {
                enabled: query(systemctl, "is-enabled"),
                active: query(systemctl, "is-active"),
            },
            Err(ServiceError::NoSystemctl) => UnitState::Unknown {
                reason: "systemctl not found".into(),
            },
            Err(ServiceError::NoUserManager { detail, .. }) => UnitState::Unknown {
                reason: format!("no systemd user manager ({detail})"),
            },
            Err(error) => UnitState::Unknown {
                reason: error.to_string(),
            },
        }
    };
    Some(UnitStatus { unit_path, state })
}

/// Whether a daemon accepts connections on `socket`, trying until `limit`
/// has passed; `Duration::ZERO` tries once. Always false off Unix.
pub fn daemon_answers(socket: &Path, limit: Duration) -> bool {
    #[cfg(unix)]
    {
        let start = std::time::Instant::now();
        loop {
            if std::os::unix::net::UnixStream::connect(socket).is_ok() {
                return true;
            }
            if start.elapsed() >= limit {
                return false;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (socket, limit);
        false
    }
}

/// Reach the user manager once, so a system without one is told what is
/// missing before anything is written.
fn probe(env: &ServiceEnv, systemctl: &mut dyn Systemctl) -> Result<(), ServiceError> {
    let no_manager = |detail| ServiceError::NoUserManager {
        detail,
        user: env.user_name().to_string(),
    };
    match systemctl.run(&["show-environment"]) {
        Ok(output) if output.success => Ok(()),
        Ok(output) => Err(no_manager(detail(&output))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(ServiceError::NoSystemctl),
        Err(error) => Err(no_manager(error.to_string())),
    }
}

fn call(systemctl: &mut dyn Systemctl, args: &[&str]) -> Result<SystemctlOutput, ServiceError> {
    let failed = |detail| ServiceError::Systemctl {
        command: args.join(" "),
        detail,
    };
    match systemctl.run(args) {
        Ok(output) if output.success => Ok(output),
        Ok(output) => Err(failed(detail(&output))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(ServiceError::NoSystemctl),
        Err(error) => Err(failed(error.to_string())),
    }
}

/// `is-enabled` or `is-active` print their answer even when they exit
/// non-zero (`disabled`, `inactive`, `failed`).
fn query(systemctl: &mut dyn Systemctl, verb: &str) -> String {
    systemctl
        .run(&[verb, UNIT_NAME])
        .ok()
        .map(|output| output.stdout.trim().to_string())
        .filter(|answer| !answer.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// The first line systemctl wrote, preferring stderr.
fn detail(output: &SystemctlOutput) -> String {
    [&output.stderr, &output.stdout]
        .into_iter()
        .flat_map(|text| text.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no output")
        .to_string()
}

fn io_error(action: &'static str, path: &Path, source: io::Error) -> ServiceError {
    ServiceError::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}

// The paths in these tests are Unix paths; Windows has no systemd to test.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Answers `systemctl --user` calls from a script, `success` by default,
    /// and records them.
    #[derive(Default)]
    struct FakeSystemctl {
        calls: Vec<String>,
        answers: VecDeque<(String, io::Result<SystemctlOutput>)>,
    }

    impl FakeSystemctl {
        /// The next call to `args` answers with `answer` instead of success.
        fn answer(mut self, args: &str, answer: io::Result<SystemctlOutput>) -> Self {
            self.answers.push_back((args.to_string(), answer));
            self
        }

        fn fails(self, args: &str, stderr: &str) -> Self {
            self.answer(
                args,
                Ok(SystemctlOutput {
                    success: false,
                    stdout: String::new(),
                    stderr: stderr.into(),
                }),
            )
        }

        fn prints(self, args: &str, stdout: &str) -> Self {
            self.answer(
                args,
                Ok(SystemctlOutput {
                    success: true,
                    stdout: stdout.into(),
                    stderr: String::new(),
                }),
            )
        }

        fn missing() -> Self {
            Self::default().answer(
                "show-environment",
                Err(io::Error::new(io::ErrorKind::NotFound, "no systemctl")),
            )
        }

        fn no_manager() -> Self {
            Self::default().fails(
                "show-environment",
                "Failed to connect to bus: No medium found\n",
            )
        }

        /// The usual answers after a successful install.
        fn healthy() -> Self {
            Self::default()
                .prints(&format!("is-enabled {UNIT_NAME}"), "enabled\n")
                .prints(&format!("is-active {UNIT_NAME}"), "active\n")
        }
    }

    impl Systemctl for FakeSystemctl {
        fn run(&mut self, args: &[&str]) -> io::Result<SystemctlOutput> {
            let joined = args.join(" ");
            self.calls.push(joined.clone());
            match self.answers.iter().position(|(args, _)| *args == joined) {
                Some(index) => self.answers.remove(index).expect("scripted answer").1,
                None => Ok(SystemctlOutput {
                    success: true,
                    ..Default::default()
                }),
            }
        }
    }

    struct Fixture {
        dir: tempfile::TempDir,
        env: ServiceEnv,
        exe: PathBuf,
    }

    impl Fixture {
        fn on(platform: Platform) -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let exe = dir.path().join("bin").join("super");
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(&exe, b"binary").unwrap();
            let env = ServiceEnv {
                platform,
                unit_dir: unit_dir(&dir.path().join("home"), None),
                user: Some("felix".into()),
                lingering: false,
            };
            Self { dir, env, exe }
        }

        fn linux() -> Self {
            Self::on(Platform::Linux)
        }

        fn unit(&self) -> Option<String> {
            std::fs::read_to_string(self.env.unit_path()).ok()
        }
    }

    fn unit_args(verb: &str) -> String {
        format!("{verb} {UNIT_NAME}")
    }

    #[test]
    fn the_unit_runs_the_binary_and_restarts_it_after_a_failure() {
        let unit = render_unit(Path::new("/home/felix/.local/bin/super")).unwrap();
        assert_eq!(
            unit,
            "# Written by `super install`; `super uninstall` removes it.\n\
             [Unit]\n\
             Description=SUPER daemon: hosts this user's SUPER sessions\n\
             Documentation=https://github.com/chwzr/super/blob/main/docs/daemon.md\n\
             \n\
             [Service]\n\
             ExecStart=/home/felix/.local/bin/super daemon run\n\
             Restart=on-failure\n\
             RestartSec=5\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n"
        );
    }

    #[test]
    fn exec_path_quotes_only_what_systemd_would_split_or_unescape() {
        let path = |text: &str| exec_path(Path::new(text)).unwrap();
        assert_eq!(path("/usr/local/bin/super"), "/usr/local/bin/super");
        assert_eq!(
            path("/home/felix/My Tools/super"),
            "\"/home/felix/My Tools/super\""
        );
        assert_eq!(path("/opt/it's/super"), "\"/opt/it's/super\"");
        assert_eq!(path("/opt/a\"b/super"), "\"/opt/a\\\"b/super\"");
        assert_eq!(path("/opt/a\\b/super"), "\"/opt/a\\\\b/super\"");
        assert_eq!(path("/opt/100%/super"), "/opt/100%%/super");
        assert_eq!(path("/opt/50% off/super"), "\"/opt/50%% off/super\"");
    }

    #[test]
    fn exec_path_refuses_what_a_unit_cannot_hold() {
        for (path, reason) in [
            ("bin/super", "not absolute"),
            ("/opt/$HOME/super", "`$`"),
            ("/opt/a\nb/super", "control character"),
        ] {
            let error = exec_path(Path::new(path)).unwrap_err().to_string();
            assert!(error.contains(reason), "{path}: {error}");
        }
    }

    #[test]
    fn a_path_with_spaces_lands_quoted_in_exec_start() {
        let unit = render_unit(Path::new("/home/felix/My Tools/super")).unwrap();
        assert!(
            unit.contains("\nExecStart=\"/home/felix/My Tools/super\" daemon run\n"),
            "{unit}"
        );
    }

    #[test]
    fn the_unit_dir_follows_xdg_config_home_when_it_is_absolute() {
        let home = Path::new("/home/felix");
        assert_eq!(
            unit_dir(home, None),
            PathBuf::from("/home/felix/.config/systemd/user")
        );
        assert_eq!(
            unit_dir(home, Some(OsStr::new("/srv/config"))),
            PathBuf::from("/srv/config/systemd/user")
        );
        assert_eq!(
            unit_dir(home, Some(OsStr::new("relative"))),
            PathBuf::from("/home/felix/.config/systemd/user")
        );
    }

    #[test]
    fn service_exe_prefers_a_symlink_on_path_to_the_running_binary() {
        let dir = tempfile::tempdir().unwrap();
        let cellar = dir.path().join("Cellar/super/0.0.41/bin");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&cellar).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let real = cellar.join("super");
        std::fs::write(&real, b"binary").unwrap();
        std::os::unix::fs::symlink(&real, bin.join("super")).unwrap();
        let path_var = std::env::join_paths([Path::new("relative"), &bin]).unwrap();

        assert_eq!(service_exe(&real, Some(&path_var)), bin.join("super"));
        assert_eq!(service_exe(&real, None), real);
    }

    #[test]
    fn service_exe_keeps_the_running_binary_when_path_finds_another() {
        let dir = tempfile::tempdir().unwrap();
        let running = dir.path().join("build/super");
        let other = dir.path().join("bin/super");
        for path in [&running, &other] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"binary").unwrap();
        }
        // The first `super` on PATH is a different file: run what was run.
        let path_var = std::env::join_paths([other.parent().unwrap()]).unwrap();
        assert_eq!(service_exe(&running, Some(&path_var)), running);
        // A symlink to yet another binary does not count either.
        let elsewhere = dir.path().join("links");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&other, elsewhere.join("super")).unwrap();
        let path_var = std::env::join_paths([&elsewhere]).unwrap();
        assert_eq!(service_exe(&running, Some(&path_var)), running);
    }

    #[test]
    fn install_writes_the_unit_creating_its_directories_then_enables_it() {
        let fixture = Fixture::linux();
        assert!(!fixture.env.unit_dir.exists());
        let mut systemctl = FakeSystemctl::healthy();

        let installed = install(&fixture.env, &fixture.exe, &mut systemctl).unwrap();

        assert_eq!(installed.change, UnitChange::Created);
        assert_eq!(installed.unit_path, fixture.env.unit_path());
        assert!(
            fixture
                .env
                .unit_path()
                .ends_with("home/.config/systemd/user/super-daemon.service")
        );
        assert_eq!(fixture.unit(), Some(render_unit(&fixture.exe).unwrap()));
        assert_eq!(
            systemctl.calls,
            [
                "show-environment".to_string(),
                "daemon-reload".into(),
                format!("enable --now {UNIT_NAME}"),
                unit_args("is-enabled"),
                unit_args("is-active"),
            ]
        );
        assert_eq!(
            installed.summary(),
            [
                format!("installed {}", fixture.env.unit_path().display()),
                format!("runs      {} daemon run", fixture.exe.display()),
                "service   enabled, active".to_string(),
            ]
        );
    }

    #[test]
    fn installing_the_same_unit_again_only_enables_and_starts_it() {
        let fixture = Fixture::linux();
        install(&fixture.env, &fixture.exe, &mut FakeSystemctl::healthy()).unwrap();
        let mut systemctl = FakeSystemctl::healthy();

        let installed = install(&fixture.env, &fixture.exe, &mut systemctl).unwrap();

        assert_eq!(installed.change, UnitChange::Unchanged);
        assert!(
            systemctl
                .calls
                .contains(&format!("enable --now {UNIT_NAME}"))
        );
        assert!(!systemctl.calls.contains(&unit_args("restart")));
        assert!(installed.summary()[0].starts_with("unchanged "));
    }

    #[test]
    fn a_moved_binary_rewrites_the_unit_and_restarts_the_daemon() {
        let fixture = Fixture::linux();
        install(&fixture.env, &fixture.exe, &mut FakeSystemctl::healthy()).unwrap();
        let moved = fixture.dir.path().join("elsewhere super");
        std::fs::write(&moved, b"binary").unwrap();
        let mut systemctl = FakeSystemctl::healthy();

        let installed = install(&fixture.env, &moved, &mut systemctl).unwrap();

        assert_eq!(installed.change, UnitChange::Updated);
        assert_eq!(fixture.unit(), Some(render_unit(&moved).unwrap()));
        assert_eq!(
            &systemctl.calls[..4],
            [
                "show-environment".to_string(),
                "daemon-reload".into(),
                unit_args("enable"),
                unit_args("restart"),
            ]
        );
        assert!(installed.summary()[0].ends_with("(daemon restarted)"));
    }

    #[test]
    fn install_without_systemctl_says_what_is_missing_and_writes_nothing() {
        let fixture = Fixture::linux();
        let mut systemctl = FakeSystemctl::missing();

        let error = install(&fixture.env, &fixture.exe, &mut systemctl).unwrap_err();

        assert!(matches!(error, ServiceError::NoSystemctl), "{error:?}");
        assert!(error.to_string().contains("systemctl was not found"));
        assert!(error.to_string().contains("super daemon run"));
        assert_eq!(systemctl.calls, ["show-environment"]);
        assert!(!fixture.env.unit_dir.exists());
    }

    #[test]
    fn install_without_a_user_manager_names_linger_and_writes_nothing() {
        let fixture = Fixture::linux();
        let mut systemctl = FakeSystemctl::no_manager();

        let error = install(&fixture.env, &fixture.exe, &mut systemctl).unwrap_err();

        let message = error.to_string();
        assert!(message.contains("Failed to connect to bus: No medium found"));
        assert!(message.contains("loginctl enable-linger felix"));
        assert_eq!(systemctl.calls, ["show-environment"]);
        assert_eq!(fixture.unit(), None);
    }

    #[test]
    fn a_failing_systemctl_step_is_reported_with_its_message() {
        let fixture = Fixture::linux();
        let mut systemctl = FakeSystemctl::default().fails(
            &format!("enable --now {UNIT_NAME}"),
            "Failed to enable unit: Access denied\n",
        );

        let error = install(&fixture.env, &fixture.exe, &mut systemctl).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!(
                "`systemctl --user enable --now {UNIT_NAME}` failed: Failed to enable unit: Access denied"
            )
        );
    }

    #[test]
    fn install_refuses_a_binary_that_is_gone() {
        let fixture = Fixture::linux();
        let gone = fixture.dir.path().join("super (deleted)");
        let mut systemctl = FakeSystemctl::default();

        let error = install(&fixture.env, &gone, &mut systemctl).unwrap_err();

        assert!(
            matches!(error, ServiceError::ExeMissing { .. }),
            "{error:?}"
        );
        assert!(systemctl.calls.is_empty());
    }

    #[test]
    fn macos_is_refused_with_a_pointer_to_the_app() {
        let fixture = Fixture::on(Platform::MacOs);
        let mut systemctl = FakeSystemctl::default();

        let install_error = install(&fixture.env, &fixture.exe, &mut systemctl).unwrap_err();
        let uninstall_error = uninstall(&fixture.env, &mut systemctl).unwrap_err();

        assert_eq!(
            install_error.to_string(),
            "super install is for Linux. On macOS the SUPER daemon ships with the SUPER app, \
             which starts it at login: install and open SUPER.app to run it."
        );
        assert!(uninstall_error.to_string().starts_with(
            "super uninstall is for Linux. On macOS the SUPER daemon belongs to the SUPER app"
        ));
        assert!(systemctl.calls.is_empty());
        assert!(!fixture.env.unit_dir.exists());
        assert_eq!(unit_status(&fixture.env, &mut systemctl), None);
    }

    #[test]
    fn windows_and_other_systems_are_refused() {
        let windows = unsupported(Platform::Windows, Action::Install);
        assert!(windows.contains("not available on Windows"), "{windows}");
        let other = unsupported(Platform::Other, Action::Uninstall);
        assert!(other.starts_with("super uninstall sets up a systemd user unit on Linux"));
        assert!(supported(Platform::Linux, Action::Install).is_ok());
    }

    #[test]
    fn uninstall_disables_stops_and_removes_the_unit() {
        let fixture = Fixture::linux();
        install(&fixture.env, &fixture.exe, &mut FakeSystemctl::healthy()).unwrap();
        let mut systemctl = FakeSystemctl::default();

        let uninstalled = uninstall(&fixture.env, &mut systemctl).unwrap();

        assert_eq!(
            uninstalled,
            Uninstalled::Removed {
                unit_path: fixture.env.unit_path(),
                stopped: true,
                problems: vec![],
            }
        );
        assert_eq!(fixture.unit(), None);
        assert_eq!(
            systemctl.calls,
            [
                "show-environment".to_string(),
                format!("disable --now {UNIT_NAME}"),
                "daemon-reload".into(),
                unit_args("reset-failed"),
            ]
        );
    }

    #[test]
    fn uninstall_reports_when_nothing_was_installed() {
        let fixture = Fixture::linux();
        let mut systemctl = FakeSystemctl::default();

        let uninstalled = uninstall(&fixture.env, &mut systemctl).unwrap();

        assert_eq!(
            uninstalled,
            Uninstalled::NothingInstalled {
                unit_path: fixture.env.unit_path()
            }
        );
        assert!(systemctl.calls.is_empty());
    }

    #[test]
    fn uninstall_without_a_manager_still_removes_the_unit_and_its_link() {
        let fixture = Fixture::linux();
        install(&fixture.env, &fixture.exe, &mut FakeSystemctl::healthy()).unwrap();
        // What `systemctl enable` would have left behind.
        let link = fixture.env.wants_link();
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(fixture.env.unit_path(), &link).unwrap();
        let mut systemctl = FakeSystemctl::no_manager();

        let uninstalled = uninstall(&fixture.env, &mut systemctl).unwrap();

        let Uninstalled::Removed {
            stopped, problems, ..
        } = uninstalled
        else {
            panic!("expected Removed, got {uninstalled:?}");
        };
        assert!(!stopped);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("nothing was stopped: systemctl --user did not reach"));
        assert_eq!(fixture.unit(), None);
        assert!(std::fs::symlink_metadata(&link).is_err());
        assert_eq!(systemctl.calls, ["show-environment"]);
    }

    #[test]
    fn a_failed_disable_is_reported_and_the_files_go_anyway() {
        let fixture = Fixture::linux();
        install(&fixture.env, &fixture.exe, &mut FakeSystemctl::healthy()).unwrap();
        let mut systemctl =
            FakeSystemctl::default().fails(&format!("disable --now {UNIT_NAME}"), "Boom\n");

        let uninstalled = uninstall(&fixture.env, &mut systemctl).unwrap();

        let Uninstalled::Removed {
            stopped, problems, ..
        } = uninstalled
        else {
            panic!("expected Removed, got {uninstalled:?}");
        };
        assert!(!stopped);
        assert_eq!(
            problems,
            [format!(
                "`systemctl --user disable --now {UNIT_NAME}` failed: Boom"
            )]
        );
        assert_eq!(fixture.unit(), None);
    }

    #[test]
    fn status_line_reports_the_unit() {
        let fixture = Fixture::linux();
        let line = |systemctl: &mut FakeSystemctl| {
            unit_status(&fixture.env, systemctl)
                .expect("a status on Linux")
                .line()
        };

        assert_eq!(
            line(&mut FakeSystemctl::default()),
            "service  not installed; `super install` starts the daemon at login"
        );

        install(&fixture.env, &fixture.exe, &mut FakeSystemctl::healthy()).unwrap();
        let path = fixture.env.unit_path();
        assert_eq!(
            line(&mut FakeSystemctl::healthy()),
            format!("service  enabled, active ({})", path.display())
        );
        let mut failed = FakeSystemctl::default()
            .answer(
                &unit_args("is-enabled"),
                Ok(SystemctlOutput {
                    success: true,
                    stdout: "enabled\n".into(),
                    stderr: String::new(),
                }),
            )
            .answer(
                &unit_args("is-active"),
                Ok(SystemctlOutput {
                    success: false,
                    stdout: "failed\n".into(),
                    stderr: String::new(),
                }),
            );
        assert_eq!(
            line(&mut failed),
            format!(
                "service  enabled, failed ({}); journalctl --user -u super-daemon shows why",
                path.display()
            )
        );
        assert_eq!(
            line(&mut FakeSystemctl::no_manager()),
            format!(
                "service  installed ({}), state unknown: no systemd user manager \
                 (Failed to connect to bus: No medium found)",
                path.display()
            )
        );
        assert_eq!(
            line(&mut FakeSystemctl::missing()),
            format!(
                "service  installed ({}), state unknown: systemctl not found",
                path.display()
            )
        );
    }

    #[test]
    fn daemon_answers_once_something_listens_on_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("d.sock");
        assert!(!daemon_answers(&socket, Duration::ZERO));
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(daemon_answers(&socket, Duration::ZERO));
    }
}
