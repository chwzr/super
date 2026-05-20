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
