//! `super install` and `super uninstall` as processes. Only the refusals run
//! here: on Linux the commands would change the real systemd user manager, so
//! the Linux decisions are tested in `super_daemon::service` with a fake
//! systemctl.

#![cfg(not(target_os = "linux"))]

use std::process::Command;

#[test]
fn install_and_uninstall_refuse_off_linux() {
    let home = tempfile::tempdir().expect("tempdir");
    for command in ["install", "uninstall"] {
        let output = Command::new(env!("CARGO_BIN_EXE_super"))
            .arg(command)
            .env("HOME", home.path())
            .output()
            .expect("run super");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{command}: {stderr}");
        assert!(
            stderr.starts_with(&format!("super {command} is for Linux")),
            "{command}: {stderr}"
        );
        if cfg!(target_os = "macos") {
            assert!(stderr.contains("the SUPER app"), "{command}: {stderr}");
        }
        if cfg!(windows) {
            assert!(stderr.contains("not available on Windows"), "{stderr}");
        }
        assert!(output.stdout.is_empty(), "{command} printed to stdout");
    }
    let written: Vec<_> = std::fs::read_dir(home.path()).unwrap().collect();
    assert!(written.is_empty(), "wrote {written:?}");
}
