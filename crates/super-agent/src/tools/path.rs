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

/// `path` without the `\\?\` that Windows puts in front of a canonical path,
/// which git, Git Bash and a file name made from the path cannot take:
/// `\\?\C:\work` is `C:\work` and `\\?\UNC\server\share` is
/// `\\server\share`. Any other path, and every path on other systems, is
/// returned as it is.
pub fn without_verbatim_prefix(path: &Path) -> PathBuf {
    if !cfg!(windows) {
        return path.to_path_buf();
    }
    path.to_str()
        .and_then(strip_verbatim)
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

/// The text of a `\\?\` drive or UNC path without the prefix; `None` for
/// any other text, such as a `\\?\Volume{…}` path, which has no other
/// spelling.
fn strip_verbatim(text: &str) -> Option<String> {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\{rest}"));
    }
    let rest = text.strip_prefix(r"\\?\")?;
    let drive = rest.as_bytes();
    (drive.len() >= 2 && drive[0].is_ascii_alphabetic() && drive[1] == b':')
        .then(|| rest.to_string())
}

/// Render a path relative to `cwd` when it is inside it (display only).
pub fn display(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd).unwrap_or(path).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verbatim_windows_path_loses_its_prefix_and_other_text_stays() {
        assert_eq!(
            strip_verbatim(r"\\?\C:\Users\me\work").as_deref(),
            Some(r"C:\Users\me\work")
        );
        assert_eq!(
            strip_verbatim(r"\\?\UNC\server\share\work").as_deref(),
            Some(r"\\server\share\work")
        );
        assert_eq!(strip_verbatim(r"\\?\Volume{1234}\work"), None);
        assert_eq!(strip_verbatim(r"C:\Users\me"), None);
        assert_eq!(strip_verbatim("/work"), None);
        // Only Windows has such paths.
        if !cfg!(windows) {
            assert_eq!(
                without_verbatim_prefix(Path::new(r"\\?\C:\work")),
                PathBuf::from(r"\\?\C:\work")
            );
        }
    }

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
