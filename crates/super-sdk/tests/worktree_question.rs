//! A session that ends in a worktree it made asks every surface whether to
//! keep or remove it, through the same approvals the terminal and the apps
//! answer.

// Paths are compared as git spells them, which is the Unix spelling.
#![cfg(all(feature = "native", unix))]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use super_coding::{EnteredWorktree, SessionManager, Settings};
use super_sdk::{Session, ThinkingLevel};

fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {arguments:?} failed");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A repository with one commit.
fn repository() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    for arguments in [
        vec!["init", "--quiet", "-b", "main"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "commit.gpgsign", "false"],
    ] {
        git(&root, &arguments);
    }
    std::fs::write(root.join("README"), "hello\n").unwrap();
    git(&root, &["add", "README"]);
    git(&root, &["commit", "--quiet", "-m", "first"]);
    (directory, root)
}

fn session_in(root: &Path) -> Arc<Session> {
    let (sink, events) = super_sdk::mirrored_sink(256, None);
    let registry = super_ai::Registry::from_builtin();
    let model = registry.all().first().unwrap().clone();
    let agent = super_coding::AgentSession::new(
        SessionManager::in_memory(root),
        Vec::new(),
        registry,
        Settings::default(),
        "prompt".into(),
        model,
        ThinkingLevel::Off,
        None,
        sink,
    );
    Session::attach(agent, root.to_path_buf(), events)
}

/// Make a worktree named `name` and move the session into it, as
/// `EnterWorktree` does.
fn enter(session: &Session, root: &Path, name: &str, created: bool) -> EnteredWorktree {
    let worktree = super_coding::worktree::create(root, name, None).unwrap();
    let entered = EnteredWorktree {
        worktree,
        created,
        original: root.to_path_buf(),
        entered_at: 0,
    };
    session.agent_session().enter_worktree(entered.clone());
    entered
}

async fn next_event(events: &mut super_sdk::EventStream, kind: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("the session is open");
            if event.0["type"] == kind {
                return event.0;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {kind} event"))
}

#[tokio::test]
async fn a_session_ending_in_its_worktree_asks_to_keep_or_remove_it() {
    let (_directory, root) = repository();
    let session = session_in(&root);
    assert_eq!(
        session.settle_worktree(Duration::from_secs(5)).await,
        None,
        "in no worktree nothing is asked"
    );

    // Remove, answered by another surface.
    let entered = enter(&session, &root, "asked", true);
    let mut events = session.events();
    let settling = {
        let session = session.clone();
        tokio::spawn(async move { session.settle_worktree(Duration::from_secs(30)).await })
    };
    let request = next_event(&mut events, "approval_request").await;
    assert_eq!(request["kind"], "worktree");
    assert_eq!(
        request["title"],
        super_agent::placeholder::prompt_todo("Worktree.exit_question")
    );
    assert_eq!(
        request["detail"],
        entered.worktree.path.display().to_string()
    );
    assert_eq!(request["choices"][0]["id"], "keep");
    assert_eq!(request["choices"][1]["id"], "remove");
    let id = request["approvalId"].as_str().unwrap().to_string();
    // Uncommitted work goes with it: the user chose remove.
    std::fs::write(entered.worktree.path.join("draft.txt"), "draft\n").unwrap();
    session.answer_approval(&id, "remove", "iPhone").unwrap();
    let said = settling.await.unwrap().unwrap();
    assert!(said.contains("are removed"), "{said}");
    assert!(!entered.worktree.path.exists());
    assert!(!git(&root, &["branch", "--list"]).contains("worktree-asked"));
    assert_eq!(session.agent_session().worktree(), None);
    assert_eq!(session.agent_session().working_folder(), root);

    // Keep.
    let entered = enter(&session, &root, "kept", true);
    let settling = {
        let session = session.clone();
        tokio::spawn(async move { session.settle_worktree(Duration::from_secs(30)).await })
    };
    let request = next_event(&mut events, "approval_request").await;
    session
        .answer_approval(request["approvalId"].as_str().unwrap(), "keep", "local")
        .unwrap();
    let said = settling.await.unwrap().unwrap();
    assert!(said.contains("are kept"), "{said}");
    assert!(entered.worktree.path.exists());
    assert_eq!(session.agent_session().worktree(), None);

    // Nobody answers: kept, and the question is closed on every surface.
    let entered = enter(&session, &root, "unanswered", true);
    let said = session
        .settle_worktree(Duration::from_millis(100))
        .await
        .unwrap();
    assert!(said.contains("are kept"), "{said}");
    assert!(entered.worktree.path.exists());
    let resolved = next_event(&mut events, "approval_resolved").await;
    assert_eq!(resolved["choice"], "keep");
    let open = session
        .execute(super_sdk::Command::GetApprovals {})
        .await
        .data
        .unwrap();
    assert_eq!(open["approvals"], serde_json::json!([]));

    // A worktree it entered by its path is not the session's to remove.
    enter(&session, &root, "foreign", false);
    assert_eq!(session.settle_worktree(Duration::from_secs(5)).await, None);
}
