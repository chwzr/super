use super::*;
#[cfg(unix)]
use crate::background::{ShellDirectory, TaskHost};
#[cfg(unix)]
use std::sync::Mutex;

fn tool() -> BashTool {
    BashTool::new(std::env::temp_dir())
}

/// A Bash tool for `directory` in a session whose `cd` carries over.
#[cfg(unix)]
fn main_session_tool(directory: &Path) -> BashTool {
    let mut tool = BashTool::new(directory.to_path_buf());
    tool.session.shell = Some(Arc::new(ShellDirectory::new()));
    tool
}

async fn run(tool: &BashTool, args: Value) -> anyhow::Result<ToolResult> {
    tool.execute("call_1", args, CancellationToken::new(), None)
        .await
}

#[cfg(unix)]
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

/// The folder the next command of `bash` starts in.
#[cfg(unix)]
async fn pwd(bash: &BashTool) -> String {
    run(bash, json!({"command": "pwd -P"}))
        .await
        .unwrap()
        .output_text()
}

#[test]
fn the_schema_has_exactly_the_reference_parameters_with_placeholders() {
    let schema = tool().parameters();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"], json!(["command"]));
    let properties = schema["properties"].as_object().unwrap();
    let names: Vec<&str> = properties.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "command",
            "timeout",
            "description",
            "run_in_background",
            "dangerouslyDisableSandbox"
        ]
    );
    for (name, property) in properties {
        assert_eq!(
            property["description"],
            placeholder::tool_parameter("Bash", name)
        );
    }
    assert_eq!(properties["timeout"]["type"], "number");
    assert_eq!(tool().description(), placeholder::tool_description("Bash"));
    // The old cwd parameter fails validation.
    assert!(
        crate::validate::validate_arguments(&schema, &json!({"command": "ls", "cwd": "src"}))
            .is_err()
    );
    assert!(
        crate::validate::validate_arguments(
            &schema,
            &json!({"command": "ls", "timeout": 1000, "description": "List", "run_in_background": false, "dangerouslyDisableSandbox": true})
        )
        .is_ok()
    );
}

#[test]
fn each_shell_family_has_its_own_description_placeholder() {
    for (shell, key) in [
        ("pwsh.exe", "Bash.description.pwsh"),
        ("powershell.exe", "Bash.description.powershell"),
        ("cmd.exe", "Bash.description.cmd"),
        ("/bin/zsh", "Bash.description"),
        ("bash", "Bash.description"),
    ] {
        let mut tool = tool();
        tool.shell_path = Some(shell.into());
        assert_eq!(tool.description(), placeholder::prompt_todo(key), "{shell}");
    }
}

#[test]
fn timeouts_are_milliseconds_with_a_default_and_a_most() {
    assert_eq!(time_limit(None, false).unwrap(), 120_000);
    assert_eq!(time_limit(Some(5_000.0), false).unwrap(), 5_000);
    assert_eq!(time_limit(Some(1.5), false).unwrap(), 2);
    assert_eq!(time_limit(Some(600_000.0), false).unwrap(), 600_000);
    assert_eq!(time_limit(Some(10_000_000.0), false).unwrap(), 600_000);
    assert_eq!(time_limit(None, true).unwrap(), 600_000);
    assert_eq!(time_limit(Some(3_600_000.0), true).unwrap(), 3_600_000);
    assert_eq!(time_limit(Some(10_000_000.0), true).unwrap(), 7_200_000);
    for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
        assert!(time_limit(Some(bad), false).is_err(), "{bad}");
    }
    let prepared = tool().prepare_arguments(json!({"command": "ls", "timeout": "2500", "run_in_background": "false", "description": null}));
    assert_eq!(
        prepared,
        json!({"command": "ls", "timeout": 2500.0, "run_in_background": false})
    );
}

#[tokio::test]
async fn a_timeout_is_counted_in_milliseconds() {
    // The limit leaves a login shell time to read its profile: on a CI
    // runner with bash completion in /etc/profile.d, 300 ms were too few.
    let start = std::time::Instant::now();
    let error = run(
        &tool(),
        json!({"command": "echo started; sleep 10; echo never", "timeout": 2000}),
    )
    .await
    .unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(10));
    let text = format!("{error:#}");
    // A login shell on Windows may still read its profile after 2 seconds.
    #[cfg(unix)]
    assert!(text.contains("started"), "{text}");
    assert!(text.contains("timed out after 2000 ms"), "{text}");
    assert!(!text.contains("never"), "{text}");
}

#[tokio::test]
async fn captures_stdout_and_stderr_in_the_session_folder() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("marker.txt"), "session directory").unwrap();
    let result = run(
        &BashTool::new(directory.path().to_path_buf()),
        json!({"command": "echo out; echo err 1>&2; cat marker.txt", "description": "Show the marker", "dangerouslyDisableSandbox": true}),
    )
    .await
    .unwrap();
    let text = result.output_text();
    assert!(text.contains("out"));
    assert!(text.contains("err"));
    assert!(text.contains("session directory"));
    assert_eq!(result.details["exitCode"], 0);
}

#[cfg(unix)]
#[tokio::test]
async fn the_shell_starts_from_the_profile() {
    let mut bash = tool();
    bash.shell_path = Some("bash".into());
    let result = run(
        &bash,
        json!({"command": "shopt -q login_shell && echo login shell"}),
    )
    .await
    .unwrap();
    assert_eq!(result.output_text(), "login shell");
}

#[cfg(unix)]
#[tokio::test]
async fn a_cd_carries_over_in_the_main_session_while_it_stays_inside() {
    let directory = tempfile::tempdir().unwrap();
    let root = canonical(directory.path());
    std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
    let bash = main_session_tool(directory.path());

    run(&bash, json!({"command": "cd sub"})).await.unwrap();
    assert_eq!(pwd(&bash).await, root.join("sub").display().to_string());
    run(&bash, json!({"command": "cd deeper && echo moved"}))
        .await
        .unwrap();
    assert_eq!(
        pwd(&bash).await,
        root.join("sub/deeper").display().to_string()
    );

    // A command that ends outside the session folder puts the shell back.
    let result = run(&bash, json!({"command": "cd / && echo left"}))
        .await
        .unwrap();
    let text = result.output_text();
    assert!(text.starts_with("left\nThe shell is back in"), "{text}");
    assert_eq!(pwd(&bash).await, root.display().to_string());

    run(&bash, json!({"command": "cd sub"})).await.unwrap();
    let result = run(&bash, json!({"command": "cd ../.. && echo up"}))
        .await
        .unwrap();
    assert!(result.output_text().contains("The shell is back in"));
    assert_eq!(pwd(&bash).await, root.display().to_string());

    // Back at the session folder itself is not outside.
    run(&bash, json!({"command": "cd sub"})).await.unwrap();
    let result = run(&bash, json!({"command": "cd .."})).await.unwrap();
    assert_eq!(result.output_text(), "(no output)");

    // A folder that is gone sends the next command to the session folder.
    run(&bash, json!({"command": "cd sub/deeper"}))
        .await
        .unwrap();
    std::fs::remove_dir_all(root.join("sub")).unwrap();
    let text = run(&bash, json!({"command": "pwd -P"}))
        .await
        .unwrap()
        .output_text();
    assert!(
        text.starts_with(&format!("{}\nThe shell is back in", root.display())),
        "{text}"
    );

    // A failing command's cd counts as well.
    std::fs::create_dir(root.join("other")).unwrap();
    let result = run(&bash, json!({"command": "cd other && false"}))
        .await
        .unwrap();
    assert!(result.is_error);
    assert_eq!(pwd(&bash).await, root.join("other").display().to_string());
}

/// A command's own `EXIT` trap does not lose its `cd`, and a command that
/// removes the folder it is in gets no error it did not print.
#[cfg(unix)]
#[tokio::test]
async fn a_cd_carries_over_past_the_commands_own_exit_trap() {
    let directory = tempfile::tempdir().unwrap();
    let root = canonical(directory.path());
    std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
    let bash = main_session_tool(directory.path());
    let result = run(
        &bash,
        json!({"command": "cd sub; trap 'echo cleanup-ran' EXIT; echo body"}),
    )
    .await
    .unwrap();
    assert_eq!(result.output_text(), "body\ncleanup-ran");
    assert_eq!(pwd(&bash).await, root.join("sub").display().to_string());
    // The command's own exit code stays.
    let result = run(&bash, json!({"command": "cd deeper; (exit 3)"}))
        .await
        .unwrap();
    assert!(result.is_error);
    assert_eq!(result.output_text(), "Exit code 3");
    assert_eq!(
        pwd(&bash).await,
        root.join("sub/deeper").display().to_string()
    );
    let result = run(
        &bash,
        json!({"command": format!("rmdir {}", root.join("sub/deeper").display())}),
    )
    .await
    .unwrap();
    assert_eq!(result.output_text(), "(no output)");
}

/// fish on the `PATH`, if it is installed.
#[cfg(unix)]
fn installed_fish() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join("fish"))
        .find(|path| path.is_file())
}

#[cfg(unix)]
#[tokio::test]
async fn a_cd_carries_over_in_fish_too() {
    let Some(fish) = installed_fish() else {
        eprintln!("fish is not installed; its cd carry-over is checked by its script only");
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let root = canonical(directory.path());
    std::fs::create_dir_all(root.join("sub")).unwrap();
    let mut bash = main_session_tool(directory.path());
    bash.shell_path = Some(fish.display().to_string());
    run(&bash, json!({"command": "cd sub"})).await.unwrap();
    assert_eq!(pwd(&bash).await, root.join("sub").display().to_string());
    let result = run(&bash, json!({"command": "cd /; and echo left"}))
        .await
        .unwrap();
    assert!(
        result
            .output_text()
            .starts_with("left\nThe shell is back in"),
        "{}",
        result.output_text()
    );
    assert_eq!(pwd(&bash).await, root.display().to_string());
}

#[cfg(unix)]
#[tokio::test]
async fn a_cd_does_not_carry_over_outside_the_main_session() {
    let directory = tempfile::tempdir().unwrap();
    let root = canonical(directory.path());
    std::fs::create_dir(root.join("sub")).unwrap();
    // A child session's tool, or one outside a session, has no shell state.
    let bash = BashTool::new(directory.path().to_path_buf());
    run(&bash, json!({"command": "cd sub"})).await.unwrap();
    let text = run(&bash, json!({"command": "pwd -P"}))
        .await
        .unwrap()
        .output_text();
    assert_eq!(text, root.display().to_string());
    let result = run(&bash, json!({"command": "cd / && echo left"}))
        .await
        .unwrap();
    assert_eq!(result.output_text(), "left");
}

#[tokio::test]
async fn a_lone_sleep_is_refused_in_the_foreground() {
    for command in ["sleep 5", "sleep 0.1", " sleep 2 ; "] {
        let error = run(&tool(), json!({"command": command})).await.unwrap_err();
        assert!(
            format!("{error:#}").contains("set run_in_background"),
            "{command}: {error:#}"
        );
    }
    let started = std::time::Instant::now();
    let error = run(
        &tool(),
        json!({"command": "sleep 5", "run_in_background": false}),
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("only sleeps"));
    assert!(started.elapsed() < Duration::from_secs(1));
    // A sleep inside a longer command runs.
    let result = run(&tool(), json!({"command": "sleep 0.1 && echo done"}))
        .await
        .unwrap();
    assert_eq!(result.output_text(), "done");
}

#[tokio::test]
async fn an_exit_code_other_than_zero_is_an_error_with_the_output() {
    let result = run(&tool(), json!({"command": "echo boom; exit 3"}))
        .await
        .unwrap();
    assert!(result.is_error);
    assert_eq!(result.output_text(), "Exit code 3\nboom");
    assert_eq!(result.details["exitCode"], 3);

    let result = run(&tool(), json!({"command": "false"})).await.unwrap();
    assert!(result.is_error);
    assert_eq!(result.output_text(), "Exit code 1");
}

#[cfg(unix)]
#[tokio::test]
async fn exit_code_one_of_a_search_or_comparison_is_no_error() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("a.txt"), "one\n").unwrap();
    std::fs::write(directory.path().join("b.txt"), "two\n").unwrap();
    let bash = BashTool::new(directory.path().to_path_buf());
    for command in [
        "grep needle a.txt",
        "printf 'hay\\n' | grep needle",
        "test -f missing.txt",
        "[ -f missing.txt ]",
        "find missing-folder",
    ] {
        let result = run(&bash, json!({"command": command})).await.unwrap();
        assert!(!result.is_error, "{command}: {}", result.output_text());
        assert_eq!(result.details["exitCode"], 1, "{command}");
    }
    let result = run(&bash, json!({"command": "diff a.txt b.txt"}))
        .await
        .unwrap();
    assert!(!result.is_error);
    assert!(result.output_text().contains("< one"));

    // Exit code 2 and above is an error, also for grep.
    let result = run(&bash, json!({"command": "grep needle missing.txt"}))
        .await
        .unwrap();
    assert!(result.is_error);
    assert!(result.output_text().starts_with("Exit code 2\n"));
    // A search that is not the last command does not count.
    let result = run(&bash, json!({"command": "grep needle a.txt && echo found"}))
        .await
        .unwrap();
    assert!(result.is_error);
    assert_eq!(result.output_text(), "Exit code 1");
}

#[cfg(unix)]
#[tokio::test]
async fn long_output_goes_to_a_file_with_a_preview() {
    // Exactly the most characters still come back with the result.
    let result = run(
        &tool(),
        json!({"command": format!("head -c {MAX_INLINE_CHARS} /dev/zero | tr '\\0' a")}),
    )
    .await
    .unwrap();
    assert_eq!(result.output_text(), "a".repeat(MAX_INLINE_CHARS));
    assert!(result.details.get("fullOutputPath").is_none());

    // Characters count, not bytes: 20,000 two-byte characters fit.
    let result = run(
        &tool(),
        json!({"command": "yes é | head -n 20000 | tr -d '\\n'"}),
    )
    .await
    .unwrap();
    assert_eq!(result.output_text().chars().count(), 20_000);

    for total in [MAX_INLINE_CHARS + 1, 1_000_000] {
        let result = run(
            &tool(),
            json!({"command": format!("head -c {total} /dev/zero | tr '\\0' b; exit 4")}),
        )
        .await
        .unwrap();
        assert!(result.is_error);
        let text = result.output_text();
        let path = result.details["fullOutputPath"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(std::fs::read(&path).unwrap().len(), total);
        assert_eq!(
            text,
            format!(
                "Exit code 4\n{}\n<large-output>\n<output-file>{path}</output-file>\n<total-bytes>{total}</total-bytes>\n<preview>\n{}\n</preview>\n</large-output>",
                placeholder::prompt_todo("Bash.notice.large_output"),
                "b".repeat(PREVIEW_BYTES)
            )
        );
        std::fs::remove_file(path).unwrap();
    }
}

#[tokio::test]
async fn cancel_aborts() {
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c2.cancel();
    });
    let err = tool()
        .execute("1", json!({"command": "sleep 5; echo never"}), cancel, None)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("aborted"));
}

/// A registry double: records how each task ended.
#[cfg(unix)]
struct FakeHost {
    directory: PathBuf,
    started: Mutex<Vec<(String, String, CancellationToken)>>,
    ends: Arc<Mutex<Vec<(String, TaskEnd)>>>,
}

#[cfg(unix)]
impl FakeHost {
    fn new(directory: &Path) -> Arc<Self> {
        Arc::new(FakeHost {
            directory: directory.to_path_buf(),
            started: Mutex::new(Vec::new()),
            ends: Arc::default(),
        })
    }

    async fn end_of(&self, id: &str) -> TaskEnd {
        for _ in 0..400 {
            if let Some((_, end)) = self
                .ends
                .lock()
                .unwrap()
                .iter()
                .find(|(task, _)| task == id)
            {
                return end.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("task {id} did not end");
    }
}

#[cfg(unix)]
struct FakeTask {
    id: String,
    output: PathBuf,
    stop: CancellationToken,
    ends: Arc<Mutex<Vec<(String, TaskEnd)>>>,
}

#[cfg(unix)]
impl TaskHost for FakeHost {
    fn start_shell(
        &self,
        description: &str,
        tool_use_id: &str,
    ) -> Result<Box<dyn HostedTask>, String> {
        let mut started = self.started.lock().unwrap();
        let id = format!("b{}", started.len());
        let stop = CancellationToken::new();
        started.push((
            description.to_string(),
            tool_use_id.to_string(),
            stop.clone(),
        ));
        Ok(Box::new(FakeTask {
            output: self.directory.join(format!("{id}.output")),
            id,
            stop,
            ends: self.ends.clone(),
        }))
    }
}

#[cfg(unix)]
impl HostedTask for FakeTask {
    fn id(&self) -> &str {
        &self.id
    }

    fn output_file(&self) -> &Path {
        &self.output
    }

    fn stop_signal(&self) -> CancellationToken {
        self.stop.clone()
    }

    fn finish(self: Box<Self>, end: TaskEnd) {
        self.ends.lock().unwrap().push((self.id.clone(), end));
    }
}

#[cfg(unix)]
fn background_tool(directory: &Path, host: &Arc<FakeHost>) -> BashTool {
    let mut bash = BashTool::new(directory.to_path_buf());
    bash.session.tasks = Some(host.clone());
    bash
}

#[cfg(unix)]
#[tokio::test]
async fn a_background_command_returns_at_once_and_its_end_is_reported() {
    let directory = tempfile::tempdir().unwrap();
    let host = FakeHost::new(directory.path());
    let bash = background_tool(directory.path(), &host);

    let started = std::time::Instant::now();
    let result = run(
        &bash,
        json!({"command": "echo first; echo to-stderr 1>&2; sleep 0.3; echo last; exit 3", "run_in_background": true, "description": "Count to three"}),
    )
    .await
    .unwrap();
    assert!(started.elapsed() < Duration::from_millis(250));
    let output = directory.path().join("b0.output");
    assert_eq!(
        result.output_text(),
        format!(
            "The command runs in the background as task b0. Its output is written to {}.",
            output.display()
        )
    );
    assert_eq!(result.details["backgroundTaskId"], "b0");
    {
        let started = host.started.lock().unwrap();
        assert_eq!(started[0].0, "Count to three");
        assert_eq!(started[0].1, "call_1");
    }
    let end = host.end_of("b0").await;
    assert_eq!(end.status, TaskEndStatus::Failed);
    assert_eq!(end.summary, "Count to three (exit code 3)");
    let written = std::fs::read_to_string(&output).unwrap();
    assert!(written.contains("first\n"), "{written}");
    assert!(written.contains("to-stderr\n"), "{written}");
    assert!(written.contains("last\n"), "{written}");

    // A search that finds nothing completes; a lone sleep may wait here.
    run(
        &bash,
        json!({"command": "grep needle b0.output", "run_in_background": true}),
    )
    .await
    .unwrap();
    let end = host.end_of("b1").await;
    assert_eq!(end.status, TaskEndStatus::Completed);
    assert_eq!(end.summary, "grep needle b0.output (exit code 1)");
    run(
        &bash,
        json!({"command": "sleep 0.1", "run_in_background": true}),
    )
    .await
    .unwrap();
    assert_eq!(host.end_of("b2").await.status, TaskEndStatus::Completed);
}

#[cfg(unix)]
#[tokio::test]
async fn a_background_command_ends_at_its_time_limit_or_when_stopped() {
    let directory = tempfile::tempdir().unwrap();
    let host = FakeHost::new(directory.path());
    let bash = background_tool(directory.path(), &host);
    run(
        &bash,
        json!({"command": "echo begun; sleep 30", "run_in_background": true, "timeout": 200}),
    )
    .await
    .unwrap();
    let end = host.end_of("b0").await;
    assert_eq!(end.status, TaskEndStatus::Stopped);
    assert_eq!(
        end.summary,
        "echo begun; sleep 30 (stopped at its time limit of 200 ms)"
    );

    // Stopped from outside: the process ends and no end is reported.
    let marker = directory.path().join("marker");
    run(
        &bash,
        json!({"command": format!("sleep 1; touch {}", marker.display()), "run_in_background": true}),
    )
    .await
    .unwrap();
    host.started.lock().unwrap()[1].2.cancel();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!marker.exists(), "the stopped command went on");
    assert!(
        !host
            .ends
            .lock()
            .unwrap()
            .iter()
            .any(|(task, _)| task == "b1")
    );
}

#[tokio::test]
async fn a_background_command_needs_a_session() {
    let error = run(
        &tool(),
        json!({"command": "echo hi", "run_in_background": true}),
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("run_in_background needs a session"));
}
