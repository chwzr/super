//! Git worktrees of a project, kept under `.super/worktrees/` in it.
//!
//! An agent started with `isolation: "worktree"` works in a worktree of its
//! own, on a branch of its own, and so does a session that enters one. The
//! interface is small: [`create`] makes a worktree on a new branch,
//! [`has_changes`] says whether anything was done in it since, [`remove`]
//! deletes it together with its branch, and [`recreate`] makes a removed
//! one again in the same place, for an agent that is resumed.
//!
//! Where a new worktree's branch starts follows the `worktree.baseRef`
//! setting ([`start_point`]). A session that moved into a worktree keeps an
//! [`EnteredWorktree`].
//!
//! Every call runs `git` and blocks until it is done; call them from a
//! blocking thread (`tokio::task::spawn_blocking`) in async code. Only
//! [`start_point`], which may fetch, is async.

use crate::settings::WorktreeBaseRef;
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// The folder of a project that holds its worktrees.
pub const WORKTREES_DIR: &str = ".super/worktrees";

/// The prefix of the branch a worktree gets: `worktree-<name>`.
pub const BRANCH_PREFIX: &str = "worktree-";

/// The remote whose default branch a fresh worktree starts from.
pub const REMOTE: &str = "origin";

/// How long [`start_point`] waits for the fetch of the remote's default
/// branch before it starts from what was fetched before.
pub const FETCH_LIMIT: Duration = Duration::from_secs(30);

/// The worktree a session moved into with `EnterWorktree`, and where it
/// came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnteredWorktree {
    pub worktree: Worktree,
    /// `EnterWorktree` made it (with a name, or with neither name nor
    /// path); `false` for an existing worktree it entered by its path.
    pub created: bool,
    /// The folder the session worked in before, where it goes back to.
    pub original: PathBuf,
    /// When the session moved in, in milliseconds since the epoch.
    pub entered_at: i64,
}

/// What [`changes`] found in a worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Changes {
    /// Files that differ from the branch: changed, added, deleted, or new
    /// and not ignored.
    pub files: usize,
    /// Commits after the one the worktree started from.
    pub commits: usize,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.files == 0 && self.commits == 0
    }
}

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
    let top = top(project)?;
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
    Ok(!changes(worktree)?.is_empty())
}

/// How many files differ in `worktree` and how many commits it has after
/// the one it started from: what [`has_changes`] asks. A worktree whose
/// folder is gone has none.
pub fn changes(worktree: &Worktree) -> Result<Changes> {
    if !worktree.path.exists() {
        return Ok(Changes::default());
    }
    let status = git(&worktree.path, &["status", "--porcelain"])?;
    let files = status
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    let commits = git(
        &worktree.path,
        &["rev-list", "--count", &format!("{}..HEAD", worktree.base)],
    )?;
    let commits = commits
        .trim()
        .parse()
        .with_context(|| format!("git rev-list counted '{}'", commits.trim()))?;
    Ok(Changes { files, commits })
}

/// The commit a new worktree of `project` starts from, for [`create`]:
/// `None` (the local `HEAD`) for [`WorktreeBaseRef::Head`]. For
/// [`WorktreeBaseRef::Fresh`] the remote's default branch: `origin`'s
/// `HEAD`, else its `main` or `master`. That branch is fetched first, for
/// at most [`FETCH_LIMIT`]; when the fetch fails the branch as it was last
/// fetched is used. A repository without such a remote branch starts from
/// its `HEAD` too.
pub async fn start_point(project: &Path, base: WorktreeBaseRef) -> Option<String> {
    if base == WorktreeBaseRef::Head {
        return None;
    }
    let folder = project.to_path_buf();
    let found = tokio::task::spawn_blocking(move || {
        let top = top(&folder).ok()?;
        let branch = remote_default_branch(&top)?;
        Some((top, branch))
    })
    .await
    .ok()
    .flatten();
    let (top, branch) = found?;
    fetch(&top, &branch).await;
    Some(format!("refs/remotes/{REMOTE}/{branch}"))
}

/// The default branch of [`REMOTE`] as this repository knows it.
fn remote_default_branch(top: &Path) -> Option<String> {
    let prefix = format!("refs/remotes/{REMOTE}/");
    if let Ok(head) = git(
        top,
        &[
            "symbolic-ref",
            "--quiet",
            &format!("refs/remotes/{REMOTE}/HEAD"),
        ],
    ) && let Some(branch) = head.strip_prefix(&prefix)
    {
        return Some(branch.to_string());
    }
    ["main", "master"]
        .into_iter()
        .find(|branch| {
            git(
                top,
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("{prefix}{branch}"),
                ],
            )
            .is_ok()
        })
        .map(str::to_string)
}

/// Fetch `branch` of [`REMOTE`] into its remote-tracking branch, without
/// asking for credentials and for at most [`FETCH_LIMIT`]; a failure is
/// ignored.
async fn fetch(top: &Path, branch: &str) {
    fetch_within(top, branch, FETCH_LIMIT).await;
}

/// [`fetch`] for at most `limit`. git runs in a process group of its own,
/// which is killed at the limit, so that the transport it started (`git
/// remote-http`, `ssh`) ends with it.
async fn fetch_within(top: &Path, branch: &str, limit: std::time::Duration) {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(top)
        .args([
            "fetch",
            "--quiet",
            "--no-tags",
            REMOTE,
            &format!("+refs/heads/{branch}:refs/remotes/{REMOTE}/{branch}"),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let Ok(mut process) = super_agent::tools::shell::ShellProcess::spawn(command) else {
        return;
    };
    if tokio::time::timeout(limit, process.child.wait())
        .await
        .is_err()
    {
        process.kill();
        let _ = process.child.wait().await;
    }
}

/// The top of the working tree `folder` is in.
pub fn top(folder: &Path) -> Result<PathBuf> {
    git(folder, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .with_context(|| format!("{} is not in a git repository", folder.display()))
}

/// Every working tree of the repository `folder` is in, the main one first,
/// as git lists them.
pub fn list(folder: &Path) -> Result<Vec<PathBuf>> {
    let listing = git(folder, &["worktree", "list", "--porcelain"])
        .with_context(|| format!("{} is not in a git repository", folder.display()))?;
    Ok(listing
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect())
}

/// The main working tree of the repository `folder` is in, also when
/// `folder` is in a linked worktree: where new worktrees go, so that one is
/// never made inside another. A bare repository has no main working tree:
/// then it is the working tree `folder` is in.
pub fn main_tree(folder: &Path) -> Result<PathBuf> {
    let listing = git(folder, &["worktree", "list", "--porcelain"])
        .with_context(|| format!("{} is not in a git repository", folder.display()))?;
    let first = listing.split("\n\n").next().unwrap_or_default();
    let bare = first.lines().any(|line| line == "bare");
    match first
        .lines()
        .find_map(|line| line.strip_prefix("worktree "))
    {
        Some(path) if !bare => Ok(PathBuf::from(path)),
        _ => top(folder),
    }
}

/// The commit `HEAD` is at in `folder`.
pub fn head_commit(folder: &Path) -> Result<String> {
    git(
        folder,
        &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
    )
    .with_context(|| format!("{} has no commit", folder.display()))
}

/// The worktree at `path` as it is now, for a session that enters a
/// worktree it did not make: its branch (`HEAD` when it is detached), its
/// current commit as the one it started from, and `project` as the working
/// tree its commands run in.
pub fn describe(path: &Path, project: &Path) -> Result<Worktree> {
    let branch = git(path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let base = git(path, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
        .with_context(|| format!("{} has no commit", path.display()))?;
    Ok(Worktree {
        path: path.to_path_buf(),
        branch,
        base,
        project: project.to_path_buf(),
    })
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

    /// A fetch from an origin that accepts the connection and never
    /// answers ends at its limit, and leaves no process of the transport
    /// git started behind.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_fetch_that_hangs_leaves_no_transport_running_after_its_limit() {
        let directory = repository();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let held = std::thread::spawn(move || {
            let (connection, _) = listener.accept().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2500));
            drop(connection);
        });
        let url = format!("http://127.0.0.1:{port}/repo.git");
        git(directory.path(), &["remote", "add", REMOTE, &url]).unwrap();
        let started = std::time::Instant::now();
        fetch_within(directory.path(), "main", std::time::Duration::from_secs(1)).await;
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let processes = std::process::Command::new("ps")
            .args(["-A", "-o", "pid=,command="])
            .output()
            .unwrap();
        let left: Vec<String> = String::from_utf8_lossy(&processes.stdout)
            .lines()
            .filter(|line| line.contains(&url))
            .map(str::to_string)
            .collect();
        assert!(left.is_empty(), "{left:?}");
        held.join().unwrap();
    }

    /// Run git in `root` for a test.
    pub(crate) fn run(root: &Path, arguments: &[&str]) -> String {
        git(root, arguments).unwrap()
    }

    /// A bare repository `remote.git` beside `root` that is `root`'s
    /// `origin`, with `root`'s `main` pushed to it and fetched back, so
    /// `origin/main` exists and `origin/HEAD` points at it. Then `root` gets
    /// a commit of its own that the remote does not have.
    pub(crate) fn with_remote(root: &Path) -> PathBuf {
        // git cannot make a folder at a path that starts with `\\?\`.
        let root = &super_agent::tools::path::without_verbatim_prefix(root);
        let remote = root.parent().unwrap().join(format!(
            "{}-remote.git",
            root.file_name().unwrap().to_string_lossy()
        ));
        git(
            root.parent().unwrap(),
            &[
                "init",
                "--quiet",
                "--bare",
                "-b",
                "main",
                &remote.display().to_string(),
            ],
        )
        .unwrap();
        git(
            root,
            &["remote", "add", "origin", &remote.display().to_string()],
        )
        .unwrap();
        git(root, &["push", "--quiet", "origin", "main"]).unwrap();
        git(root, &["fetch", "--quiet", "origin"]).unwrap();
        git(root, &["remote", "set-head", "origin", "main"]).unwrap();
        std::fs::write(root.join("local.txt"), "only here\n").unwrap();
        git(root, &["add", "local.txt"]).unwrap();
        git(root, &["commit", "--quiet", "-m", "local"]).unwrap();
        remote
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_fresh_worktree_starts_from_the_remotes_default_branch_fetched_first() {
        let repository = repository();
        let root = repository.path().canonicalize().unwrap();
        let remote = with_remote(&root);
        // Someone else pushes a commit the repository has not fetched yet.
        let other = tempfile::tempdir().unwrap();
        let clone = other.path().join("clone");
        git(
            other.path(),
            &[
                "clone",
                "--quiet",
                &remote.display().to_string(),
                &clone.display().to_string(),
            ],
        )
        .unwrap();
        for arguments in [
            vec!["config", "user.name", "Other"],
            vec!["config", "user.email", "other@example.com"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            git(&clone, &arguments).unwrap();
        }
        std::fs::write(clone.join("pushed.txt"), "from elsewhere\n").unwrap();
        git(&clone, &["add", "pushed.txt"]).unwrap();
        git(&clone, &["commit", "--quiet", "-m", "pushed"]).unwrap();
        git(&clone, &["push", "--quiet", "origin", "main"]).unwrap();

        let start = start_point(&root, WorktreeBaseRef::Fresh).await;
        assert_eq!(start.as_deref(), Some("refs/remotes/origin/main"));
        let fresh = create(&root, "fresh", start.as_deref()).unwrap();
        assert!(fresh.path.join("pushed.txt").exists(), "fetched first");
        assert!(!fresh.path.join("local.txt").exists());
        assert_eq!(fresh.base, run(&clone, &["rev-parse", "HEAD"]));

        // `head` starts from the local state.
        assert_eq!(start_point(&root, WorktreeBaseRef::Head).await, None);
        let head = create(&root, "head", None).unwrap();
        assert!(head.path.join("local.txt").exists());
        assert!(!head.path.join("pushed.txt").exists());
        assert_eq!(head.base, run(&root, &["rev-parse", "HEAD"]));
    }

    #[tokio::test]
    async fn a_fresh_worktree_of_a_repository_without_a_remote_starts_from_head() {
        let repository = repository();
        let root = repository.path().canonicalize().unwrap();
        assert_eq!(start_point(&root, WorktreeBaseRef::Fresh).await, None);
        // origin without a HEAD falls back to its main branch.
        with_remote(&root);
        run(&root, &["remote", "set-head", "origin", "--delete"]);
        assert_eq!(
            start_point(&root, WorktreeBaseRef::Fresh).await.as_deref(),
            Some("refs/remotes/origin/main")
        );
        // Not a repository at all: nothing to start from.
        let plain = tempfile::tempdir().unwrap();
        assert_eq!(
            start_point(plain.path(), WorktreeBaseRef::Fresh).await,
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn changes_count_files_and_commits_and_worktrees_are_listed() {
        let repository = repository();
        let root = repository.path().canonicalize().unwrap();
        let worktree = create(&root, "counted", None).unwrap();
        assert_eq!(changes(&worktree).unwrap(), Changes::default());
        std::fs::write(worktree.path.join("README"), "changed\n").unwrap();
        std::fs::write(worktree.path.join("new.txt"), "new\n").unwrap();
        assert_eq!(
            changes(&worktree).unwrap(),
            Changes {
                files: 2,
                commits: 0
            }
        );
        run(&worktree.path, &["add", "-A"]);
        run(&worktree.path, &["commit", "--quiet", "-m", "one"]);
        std::fs::write(worktree.path.join("README"), "again\n").unwrap();
        run(&worktree.path, &["commit", "--quiet", "-am", "two"]);
        assert_eq!(
            changes(&worktree).unwrap(),
            Changes {
                files: 0,
                commits: 2
            }
        );

        let listed = list(&root).unwrap();
        assert_eq!(listed.first(), Some(&root));
        assert!(listed.contains(&worktree.path), "{listed:?}");
        assert_eq!(top(&worktree.path).unwrap(), worktree.path);
        let described = describe(&worktree.path, &root).unwrap();
        assert_eq!(described.branch, "worktree-counted");
        assert_eq!(described.base, run(&worktree.path, &["rev-parse", "HEAD"]));
        assert_eq!(described.project, root);
    }

    #[test]
    fn a_worktree_without_changes_is_removed_with_its_branch() {
        let repository = repository();
        let root = repository.path().canonicalize().unwrap();
        let worktree = create(&root, "agent-a1", None).unwrap();
        // On Windows git names the folders with forward slashes.
        let canonical = |path: &Path| path.canonicalize().unwrap();
        assert_eq!(
            canonical(&worktree.path),
            canonical(&root.join(".super/worktrees/agent-a1"))
        );
        assert_eq!(worktree.branch, "worktree-agent-a1");
        assert_eq!(canonical(&worktree.project), root);
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
