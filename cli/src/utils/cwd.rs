use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

static CURRENT_CWD: OnceLock<RwLock<PathBuf>> = OnceLock::new();

fn init() -> &'static RwLock<PathBuf> {
    CURRENT_CWD.get_or_init(|| {
        let initial = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        RwLock::new(initial)
    })
}

/// Returns the atomically-tracked CWD. Falls back to `std::env::current_dir()`
/// on the first call if not yet initialized.
pub fn get_cwd() -> PathBuf {
    init().read().unwrap().clone()
}

/// Sets the atomically-tracked CWD AND calls `std::env::set_current_dir()`.
pub fn set_cwd(path: PathBuf) -> std::io::Result<()> {
    std::env::set_current_dir(&path)?;
    if let Some(lock) = CURRENT_CWD.get() {
        *lock.write().unwrap() = path;
    } else {
        // If init hasn't been called yet, force-init with the new path
        let _ = CURRENT_CWD.set(RwLock::new(path));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_cwd_returns_current_dir_initially() {
        let cwd = get_cwd();
        assert!(cwd.is_absolute());
    }

    #[test]
    fn set_cwd_fails_on_nonexistent_path() {
        let result = set_cwd(PathBuf::from("/nonexistent/path/xyzzy"));
        assert!(result.is_err());
    }
}
