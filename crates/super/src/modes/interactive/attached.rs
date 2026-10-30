//! The TUI on a session the SUPER daemon runs.
//!
//! `run_attached` builds the TUI's session the way `run` does, with an
//! in-memory session file, and hands it to
//! [`super_daemon::attached::RemoteSession`]: the session becomes a view of
//! the daemon's session. Prompts, steers, abort, model and thinking changes
//! and compaction go to the daemon; the daemon's events are rendered by the
//! same code that renders a local run; the transcript is replayed from the
//! daemon's entries on attach. Closing the terminal detaches and leaves the
//! session running there.
//!
//! What the TUI does differently while attached lives here; each hook in
//! `interactive.rs` is a line that asks this module first.

use super::*;
use crate::attach::Target;
use super_daemon::attached::{
    AppliedCommand, AttachedNotice, Attachment, Connector, RemoteSession,
};
use super_daemon::client::NewSession;
use super_sdk::daemon::EndReason;

/// The daemon's side of an attached TUI.
pub(super) struct Attached {
    remote: Arc<RemoteSession>,
    /// The daemon's question the picker shows.
    approval: Option<OpenQuestion>,
    reconnecting: bool,
    /// No daemon came back; the session can continue in this process.
    lost: bool,
}

struct OpenQuestion {
    id: String,
    kind: ApprovalKind,
    title: String,
    /// This terminal answered; the daemon has not said so yet.
    answered: bool,
}

/// The pickers this module opens.
pub(super) enum AttachedPicker {
    /// The daemon asks; the values index `choices`.
    Approval {
        approval_id: String,
        choices: Vec<String>,
    },
    /// The daemon is gone: continue in this process?
    Lost,
}

/// What reaches the TUI loop from the daemon or from work this module
/// started.
pub(super) enum Update {
    Notice(AttachedNotice),
    /// `/new` created a session and switched to it.
    Switched(std::result::Result<super_sdk::daemon::SessionSummary, String>),
    Failed(String),
}

/// The arguments the TUI's own session is built with: everything that
/// shapes tools, prompts and themes stays; the session itself, and what
/// configures a new one, belongs to the daemon.
pub(super) fn view_args(args: &Args) -> Args {
    let mut view = args.clone();
    view.no_session = true;
    view.continue_recent = false;
    view.session = None;
    view.model = None;
    view.provider = None;
    view.thinking = None;
    view.name = None;
    view
}

/// Make `view` a view of the attached session.
pub(super) async fn attach(
    target: Target,
    view: &Arc<super_coding::AgentSession>,
    sink: super_coding::SessionEventSink,
) -> Result<(Attached, mpsc::UnboundedReceiver<AttachedNotice>)> {
    let Target {
        client,
        events,
        attached,
        info,
    } = target;
    let (remote, notices) = RemoteSession::attach(
        Attachment {
            client,
            events,
            attached,
            info,
            connector: Connector::Discover,
        },
        view,
        sink,
    )
    .await
    .map_err(|error| {
        anyhow::anyhow!(
            "could not attach through the SUPER daemon: {error}. `super --no-remote` runs in this process"
        )
    })?;
    Ok((
        Attached {
            remote,
            approval: None,
            reconnecting: false,
            lost: false,
        },
        notices,
    ))
}

/// Hand the daemon's notices to the TUI loop.
pub(super) fn forward(
    mut notices: mpsc::UnboundedReceiver<AttachedNotice>,
    command_tx: &mpsc::UnboundedSender<CommandEvent>,
) {
    let tx = command_tx.clone();
    tokio::spawn(async move {
        while let Some(notice) = notices.recv().await {
            if tx
                .send(CommandEvent::Attached(Update::Notice(notice)))
                .is_err()
            {
                break;
            }
        }
    });
}

/// The first screen: the whole transcript, then where it runs.
pub(super) fn show_transcript(app: &mut App, session: &Arc<super_coding::AgentSession>) {
    let Some(attached) = &app.attached else {
        return;
    };
    let summary = attached.remote.session();
    let line = format!(
        "attached to session {} in the SUPER daemon {} · ctrl+d detaches and leaves it running",
        short_id(&summary.session_id),
        attached.remote.daemon_version(),
    );
    app.cells = transcript_cells(session);
    app.cell_render_cache.clear();
    app.cells.push(Cell::Notice(line));
    // Joining a turn in flight: it streams in from its next event.
    app.working = session.is_running();
}

/// The active branch of the session as the TUI renders it live: messages,
/// thinking, tool calls with their results, shell commands, compactions.
pub(super) fn transcript_cells(session: &Arc<super_coding::AgentSession>) -> Vec<Cell> {
    let manager = session.manager.lock().unwrap();
    let entries = manager.branch_entries(None);
    let results: std::collections::HashMap<&str, &super_ai::ToolResultMessage> = entries
        .iter()
        .filter_map(|entry| match entry {
            super_coding::SessionEntry::Message {
                message: AgentMessage::ToolResult(result),
                ..
            } => Some((result.tool_call_id.as_str(), result)),
            _ => None,
        })
        .collect();
    let mut cells = Vec::new();
    for entry in entries {
        match entry {
            super_coding::SessionEntry::Message { message, .. } => match message {
                AgentMessage::User(user) => {
                    cells.push(Cell::User(visible_user_text(&user.content.as_text())))
                }
                AgentMessage::Assistant(assistant) => {
                    let thinking: String = assistant
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
                            _ => None,
                        })
                        .collect();
                    if !thinking.is_empty() {
                        cells.push(Cell::Thinking(thinking));
                    }
                    cells.push(Cell::AssistantFinal(assistant.text()));
                    match assistant.stop_reason {
                        StopReason::Error => cells.push(Cell::Error(
                            assistant
                                .error_message
                                .clone()
                                .unwrap_or_else(|| "unknown error".into()),
                        )),
                        StopReason::Aborted => cells.push(Cell::Notice("aborted".into())),
                        _ => {}
                    }
                    for block in &assistant.content {
                        if let ContentBlock::ToolCall(call) = block {
                            let result = results.get(call.id.as_str());
                            cells.push(Cell::ToolCall {
                                title: tool_title(&call.name, &call.arguments),
                                output: result
                                    .map(|result| {
                                        result
                                            .content
                                            .iter()
                                            .filter_map(|block| match block {
                                                ContentBlock::Text { text, .. } => {
                                                    Some(text.as_str())
                                                }
                                                _ => None,
                                            })
                                            .collect::<Vec<_>>()
                                            .join("\n")
                                    })
                                    .unwrap_or_default(),
                                is_error: result.is_some_and(|result| result.is_error),
                                done: true,
                            });
                        }
                    }
                }
                AgentMessage::BashExecution(bash) => cells.push(Cell::BashExecution {
                    command: bash.command.clone(),
                    output: bash.output.clone(),
                    exclude_from_context: bash.exclude_from_context,
                }),
                _ => {}
            },
            super_coding::SessionEntry::Compaction { tokens_before, .. } => cells.push(
                Cell::Notice(format!("compacted {tokens_before} tokens of history")),
            ),
            _ => {}
        }
    }
    cells
}

/// ` · ♦ daemon` while connected, `…` while reconnecting.
pub(super) fn footer_label(attached: &Attached) -> String {
    if attached.lost {
        " · ♦ daemon gone".into()
    } else if attached.reconnecting || !attached.remote.is_connected() {
        " · ♦ daemon…".into()
    } else {
        " · ♦ daemon".into()
    }
}

/// React to what the daemon or this module's own work reported.
pub(super) fn handle_update(
    app: &mut App,
    update: Update,
    session: &Arc<super_coding::AgentSession>,
) {
    match update {
        Update::Notice(notice) => handle_notice(app, notice, session),
        Update::Switched(Ok(_)) => {
            commands::show(app, session, commands::Shown::NewSession);
            update_thinking_border(app, session.thinking_level());
        }
        Update::Switched(Err(error)) => app
            .cells
            .push(Cell::Error(format!("could not create session: {error}"))),
        Update::Failed(error) => app.cells.push(Cell::Error(error)),
    }
}

fn handle_notice(app: &mut App, notice: AttachedNotice, session: &Arc<super_coding::AgentSession>) {
    match notice {
        AttachedNotice::ApprovalRequested {
            approval_id,
            request,
        } => open_approval(app, approval_id, request),
        AttachedNotice::ApprovalResolved {
            approval_id,
            choice,
            ..
        } => {
            let Some(attached) = &mut app.attached else {
                return;
            };
            let question = match attached.approval.take() {
                Some(question) if question.id == approval_id => question,
                other => {
                    // Not a question this terminal was shown.
                    attached.approval = other;
                    return;
                }
            };
            if matches!(
                app.picker.as_ref().map(|picker| &picker.kind),
                Some(PickerKind::Attached(AttachedPicker::Approval { .. }))
            ) {
                app.picker = None;
            }
            // Whichever client answered, it reads as answered here.
            let shown = match question.kind {
                ApprovalKind::Workflow => commands::Shown::WorkflowAnswered {
                    name: workflow_name(&question.title),
                    approved: choice == "approve",
                    progress_view: false,
                },
                _ => commands::Shown::Answered {
                    title: question.title,
                    choice,
                },
            };
            commands::show(app, session, shown);
        }
        AttachedNotice::CommandApplied(applied) => command_applied(app, applied, session),
        AttachedNotice::CommandFailed { command, error } => {
            app.cells.push(Cell::Error(format!("{command}: {error}")));
            if app.attached.as_ref().is_some_and(|attached| attached.lost) {
                open_lost_picker(app);
            }
        }
        AttachedNotice::ShellOutput(chunk) => append_shell_output_cell(app, &chunk),
        AttachedNotice::SessionUpdated(summary) => {
            if summary.name.is_some() && summary.name != app.session_title {
                set_session_title(app, summary.name);
            }
        }
        AttachedNotice::SessionEnded(reason) => {
            app.working = false;
            let why = match reason {
                EndReason::Stopped => "was stopped",
                EndReason::Idle => "was unloaded while idle",
                EndReason::Shutdown => "ended with the SUPER daemon",
            };
            app.cells.push(Cell::Notice(format!(
                "this session {why}; your next message loads it again"
            )));
        }
        AttachedNotice::Reloaded => app
            .cells
            .push(Cell::Notice("loaded the session again".into())),
        AttachedNotice::DaemonShutdown => app
            .cells
            .push(Cell::Notice("the SUPER daemon is shutting down".into())),
        AttachedNotice::Disconnected(reason) => {
            app.working = false;
            if let Some(attached) = &mut app.attached {
                attached.reconnecting = true;
            }
            app.cells
                .push(Cell::Notice(format!("{reason}; reconnecting…")));
        }
        AttachedNotice::Reconnected => {
            if let Some(attached) = &mut app.attached {
                attached.reconnecting = false;
            }
            // What happened while the connection was down is in the
            // daemon's entries; show the transcript as they have it.
            app.cells = transcript_cells(session);
            app.cell_render_cache.clear();
            app.cells
                .push(Cell::Notice("reconnected to the SUPER daemon".into()));
            update_thinking_border(app, session.thinking_level());
        }
        AttachedNotice::Resynced => {
            app.cells = transcript_cells(session);
            app.cell_render_cache.clear();
            app.cells.push(Cell::Notice(
                "some output was skipped; the transcript was read again".into(),
            ));
        }
        AttachedNotice::Lost(reason) => {
            if let Some(attached) = &mut app.attached {
                attached.reconnecting = false;
                attached.lost = true;
            }
            app.working = false;
            app.cells
                .push(Cell::Error(format!("the SUPER daemon is gone ({reason})")));
            open_lost_picker(app);
        }
    }
}

/// A command another client ran on the session reads as the same command
/// typed here; this terminal's own are on screen already.
fn command_applied(
    app: &mut App,
    applied: AppliedCommand,
    session: &Arc<super_coding::AgentSession>,
) {
    if applied.own || !applied.success {
        return;
    }
    commands::show_remote(app, session, &applied.command, applied.data.as_ref());
}

/// `Run workflow greet?`, as the daemon's gate asks it, names `greet`.
fn workflow_name(title: &str) -> String {
    title
        .strip_prefix("Run workflow ")
        .and_then(|rest| rest.strip_suffix('?'))
        .unwrap_or(title)
        .to_string()
}

fn append_shell_output_cell(app: &mut App, chunk: &str) {
    if let Some(Cell::BashExecution { output, .. }) = app
        .cells
        .iter_mut()
        .rev()
        .find(|cell| matches!(cell, Cell::BashExecution { .. }))
    {
        output.push_str(chunk);
        if output.len() > 50_000 {
            output.truncate(output.floor_char_boundary(50_000));
        }
    }
}

fn open_approval(app: &mut App, approval_id: String, request: super_sdk::ApprovalRequest) {
    let Some(attached) = &mut app.attached else {
        return;
    };
    let items = request
        .choices
        .iter()
        .enumerate()
        .map(|(index, choice)| SelectItem {
            label: choice.label.clone(),
            detail: choice
                .detail
                .clone()
                .or_else(|| (index == 0).then(|| request.detail.clone()).flatten()),
            value: index,
        })
        .collect();
    if attached
        .approval
        .as_ref()
        .is_some_and(|question| question.id == approval_id)
    {
        // Already asked (attaching while it was open, then its event).
        return;
    }
    attached.approval = Some(OpenQuestion {
        id: approval_id.clone(),
        kind: request.kind,
        title: request.title.clone(),
        answered: false,
    });
    app.picker = Some(Picker {
        kind: PickerKind::Attached(AttachedPicker::Approval {
            approval_id,
            choices: request
                .choices
                .iter()
                .map(|choice| choice.id.clone())
                .collect(),
        }),
        list: SelectList::new(request.title, items, app.theme.clone()),
    });
}

fn open_lost_picker(app: &mut App) {
    let items = vec![
        SelectItem {
            label: "Continue in this process".into(),
            detail: Some("open the session file here and run it without the daemon".into()),
            value: 0,
        },
        SelectItem {
            label: "Not now".into(),
            detail: Some("ctrl+d quits; `super` here runs it in-process later".into()),
            value: 1,
        },
    ];
    app.picker = Some(Picker {
        kind: PickerKind::Attached(AttachedPicker::Lost),
        list: SelectList::new(
            "The SUPER daemon is gone. Continue in this process?",
            items,
            app.theme.clone(),
        ),
    });
}

/// A choice in one of this module's pickers.
pub(super) fn apply_picker(
    app: &mut App,
    session: &Arc<super_coding::AgentSession>,
    picker: AttachedPicker,
    value: usize,
    resources: &InteractiveResources,
    command_tx: &mpsc::UnboundedSender<CommandEvent>,
) {
    match picker {
        AttachedPicker::Approval {
            approval_id,
            choices,
        } => {
            let Some(attached) = &mut app.attached else {
                return;
            };
            let Some(choice) = choices.get(value).cloned() else {
                return;
            };
            // Shown when the daemon says it was answered, as for any client.
            if let Some(question) = &mut attached.approval {
                question.answered = true;
            }
            answer(&attached.remote, approval_id, choice, command_tx);
        }
        AttachedPicker::Lost if value == 0 => continue_here(app, session, resources, command_tx),
        AttachedPicker::Lost => {}
    }
}

fn answer(
    remote: &Arc<RemoteSession>,
    approval_id: String,
    choice: String,
    command_tx: &mpsc::UnboundedSender<CommandEvent>,
) {
    let remote = remote.clone();
    let tx = command_tx.clone();
    tokio::spawn(async move {
        if let Err(error) = remote.answer_approval(&approval_id, &choice).await {
            let _ = tx.send(CommandEvent::Attached(Update::Failed(format!(
                "could not answer: {error}"
            ))));
        }
    });
}

/// The approval picker went away without a choice (esc): cancel, as the
/// terminal does with its own questions.
pub(super) fn close_dismissed(app: &mut App, command_tx: &mpsc::UnboundedSender<CommandEvent>) {
    if app.picker.is_some() {
        return;
    }
    let Some(attached) = &mut app.attached else {
        return;
    };
    if let Some(question) = &mut attached.approval
        && !question.answered
    {
        question.answered = true;
        answer(
            &attached.remote,
            question.id.clone(),
            "cancel".into(),
            command_tx,
        );
    }
}

/// Take the engine back: open the session file in this process.
fn continue_here(
    app: &mut App,
    session: &Arc<super_coding::AgentSession>,
    resources: &InteractiveResources,
    command_tx: &mpsc::UnboundedSender<CommandEvent>,
) {
    let Some(attached) = &app.attached else {
        return;
    };
    let file = PathBuf::from(attached.remote.session().session_file);
    if super_daemon::session_lock::is_held(&file) {
        app.cells.push(Cell::Error(format!(
            "{} is still open in another SUPER process; it cannot continue here",
            shorten_path(&file.display().to_string())
        )));
        return;
    }
    let manager = match super_coding::SessionManager::open(&file) {
        Ok(manager) => manager,
        Err(error) => {
            app.cells.push(Cell::Error(format!(
                "could not open the session: {error:#}"
            )));
            return;
        }
    };
    if let Some(attached) = app.attached.take() {
        attached.remote.release();
    }
    // The lock keeper follows the session file and takes it within a beat.
    session.replace_manager(manager);
    app.cells = transcript_cells(session);
    app.cell_render_cache.clear();
    app.cells.push(Cell::Notice(format!(
        "continuing in this process: {}",
        shorten_path(&file.display().to_string())
    )));
    update_thinking_border(app, session.thinking_level());
    if app.remote_allowed && crate::remote::signed_in() {
        start_remote(app, session, &resources.settings, command_tx);
    }
}

/// Slash commands that mean something else while attached. `None` lets
/// the usual handler run.
pub(super) fn slash_command(
    app: &mut App,
    session: &Arc<super_coding::AgentSession>,
    name: &str,
    rest: &str,
    resources: &InteractiveResources,
    command_tx: &mpsc::UnboundedSender<CommandEvent>,
) -> Option<Flow> {
    let attached = app.attached.as_ref()?;
    let remote = attached.remote.clone();
    match name {
        "name" if !rest.is_empty() => {
            let name = rest.to_string();
            let tx = command_tx.clone();
            commands::show(app, session, commands::Shown::SessionName(name.clone()));
            tokio::spawn(async move {
                if let Err(error) = remote.set_session_name(&name).await {
                    let _ = tx.send(CommandEvent::Attached(Update::Failed(format!(
                        "could not name session: {error}"
                    ))));
                }
            });
        }
        "new" => {
            let cwd = session.manager.lock().unwrap().cwd().to_path_buf();
            let tx = command_tx.clone();
            tokio::spawn(async move {
                let result = remote
                    .new_session(NewSession {
                        cwd,
                        ..Default::default()
                    })
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(CommandEvent::Attached(Update::Switched(result)));
            });
        }
        "session" => app.cells.push(Cell::Notice(session_info(session, &remote))),
        "remote" if matches!(rest, "" | "status" | "on" | "off") => {
            let summary = remote.session();
            app.cells.push(Cell::Notice(format!(
                "This terminal is attached to session {} in the SUPER daemon {}, which keeps it visible in the SUPER app while signed in. `daemon.attach: false` in settings keeps terminals in-process.",
                short_id(&summary.session_id),
                remote.daemon_version(),
            )));
        }
        "resume" | "fork" | "clone" | "tree" | "import" | "loop" | "autoresearch" | "jobs"
        | "fast" => refuse(app, name),
        other
            if resources
                .saved_workflows
                .iter()
                .any(|workflow| workflow.name == other) =>
        {
            refuse(app, other)
        }
        _ => return None,
    }
    Some(Flow::Continue)
}

fn refuse(app: &mut App, name: &str) {
    app.cells.push(Cell::Notice(format!(
        "/{name} is not available while this terminal is attached to the SUPER daemon; `super --no-remote` starts a session in this process"
    )));
}

fn session_info(session: &Arc<super_coding::AgentSession>, remote: &RemoteSession) -> String {
    let summary = remote.session();
    let manager = session.manager.lock().unwrap();
    let totals = session.totals();
    let model = session.model();
    format!(
        "Session {}\n  name: {}\n  cwd: {}\n  file: {} (in the SUPER daemon {}, {} attached)\n  entries: {}\n  model: {}/{} · thinking {}\n  tokens: input {} · output {} · cache read {} · cache write {} · total {}\n  cost: ${:.4}",
        summary.session_id,
        manager.session_name().unwrap_or_else(|| "(unnamed)".into()),
        manager.cwd().display(),
        summary.session_file,
        remote.daemon_version(),
        summary.attached_clients,
        manager.entries().len(),
        model.provider,
        model.id,
        session.thinking_level().as_str(),
        totals.input,
        totals.output,
        totals.cache_read,
        totals.cache_write,
        totals.total_tokens,
        totals.cost.total,
    )
}

/// The title the TUI generated for the session goes to the daemon. `true`
/// when attached (the caller does nothing else).
pub(super) fn title_generated(
    app: &mut App,
    session_id: &str,
    result: &std::result::Result<String, String>,
    command_tx: &mpsc::UnboundedSender<CommandEvent>,
) -> bool {
    let Some(attached) = &app.attached else {
        return false;
    };
    let remote = attached.remote.clone();
    if remote.session_id() != session_id {
        return true;
    }
    app.title_generation_pending = false;
    if let Ok(title) = result
        && app.session_title.is_none()
    {
        set_session_title(app, Some(title.clone()));
        let title = title.clone();
        let tx = command_tx.clone();
        tokio::spawn(async move {
            if let Err(error) = remote.set_session_name(&title).await {
                let _ = tx.send(CommandEvent::Attached(Update::Failed(format!(
                    "could not name session: {error}"
                ))));
            }
        });
    }
    true
}

/// `!command` runs in the daemon, which records it for the next model
/// request the way the terminal records it in-process. `true` when
/// attached.
pub(super) fn shell(
    app: &mut App,
    command: &str,
    command_tx: &mpsc::UnboundedSender<CommandEvent>,
) -> bool {
    let Some(attached) = &app.attached else {
        return false;
    };
    let remote = attached.remote.clone();
    let command = command.to_string();
    let cancel = CancellationToken::new();
    app.command_cancel = Some(cancel.clone());
    app.command_status = Some(format!("running !{command}"));
    app.cells.push(Cell::BashExecution {
        command: command.clone(),
        output: String::new(),
        exclude_from_context: false,
    });
    let tx = command_tx.clone();
    tokio::spawn(async move {
        let shell = remote.clone();
        let line = command.clone();
        let run = async move { shell.bash(&line).await };
        tokio::pin!(run);
        let mut aborted = false;
        let result = loop {
            tokio::select! {
                result = &mut run => break result,
                _ = cancel.cancelled(), if !aborted => {
                    aborted = true;
                    remote.abort_bash();
                }
            }
        };
        let result = result
            .map(|data| ShellRunResult {
                command,
                output: data["output"].as_str().unwrap_or_default().to_string(),
                exit_code: data["exitCode"].as_i64().map(|code| code as i32),
                cancelled: data["cancelled"].as_bool().unwrap_or(false),
                truncated: data["truncated"].as_bool().unwrap_or(false),
                exclude_from_context: false,
            })
            .map_err(|error| error.to_string());
        let _ = tx.send(CommandEvent::ShellFinished(result));
    });
    true
}

/// Leave: detach, say where the session went.
pub(super) async fn finish(attached: Attached) {
    let summary = attached.remote.session();
    attached.remote.detach().await;
    println!(
        "detached from session {}; it keeps running in the SUPER daemon",
        short_id(&summary.session_id)
    );
    println!("`super` here attaches again");
}

fn short_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    fn test_session() -> Arc<super_coding::AgentSession> {
        let registry = super_ai::Registry::from_builtin();
        let model = registry.all().first().expect("built-in model").clone();
        super_coding::AgentSession::new(
            super_coding::SessionManager::in_memory(Path::new("/tmp/project")),
            Vec::new(),
            registry,
            super_coding::Settings::default(),
            "test prompt".into(),
            model,
            ThinkingLevel::Off,
            None,
            Arc::new(|_| {}),
        )
    }

    #[test]
    fn the_view_keeps_engine_options_and_leaves_the_session_to_the_daemon() {
        let args = Args::parse_from([
            "super",
            "-c",
            "--model",
            "mock/mock-1",
            "--thinking",
            "high",
            "--name",
            "x",
            "hello",
        ]);
        let view = view_args(&args);
        assert!(view.no_session);
        assert!(!view.continue_recent);
        assert!(view.model.is_none() && view.thinking.is_none() && view.name.is_none());
        assert_eq!(view.messages, ["hello"], "the first prompt still goes out");
    }

    #[test]
    fn the_transcript_shows_tool_calls_with_their_results() {
        let session = test_session();
        {
            let mut manager = session.manager.lock().unwrap();
            manager
                .append_message(AgentMessage::user("list it"))
                .unwrap();
            let mut assistant = super_ai::AssistantMessage::empty("api", "mock", "mock-1");
            assistant.content = vec![
                ContentBlock::text("Looking."),
                ContentBlock::ToolCall(super_ai::ToolCall {
                    id: "call_1".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({"command": "ls"}),
                    thought_signature: None,
                }),
            ];
            manager
                .append_message(AgentMessage::Assistant(assistant))
                .unwrap();
            manager
                .append_message(AgentMessage::ToolResult(super_ai::ToolResultMessage {
                    tool_call_id: "call_1".into(),
                    tool_name: "bash".into(),
                    content: vec![ContentBlock::text("a.rs")],
                    details: None,
                    usage: None,
                    is_error: false,
                    timestamp: 0,
                }))
                .unwrap();
        }
        let cells = transcript_cells(&session);
        assert!(matches!(&cells[0], Cell::User(text) if text == "list it"));
        assert!(matches!(&cells[1], Cell::AssistantFinal(text) if text == "Looking."));
        match &cells[2] {
            Cell::ToolCall {
                title,
                output,
                done,
                is_error,
            } => {
                assert_eq!(title, "bash ls");
                assert_eq!(output, "a.rs");
                assert!(*done && !*is_error);
            }
            _ => panic!("a tool call"),
        }
        assert_eq!(cells.len(), 3);
    }
}
