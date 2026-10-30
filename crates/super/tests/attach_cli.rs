//! A signed-in `super` attaches to a running daemon instead of running the
//! engine itself. The daemon and the client get their own temporary HOME;
//! only the client's holds a (fake) platform sign-in, so the daemon stays
//! off the network.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use super_sdk::mock::{MockProvider, MockScript};
use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("at")
            .tempdir()
            .expect("tempdir");
        for sub in ["daemon-home", "client-home", "work", "sessions"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        let auth = dir.path().join("client-home/.super/agent/auth.json");
        std::fs::create_dir_all(auth.parent().unwrap()).unwrap();
        std::fs::write(
            &auth,
            r#"{"super":{"type":"oauth","access":"test","refresh":"test","expires":0}}"#,
        )
        .unwrap();
        Self { dir }
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub)
    }

    fn socket(&self) -> PathBuf {
        self.path("d.sock")
    }

    fn command(&self, home: &str, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_super"));
        command
            .args(args)
            .current_dir(self.path("work"))
            .env("HOME", self.path(home))
            .env("SUPER_SESSION_DIR", self.path("sessions"))
            .env("SUPER_DAEMON_SOCKET", self.socket())
            // Nothing in this test may reach a real platform.
            .env("SUPER_PLATFORM_URL", "http://127.0.0.1:9")
            .env_remove("SHELL")
            .env_remove("SUPER_LOG")
            .kill_on_drop(true);
        command
    }
}

async fn wait_for(socket: &Path) {
    let start = Instant::now();
    while !socket.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the daemon did not create {}",
            socket.display()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn output(mut command: Command, stdin: &str) -> std::process::Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn super");
    let mut input = child.stdin.take().unwrap();
    input.write_all(stdin.as_bytes()).await.unwrap();
    drop(input);
    tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .expect("super finished in time")
        .expect("super ran")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signed_in_super_attaches_to_the_daemon_and_leaves_the_session_running() {
    let sandbox = Sandbox::new();
    let provider = MockProvider::start(
        sandbox.path("mock"),
        MockScript::text("Hello from the daemon."),
    )
    .await
    .expect("mock provider");
    let socket = sandbox.socket();
    let mut daemon = sandbox.command(
        "daemon-home",
        &["daemon", "run", "--socket", socket.to_str().unwrap()],
    );
    daemon
        .env("SUPER_MODELS_FILE", provider.catalog_path())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut daemon = daemon.spawn().expect("spawn the daemon");
    wait_for(&socket).await;

    let attached = output(
        sandbox.command("client-home", &["--model", "mock/mock-1"]),
        "say hi\n",
    )
    .await;
    let stdout = String::from_utf8_lossy(&attached.stdout);
    let stderr = String::from_utf8_lossy(&attached.stderr);
    assert_eq!(attached.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("attached to session"), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("Hello from the daemon."),
        "{stdout}\n{stderr}"
    );
    assert!(stdout.contains("detached from"), "{stdout}\n{stderr}");

    // The session outlives the terminal: still hosted, nobody attached.
    let status = output(sandbox.command("client-home", &["daemon", "status"]), "").await;
    let status_out = String::from_utf8_lossy(&status.stdout);
    assert_eq!(status.status.code(), Some(0), "{status_out}");
    assert!(status_out.contains("sessions 1 hosted"), "{status_out}");

    // While the daemon hosts it, an in-process `super` keeps its hands off
    // the same file.
    let mut in_process = sandbox.command(
        "client-home",
        &["--no-remote", "-p", "-c", "--model", "mock/mock-1", "hello"],
    );
    in_process.env("SUPER_MODELS_FILE", provider.catalog_path());
    let refused = output(in_process, "").await;
    let refused_err = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(1), "{refused_err}");
    assert!(
        refused_err.contains("is open in another SUPER process"),
        "{refused_err}"
    );

    // `super` there again joins the same session and sees its transcript.
    let again = output(sandbox.command("client-home", &[]), "/detach\n").await;
    let again_out = String::from_utf8_lossy(&again.stdout);
    assert_eq!(again.status.code(), Some(0), "{again_out}");
    assert!(again_out.contains("› say hi"), "{again_out}");
    assert!(again_out.contains("Hello from the daemon."), "{again_out}");

    // `--session` with the short id from the header finds the same session.
    let short = stdout
        .split("attached to session ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("the header names the session")
        .to_string();
    let by_id = output(
        sandbox.command("client-home", &["--session", &short]),
        "/detach\n",
    )
    .await;
    let by_id_out = String::from_utf8_lossy(&by_id.stdout);
    assert_eq!(by_id.status.code(), Some(0), "{by_id_out}");
    assert!(
        by_id_out.contains(&format!("attached to session {short}")),
        "{by_id_out}"
    );

    let stopped = output(sandbox.command("client-home", &["daemon", "stop"]), "").await;
    assert!(String::from_utf8_lossy(&stopped.stdout).contains("stopped"));
    let exit = tokio::time::timeout(Duration::from_secs(20), daemon.wait())
        .await
        .expect("the daemon exited")
        .expect("wait");
    assert_eq!(exit.code(), Some(0));
}
