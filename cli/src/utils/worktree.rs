use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use crate::utils::git::{git, git_in_dir};

// ── Session state ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WorktreeSession {
    pub original_cwd: PathBuf,
    pub worktree_path: PathBuf,
    pub worktree_name: String,
    pub worktree_branch: String,
    pub original_head_commit: String,
    pub session_id: String,
}

static WORKTREE_SESSION: OnceLock<RwLock<Option<WorktreeSession>>> = OnceLock::new();

fn session_lock() -> &'static RwLock<Option<WorktreeSession>> {
    WORKTREE_SESSION.get_or_init(|| RwLock::new(None))
}

pub fn get_session() -> Option<WorktreeSession> {
    session_lock().read().unwrap().clone()
}

pub fn set_session(session: Option<WorktreeSession>) {
    *session_lock().write().unwrap() = session;
}

// ── Validation ─────────────────────────────────────────────────────────────

/// Validate a worktree slug. Each `/`-separated segment must contain only
/// `[a-zA-Z0-9._-]`, no empty segments, no `.` or `..`, max 64 chars total.
pub fn validate_worktree_slug(name: &str) -> Result<String, String> {
    if name.is_empty() {
        return Err("worktree name must not be empty".into());
    }
    if name.len() > 64 {
        return Err(format!(
            "worktree name too long ({} chars, max 64)",
            name.len()
        ));
    }
    for segment in name.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(format!("invalid segment in worktree name: '{segment}'"));
        }
        if !segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            return Err(format!(
                "invalid character in worktree name segment: '{segment}' — \
                 only [a-zA-Z0-9._-] allowed"
            ));
        }
    }
    Ok(name.replace('/', "+"))
}

// ── Repository root ────────────────────────────────────────────────────────

/// Walk up from `cwd` to find the canonical git root (the directory containing
/// `.git`). If `cwd` is inside a worktree, follow the `.git` file back to the
/// main repo and return that instead.
pub async fn find_canonical_git_root(cwd: &Path) -> Result<PathBuf, String> {
    let output = git_in_dir(cwd, &["rev-parse", "--show-toplevel"])
        .await
        .map_err(|e| format!("not in a git repository: {e}"))?;
    let toplevel = PathBuf::from(output);
    // Check if .git is a file (worktree pointer)
    let git_path = toplevel.join(".git");
    if git_path.is_file() {
        // Read the gitdir pointer: "gitdir: /path/to/main/.git/worktrees/name"
        let contents = std::fs::read_to_string(&git_path)
            .map_err(|e| format!("failed to read .git file: {e}"))?;
        if let Some(gitdir_line) = contents.strip_prefix("gitdir: ") {
            let gitdir_path = Path::new(gitdir_line.trim());
            // Walk up from .git/worktrees/name to find the main repo .git
            if let Some(main_git) = gitdir_path.parent().and_then(|p| p.parent()) {
                if let Some(main_repo) = main_git.parent() {
                    return Ok(main_repo.to_path_buf());
                }
            }
        }
    }
    Ok(toplevel)
}

// ── Base ref resolution ────────────────────────────────────────────────────

async fn resolve_base_ref(repo_root: &Path) -> Result<String, String> {
    // Try reading origin/HEAD from .git/refs directly first
    let origin_head = repo_root.join(".git/refs/remotes/origin/HEAD");
    if let Ok(contents) = std::fs::read_to_string(&origin_head) {
        let reference = contents.trim();
        if let Some(branch) = reference.strip_prefix("ref: refs/remotes/origin/") {
            return Ok(format!("origin/{branch}"));
        }
    }
    // Fallback: determine default branch by reading symbolic-ref
    match git_in_dir(repo_root, &["symbolic-ref", "refs/remotes/origin/HEAD"]).await {
        Ok(output) => {
            if let Some(branch) = output.strip_prefix("refs/remotes/origin/") {
                return Ok(format!("origin/{branch}"));
            }
        }
        Err(_) => {
            // Fetch origin and try again
            let _ = git_in_dir(repo_root, &["fetch", "origin"]).await;
            if let Ok(output) =
                git_in_dir(repo_root, &["symbolic-ref", "refs/remotes/origin/HEAD"]).await
            {
                if let Some(branch) = output.strip_prefix("refs/remotes/origin/") {
                    return Ok(format!("origin/{branch}"));
                }
            }
        }
    }
    Ok("HEAD".to_string())
}

// ── Worktree creation ──────────────────────────────────────────────────────

/// Create a new worktree at `.claude/worktrees/<flat_slug>` on branch
/// `worktree-<flat_slug>` from the resolved base ref.
pub async fn get_or_create_worktree(
    repo_root: &Path,
    flat_slug: &str,
) -> Result<WorktreeSession, String> {
    let worktrees_dir = repo_root.join(".claude/worktrees");
    let worktree_path = worktrees_dir.join(flat_slug);
    let branch_name = format!("worktree-{flat_slug}");

    // Fast resume: if the directory already exists, read the .git pointer
    if worktree_path.exists() {
        let git_file = worktree_path.join(".git");
        if let Ok(contents) = std::fs::read_to_string(&git_file) {
            if contents.strip_prefix("gitdir: ").is_some() {
                // Worktree exists — construct session from it
                let _head = git_in_dir(&worktree_path, &["rev-parse", "HEAD"])
                    .await
                    .map_err(|e| format!("failed to read HEAD in existing worktree: {e}"))?;
                let original_head = git_in_dir(repo_root, &["rev-parse", "HEAD"])
                    .await
                    .map_err(|e| format!("failed to read HEAD: {e}"))?;
                return Ok(WorktreeSession {
                    original_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                    worktree_path,
                    worktree_name: flat_slug.to_string(),
                    worktree_branch: branch_name,
                    original_head_commit: original_head,
                    session_id: uuid::Uuid::new_v4().to_string(),
                });
            }
        }
    }

    // Ensure .claude/worktrees directory exists
    std::fs::create_dir_all(&worktrees_dir)
        .map_err(|e| format!("failed to create .claude/worktrees/: {e}"))?;

    let base = resolve_base_ref(repo_root).await?;
    let original_head = git_in_dir(repo_root, &["rev-parse", "HEAD"])
        .await
        .map_err(|e| format!("failed to read HEAD: {e}"))?;

    git_in_dir(
        repo_root,
        &[
            "worktree",
            "add",
            "-B",
            &branch_name,
            worktree_path.to_str().unwrap_or(""),
            &base,
        ],
    )
    .await
    .map_err(|e| format!("failed to create worktree: {e}"))?;

    Ok(WorktreeSession {
        original_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        worktree_path,
        worktree_name: flat_slug.to_string(),
        worktree_branch: branch_name,
        original_head_commit: original_head,
        session_id: uuid::Uuid::new_v4().to_string(),
    })
}

// ── Enter existing worktree ────────────────────────────────────────────────

/// Validate that `path` is registered in `git worktree list`, then build a session.
pub async fn enter_existing_worktree(
    repo_root: &Path,
    path: &Path,
) -> Result<WorktreeSession, String> {
    let list_output = git_in_dir(repo_root, &["worktree", "list", "--porcelain"])
        .await
        .map_err(|e| format!("failed to list worktrees: {e}"))?;

    let canonical_path = path
        .canonicalize()
        .map_err(|e| format!("invalid path: {e}"))?;

    // Parse porcelain output: each worktree starts with "worktree <path>"
    let mut found = false;
    for line in list_output.lines() {
        if let Some(wt_path) = line.strip_prefix("worktree ") {
            let wt_canonical = Path::new(wt_path)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(wt_path));
            if wt_canonical == canonical_path {
                found = true;
                break;
            }
        }
    }

    if !found {
        return Err(format!(
            "path '{}' is not a registered git worktree of this repository",
            path.display()
        ));
    }

    let _head = git_in_dir(path, &["rev-parse", "HEAD"])
        .await
        .map_err(|e| format!("failed to read HEAD: {e}"))?;
    let branch = git_in_dir(path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .unwrap_or_else(|_| "detached".to_string());
    let original_head = git_in_dir(repo_root, &["rev-parse", "HEAD"])
        .await
        .map_err(|e| format!("failed to read HEAD: {e}"))?;

    Ok(WorktreeSession {
        original_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        worktree_path: canonical_path.clone(),
        worktree_name: canonical_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        worktree_branch: branch,
        original_head_commit: original_head,
        session_id: uuid::Uuid::new_v4().to_string(),
    })
}

// ── Keep / cleanup ─────────────────────────────────────────────────────────

/// Return to original CWD and clear the session without removing anything.
pub fn keep_worktree(session: &WorktreeSession) {
    let _ = std::env::set_current_dir(&session.original_cwd);
    crate::utils::cwd::set_cwd(session.original_cwd.clone()).ok();
    set_session(None);
}

/// Return to original CWD, remove the worktree and its branch, clear session.
pub async fn cleanup_worktree(session: &WorktreeSession) -> Result<(), String> {
    let _ = std::env::set_current_dir(&session.original_cwd);
    crate::utils::cwd::set_cwd(session.original_cwd.clone()).ok();

    // git worktree remove --force
    let _ = git(&[
        "worktree",
        "remove",
        "--force",
        session.worktree_path.to_str().unwrap_or(""),
    ])
    .await;

    // git branch -D the worktree branch
    let _ = git(&["branch", "-D", &session.worktree_branch]).await;

    set_session(None);
    Ok(())
}

// ── Change detection ────────────────────────────────────────────────────────

pub struct WorktreeChanges {
    pub uncommitted_files: Vec<String>,
    pub commits_ahead: usize,
}

/// Count uncommitted changes and commits ahead of the original HEAD.
pub async fn count_worktree_changes(
    worktree_path: &Path,
    original_head_commit: &str,
) -> Result<WorktreeChanges, String> {
    // Uncommitted files (tracked modified + untracked)
    let status = git_in_dir(worktree_path, &["status", "--porcelain"])
        .await
        .map_err(|e| format!("failed to check status: {e}"))?;
    let uncommitted_files: Vec<String> = status
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();

    // Commits on the worktree branch not on the original branch
    let count_str = git_in_dir(
        worktree_path,
        &[
            "rev-list",
            "--count",
            &format!("{original_head_commit}..HEAD"),
        ],
    )
    .await
    .map_err(|e| format!("failed to count commits: {e}"))?;
    let commits_ahead: usize = count_str.parse().unwrap_or(0);

    Ok(WorktreeChanges {
        uncommitted_files,
        commits_ahead,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_valid_slug() {
        assert!(validate_worktree_slug("my-feature").is_ok());
        assert!(validate_worktree_slug("fix_bug_123").is_ok());
        assert!(validate_worktree_slug("a.b.c").is_ok());
    }

    #[test]
    fn validate_rejects_empty() {
        assert!(validate_worktree_slug("").is_err());
    }

    #[test]
    fn validate_rejects_dot_segments() {
        assert!(validate_worktree_slug("foo/../bar").is_err());
        assert!(validate_worktree_slug(".").is_err());
    }

    #[test]
    fn validate_rejects_invalid_chars() {
        assert!(validate_worktree_slug("my feature").is_err());
        assert!(validate_worktree_slug("foo!").is_err());
    }

    #[test]
    fn validate_rejects_too_long() {
        let long = "a".repeat(65);
        assert!(validate_worktree_slug(&long).is_err());
    }

    #[test]
    fn validate_flattens_slashes() {
        let result = validate_worktree_slug("foo/bar").unwrap();
        assert_eq!(result, "foo+bar");
    }

    #[test]
    fn validate_accepts_max_length() {
        let max = "a".repeat(64);
        assert!(validate_worktree_slug(&max).is_ok());
    }
}
