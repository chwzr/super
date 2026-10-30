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
        self.spawn_daemon_with(&[])
    }

    fn spawn_daemon_with(&self, env: &[(&str, &str)]) -> Child {
        let socket = self.socket();
        let mut command = self.command(&["daemon", "run", "--socket", socket.to_str().unwrap()]);
        for (key, value) in env {
            command.env(key, value);
        }
        let child = command
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

#[test]
fn status_finds_the_socket_in_the_environment() {
    let sandbox = Sandbox::new();
    let child = sandbox.spawn_daemon();
    let output = sandbox
        .command(&["daemon", "status"])
        .env("SUPER_DAEMON_SOCKET", sandbox.socket())
        .output()
        .expect("run status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("clients  0"), "{stdout}");

    // One other client (an attached terminal, say): status counts it and
    // not its own connection.
    let terminal = std::os::unix::net::UnixStream::connect(sandbox.socket()).expect("connect");
    {
        use std::io::{BufRead as _, Write as _};
        let mut writer = &terminal;
        writeln!(
            writer,
            r#"{{"type":"hello","id":"h1","client":"tui","name":"Terminal","version":"0.0.0","protocolVersions":[1,1]}}"#
        )
        .expect("hello");
        let mut reply = String::new();
        std::io::BufReader::new(&terminal)
            .read_line(&mut reply)
            .expect("hello reply");
        assert!(reply.contains(r#""success":true"#), "{reply}");
    }
    let output = sandbox
        .command(&["daemon", "status"])
        .env("SUPER_DAEMON_SOCKET", sandbox.socket())
        .output()
        .expect("run status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("clients  1"), "{stdout}");
    drop(terminal);

    let stopped = sandbox
        .command(&["daemon", "stop"])
        .env("SUPER_DAEMON_SOCKET", sandbox.socket())
        .output()
        .expect("run stop");
    assert!(String::from_utf8_lossy(&stopped.stdout).contains("stopped"));
    assert_eq!(wait_exit(child), 0);
}

#[test]
fn a_daemon_on_another_protocol_range_names_both_and_the_update() {
    let sandbox = Sandbox::new();
    let child = sandbox.spawn_daemon_with(&[("SUPER_DAEMON_PROTOCOL_VERSIONS", "2-3")]);
    let output = sandbox
        .command(&[
            "daemon",
            "status",
            "--socket",
            sandbox.socket().to_str().unwrap(),
        ])
        .output()
        .expect("run status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("the SUPER daemon speaks protocol 2 to 3, this build speaks 1: "),
        "{stderr}"
    );
    // It cannot be asked to stop, but `stop` finds its pid.
    let stopped = sandbox
        .command(&[
            "daemon",
            "stop",
            "--socket",
            sandbox.socket().to_str().unwrap(),
        ])
        .output()
        .expect("run stop");
    assert_eq!(stopped.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&stopped.stdout).contains("stopped (pid"));
    assert_eq!(wait_exit(child), 0);
}
