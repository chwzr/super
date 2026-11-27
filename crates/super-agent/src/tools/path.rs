//! Path resolution helpers for tools.

use std::path::{Path, PathBuf};

/// Resolve a tool-supplied path against `cwd`, expanding a leading `~`.
pub fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let expanded: PathBuf = if let Ok(rest) = Path::new(path).strip_prefix("~") {
        dirs::home_dir()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| PathBuf::from(path))
    } else {
        PathBuf::from(path)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    }
}

/// The path a file tool (`Read`, `Edit`, `Write`) was given, which must be
/// absolute. Any other path is refused with an error that names the
/// absolute path it most likely means.
pub fn absolute(cwd: &Path, path: &str) -> anyhow::Result<PathBuf> {
    if path.trim().is_empty() {
        anyhow::bail!("file_path is empty; give the absolute path of the file.");
    }
    let given = PathBuf::from(path);
    if given.is_absolute() {
        return Ok(given);
    }
    anyhow::bail!(
        "file_path must be an absolute path, and {path} is not; give the full path, such as {}.",
        resolve(cwd, path).display()
    )
}

/// The key a file is known by in a session's record of reads: its canonical
/// path when it exists, so that two spellings of one file (a symbolic link,
/// `..` in the path) share one entry; otherwise the path as given.
pub async fn file_key(path: &Path) -> PathBuf {
    tokio::fs::canonicalize(path)
        .await
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Render a path relative to `cwd` when it is inside it (display only).
pub fn display(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd).unwrap_or(path).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_and_absolute() {
        let cwd = Path::new("/work");
        assert_eq!(
            resolve(cwd, "src/main.rs"),
            PathBuf::from("/work/src/main.rs")
        );
        assert_eq!(resolve(cwd, "/etc/hosts"), PathBuf::from("/etc/hosts"));
    }

    #[test]
    fn file_tools_take_absolute_paths_only() {
        let cwd = std::env::temp_dir();
        let inside = cwd.join("main.rs");
        assert_eq!(
            absolute(&cwd, &inside.display().to_string()).unwrap(),
            inside
        );
        let error = absolute(&cwd, "src/main.rs").unwrap_err().to_string();
        assert!(error.contains("must be an absolute path"), "{error}");
        assert!(
            error.contains(&cwd.join("src/main.rs").display().to_string()),
            "{error}"
        );
        assert!(absolute(&cwd, "~/notes.txt").is_err());
        assert!(
            absolute(&cwd, " ")
                .unwrap_err()
                .to_string()
                .contains("empty")
        );
    }

    #[tokio::test]
    async fn a_file_has_one_key_under_every_spelling() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("f.txt"), "x").unwrap();
        let direct = file_key(&dir.path().join("f.txt")).await;
        let around = file_key(&dir.path().join("sub/../f.txt")).await;
        assert_eq!(direct, around);
        let missing = dir.path().join("missing.txt");
        assert_eq!(file_key(&missing).await, missing);
    }
}
