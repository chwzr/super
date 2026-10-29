//! launchd and systemd start the daemon with a bare environment. The sessions
//! it hosts spawn the user's tools (`git`, `node`, ...), so the daemon reads
//! the login shell's PATH once at start and adopts it.

use std::process::{Command, Stdio};
use std::time::Duration;

/// The PATH the user's login shell exports, or `None` when `SHELL` is unset
/// or the shell does not answer within five seconds.
pub fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|value| !value.is_empty())?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let output = Command::new(shell)
            .args(["-lc", "printf '%s' \"$PATH\""])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        let _ = sender.send(output);
    });
    let output = receiver.recv_timeout(Duration::from_secs(5)).ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// Login-shell entries first, then whatever the current PATH adds, each once.
pub fn merge_paths(login: &str, current: &str) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for entry in login.split(':').chain(current.split(':')) {
        if !entry.is_empty() && !seen.contains(&entry) {
            seen.push(entry);
        }
    }
    seen.join(":")
}

/// Resolve the login shell's PATH and set it on this process. Call this
/// before any other thread exists: setting an environment variable while
/// another thread may read the environment is unsound.
pub fn apply_login_shell_path() -> Option<String> {
    let login = login_shell_path()?;
    let current = std::env::var("PATH").unwrap_or_default();
    let merged = merge_paths(&login, &current);
    // SAFETY: the caller runs this before the tokio runtime is built, so no
    // other thread of this process reads the environment concurrently.
    unsafe { std::env::set_var("PATH", &merged) };
    Some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_entries_come_first_and_nothing_repeats() {
        let merged = merge_paths(
            "/opt/homebrew/bin:/usr/bin",
            "/usr/bin:/bin:/opt/homebrew/bin:/usr/sbin",
        );
        assert_eq!(merged, "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin");
        assert_eq!(merge_paths("", "/bin"), "/bin");
    }
}
