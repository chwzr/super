//! `super daemon run|status|stop` as processes. HOME points at a temporary
//! directory so nothing under the real ~/.super is touched.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn super_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_super"))
}

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: tempfile::Builder::new()
                .prefix("sb")
                .tempdir()
                .expect("tempdir"),
        }
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("d.sock")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = super_bin();
        command
            .args(args)
            .env("HOME", self.dir.path())
            .env("SUPER_SESSION_DIR", self.dir.path().join("sessions"))
            .env_remove("SHELL")
            .env_remove("SUPER_LOG");
        command
    }

    fn spawn_daemon(&self) -> Child {
        let socket = self.socket();
        let child = self
            .command(&["daemon", "run", "--socket", socket.to_str().unwrap()])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn super daemon run");
        wait_for(&socket, Duration::from_secs(20));
        child
    }
}

fn wait_for(socket: &Path, limit: Duration) {
    let start = Instant::now();
    while !socket.exists() {
        assert!(
            start.elapsed() < limit,
            "the daemon did not create {}",
            socket.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_exit(mut child: Child) -> i32 {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status.code().unwrap_or(-1);
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the daemon did not exit"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn status_without_a_daemon_exits_one() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command(&[
            "daemon",
            "status",
            "--socket",
            sandbox.socket().to_str().unwrap(),
        ])
        .output()
        .expect("run status");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("no SUPER daemon is running"));
}

#[test]
fn run_answers_status_and_exits_zero_on_sigterm() {
    let sandbox = Sandbox::new();
    let child = sandbox.spawn_daemon();
    let output = sandbox
        .command(&[
            "daemon",
            "status",
            "--socket",
            sandbox.socket().to_str().unwrap(),
        ])
        .output()
        .expect("run status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("running (pid"), "{stdout}");
    assert!(stdout.contains("sessions 0 hosted"), "{stdout}");
    // SAFETY: SIGTERM to a child we own.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(wait_exit(child), 0);
    assert!(!sandbox.socket().exists());
}

#[test]
fn stop_stops_a_running_daemon() {
    let sandbox = Sandbox::new();
    let child = sandbox.spawn_daemon();
    let output = sandbox
        .command(&[
            "daemon",
            "stop",
            "--socket",
            sandbox.socket().to_str().unwrap(),
        ])
        .output()
        .expect("run stop");
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("stopped"));
    assert_eq!(wait_exit(child), 0);
    let again = sandbox
        .command(&[
            "daemon",
            "stop",
            "--socket",
            sandbox.socket().to_str().unwrap(),
        ])
        .output()
        .expect("run stop again");
    assert_eq!(again.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&again.stdout).contains("no SUPER daemon is running"));
}
