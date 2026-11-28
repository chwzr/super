//! EnterWorktree and ExitWorktree tools: move the session into a git
//! worktree and back.
//!
//! `EnterWorktree` with a `name`, or with neither `name` nor `path`, makes a
//! worktree under `.super/worktrees/` on a new branch
//! ([`crate::worktree::create`]), starting where the `worktree.baseRef`
//! setting says ([`crate::worktree::start_point`]); with a `path` it enters
//! a worktree of the same repository that exists. Either way the session
//! works there from then on: Bash, the file tools and the working-directory
//! line of the system prompt use the worktree.
//!
//! `ExitWorktree` acts only on a worktree `EnterWorktree` made in this
//! session. `keep` leaves it as it is; `remove` deletes it with its branch,
//! unless it has changes and `discard_changes` is not set, or a background
//! task started in it still runs. Either way the session is back in its
//! own folder. [`exit_worktree`] is the same step for a session that ends.

use crate::session_runner::AgentSession;
use crate::worktree::{self, Changes, EnteredWorktree};
use anyhow::Context as _;
use rand::Rng as _;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Weak;
use super_agent::placeholder;
use super_agent::{AgentTool, ExecutionMode, ToolResult, ToolUpdateSink};
use tokio_util::sync::CancellationToken;

/// What `ExitWorktree` does with the worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitAction {
    Keep,
    Remove,
}

impl ExitAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "keep" => Some(Self::Keep),
            "remove" => Some(Self::Remove),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Remove => "remove",
        }
    }
}

/// What [`exit_worktree`] did: the text the model reads and the details
/// for clients.
#[derive(Debug, Clone, PartialEq)]
pub struct Exited {
    pub message: String,
    pub details: Value,
}

/// Leave the worktree this session made with `EnterWorktree`: keep it, or
/// remove it with its branch. `remove` is refused while the worktree has
/// uncommitted files or commits after the one it started from, unless
/// `discard_changes` is set, and while a background task started in it
/// still runs. A session in no worktree, or in one it entered by its path,
/// is left as it is, and the result says so. Runs git and blocks: call it
/// from a blocking thread in async code.
pub fn exit_worktree(
    session: &AgentSession,
    action: ExitAction,
    discard_changes: bool,
) -> Result<Exited, String> {
    let Some(entered) = session.worktree() else {
        return Ok(Exited {
            message: "This session works in no worktree that EnterWorktree made, so ExitWorktree did nothing.".into(),
            details: json!({"action": action.as_str(), "left": false}),
        });
    };
    let path = entered.worktree.path.display().to_string();
    if !entered.created {
        return Ok(Exited {
            message: format!(
                "The session entered the worktree {path} by its path, and ExitWorktree acts only on a worktree EnterWorktree made, so it did nothing."
            ),
            details: json!({"action": action.as_str(), "left": false, "worktree": entered}),
        });
    }
    let original = entered.original.display().to_string();
    let branch = entered.worktree.branch.clone();
    if action == ExitAction::Keep {
        session.leave_worktree();
        return Ok(Exited {
            message: format!(
                "The session works in {original} again. The worktree {path} and its branch {branch} are kept."
            ),
            details: json!({"action": "keep", "left": true, "worktree": entered}),
        });
    }
    let running: Vec<String> = session
        .background_tasks()
        .running()
        .into_iter()
        .filter(|task| task.started_at >= entered.entered_at)
        .map(|task| task.id)
        .collect();
    if !running.is_empty() {
        return Err(format!(
            "Background work still runs in the worktree {path}: {}. Stop it with TaskStop first, or keep the worktree.",
            running.join(", ")
        ));
    }
    if !discard_changes {
        match worktree::changes(&entered.worktree) {
            Ok(changes) if !changes.is_empty() => {
                return Err(format!(
                    "The worktree {path} has {}; set discard_changes to remove it with them, or keep the worktree.",
                    describe(changes)
                ));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(format!(
                    "Could not check the worktree {path} for changes: {error}. Set discard_changes to remove it anyway, or keep it."
                ));
            }
        }
    }
    worktree::remove(&entered.worktree)
        .map_err(|error| format!("Could not remove the worktree {path}: {error}."))?;
    session.leave_worktree();
    Ok(Exited {
        message: format!(
            "The worktree {path} and its branch {branch} are removed. The session works in {original} again."
        ),
        details: json!({"action": "remove", "left": true, "removed": true, "worktree": entered}),
    })
}

/// "2 uncommitted files and 1 commit that …".
fn describe(changes: Changes) -> String {
    let mut parts = Vec::new();
    if changes.files > 0 {
        parts.push(format!(
            "{} uncommitted {}",
            changes.files,
            if changes.files == 1 { "file" } else { "files" }
        ));
    }
    if changes.commits > 0 {
        parts.push(format!(
            "{} {} after the one it started from",
            changes.commits,
            if changes.commits == 1 {
                "commit"
            } else {
                "commits"
            }
        ));
    }
    parts.join(" and ")
}

pub struct EnterWorktreeTool {
    session: Weak<AgentSession>,
}

impl EnterWorktreeTool {
    pub(crate) fn new(session: Weak<AgentSession>) -> Self {
        EnterWorktreeTool { session }
    }
}

#[async_trait::async_trait]
impl AgentTool for EnterWorktreeTool {
    fn name(&self) -> &str {
        "EnterWorktree"
    }

    fn advanced_only(&self) -> bool {
        true
    }

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Sequential
    }

    fn description(&self) -> String {
        placeholder::tool_description("EnterWorktree")
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": placeholder::tool_parameter("EnterWorktree", "name"),
                },
                "path": {
                    "type": "string",
                    "description": placeholder::tool_parameter("EnterWorktree", "path"),
                },
            },
            "additionalProperties": false,
        })
    }

    fn prepare_arguments(&self, mut args: Value) -> Value {
        if let Some(object) = args.as_object_mut() {
            super_agent::tools::arguments::drop_nulls(object, &["name", "path"]);
        }
        args
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        args: Value,
        _cancel: CancellationToken,
        _on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        let given = |name: &str| {
            args[name]
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let (name, path) = (given("name"), given("path"));
        if name.is_some() && path.is_some() {
            anyhow::bail!(
                "Give name or path, not both: name makes a new worktree, path enters one that exists."
            );
        }
        let session = self
            .session
            .upgrade()
            .context("The session has ended, so it cannot enter a worktree.")?;
        if let Some(entered) = session.worktree() {
            anyhow::bail!(
                "This session works in the worktree {} already; leave it with ExitWorktree before you enter another.",
                entered.worktree.path.display()
            );
        }
        let folder = session.working_folder();
        let (worktree, created, start) = match path {
            Some(path) => {
                let worktree = tokio::task::spawn_blocking(move || existing(&folder, &path))
                    .await
                    .context("The worktree could not be checked.")?
                    .map_err(|error| anyhow::anyhow!(error))?;
                (worktree, false, None)
            }
            None => {
                // Worktrees go to the main working tree, as an agent's do,
                // also from a linked worktree; `HEAD` is the commit the
                // session's folder is at.
                let (project, head) = {
                    let folder = folder.clone();
                    tokio::task::spawn_blocking(move || {
                        let project =
                            worktree::main_tree(&folder).unwrap_or_else(|_| folder.clone());
                        (project, worktree::head_commit(&folder).ok())
                    })
                    .await
                    .context("The worktree could not be made.")?
                };
                let start =
                    worktree::start_point(&project, session.settings().worktree.base_ref).await;
                let made = {
                    let start = start.clone().or(head);
                    tokio::task::spawn_blocking(move || {
                        let name = name.unwrap_or_else(|| generated_name(&folder));
                        worktree::create(&project, &name, start.as_deref())
                    })
                };
                let worktree = made
                    .await
                    .context("The worktree could not be made.")?
                    .map_err(|error| anyhow::anyhow!("Could not make the worktree: {error}."))?;
                (worktree, true, start)
            }
        };
        let entered = EnteredWorktree {
            worktree,
            created,
            original: session.working_folder(),
            entered_at: super_ai::now_ms(),
        };
        session.enter_worktree(entered.clone());
        let mut message = format!(
            "The session works in the worktree {} on the branch {} now.",
            entered.worktree.path.display(),
            entered.worktree.branch
        );
        if created {
            message.push_str(&format!(
                " The branch starts from {}.",
                start
                    .as_deref()
                    .map(|start| start.trim_start_matches("refs/remotes/"))
                    .unwrap_or("HEAD")
            ));
        }
        Ok(ToolResult {
            content: vec![super_ai::ContentBlock::text(message)],
            details: json!({"worktree": entered, "start": start}),
            ..Default::default()
        })
    }
}

/// The worktree at `path` (absolute, or relative to `folder`) when it is one
/// of the repository `folder` is in, other than the one `folder` is in.
fn existing(folder: &Path, path: &str) -> Result<worktree::Worktree, String> {
    let given = Path::new(path);
    let target = if given.is_absolute() {
        given.to_path_buf()
    } else {
        folder.join(given)
    };
    let target = target.canonicalize().map_err(|_| {
        format!(
            "{} does not exist; give the folder of a worktree of this repository.",
            target.display()
        )
    })?;
    let top = worktree::top(folder).map_err(|error| format!("{error}."))?;
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if canonical(&top) == target {
        return Err(format!(
            "{} is the working tree this session works in already.",
            target.display()
        ));
    }
    let listed = worktree::list(folder).map_err(|error| format!("{error}."))?;
    if !listed.iter().any(|listed| canonical(listed) == target) {
        return Err(format!(
            "{} is not a worktree of the repository at {}; git worktree list shows its worktrees.",
            target.display(),
            top.display()
        ));
    }
    worktree::describe(&target, &top).map_err(|error| format!("{error}."))
}

/// A name for a worktree the model did not name: eight random letters and
/// digits that no worktree of the project has.
fn generated_name(folder: &Path) -> String {
    let top: PathBuf = worktree::top(folder).unwrap_or_else(|_| folder.to_path_buf());
    loop {
        let name: String = (0..8)
            .map(|_| {
                let index = rand::rng().random_range(0..36u8);
                char::from_digit(u32::from(index), 36).unwrap_or('0')
            })
            .collect();
        if !top.join(worktree::WORKTREES_DIR).join(&name).exists() {
            return name;
        }
    }
}

pub struct ExitWorktreeTool {
    session: Weak<AgentSession>,
}

impl ExitWorktreeTool {
    pub(crate) fn new(session: Weak<AgentSession>) -> Self {
        ExitWorktreeTool { session }
    }
}

#[async_trait::async_trait]
impl AgentTool for ExitWorktreeTool {
    fn name(&self) -> &str {
        "ExitWorktree"
    }

    fn advanced_only(&self) -> bool {
        true
    }

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::Sequential
    }

    fn description(&self) -> String {
        placeholder::tool_description("ExitWorktree")
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["keep", "remove"],
                    "description": placeholder::tool_parameter("ExitWorktree", "action"),
                },
                "discard_changes": {
                    "type": "boolean",
                    "description": placeholder::tool_parameter("ExitWorktree", "discard_changes"),
                },
            },
            "required": ["action"],
            "additionalProperties": false,
        })
    }

    fn prepare_arguments(&self, mut args: Value) -> Value {
        if let Some(object) = args.as_object_mut() {
            super_agent::tools::arguments::drop_nulls(object, &["discard_changes"]);
            super_agent::tools::arguments::boolean(object, "discard_changes");
        }
        args
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        args: Value,
        _cancel: CancellationToken,
        _on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        let action = args["action"]
            .as_str()
            .and_then(ExitAction::parse)
            .context("action must be keep or remove.")?;
        let discard_changes = args["discard_changes"].as_bool().unwrap_or(false);
        let session = self
            .session
            .upgrade()
            .context("The session has ended, so it has no worktree to leave.")?;
        let exited =
            tokio::task::spawn_blocking(move || exit_worktree(&session, action, discard_changes))
                .await
                .context("The worktree could not be left.")?
                .map_err(|error| anyhow::anyhow!(error))?;
        Ok(ToolResult {
            content: vec![super_ai::ContentBlock::text(exited.message)],
            details: exited.details,
            ..Default::default()
        })
    }
}
