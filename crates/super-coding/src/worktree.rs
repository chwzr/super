//! Git worktrees of a project, kept under `.super/worktrees/` in it.
//!
//! An agent started with `isolation: "worktree"` works in a worktree of its
//! own, on a branch of its own, and so does a session that enters one. The
//! interface is small: [`create`] makes a worktree on a new branch,
//! [`has_changes`] says whether anything was done in it since, [`remove`]
//! deletes it together with its branch, and [`recreate`] makes a removed
//! one again in the same place, for an agent that is resumed.
//!
//! Every call runs `git` and blocks until it is done; call them from a
//! blocking thread (`tokio::task::spawn_blocking`) in async code.

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The folder of a project that holds its worktrees.
pub const WORKTREES_DIR: &str = ".super/worktrees";

/// The prefix of the branch a worktree gets: `worktree-<name>`.
pub const BRANCH_PREFIX: &str = "worktree-";

/// One worktree [`create`] made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    /// The worktree's folder, `<project>/.super/worktrees/<name>`.
    pub path: PathBuf,
    /// The branch made for it.
    pub branch: String,
    /// The commit it started from: a commit after it is a change.
    pub base: String,
    /// The working tree of the project it belongs to, where the commands
    /// that manage it run.
    pub project: PathBuf,
}

/// Make a worktree named `name` for the git repository `project` is in: the
/// folder `.super/worktrees/<name>` at the top of `project`'s working tree,
/// checked out on a new branch `worktree-<name>` that starts at `base_ref`
/// (`HEAD` when `None`). The folder is added to the repository's own
/// exclude file, so the project's `git status` does not list it.
pub fn create(project: &Path, name: &str, base_ref: Option<&str>) -> Result<Worktree> {
    check_name(name)?;
    let top = git(project, &["rev-parse", "--show-toplevel"])
        .with_context(|| format!("{} is not in a git repository", project.display()))?;
    let top = PathBuf::from(top);
    let base_ref = base_ref.unwrap_or("HEAD");
    let base = git(
        &top,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{base_ref}^{{commit}}"),
        ],
    )
    .with_context(|| {
        format!(
            "the repository at {} has no commit '{base_ref}' to start a worktree from",
            top.display()
        )
    })?;
    let path = top.join(WORKTREES_DIR).join(name);
    if path.exists() {
        bail!("{} exists already", path.display());
    }
    let branch = format!("{BRANCH_PREFIX}{name}");
    if git(
        &top,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
    {
        bail!("the branch {branch} exists already");
    }
    exclude_worktrees(&top)?;
    git(
        &top,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            &branch,
            &path.display().to_string(),
            &base,
        ],
    )?;
    Ok(Worktree {
        path,
        branch,
        base,
        project: top,
    })
}

/// Whether anything was done in `worktree` since [`create`]: a file that
/// differs from its branch (a new file too, unless it is ignored), or a
/// commit after the one it started from. A worktree whose folder is gone has
/// nothing to keep.
pub fn has_changes(worktree: &Worktree) -> Result<bool> {
    if !worktree.path.exists() {
        return Ok(false);
    }
    let status = git(&worktree.path, &["status", "--porcelain"])?;
    if !status.is_empty() {
        return Ok(true);
    }
    let commits = git(
        &worktree.path,
        &["rev-list", "--count", &format!("{}..HEAD", worktree.base)],
    )?;
    Ok(commits.trim() != "0")
}

/// Make the folder of `worktree` again after [`remove`], or after it was
/// deleted by hand: on its branch when that is still there, so commits on it
/// are not lost, else on a new branch of that name from the project's
/// `HEAD`. A worktree whose folder exists is returned as it is.
pub fn recreate(worktree: &Worktree) -> Result<Worktree> {
    if worktree.path.exists() {
        return Ok(worktree.clone());
    }
    let _ = git(&worktree.project, &["worktree", "prune"]);
    let path = worktree.path.display().to_string();
    let branch_exists = git(
        &worktree.project,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", worktree.branch),
        ],
    )
    .is_ok();
    if branch_exists {
        git(
            &worktree.project,
            &["worktree", "add", "--quiet", &path, &worktree.branch],
        )?;
        return Ok(worktree.clone());
    }
    let base = git(
        &worktree.project,
        &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
    )
    .with_context(|| {
        format!(
            "the repository at {} has no commit to start a worktree from",
            worktree.project.display()
        )
    })?;
    exclude_worktrees(&worktree.project)?;
    git(
        &worktree.project,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            &worktree.branch,
            &path,
            &base,
        ],
    )?;
    Ok(Worktree {
        base,
        ..worktree.clone()
    })
}

/// Delete `worktree`'s folder and its branch, whatever is in them.
pub fn remove(worktree: &Worktree) -> Result<()> {
    if worktree.path.exists() {
        git(
            &worktree.project,
            &[
                "worktree",
                "remove",
                "--force",
                &worktree.path.display().to_string(),
            ],
        )?;
    }
    // Forget a worktree whose folder was deleted by hand.
    let _ = git(&worktree.project, &["worktree", "prune"]);
    if git(
        &worktree.project,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", worktree.branch),
        ],
    )
    .is_ok()
    {
        git(
            &worktree.project,
            &["branch", "-D", "--quiet", &worktree.branch],
        )?;
    }
    Ok(())
}

/// A name is one folder: letters, digits, `.`, `_` and `-`, not starting
/// with `.` or `-`.
fn check_name(name: &str) -> Result<()> {
    let fits = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(['.', '-'])
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character));
    if !fits {
        bail!(
            "'{name}' cannot name a worktree; use letters, digits, '.', '_' and '-', at most 64, starting with a letter or digit"
        );
    }
    Ok(())
}

/// Add `/.super/worktrees/` to the repository's exclude file, which every
/// worktree of it shares, unless it is there.
fn exclude_worktrees(top: &Path) -> Result<()> {
    let line = format!("/{WORKTREES_DIR}/");
    let common = git(top, &["rev-parse", "--git-common-dir"])?;
    let common = top.join(common);
    let file = common.join("info").join("exclude");
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    if existing.lines().any(|known| known.trim() == line) {
        return Ok(());
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&line);
    text.push('\n');
    std::fs::write(&file, text).with_context(|| format!("write {}", file.display()))
}

/// Run `git` in `directory` and return its output without the line break at
/// the end; an exit code other than 0 is an error that carries git's own
/// message.
fn git(directory: &Path, arguments: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .context("could not run git")?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "git {} failed{}",
            arguments.first().copied().unwrap_or_default(),
            if message.is_empty() {
                String::new()
            } else {
                format!(": {message}")
            }
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\n', '\r'])
        .to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A repository with one commit in a folder of its own.
    pub(crate) fn repository() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        init_repository(directory.path());
        directory
    }

    /// Make `root` a repository with one commit.
    pub(crate) fn init_repository(root: &Path) {
        for arguments in [
            vec!["init", "--quiet", "-b", "main"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            git(root, &arguments).unwrap();
        }
        std::fs::write(root.join("README"), "hello\n").unwrap();
        git(root, &["add", "README"]).unwrap();
        git(root, &["commit", "--quiet", "-m", "first"]).unwrap();
    }

    pub(crate) fn branches(root: &Path) -> String {
        git(root, &["branch", "--list"]).unwrap()
    }

    #[test]
    fn a_worktree_without_changes_is_removed_with_its_branch() {
        let repository = repository();
        let root = repository.path().canonicalize().unwrap();
        let worktree = create(&root, "agent-a1", None).unwrap();
        assert_eq!(worktree.path, root.join(".super/worktrees/agent-a1"));
        assert_eq!(worktree.branch, "worktree-agent-a1");
        assert_eq!(worktree.project, root);
        assert!(worktree.path.join("README").exists());
        assert!(branches(&root).contains("worktree-agent-a1"));
        // The project's own status does not list the worktrees.
        assert_eq!(git(&root, &["status", "--porcelain"]).unwrap(), "");
        assert!(!has_changes(&worktree).unwrap());

        remove(&worktree).unwrap();
        assert!(!worktree.path.exists());
        assert!(!branches(&root).contains("worktree-agent-a1"));
        // Gone is gone: nothing to keep, and removing again does nothing.
        assert!(!has_changes(&worktree).unwrap());
        remove(&worktree).unwrap();

        // It can be made again, in the same place on a branch of that name.
        let again = recreate(&worktree).unwrap();
        assert_eq!(again.path, worktree.path);
        assert_eq!(again.branch, worktree.branch);
        assert!(again.path.join("README").exists());
        assert!(!has_changes(&again).unwrap());
        assert_eq!(recreate(&again).unwrap(), again);
    }

    #[test]
    fn a_worktree_deleted_by_hand_comes_back_on_its_branch_with_its_commits() {
        let repository = repository();
        let root = repository.path().canonicalize().unwrap();
        let worktree = create(&root, "kept", None).unwrap();
        std::fs::write(worktree.path.join("work.txt"), "work\n").unwrap();
        git(&worktree.path, &["add", "work.txt"]).unwrap();
        git(&worktree.path, &["commit", "--quiet", "-m", "work"]).unwrap();
        std::fs::remove_dir_all(&worktree.path).unwrap();
        assert!(!has_changes(&worktree).unwrap());

        let again = recreate(&worktree).unwrap();
        assert_eq!(again.base, worktree.base);
        assert!(again.path.join("work.txt").exists());
        assert!(has_changes(&again).unwrap());
    }

    #[test]
    fn a_changed_file_a_new_file_or_a_commit_is_a_change() {
        let repository = repository();
        let root = repository.path().canonicalize().unwrap();

        let edited = create(&root, "edited", None).unwrap();
        std::fs::write(edited.path.join("README"), "changed\n").unwrap();
        assert!(has_changes(&edited).unwrap());

        let added = create(&root, "added", None).unwrap();
        std::fs::write(added.path.join("new.txt"), "new\n").unwrap();
        assert!(has_changes(&added).unwrap());

        let committed = create(&root, "committed", None).unwrap();
        std::fs::write(committed.path.join("README"), "committed\n").unwrap();
        git(&committed.path, &["commit", "--quiet", "-am", "work"]).unwrap();
        assert_eq!(
            git(&committed.path, &["status", "--porcelain"]).unwrap(),
            ""
        );
        assert!(has_changes(&committed).unwrap());

        // A worktree made from inside another one belongs to that one's
        // folder, and neither counts the other as a change.
        let nested = create(&committed.path, "nested", None).unwrap();
        assert!(nested.path.starts_with(&committed.path));
        assert!(!has_changes(&nested).unwrap());
        assert_eq!(
            git(&committed.path, &["status", "--porcelain"]).unwrap(),
            ""
        );
    }

    #[test]
    fn a_worktree_needs_a_repository_a_fitting_name_and_a_free_folder() {
        let plain = tempfile::tempdir().unwrap();
        let error = create(plain.path(), "a", None).unwrap_err();
        assert!(
            error.to_string().contains("is not in a git repository"),
            "{error}"
        );

        let repository = repository();
        let root = repository.path().canonicalize().unwrap();
        for name in ["", "../up", "a/b", ".hidden", "-flag"] {
            assert!(create(&root, name, None).is_err(), "{name:?}");
        }
        create(&root, "twice", None).unwrap();
        let error = create(&root, "twice", None).unwrap_err();
        assert!(error.to_string().contains("exists already"), "{error}");
        let error = create(&root, "other", Some("no-such-ref")).unwrap_err();
        assert!(
            error.to_string().contains("no commit 'no-such-ref'"),
            "{error}"
        );
        // The exclude line is written once.
        let exclude = std::fs::read_to_string(root.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.matches("/.super/worktrees/").count(), 1);
    }
}
