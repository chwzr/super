//! EnterWorktree and ExitWorktree: a session moves into a git worktree, its
//! Bash, file tools and prompt work there, and it comes back keeping or
//! removing the worktree.

use crate::agent_tools_tests::{core_tools, lanes, last_tool_result, texts};
use crate::background_tests::{model, text, tool_call};
use crate::session::manager::SessionManager;
use crate::session_runner::AgentSession;
use crate::settings::{Settings, WorktreeBaseRef};
use crate::tasks::{TaskKind, TaskSpec};
use crate::tools::worktree::{EnterWorktreeTool, ExitWorktreeTool};
use crate::worktree::tests::{branches, init_repository, run, with_remote};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use super_agent::{AgentMessage, AgentTool, StreamFn, ToolResult};
use super_ai::{Registry, ThinkingLevel};
use tokio_util::sync::CancellationToken;

struct Repository {
    root: PathBuf,
    _directory: tempfile::TempDir,
}

/// A git repository with one commit; `remote` gives it an `origin` whose
/// `main` lacks the repository's last commit (`local.txt`).
fn repository(remote: bool) -> Repository {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    init_repository(&root);
    if remote {
        with_remote(&root);
    }
    Repository {
        root,
        _directory: directory,
    }
}

fn settings(base_ref: WorktreeBaseRef) -> Settings {
    let mut settings = Settings {
        retry: crate::settings::RetrySettings {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    settings.worktree.base_ref = base_ref;
    settings
}

fn prompt_in(folder: &Path) -> String {
    format!(
        "main prompt\n\nCurrent working directory: {}",
        folder.display()
    )
}

fn session_in(root: &Path, settings: Settings, stream: Option<StreamFn>) -> Arc<AgentSession> {
    let session = AgentSession::new(
        SessionManager::in_memory(root),
        core_tools(root),
        Registry::from_builtin(),
        settings,
        prompt_in(root),
        model(),
        ThinkingLevel::Off,
        None,
        Arc::new(|_| {}),
    );
    session.set_stream_fn(stream);
    session
}

async fn call(tool: &dyn AgentTool, args: Value) -> Result<ToolResult, String> {
    tool.execute(
        "call",
        tool.prepare_arguments(args),
        CancellationToken::new(),
        None,
    )
    .await
    .map_err(|error| format!("{error:#}"))
}

async fn enter(session: &Arc<AgentSession>, args: Value) -> Result<ToolResult, String> {
    call(&EnterWorktreeTool::new(Arc::downgrade(session)), args).await
}

async fn exit(session: &Arc<AgentSession>, args: Value) -> Result<ToolResult, String> {
    call(&ExitWorktreeTool::new(Arc::downgrade(session)), args).await
}

fn result_text(result: &ToolResult) -> String {
    result.output_text()
}

#[tokio::test]
async fn a_new_worktree_moves_bash_the_file_tools_and_the_prompt_into_it() {
    let repository = repository(true);
    let root = &repository.root;
    let (stream, seen) = lanes(vec![(
        "go",
        vec![
            Some(tool_call("c1", "EnterWorktree", json!({"name": "feature"}))),
            Some(tool_call("c2", "Bash", json!({"command": "pwd -P; ls"}))),
            Some(tool_call("c3", "Read", json!({"file_path": "README"}))),
            Some(text("done")),
        ],
    )]);
    let session = session_in(root, settings(WorktreeBaseRef::Fresh), Some(stream));
    session.prompt(vec![AgentMessage::user("go")]).await;

    let worktree = root.join(".super/worktrees/feature");
    let requests = seen.of("go");
    assert_eq!(requests.len(), 4);
    let system = |index: usize| {
        requests[index]
            .context
            .system_prompt
            .clone()
            .unwrap_or_default()
    };
    assert!(system(0).ends_with(&format!("Current working directory: {}", root.display())));
    let entered = texts(&last_tool_result(&requests[1].context)).join("\n");
    assert_eq!(
        entered,
        format!(
            "The session works in the worktree {} on the branch worktree-feature now. The branch starts from origin/main.",
            worktree.display()
        )
    );
    // From the next request on, the prompt names the worktree.
    assert!(
        system(1).ends_with(&format!(
            "Current working directory: {}",
            worktree.display()
        )),
        "{}",
        system(1)
    );
    // Bash runs there, on the remote's default branch: the local commit is
    // not in it.
    let listing = texts(&last_tool_result(&requests[2].context)).join("\n");
    assert!(
        listing.starts_with(&worktree.display().to_string()),
        "{listing}"
    );
    assert!(listing.contains("README"), "{listing}");
    assert!(!listing.contains("local.txt"), "{listing}");
    // The file tools name the worktree as the folder a relative path is in.
    let read = texts(&last_tool_result(&requests[3].context)).join("\n");
    assert!(
        read.contains(&format!("{}/README", worktree.display())),
        "{read}"
    );
    assert!(branches(root).contains("worktree-feature"));
    assert_eq!(session.working_folder(), worktree);
    let state = session.worktree().unwrap();
    assert!(state.created);
    assert_eq!(state.original, *root);
    assert_eq!(
        state.worktree.base,
        run(root, &["rev-parse", "origin/main"])
    );
}

#[tokio::test]
async fn a_new_worktree_starts_from_head_when_the_setting_says_so() {
    let repository = repository(true);
    let root = &repository.root;
    let session = session_in(root, settings(WorktreeBaseRef::Head), None);
    let entered = enter(&session, json!({"name": "local"})).await.unwrap();
    assert!(result_text(&entered).ends_with("The branch starts from HEAD."));
    let worktree = root.join(".super/worktrees/local");
    assert!(worktree.join("local.txt").exists());
    assert_eq!(
        session.worktree().unwrap().worktree.base,
        run(root, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        entered.details["worktree"]["worktree"]["branch"],
        "worktree-local"
    );
    assert_eq!(entered.details["start"], Value::Null);
}

/// A session that works in a linked worktree of the user's makes its
/// worktree in the main working tree's folder of worktrees, as an agent's
/// worktree is, not inside the linked one, starting from the linked
/// worktree's `HEAD`.
#[tokio::test]
async fn a_worktree_made_from_a_linked_worktree_goes_to_the_main_tree() {
    let repository = repository(false);
    let root = &repository.root;
    let linked = root.parent().unwrap().join("linked");
    run(
        root,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "user-branch",
            &linked.display().to_string(),
        ],
    );
    let linked = linked.canonicalize().unwrap();
    std::fs::write(linked.join("linked.txt"), "on the user's branch\n").unwrap();
    run(&linked, &["add", "linked.txt"]);
    run(&linked, &["commit", "--quiet", "-m", "linked"]);
    let session = session_in(&linked, settings(WorktreeBaseRef::Head), None);
    let entered = enter(&session, json!({"name": "mine"})).await.unwrap();
    assert!(result_text(&entered).ends_with("The branch starts from HEAD."));
    let made = &session.worktree().unwrap().worktree;
    assert_eq!(made.path, root.join(".super/worktrees/mine"));
    assert!(!linked.join(".super/worktrees/mine").exists());
    assert!(made.path.join("linked.txt").exists());
    assert_eq!(made.base, run(&linked, &["rev-parse", "HEAD"]));
    // Leaving it takes the session back to the linked worktree.
    exit(&session, json!({"action": "remove"})).await.unwrap();
    assert_eq!(session.working_folder(), linked);
    assert!(!made.path.exists());
}

/// A linked worktree of a bare repository has no main working tree to make
/// a worktree in: the worktree goes into the linked worktree's own folder
/// of worktrees, for the session and for an isolated agent.
#[tokio::test]
async fn a_worktree_made_from_a_linked_worktree_of_a_bare_repository_goes_there() {
    let repository = repository(false);
    let root = &repository.root;
    let bare = root.parent().unwrap().join("repo.git");
    run(
        root.parent().unwrap(),
        &[
            "clone",
            "--quiet",
            "--bare",
            &root.display().to_string(),
            &bare.display().to_string(),
        ],
    );
    let linked = root.parent().unwrap().join("wt-main");
    run(
        &bare,
        &[
            "worktree",
            "add",
            "--quiet",
            &linked.display().to_string(),
            "main",
        ],
    );
    let linked = linked.canonicalize().unwrap();
    assert_eq!(
        crate::worktree::main_tree(&linked).unwrap(),
        crate::worktree::top(&linked).unwrap()
    );
    let session = session_in(&linked, settings(WorktreeBaseRef::Head), None);
    enter(&session, json!({"name": "lb"})).await.unwrap();
    let made = &session.worktree().unwrap().worktree;
    assert_eq!(made.path, linked.join(".super/worktrees/lb"));
    assert!(made.path.is_dir());
    exit(&session, json!({"action": "remove"})).await.unwrap();
    assert_eq!(session.working_folder(), linked);
}

#[tokio::test]
async fn a_worktree_without_a_name_gets_one_and_fresh_falls_back_to_head_without_a_remote() {
    let repository = repository(false);
    let root = &repository.root;
    let session = session_in(root, settings(WorktreeBaseRef::Fresh), None);
    let entered = enter(&session, json!({"name": null})).await.unwrap();
    let state = session.worktree().unwrap();
    let name = state
        .worktree
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert_eq!(name.len(), 8, "{name}");
    assert!(name.chars().all(|c| c.is_ascii_alphanumeric()), "{name}");
    assert_eq!(
        state.worktree.path,
        root.join(".super/worktrees").join(&name)
    );
    assert_eq!(state.worktree.branch, format!("worktree-{name}"));
    assert!(result_text(&entered).ends_with("The branch starts from HEAD."));
    // The project's own status does not list the worktrees.
    assert_eq!(run(root, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn an_existing_worktree_is_entered_by_its_path_and_exit_leaves_it_alone() {
    let repository = repository(false);
    let root = &repository.root;
    let other = root.parent().unwrap().join("other");
    run(
        root,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "other-branch",
            &other.display().to_string(),
        ],
    );
    let other = other.canonicalize().unwrap();
    let session = session_in(root, settings(WorktreeBaseRef::Fresh), None);
    // Relative to the session folder.
    let entered = enter(&session, json!({"path": "../other"})).await.unwrap();
    assert_eq!(
        result_text(&entered),
        format!(
            "The session works in the worktree {} on the branch other-branch now.",
            other.display()
        )
    );
    let state = session.worktree().unwrap();
    assert!(!state.created);
    assert_eq!(session.working_folder(), other);

    // ExitWorktree acts only on a worktree EnterWorktree made.
    for action in ["keep", "remove"] {
        let exited = exit(&session, json!({"action": action, "discard_changes": true}))
            .await
            .unwrap();
        assert!(!exited.is_error);
        assert!(
            result_text(&exited).ends_with(
                "by its path, and ExitWorktree acts only on a worktree EnterWorktree made, so it did nothing."
            ),
            "{}",
            result_text(&exited)
        );
        assert_eq!(exited.details["left"], false);
    }
    assert!(other.exists());
    assert_eq!(session.working_folder(), other);
}

#[tokio::test]
async fn exit_keeps_or_removes_the_worktree_and_the_session_goes_back() {
    let repository = repository(false);
    let root = &repository.root;
    let session = session_in(root, settings(WorktreeBaseRef::Head), None);

    enter(&session, json!({"name": "kept"})).await.unwrap();
    std::fs::write(root.join(".super/worktrees/kept/work.txt"), "work\n").unwrap();
    let kept = exit(&session, json!({"action": "keep"})).await.unwrap();
    let path = root.join(".super/worktrees/kept");
    assert_eq!(
        result_text(&kept),
        format!(
            "The session works in {} again. The worktree {} and its branch worktree-kept are kept.",
            root.display(),
            path.display()
        )
    );
    assert!(path.join("work.txt").exists());
    assert!(branches(root).contains("worktree-kept"));
    assert_eq!(session.worktree(), None);
    assert_eq!(session.working_folder(), *root);

    enter(&session, json!({"name": "gone"})).await.unwrap();
    let removed = exit(&session, json!({"action": "remove"})).await.unwrap();
    let path = root.join(".super/worktrees/gone");
    assert_eq!(
        result_text(&removed),
        format!(
            "The worktree {} and its branch worktree-gone are removed. The session works in {} again.",
            path.display(),
            root.display()
        )
    );
    assert!(!path.exists());
    assert!(!branches(root).contains("worktree-gone"));
    assert_eq!(session.working_folder(), *root);
    assert_eq!(removed.details["removed"], true);
}

#[tokio::test]
async fn remove_is_refused_for_changes_and_running_work_unless_changes_are_discarded() {
    let repository = repository(false);
    let root = &repository.root;
    let session = session_in(root, settings(WorktreeBaseRef::Head), None);
    // A task that runs from before the session entered is not in it.
    let earlier = session
        .background_tasks()
        .register(TaskSpec::new(TaskKind::Shell, "earlier"))
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    enter(&session, json!({"name": "busy"})).await.unwrap();
    let path = root.join(".super/worktrees/busy");

    std::fs::write(path.join("new.txt"), "new\n").unwrap();
    std::fs::write(path.join("README"), "changed\n").unwrap();
    let refused = exit(&session, json!({"action": "remove"}))
        .await
        .unwrap_err();
    assert_eq!(
        refused,
        format!(
            "The worktree {} has 2 uncommitted files; set discard_changes to remove it with them, or keep the worktree.",
            path.display()
        )
    );
    run(&path, &["add", "-A"]);
    run(&path, &["commit", "--quiet", "-m", "work"]);
    let refused = exit(&session, json!({"action": "remove"}))
        .await
        .unwrap_err();
    assert!(
        refused.ends_with(
            "has 1 commit after the one it started from; set discard_changes to remove it with them, or keep the worktree."
        ),
        "{refused}"
    );
    assert!(path.exists());
    assert!(session.worktree().is_some());

    // Work started in the worktree that still runs: refused even with
    // discard_changes.
    let running = session
        .background_tasks()
        .register(TaskSpec::new(TaskKind::Shell, "npm run dev"))
        .unwrap();
    let refused = exit(
        &session,
        json!({"action": "remove", "discard_changes": "true"}),
    )
    .await
    .unwrap_err();
    assert_eq!(
        refused,
        format!(
            "Background work still runs in the worktree {}: {}. Stop it with TaskStop first, or keep the worktree.",
            path.display(),
            running.id()
        )
    );
    session.stop_task(running.id()).unwrap();

    let removed = exit(
        &session,
        json!({"action": "remove", "discard_changes": true}),
    )
    .await
    .unwrap();
    assert!(!removed.is_error);
    assert!(!path.exists());
    assert!(!branches(root).contains("worktree-busy"));
    assert_eq!(session.working_folder(), *root);
    assert!(!earlier.is_stopped());
}

#[tokio::test]
async fn enter_and_exit_refuse_what_does_not_fit() {
    let repository = repository(false);
    let root = &repository.root;
    let session = session_in(root, settings(WorktreeBaseRef::Head), None);

    let nothing = exit(&session, json!({"action": "keep"})).await.unwrap();
    assert_eq!(
        result_text(&nothing),
        "This session works in no worktree that EnterWorktree made, so ExitWorktree did nothing."
    );
    let both = enter(&session, json!({"name": "a", "path": "/tmp"}))
        .await
        .unwrap_err();
    assert_eq!(
        both,
        "Give name or path, not both: name makes a new worktree, path enters one that exists."
    );
    let missing = enter(&session, json!({"path": "nowhere"}))
        .await
        .unwrap_err();
    assert_eq!(
        missing,
        format!(
            "{}/nowhere does not exist; give the folder of a worktree of this repository.",
            root.display()
        )
    );
    let itself = enter(&session, json!({"path": root})).await.unwrap_err();
    assert_eq!(
        itself,
        format!(
            "{} is the working tree this session works in already.",
            root.display()
        )
    );
    std::fs::create_dir(root.join("plain")).unwrap();
    let plain = enter(&session, json!({"path": "plain"})).await.unwrap_err();
    assert!(
        plain.starts_with(&format!(
            "{}/plain is not a worktree of the repository at {};",
            root.display(),
            root.display()
        )),
        "{plain}"
    );
    let bad_name = enter(&session, json!({"name": "../up"})).await.unwrap_err();
    assert!(
        bad_name.starts_with("Could not make the worktree: '../up' cannot name a worktree"),
        "{bad_name}"
    );

    enter(&session, json!({"name": "first"})).await.unwrap();
    let again = enter(&session, json!({"name": "second"}))
        .await
        .unwrap_err();
    assert_eq!(
        again,
        format!(
            "This session works in the worktree {} already; leave it with ExitWorktree before you enter another.",
            root.join(".super/worktrees/first").display()
        )
    );
    let unknown = exit(&session, json!({"action": "drop"})).await.unwrap_err();
    assert_eq!(unknown, "action must be keep or remove.");

    // Outside a repository nothing can be made.
    let plain = tempfile::tempdir().unwrap();
    let folder = plain.path().canonicalize().unwrap();
    let outside = session_in(&folder, settings(WorktreeBaseRef::Fresh), None);
    let refused = enter(&outside, json!({})).await.unwrap_err();
    assert_eq!(
        refused,
        format!(
            "Could not make the worktree: {} is not in a git repository.",
            folder.display()
        )
    );
}

#[tokio::test]
async fn a_new_conversation_starts_in_the_session_folder_and_keeps_the_worktree() {
    let repository = repository(false);
    let root = &repository.root;
    let session = session_in(root, settings(WorktreeBaseRef::Head), None);
    enter(&session, json!({"name": "left"})).await.unwrap();
    session.replace_manager(SessionManager::in_memory(root));
    assert_eq!(session.worktree(), None);
    assert_eq!(session.working_folder(), *root);
    assert!(root.join(".super/worktrees/left").exists());
}

#[tokio::test]
async fn the_worktree_tools_are_offered_to_the_main_session_in_the_advanced_mode_only() {
    let repository = repository(false);
    let root = &repository.root;
    let session = session_in(root, settings(WorktreeBaseRef::Head), None);
    let names = session.available_tool_names();
    for name in ["ScheduleWakeup", "EnterWorktree", "ExitWorktree"] {
        assert!(names.iter().any(|known| known == name), "{names:?}");
    }
    let mut basic = session.settings();
    basic.mode = crate::settings::Mode::Basic;
    session.update_settings(basic);
    let names = session.available_tool_names();
    for name in ["ScheduleWakeup", "EnterWorktree", "ExitWorktree"] {
        assert!(!names.iter().any(|known| known == name), "{names:?}");
    }
    // The schemas are the interface's.
    let enter = EnterWorktreeTool::new(Arc::downgrade(&session));
    assert_eq!(
        enter.parameters(),
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": super_agent::placeholder::tool_parameter("EnterWorktree", "name")},
                "path": {"type": "string", "description": super_agent::placeholder::tool_parameter("EnterWorktree", "path")},
            },
            "additionalProperties": false,
        })
    );
    assert_eq!(
        enter.description(),
        super_agent::placeholder::tool_description("EnterWorktree")
    );
    let exit = ExitWorktreeTool::new(Arc::downgrade(&session));
    assert_eq!(
        exit.parameters(),
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["keep", "remove"], "description": super_agent::placeholder::tool_parameter("ExitWorktree", "action")},
                "discard_changes": {"type": "boolean", "description": super_agent::placeholder::tool_parameter("ExitWorktree", "discard_changes")},
            },
            "required": ["action"],
            "additionalProperties": false,
        })
    );
    assert_eq!(
        exit.description(),
        super_agent::placeholder::tool_description("ExitWorktree")
    );
}
