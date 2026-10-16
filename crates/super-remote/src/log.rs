//! Opt-in diagnostics. `SUPER_LOG=remote` (or `remote=debug`) appends one
//! line per message to `~/.super/agent/logs/remote.log`. Tokens and record
//! bodies are never logged.

use std::io::Write as _;
use std::sync::OnceLock;

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("SUPER_LOG")
            .map(|value| {
                value
                    .split(',')
                    .any(|part| part.trim().starts_with("remote"))
            })
            .unwrap_or(false)
    })
}

pub(crate) fn line(message: impl AsRef<str>) {
    if !enabled() {
        return;
    }
    let Some(home) = dirs::home_dir() else { return };
    let dir = home.join(".super/agent/logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("remote.log"))
    {
        let _ = writeln!(
            file,
            "{} {}",
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ"),
            message.as_ref()
        );
    }
}
