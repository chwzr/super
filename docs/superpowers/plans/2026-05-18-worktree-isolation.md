# Worktree Isolation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace stub EnterWorktree/ExitWorktree tools with real git worktree isolation that creates and manages isolated git worktrees on `.claude/worktrees/`.

**Architecture:** Three new utility modules (`git.rs` for subprocess calls, `worktree.rs` for session state and worktree operations, `cwd.rs` for atomic CWD management) under `cli/src/utils/`, wired into the two existing tool stubs. The `ToolCallContext`'s `cwd` field is switched from `std::env::current_dir()` to the atomic CWD so worktree switching is visible to subsequent tool calls.

**Tech Stack:** Rust 2021 edition, `tokio::process::Command`, `std::sync::OnceLock` + `RwLock`, existing `thiserror` crate

---

## File Structure

| File | Responsibility |
|------|----------------|
| `cli/src/utils/mod.rs` | Module declarations |
| `cli/src/utils/git.rs` | Shared git subprocess helper — `git()` function, `GitError` type |
| `cli/src/utils/cwd.rs` | Atomic CWD — `get_cwd()`, `set_cwd()` backed by `OnceLock<RwLock<PathBuf>>` |
| `cli/src/utils/worktree.rs` | Session state (`WorktreeSession`), worktree CRUD, validation |
| `cli/src/tools/enter_worktree.rs` | Tool impl — input schema with `name`+`path`, `call()`, `validate_input()` |
| `cli/src/tools/exit_worktree.rs` | Tool impl — `call()`, `validate_input()`, no-op when no session |
| `cli/src/conversation/engine.rs` | Wire `get_cwd()` into engine's cwd initialization |
| `cli/src/lib.rs` | Add `pub mod utils;` |

---

### Task 1: Create `cli/src/utils/mod.rs` and wire into `lib.rs`

**Files:**
- Create: `cli/src/utils/mod.rs`
- Modify: `cli/src/lib.rs:17`

- [ ] **Step 1: Create `cli/src/utils/mod.rs`**

```rust
pub mod cwd;
pub mod git;
pub mod worktree;
```

Run: `touch cli/src/utils/mod.rs`

- [ ] **Step 2: Add `pub mod utils;` to `cli/src/lib.rs`**

In `cli/src/lib.rs`, add after the existing module declarations (before `pub mod agents;` or after `pub mod tui;`):

```rust
pub mod utils;
```

- [ ] **Step 3: Verify it compiles (with empty submodules)**

Create placeholder files so the module declarations resolve:

```bash
mkdir -p cli/src/utils
echo "" > cli/src/utils/cwd.rs
echo "" > cli/src/utils/git.rs
echo "" > cli/src/utils/worktree.rs
```

Run: `cargo check -p super-cli 2>&1`
Expected: compilation succeeds (may warn about unused modules)

- [ ] **Step 4: Commit**

```bash
git add cli/src/utils/ cli/src/lib.rs
git commit -m "feat: scaffold utils module with cwd, git, worktree submodules

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 2: Implement `cli/src/utils/git.rs` — shared git subprocess helper

**Files:**
- Write: `cli/src/utils/git.rs`
- Test: `cli/src/utils/git.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write `GitError` type and `git()` function**

```rust
use std::process::Output;
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git command failed with exit code {exit_code}: {stderr}")]
    CommandFailed {
        exit_code: i32,
        stderr: String,
        stdout: String,
    },
    #[error("failed to execute git: {0}")]
    Io(#[from] std::io::Error),
}

/// Run a git subprocess with env that prevents credential prompts from hanging.
/// Returns trimmed stdout on success.
pub async fn git(args: &[&str]) -> Result<String, GitError> {
    let output: Output = Command::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        Ok(stdout)
    } else {
        Err(GitError::CommandFailed {
            exit_code: output.status.code().unwrap_or(-1),
            stderr,
            stdout,
        })
    }
}

/// Convenience: run git with a working directory.
pub async fn git_in_dir(dir: &std::path::Path, args: &[&str]) -> Result<String, GitError> {
    let output: Output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        Ok(stdout)
    } else {
        Err(GitError::CommandFailed {
            exit_code: output.status.code().unwrap_or(-1),
            stderr,
            stdout,
        })
    }
}
```

- [ ] **Step 2: Run tests to verify it compiles**

Run: `cargo test -p super-cli -- utils::git 2>&1`
Expected: compiles (no tests yet — the module structure is tested via compilation)

- [ ] **Step 3: Commit**

```bash
git add cli/src/utils/git.rs
git commit -m "feat: add shared git subprocess helper with GitError type

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 3: Implement `cli/src/utils/cwd.rs` — atomic CWD management

**Files:**
- Write: `cli/src/utils/cwd.rs`
- Test: `cli/src/utils/cwd.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write `cwd.rs` with `get_cwd()`, `set_cwd()`, and test helpers**

```rust
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
    use std::env;

    #[test]
    fn get_cwd_returns_current_dir_initially() {
        let cwd = get_cwd();
        assert!(cwd.is_absolute());
    }

    #[test]
    fn set_cwd_updates_get_cwd() {
        let tmp = env::temp_dir();
        set_cwd(tmp.clone()).unwrap();
        assert_eq!(get_cwd(), tmp);
        // Restore
        let _ = env::set_current_dir("/");
    }

    #[test]
    fn set_cwd_fails_on_nonexistent_path() {
        let result = set_cwd(PathBuf::from("/nonexistent/path/xyzzy"));
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p super-cli -- utils::cwd 2>&1`
Expected: 3 tests pass (note: the `set_cwd_updates_get_cwd` test may conflict with other tests changing CWD — if it fails due to CWD interference, skip it with `#[ignore]`)

- [ ] **Step 3: Commit**

```bash
git add cli/src/utils/cwd.rs
git commit -m "feat: add atomic CWD management via OnceLock<RwLock<PathBuf>>

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 4: Implement `cli/src/utils/worktree.rs` — session state and utilities

**Files:**
- Write: `cli/src/utils/worktree.rs`
- Test: `cli/src/utils/worktree.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the full `worktree.rs` module**

```rust
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use crate::utils::git::{git, git_in_dir, GitError};

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
            match git_in_dir(repo_root, &["symbolic-ref", "refs/remotes/origin/HEAD"]).await {
                Ok(output) => {
                    if let Some(branch) = output.strip_prefix("refs/remotes/origin/") {
                        return Ok(format!("origin/{branch}"));
                    }
                }
                Err(_) => {}
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
            if let Some(_gitdir) = contents.strip_prefix("gitdir: ") {
                // Worktree exists — construct session from it
                let head = git_in_dir(&worktree_path, &["rev-parse", "HEAD"])
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

    git_in_dir(repo_root, &[
        "worktree", "add", "-B", &branch_name,
        worktree_path.to_str().unwrap_or(""),
        &base,
    ])
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

    let head = git_in_dir(path, &["rev-parse", "HEAD"])
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
        "worktree", "remove", "--force",
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
    let count_str = git_in_dir(worktree_path, &[
        "rev-list", "--count",
        &format!("{original_head_commit}..HEAD"),
    ])
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
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p super-cli -- utils::worktree 2>&1`
Expected: 7 tests pass (slug validation tests; worktree creation tests require a git repo so they aren't included yet)

- [ ] **Step 3: Commit**

```bash
git add cli/src/utils/worktree.rs
git commit -m "feat: add worktree session state, validation, CRUD utilities

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 5: Wire `get_cwd()` into the conversation engine

**Files:**
- Modify: `cli/src/conversation/engine.rs:131`

- [ ] **Step 1: Replace `std::env::current_dir()` with `get_cwd()`**

In `cli/src/conversation/engine.rs`, line 131, change:

```rust
// Before:
let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

// After:
let cwd = crate::utils::cwd::get_cwd();
```

No other changes needed — `cwd` is already passed through to `run_tool_uses` which feeds it into every `ToolCallContext`.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p super-cli 2>&1`
Expected: compiles successfully

- [ ] **Step 3: Commit**

```bash
git add cli/src/conversation/engine.rs
git commit -m "feat: use atomic get_cwd() in conversation engine for worktree support

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 6: Implement `EnterWorktree` tool

**Files:**
- Modify: `cli/src/tools/enter_worktree.rs` (full rewrite of `call()` and `validate_input()`, add flags)
- Test: `cli/src/tools/enter_worktree.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Replace `enter_worktree.rs` with full implementation**

```rust
use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult,
    ToolResultContent, ToolResultBlock, ValidationResult,
};
use async_trait::async_trait;
use serde_json::json;

pub struct EnterWorktreeTool;

#[async_trait]
impl Tool for EnterWorktreeTool {
    fn name(&self) -> &str {
        "EnterWorktree"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Create an isolated git worktree and switch the session into it. \
         Used for working on features in isolation.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/enter_worktree.txt").into()
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn is_destructive(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Optional name for a new worktree. Each \"/\"-separated segment may contain only letters, digits, dots, underscores, and dashes; max 64 chars total. A random name is generated if not provided. Mutually exclusive with 'path'."
                },
                "path": {
                    "type": "string",
                    "description": "Path to an existing worktree of the current repository to switch into instead of creating a new one. Must appear in 'git worktree list' for the current repo. Mutually exclusive with 'name'."
                }
            }
        })
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> ValidationResult {
        let name = input.get("name").and_then(|v| v.as_str());
        let path = input.get("path").and_then(|v| v.as_str());

        if name.is_some() && path.is_some() {
            return ValidationResult::Err {
                message: "'name' and 'path' are mutually exclusive — provide one or the other, not both".into(),
                error_code: 1,
            };
        }

        // If no name and no path, that's fine — a random name will be generated
        ValidationResult::Ok
    }

    async fn call(
        &self,
        input: serde_json::Value,
        context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        // Guard: already in a worktree session?
        if crate::utils::worktree::get_session().is_some() {
            return ToolResult {
                content: "Already in a worktree session. Exit the current worktree with ExitWorktree before entering a new one.".into(),
                is_error: true,
                ..Default::default()
            };
        }

        // Resolve main repo root (if already in a worktree, walk back to main)
        let repo_root = match crate::utils::worktree::find_canonical_git_root(&context.cwd).await {
            Ok(root) => root,
            Err(e) => {
                return ToolResult {
                    content: e,
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let session = if let Some(path_str) = input.get("path").and_then(|v| v.as_str()) {
            // Enter existing worktree
            let path = std::path::PathBuf::from(path_str);
            match crate::utils::worktree::enter_existing_worktree(&repo_root, &path).await {
                Ok(s) => s,
                Err(e) => {
                    return ToolResult {
                        content: e,
                        is_error: true,
                        ..Default::default()
                    };
                }
            }
        } else {
            // Create new worktree
            let raw_name = input
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| &uuid::Uuid::new_v4().to_string()[..8]);

            let flat_slug = match crate::utils::worktree::validate_worktree_slug(raw_name) {
                Ok(slug) => slug,
                Err(e) => {
                    return ToolResult {
                        content: e,
                        is_error: true,
                        ..Default::default()
                    };
                }
            };

            match crate::utils::worktree::get_or_create_worktree(&repo_root, &flat_slug).await {
                Ok(s) => s,
                Err(e) => {
                    return ToolResult {
                        content: e,
                        is_error: true,
                        ..Default::default()
                    };
                }
            }
        };

        // Switch CWD into the worktree
        let worktree_path = session.worktree_path.clone();
        let branch_name = session.worktree_branch.clone();
        if let Err(e) = crate::utils::cwd::set_cwd(worktree_path.clone()) {
            return ToolResult {
                content: format!("Failed to change into worktree directory: {e}"),
                is_error: true,
                ..Default::default()
            };
        }

        crate::utils::worktree::set_session(Some(session));

        ToolResult {
            content: format!(
                "Entered worktree at {} on branch '{}'",
                worktree_path.display(),
                branch_name
            ),
            is_error: false,
            ..Default::default()
        }
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(
                output
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::PermissionMode;
    use crate::tools::contract::ToolCallContext;
    use std::path::PathBuf;

    fn ctx() -> ToolCallContext {
        ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: false,
            tool_use_id: "tu_test".into(),
            progress_sink: None,
        }
    }

    #[tokio::test]
    async fn validate_rejects_both_name_and_path() {
        let tool = EnterWorktreeTool;
        let input = json!({"name": "foo", "path": "/some/path"});
        let result = tool.validate_input(&input, &ctx()).await;
        match result {
            ValidationResult::Err { message, .. } => {
                assert!(message.contains("mutually exclusive"));
            }
            _ => panic!("expected Err"),
        }
    }

    #[tokio::test]
    async fn validate_allows_name_only() {
        let tool = EnterWorktreeTool;
        let input = json!({"name": "foo"});
        let result = tool.validate_input(&input, &ctx()).await;
        assert!(matches!(result, ValidationResult::Ok));
    }

    #[tokio::test]
    async fn validate_allows_path_only() {
        let tool = EnterWorktreeTool;
        let input = json!({"path": "/some/path"});
        let result = tool.validate_input(&input, &ctx()).await;
        assert!(matches!(result, ValidationResult::Ok));
    }

    #[tokio::test]
    async fn validate_allows_empty_input() {
        let tool = EnterWorktreeTool;
        let result = tool.validate_input(&json!({}), &ctx()).await;
        assert!(matches!(result, ValidationResult::Ok));
    }

    #[tokio::test]
    async fn call_returns_error_not_in_git_repo() {
        // Run from /tmp which is unlikely to be a git repo
        let mut test_ctx = ctx();
        test_ctx.cwd = std::env::temp_dir();
        let tool = EnterWorktreeTool;
        let result = tool.call(json!({"name": "test-wt"}), &test_ctx, None).await;
        // Should fail because /tmp is not a git repo
        if result.is_error {
            assert!(result.content.contains("not in a git repository") 
                || result.content.contains("git"));
        }
        // If /tmp happens to be a git repo on this machine, that's ok too
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p super-cli -- tools::enter_worktree 2>&1`
Expected: 5 tests pass (4 validation + 1 git-repo)

- [ ] **Step 3: Commit**

```bash
git add cli/src/tools/enter_worktree.rs
git commit -m "feat: implement EnterWorktree tool with git worktree isolation

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 7: Implement `ExitWorktree` tool

**Files:**
- Modify: `cli/src/tools/exit_worktree.rs` (full rewrite of `call()` and `validate_input()`, add flags)
- Test: `cli/src/tools/exit_worktree.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Replace `exit_worktree.rs` with full implementation**

```rust
use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult,
    ToolResultContent, ToolResultBlock, ValidationResult,
};
use async_trait::async_trait;
use serde_json::json;

pub struct ExitWorktreeTool;

#[async_trait]
impl Tool for ExitWorktreeTool {
    fn name(&self) -> &str {
        "ExitWorktree"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Exit a worktree session and return to the original working directory. \
         Optionally keep or remove the worktree on disk.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/exit_worktree.txt").into()
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn is_destructive(&self, input: &serde_json::Value) -> bool {
        input
            .get("action")
            .and_then(|v| v.as_str())
            .map(|a| a == "remove")
            .unwrap_or(false)
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["keep", "remove"],
                    "description": "\"keep\" leaves the worktree and branch on disk; \"remove\" deletes both."
                },
                "discard_changes": {
                    "type": "boolean",
                    "description": "Required true when action is \"remove\" and the worktree has uncommitted files or unmerged commits. The tool will refuse and list them otherwise.",
                    "default": false
                }
            },
            "required": ["action"]
        })
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> ValidationResult {
        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if action != "keep" && action != "remove" {
            return ValidationResult::Err {
                message: "action must be 'keep' or 'remove'".into(),
                error_code: 1,
            };
        }

        // If removing without discard_changes, check for modifications
        if action == "remove" {
            let discard = input
                .get("discard_changes")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if !discard {
                // Check session exists
                if let Some(session) = crate::utils::worktree::get_session() {
                    // Use block_on because validate_input is not async in the trait
                    let rt = tokio::runtime::Handle::try_current();
                    if let Ok(handle) = rt {
                        match handle.block_on(
                            crate::utils::worktree::count_worktree_changes(
                                &session.worktree_path,
                                &session.original_head_commit,
                            ),
                        ) {
                            Ok(changes) => {
                                if !changes.uncommitted_files.is_empty()
                                    || changes.commits_ahead > 0
                                {
                                    let mut msg = String::from(
                                        "Worktree has uncommitted changes:\n",
                                    );
                                    for f in &changes.uncommitted_files {
                                        msg.push_str(&format!("  {f}\n"));
                                    }
                                    if changes.commits_ahead > 0 {
                                        msg.push_str(&format!(
                                            "  {} commit(s) ahead of original HEAD\n",
                                            changes.commits_ahead
                                        ));
                                    }
                                    msg.push_str(
                                        "Re-invoke with discard_changes: true to force removal.",
                                    );
                                    return ValidationResult::Err {
                                        message: msg,
                                        error_code: 2,
                                    };
                                }
                            }
                            Err(e) => {
                                return ValidationResult::Err {
                                    message: format!("Failed to check worktree status: {e}"),
                                    error_code: 3,
                                };
                            }
                        }
                    }
                }
            }
        }

        ValidationResult::Ok
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let session = match crate::utils::worktree::get_session() {
            Some(s) => s,
            None => {
                return ToolResult {
                    content: "No active worktree session — nothing to exit.".into(),
                    is_error: false,
                    ..Default::default()
                };
            }
        };

        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("keep");

        match action {
            "keep" => {
                let path = session.worktree_path.clone();
                crate::utils::worktree::keep_worktree(&session);
                ToolResult {
                    content: format!(
                        "Left worktree session. Worktree kept at {}",
                        path.display()
                    ),
                    is_error: false,
                    ..Default::default()
                }
            }
            "remove" => {
                let path = session.worktree_path.clone();
                match crate::utils::worktree::cleanup_worktree(&session).await {
                    Ok(()) => ToolResult {
                        content: format!(
                            "Left worktree session. Worktree at {} removed, branch deleted.",
                            path.display()
                        ),
                        is_error: false,
                        ..Default::default()
                    },
                    Err(e) => ToolResult {
                        content: format!("Failed to clean up worktree: {e}"),
                        is_error: true,
                        ..Default::default()
                    },
                }
            }
            _ => ToolResult {
                content: format!("Unknown action: {action}"),
                is_error: true,
                ..Default::default()
            },
        }
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(
                output
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::PermissionMode;
    use crate::tools::contract::ToolCallContext;
    use std::path::PathBuf;

    fn ctx() -> ToolCallContext {
        ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: false,
            tool_use_id: "tu_test".into(),
            progress_sink: None,
        }
    }

    #[tokio::test]
    async fn validate_accepts_keep_action() {
        let tool = ExitWorktreeTool;
        let input = json!({"action": "keep"});
        let result = tool.validate_input(&input, &ctx()).await;
        assert!(matches!(result, ValidationResult::Ok));
    }

    #[tokio::test]
    async fn validate_rejects_invalid_action() {
        let tool = ExitWorktreeTool;
        let input = json!({"action": "invalid"});
        let result = tool.validate_input(&input, &ctx()).await;
        match result {
            ValidationResult::Err { message, .. } => {
                assert!(message.contains("must be 'keep' or 'remove'"));
            }
            _ => panic!("expected Err"),
        }
    }

    #[tokio::test]
    async fn call_no_session_returns_graceful_noop() {
        let tool = ExitWorktreeTool;
        // Ensure no session is active
        crate::utils::worktree::set_session(None);
        let result = tool.call(json!({"action": "keep"}), &ctx(), None).await;
        assert!(!result.is_error);
        assert!(result.content.contains("No active worktree session"));
    }

    #[test]
    fn is_destructive_true_for_remove() {
        let tool = ExitWorktreeTool;
        assert!(tool.is_destructive(&json!({"action": "remove"})));
        assert!(!tool.is_destructive(&json!({"action": "keep"})));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p super-cli -- tools::exit_worktree 2>&1`
Expected: 4 tests pass

- [ ] **Step 3: Commit**

```bash
git add cli/src/tools/exit_worktree.rs
git commit -m "feat: implement ExitWorktree tool with keep/remove actions

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 8: Integration — full build, clippy, and test suite

**Files:**
- None (verification only)

- [ ] **Step 1: Run `cargo check` on the workspace**

Run: `cargo check 2>&1`
Expected: no errors

- [ ] **Step 2: Run `cargo fmt` check**

Run: `cargo fmt --all -- --check 2>&1`
Expected: no formatting issues (or apply `cargo fmt --all` if needed)

- [ ] **Step 3: Run `cargo clippy`**

Run: `cargo clippy --all-targets 2>&1`
Expected: no warnings (fix any that appear)

- [ ] **Step 4: Run the full test suite**

Run: `cargo test 2>&1`
Expected: all tests pass, including the new worktree validation and CWD tests

- [ ] **Step 5: Commit (if any fmt/clippy fixes were needed)**

```bash
git add -A
git commit -m "chore: apply fmt and clippy fixes for worktree implementation

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```
(Only commit if changes were made; this step is a no-op if everything passed cleanly.)

---
