//! One rendering for a command, wherever it was typed.
//!
//! A command typed here (`/thinking low`, a prompt, `!ls`) and the same
//! command run by the SUPER app or another terminal (`set_thinking_level`
//! through the platform or the daemon) leave the same lines in the
//! transcript. Every protocol command maps to what a user types for it here
//! ([`typed`]); the local handlers and the remote paths show what a command
//! did with the one function [`show`]. Where a command's effect is a session
//! event the terminal renders anyway (a model change, a compaction, an
//! abort, a queued steer), nothing more is shown, typed here or not.
//!
//! The origin is not shown. A transcript reads the same whoever typed what,
//! a user message has no place for it, and a resumed session, which does not
//! record origins, then reads exactly like the live one.

use super::*;
use super_sdk::protocol::Command;

/// What a user types for a command in this terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Typed {
    /// A message: a prompt, or a steer or follow-up while the agent works.
    Input(String),
    /// A slash command with its arguments.
    Slash(String),
    /// A shell line.
    Shell(String),
    /// A key: esc aborts, alt+up takes the queue back.
    Key(&'static str),
    /// A toggle in `/settings`.
    Setting(&'static str),
    /// Nothing is typed: a read, a ping, an answer in a picker.
    Nothing,
}

/// The user's form of `command`. Every protocol command has one, so a new
/// command cannot reach the transcript under its internal name.
pub(super) fn typed(command: &Command) -> Typed {
    match command {
        Command::Prompt { message, .. }
        | Command::Steer { message, .. }
        | Command::FollowUp { message, .. } => Typed::Input(visible_user_text(message)),
        Command::Abort {} | Command::AbortBash {} => Typed::Key("esc"),
        Command::ClearQueue {} => Typed::Key("alt+up"),
        Command::NewSession {} => Typed::Slash("/new".into()),
        Command::SetSessionName { name } => Typed::Slash(format!("/name {name}")),
        Command::SetModel { provider, model_id } => {
            Typed::Slash(format!("/model {provider}/{model_id}"))
        }
        Command::SetThinkingLevel { level } => Typed::Slash(format!("/thinking {level}")),
        Command::Compact {
            custom_instructions,
        } => Typed::Slash(match custom_instructions {
            Some(instructions) => format!("/compact {instructions}"),
            None => "/compact".into(),
        }),
        Command::Bash { command } => Typed::Shell(format!("!{command}")),
        Command::ExportHtml { output_path } => Typed::Slash(match output_path {
            Some(path) => format!("/export {path}"),
            None => "/export".into(),
        }),
        Command::SwitchSession { .. } => Typed::Slash("/resume".into()),
        Command::Fork { .. } => Typed::Slash("/tree".into()),
        Command::SetSteeringMode { .. } => Typed::Setting("steering mode"),
        Command::SetFollowUpMode { .. } => Typed::Setting("follow-up mode"),
        Command::SetAutoCompaction { .. } => Typed::Setting("auto-compaction"),
        Command::SetAutoRetry { .. } => Typed::Setting("auto-retry"),
        Command::GetState {}
        | Command::GetMessages {}
        | Command::GetEntries { .. }
        | Command::GetTree {}
        | Command::GetLastAssistantText {}
        | Command::GetSessionStats {}
        | Command::GetAvailableModels { .. }
        | Command::GetAvailableThinkingLevels {}
        | Command::GetTools {}
        | Command::GetForkMessages {}
        | Command::GetApprovals {}
        | Command::AnswerApproval { .. }
        | Command::Ping {} => Typed::Nothing,
    }
}

/// What the transcript shows once a command took effect.
pub(super) enum Shown {
    /// A prompt that started a run: the user's message.
    Prompt(String),
    ThinkingLevel(ThinkingLevel),
    SessionName(String),
    NewSession,
    /// A shell command run elsewhere, with its whole output.
    Shell {
        command: String,
        output: String,
        exit_code: Option<i32>,
        cancelled: bool,
    },
    /// The end of a shell command whose output is on screen already.
    ShellFinished {
        exit_code: Option<i32>,
        cancelled: bool,
    },
    Exported(String),
    /// The session switched to another file.
    Resumed {
        source: &'static str,
        id: String,
    },
    MovedInTree {
        summarized: bool,
    },
    /// A workflow's approval was answered. `progress_view` says whether
    /// `/workflows` can show the run, which it cannot for a run in the daemon.
    WorkflowAnswered {
        name: String,
        approved: bool,
        progress_view: bool,
    },
    TrustSaved(std::result::Result<bool, String>),
    /// Another question was answered. Only the daemon asks those, so a
    /// build without the daemon client never shows one.
    #[cfg_attr(not(unix), allow(dead_code))]
    Answered {
        title: String,
        choice: String,
    },
    /// The session's own events show it, or nothing does.
    Events,
}

/// Show what a command did, the same for every origin.
pub(super) fn show(app: &mut App, session: &Arc<super_coding::AgentSession>, shown: Shown) {
    match shown {
        Shown::Prompt(text) => {
            app.cells.push(Cell::User(text));
            app.working = true;
        }
        Shown::ThinkingLevel(level) => {
            update_thinking_border(app, level);
            app.cells.push(Cell::Notice(format!(
                "thinking level set to {}",
                level.as_str()
            )));
        }
        Shown::SessionName(name) => {
            set_session_title(app, Some(name.clone()));
            app.cells
                .push(Cell::Notice(format!("session named: {name}")));
        }
        Shown::NewSession => {
            sync_session_title(app, session);
            app.cells.clear();
            app.cell_render_cache.clear();
            app.cells.push(Cell::Notice("started a new session".into()));
            refresh_git_branch(app, session);
        }
        Shown::Shell {
            command,
            output,
            exit_code,
            cancelled,
        } => {
            app.cells.push(Cell::BashExecution {
                command,
                output,
                exclude_from_context: false,
            });
            show(
                app,
                session,
                Shown::ShellFinished {
                    exit_code,
                    cancelled,
                },
            );
        }
        Shown::ShellFinished {
            exit_code,
            cancelled,
        } => {
            if !cancelled && exit_code != Some(0) {
                app.cells.push(Cell::Notice(format!(
                    "shell command exited with {}",
                    exit_code
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "no status".into())
                )));
            }
        }
        Shown::Exported(path) => app.cells.push(Cell::Notice(format!("exported to {path}"))),
        Shown::Resumed { source, id } => {
            sync_session_title(app, session);
            app.cells = session_cells(session);
            app.cell_render_cache.clear();
            app.cells
                .push(Cell::Notice(format!("resumed {source} session {id}")));
            update_thinking_border(app, session.thinking_level());
            refresh_git_branch(app, session);
        }
        Shown::MovedInTree { summarized } => app.cells.push(Cell::Notice(if summarized {
            "summarized the old branch and moved to the selected entry".into()
        } else {
            "moved to the selected entry".into()
        })),
        Shown::WorkflowAnswered {
            name,
            approved,
            progress_view,
        } => app
            .cells
            .push(Cell::Notice(match (approved, progress_view) {
                (true, true) => {
                    format!("running workflow {name} · /workflows opens the progress view")
                }
                (true, false) => format!("running workflow {name}"),
                (false, _) => "workflow cancelled".into(),
            })),
        Shown::TrustSaved(Ok(trusted)) => app.cells.push(Cell::Notice(format!(
            "saved project trust as {}. Run /reload to apply it",
            if trusted { "trusted" } else { "untrusted" }
        ))),
        Shown::TrustSaved(Err(error)) => app.cells.push(Cell::Error(format!(
            "could not save project trust: {error}"
        ))),
        Shown::Answered { title, choice } => {
            app.cells.push(Cell::Notice(format!("{title} {choice}")))
        }
        Shown::Events => {}
    }
}

/// What `command`, run by another surface, did here: what its typed form
/// ([`typed`]) shows when it is typed here. `data` is its response's data.
pub(super) fn shown_for(
    command: &Command,
    data: Option<&serde_json::Value>,
    session: &Arc<super_coding::AgentSession>,
) -> Shown {
    let data = data.cloned().unwrap_or_default();
    match (typed(command), command) {
        // A queued message shows up when the run takes it, as one typed here
        // while the agent works does.
        (Typed::Input(text), Command::Prompt { .. }) if data["disposition"] == "started" => {
            Shown::Prompt(text)
        }
        (Typed::Slash(line), _) => slash_shown(&line, &data, session),
        (Typed::Shell(_), Command::Bash { command }) => Shown::Shell {
            command: command.clone(),
            output: data["output"].as_str().unwrap_or_default().to_string(),
            exit_code: data["exitCode"].as_i64().map(|code| code as i32),
            cancelled: data["cancelled"].as_bool().unwrap_or(false),
        },
        _ => Shown::Events,
    }
}

/// What a slash command shows once it took effect. `/model` and `/compact`
/// show through the events they cause.
fn slash_shown(
    line: &str,
    data: &serde_json::Value,
    session: &Arc<super_coding::AgentSession>,
) -> Shown {
    let line = line.trim_start_matches('/');
    let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
    match name {
        "thinking" => ThinkingLevel::parse(rest)
            .map(Shown::ThinkingLevel)
            .unwrap_or(Shown::Events),
        "name" => Shown::SessionName(rest.to_string()),
        "new" => Shown::NewSession,
        "export" => Shown::Exported(
            data["path"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| rest.to_string()),
        ),
        "resume" => Shown::Resumed {
            source: crate::session_sources::SessionSource::Super(PathBuf::new()).name(),
            id: session.manager.lock().unwrap().session_id().to_string(),
        },
        "tree" if data["cancelled"] != true => Shown::MovedInTree { summarized: false },
        _ => Shown::Events,
    }
}

/// A command another surface ran on this session, shown as if typed here.
/// `command` is the command as `command_applied` carries it.
pub(super) fn show_remote(
    app: &mut App,
    session: &Arc<super_coding::AgentSession>,
    command: &serde_json::Value,
    data: Option<&serde_json::Value>,
) {
    let Ok(command) = serde_json::from_value::<Command>(command.clone()) else {
        return;
    };
    let shown = shown_for(&command, data, session);
    show(app, session, shown);
    update_thinking_border(app, session.thinking_level());
}

#[cfg(test)]
mod tests {
    use super::super::tests::{test_app, test_resources, test_session};
    use super::*;
    use clap::Parser as _;
    use std::sync::Mutex;
    use super_sdk::protocol::StreamingBehavior;

    /// A cell as text, to compare two transcripts.
    fn describe(cell: &Cell) -> String {
        match cell {
            Cell::User(text) => format!("user {text}"),
            Cell::AssistantStreaming(text) => format!("streaming {text}"),
            Cell::AssistantFinal(text) => format!("assistant {text}"),
            Cell::Thinking(text) => format!("thinking {text}"),
            Cell::ToolCall {
                title,
                output,
                is_error,
                done,
            } => format!("tool {title} {output} {is_error} {done}"),
            Cell::Notice(text) => format!("notice {text}"),
            Cell::ReasoningEffort { level, generations } => {
                format!("effort {} {generations}", level.as_str())
            }
            Cell::Error(text) => format!("error {text}"),
            Cell::BashExecution {
                command,
                output,
                exclude_from_context,
            } => format!("shell {command} {output} {exclude_from_context}"),
        }
    }

    fn transcript(app: &App) -> Vec<String> {
        app.cells.iter().map(describe).collect()
    }

    /// A model with thinking levels, so `/thinking low` means something.
    fn reasoning_session() -> (
        Arc<super_coding::AgentSession>,
        Arc<Mutex<Vec<SessionEvent>>>,
    ) {
        let events: Arc<Mutex<Vec<SessionEvent>>> = Default::default();
        let sink_events = events.clone();
        let registry = super_ai::Registry::from_builtin();
        let model = registry
            .all()
            .iter()
            .find(|model| {
                model
                    .supported_thinking_levels()
                    .contains(&ThinkingLevel::Low)
            })
            .expect("a built-in model with thinking levels")
            .clone();
        let session = super_coding::AgentSession::new(
            super_coding::SessionManager::in_memory(&std::env::temp_dir()),
            Vec::new(),
            registry,
            super_coding::Settings::default(),
            "test prompt".into(),
            model,
            ThinkingLevel::Off,
            None,
            Arc::new(move |event| sink_events.lock().unwrap().push(event)),
        );
        (session, events)
    }

    /// Render what the session emitted since the last call, as the TUI does.
    fn drain(
        app: &mut App,
        session: &Arc<super_coding::AgentSession>,
        events: &Mutex<Vec<SessionEvent>>,
    ) {
        let pending: Vec<SessionEvent> = std::mem::take(&mut *events.lock().unwrap());
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut file_search = FileSearchService::new(tx);
        for event in pending {
            handle_session_event(app, event, &mut file_search, session);
        }
    }

    /// Runs nothing, so a prompt goes nowhere in a test.
    struct Idle;

    impl super_coding::SessionBackend for Idle {
        fn prompt(
            &self,
            _: Vec<AgentMessage>,
            _: PromptMode,
        ) -> futures::future::BoxFuture<'static, ()> {
            Box::pin(async {})
        }
        fn queue(&self, _: AgentMessage, _: PromptMode, _: bool) {}
        fn reclaim_queued(&self) -> Vec<AgentMessage> {
            Vec::new()
        }
        fn abort(&self) {}
        fn set_model(&self, _: &super_ai::Model) {}
        fn set_thinking_level(&self, _: ThinkingLevel) {}
        fn compact(&self, _: Option<String>) -> futures::future::BoxFuture<'static, ()> {
            Box::pin(async {})
        }
        fn is_running(&self) -> bool {
            false
        }
    }

    fn plan() -> super_coding::workflows::WorkflowPlan {
        super_coding::workflows::WorkflowPlan {
            name: "greet".into(),
            description: "Say hi".into(),
            phases: Vec::new(),
            estimated_agents: Some(1),
            source: Arc::from("export const meta = { name: 'greet', description: 'Say hi' }"),
        }
    }

    /// The terminal where the commands are typed.
    struct Here {
        app: App,
        session: Arc<super_coding::AgentSession>,
        events: Arc<Mutex<Vec<SessionEvent>>>,
        resources: InteractiveResources,
        running_task: Option<tokio::task::JoinHandle<()>>,
        tx: mpsc::UnboundedSender<CommandEvent>,
        rx: mpsc::UnboundedReceiver<CommandEvent>,
    }

    impl Here {
        fn new() -> Self {
            let (session, events) = reasoning_session();
            let (tx, rx) = mpsc::unbounded_channel();
            Here {
                app: test_app(),
                session,
                events,
                resources: test_resources(),
                running_task: None,
                tx,
                rx,
            }
        }

        fn slash(&mut self, line: &str) {
            run_slash_command(
                &mut self.app,
                &self.session,
                line.trim_start_matches('/'),
                &Args::parse_from(["super"]),
                &mut self.resources,
                &mut self.running_task,
                &self.tx,
            );
            drain(&mut self.app, &self.session, &self.events);
        }

        fn input(&mut self, text: &str) {
            submit(
                &mut self.app,
                &self.session,
                text.into(),
                &mut self.running_task,
                &self.tx,
            );
        }

        async fn shell(&mut self, command: &str) {
            start_shell_passthrough(&mut self.app, &self.session, command.into(), &self.tx);
            loop {
                let event = self.rx.recv().await.expect("shell events");
                let finished = matches!(event, CommandEvent::ShellFinished(_));
                let (search_tx, _) = mpsc::unbounded_channel();
                let mut file_search = FileSearchService::new(search_tx);
                handle_command_event(
                    &mut self.app,
                    event,
                    &mut self.resources,
                    &mut file_search,
                    &self.session,
                );
                if finished {
                    return;
                }
            }
        }

        fn cancel_workflow(&mut self) {
            open_workflow_approval(
                &mut self.app,
                Box::new(plan()),
                tokio::sync::oneshot::channel().0,
            );
            let kind = self.app.picker.take().expect("the approval picker").kind;
            apply_picker_selection(
                &mut self.app,
                &self.session,
                PickerSelection {
                    kind,
                    value: 2,
                    filter: String::new(),
                },
                false,
                &mut self.resources,
                &self.tx,
            );
        }
    }

    /// The terminal a remote controller drives, through the SDK view of its
    /// session: the path both the platform and the daemon's clients take.
    struct There {
        app: App,
        session: Arc<super_coding::AgentSession>,
        events: Arc<Mutex<Vec<SessionEvent>>>,
        sdk: Arc<super_sdk::Session>,
        notices: Arc<Mutex<Vec<SessionNotice>>>,
    }

    impl There {
        fn new() -> Self {
            let (session, events) = reasoning_session();
            let (_, sdk_events) = super_sdk::mirrored_sink(64, None);
            let sdk = super_sdk::Session::attach(session.clone(), std::env::temp_dir(), sdk_events);
            let notices: Arc<Mutex<Vec<SessionNotice>>> = Default::default();
            let observed = notices.clone();
            sdk.set_observer(Some(Arc::new(move |notice| {
                observed.lock().unwrap().push(notice)
            })));
            let mut app = test_app();
            app.sdk = Some(sdk.clone());
            There {
                app,
                session,
                events,
                sdk,
                notices,
            }
        }

        async fn run(&mut self, command: Command) {
            let response = self
                .sdk
                .execute_from(command, None, Some("iPhone".into()))
                .await;
            assert!(response.success, "{response:?}");
            self.observe();
        }

        fn observe(&mut self) {
            for notice in std::mem::take(&mut *self.notices.lock().unwrap()) {
                handle_remote_notice(&mut self.app, notice, &self.session);
            }
            drain(&mut self.app, &self.session, &self.events);
        }

        async fn cancel_workflow(&mut self) {
            open_workflow_approval(
                &mut self.app,
                Box::new(plan()),
                tokio::sync::oneshot::channel().0,
            );
            let approval_id = self.app.open_approval.as_ref().expect("asked").id.clone();
            self.run(Command::AnswerApproval {
                approval_id,
                choice: "cancel".into(),
            })
            .await;
            assert!(self.app.picker.is_none(), "the picker closes");
        }
    }

    /// The same commands, typed here and run from a remote controller,
    /// leave the same transcript.
    #[tokio::test]
    async fn remote_commands_read_like_the_same_commands_typed_here() {
        let mut here = Here::new();
        let mut there = There::new();

        // A prompt starts a run (nowhere: the backend runs nothing).
        here.session.set_backend(Some(Arc::new(Idle)));
        there.session.set_backend(Some(Arc::new(Idle)));
        here.input("hello there");
        there
            .run(Command::Prompt {
                message: "hello there".into(),
                images: Vec::new(),
                streaming_behavior: Some(StreamingBehavior::Steer),
            })
            .await;
        if let Some(run) = here.running_task.take() {
            run.await.expect("the run ends");
        }
        there.sdk.wait_idle().await;
        here.session.set_backend(None);
        there.session.set_backend(None);

        // Thinking level, name and model, each in its typed form here.
        let model = here.session.model();
        for command in [
            Command::SetThinkingLevel {
                level: "low".into(),
            },
            Command::SetSessionName {
                name: "Release notes".into(),
            },
            Command::SetModel {
                provider: model.provider.clone(),
                model_id: model.id.clone(),
            },
        ] {
            let Typed::Slash(line) = typed(&command) else {
                panic!("{command:?} is a slash command");
            };
            here.slash(&line);
            there.run(command).await;
        }

        // A workflow's approval, answered here and from the phone.
        here.cancel_workflow();
        there.cancel_workflow().await;

        assert_eq!(transcript(&here.app), transcript(&there.app));
        let lines = transcript(&there.app);
        for expected in [
            "user hello there",
            "notice thinking level set to low",
            "notice session named: Release notes",
            "notice workflow cancelled",
        ] {
            assert!(
                lines.contains(&expected.to_string()),
                "{expected}: {lines:?}"
            );
        }
        assert!(
            lines.iter().all(|line| !line.contains("set_thinking_level")
                && !line.contains("iPhone")
                && !line.contains('♦')),
            "{lines:?}"
        );
        assert_eq!(there.app.session_title.as_deref(), Some("Release notes"));
        assert_eq!(
            there.app.editor.border_color_token,
            here.app.editor.border_color_token
        );

        // A shell command, which fails.
        here.app.cells.clear();
        there.app.cells.clear();
        here.shell("echo hi; exit 3").await;
        there
            .run(Command::Bash {
                command: "echo hi; exit 3".into(),
            })
            .await;
        assert_eq!(transcript(&here.app), transcript(&there.app));
        assert_eq!(
            transcript(&there.app),
            [
                "shell echo hi; exit 3 hi\n false",
                "notice shell command exited with 3"
            ]
        );

        // Resumed, the two sessions read the same: their files hold the
        // same entries whoever ran the commands.
        let replay = |session: &Arc<super_coding::AgentSession>| -> Vec<String> {
            session_cells(session).iter().map(describe).collect()
        };
        assert_eq!(replay(&here.session), replay(&there.session));
        assert!(replay(&there.session).contains(&"shell echo hi; exit 3 hi\n false".to_string()));
        #[cfg(unix)]
        {
            let attached = |session: &Arc<super_coding::AgentSession>| -> Vec<String> {
                super::super::attached::transcript_cells(session)
                    .iter()
                    .map(describe)
                    .collect()
            };
            assert_eq!(attached(&here.session), attached(&there.session));
        }

        // A new session.
        here.slash("/new");
        there.run(Command::NewSession {}).await;
        assert_eq!(transcript(&here.app), transcript(&there.app));
        assert_eq!(transcript(&there.app), ["notice started a new session"]);
    }

    #[test]
    fn every_command_has_the_form_a_user_types() {
        let cases = [
            (
                Command::SetThinkingLevel {
                    level: "low".into(),
                },
                Typed::Slash("/thinking low".into()),
            ),
            (
                Command::SetModel {
                    provider: "anthropic".into(),
                    model_id: "claude".into(),
                },
                Typed::Slash("/model anthropic/claude".into()),
            ),
            (
                Command::Steer {
                    message: "also cats".into(),
                    images: Vec::new(),
                },
                Typed::Input("also cats".into()),
            ),
            (Command::Abort {}, Typed::Key("esc")),
            (Command::NewSession {}, Typed::Slash("/new".into())),
            (
                Command::Bash {
                    command: "ls".into(),
                },
                Typed::Shell("!ls".into()),
            ),
            (
                Command::Compact {
                    custom_instructions: None,
                },
                Typed::Slash("/compact".into()),
            ),
            (Command::GetState {}, Typed::Nothing),
        ];
        for (command, expected) in cases {
            assert_eq!(typed(&command), expected, "{command:?}");
        }
    }

    #[test]
    fn queued_prompts_wait_for_their_turn_and_failed_ones_show_nothing() {
        let session = test_session(super_coding::SessionManager::in_memory(Path::new("/tmp/p")));
        let prompt = Command::Prompt {
            message: "later".into(),
            images: Vec::new(),
            streaming_behavior: Some(StreamingBehavior::Steer),
        };
        assert!(matches!(
            shown_for(
                &prompt,
                Some(&serde_json::json!({"disposition": "queued"})),
                &session
            ),
            Shown::Events
        ));
        assert!(matches!(
            shown_for(&prompt, Some(&serde_json::json!({"disposition": "started"})), &session),
            Shown::Prompt(text) if text == "later"
        ));
        assert!(matches!(
            shown_for(&Command::Abort {}, None, &session),
            Shown::Events
        ));
    }
}
