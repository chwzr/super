//! `super` attached to the SUPER daemon.
//!
//! Signed in to the platform, with a daemon running or registered to start
//! on demand, a plain `super` in a terminal does not run the engine itself:
//! it attaches to the daemon's session for the current directory, so the
//! session is the one the apps see and it keeps running after the terminal
//! closes. Without an account, without a daemon, with `--no-remote` or
//! `daemon.attach: false`, `super` runs in-process exactly as before.
//!
//! The front end here is line based: it streams the reply, names each tool
//! call, asks approvals as numbered choices and sends what you type as a
//! prompt (a steer while the session works). The full TUI over the same
//! connection is ENG-13.

use crate::args::Args;
use anyhow::Result;
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{IsTerminal as _, Write as _};
use std::path::Path;
use super_coding::Settings;
use super_daemon::client::{
    Attached, ClientError, ClientEvent, ClientInfo, DaemonClient, NewSession,
};
use super_sdk::daemon::{ClientKind, DaemonEvent, SessionSummary};
use super_sdk::protocol::{Command, StreamingBehavior};
use tokio::io::AsyncBufReadExt as _;
use tokio::sync::mpsc::UnboundedReceiver;

/// Whether this invocation goes to the daemon. Only a plain interactive
/// `super` does: options that shape how the engine is built (tools, skills,
/// prompts, trust, themes, another session directory) and modes other than
/// the terminal keep running in-process, where they mean what they always
/// meant.
pub fn wanted(args: &Args, settings: &Settings, signed_in: bool) -> bool {
    let terminal_mode = args.command.is_none()
        && args.mode.is_none()
        && !args.print
        && args.rpc_listen.is_none()
        && args.export.is_none()
        && args.list_models.is_none();
    let engine_options = args.resume
        || args.fork.is_some()
        || args.api_key.is_some()
        || args.models.is_some()
        || args.session_dir.is_some()
        || args.experimental_context_file
        || args.tools.is_some()
        || args.exclude_tools.is_some()
        || args.no_tools
        || !args.skills.is_empty()
        || args.no_skills
        || !args.prompt_templates.is_empty()
        || args.no_prompt_templates
        || !args.themes.is_empty()
        || args.no_themes
        || args.no_context_files
        || args.system_prompt.is_some()
        || args.append_system_prompt.is_some()
        || args.approve
        || args.no_approve;
    // Model options configure a new session; they do not apply to one that
    // is resumed, so that combination stays in-process too.
    let resumes_with_new_options =
        (args.continue_recent || args.session.is_some()) && configures_new_session(args);
    let opted_out = args.no_remote
        || args.no_session
        || !settings.remote.is_enabled()
        || !settings.daemon.attach_enabled();
    terminal_mode && signed_in && !(engine_options || resumes_with_new_options || opted_out)
}

fn configures_new_session(args: &Args) -> bool {
    args.model.is_some()
        || args.provider.is_some()
        || args.thinking.is_some()
        || args.name.is_some()
}

/// The session to attach to without creating one: `--continue` takes the
/// newest session in the directory, hosted or stored; a plain `super` joins
/// the newest one the daemon is running there. `None` creates a session.
fn existing_session(args: &Args, sessions: &[SessionSummary]) -> Option<String> {
    if args.continue_recent {
        return sessions.first().map(|session| session.session_id.clone());
    }
    if configures_new_session(args) {
        return None;
    }
    sessions
        .iter()
        .find(|session| session.hosted)
        .map(|session| session.session_id.clone())
}

/// Attach and run the terminal front end. `Ok(None)` means "run in-process":
/// the ecosystem is off, or no daemon answers or can be started.
pub async fn try_run(args: &Args) -> Result<Option<i32>> {
    let cwd = std::env::current_dir()?;
    let settings = Settings::load(&cwd, false);
    if !wanted(args, &settings, crate::remote::signed_in()) {
        return Ok(None);
    }
    // `@file` arguments are sent with the file's contents and shown as typed.
    let initial = crate::setup::initial_message(args, &cwd)?
        .map(|message| (args.messages.join(" "), message));
    let info = ClientInfo::new(ClientKind::Tui, "Terminal");
    let mut client = match DaemonClient::connect_or_start(&info).await {
        Ok(client) => client,
        Err(ClientError::Skew(skew)) => {
            eprintln!("error: {skew}");
            eprintln!("`super --no-remote` runs without the daemon until then.");
            return Ok(Some(2));
        }
        Err(error @ (ClientError::NotRunning { .. } | ClientError::Unavailable(_))) => {
            if args.verbose {
                eprintln!("{error}; running in this process");
            }
            return Ok(None);
        }
        Err(error) => {
            eprintln!("warning: {error}; running in this process");
            return Ok(None);
        }
    };
    let Some(events) = client.take_events() else {
        return Ok(None);
    };
    let attached = match choose_and_attach(&client, args, &cwd).await {
        Ok(attached) => attached,
        Err(error) => {
            eprintln!(
                "warning: could not attach through the SUPER daemon: {error}; running in this process"
            );
            return Ok(None);
        }
    };
    let input: Input = Box::new(tokio::io::BufReader::new(tokio::io::stdin()));
    run(client, events, attached, initial, input, true)
        .await
        .map(Some)
}

/// Lines typed into the attached front end.
pub(crate) type Input = Box<dyn tokio::io::AsyncBufRead + Unpin + Send>;

/// A connection attached to the session `/remote` just handed to the daemon.
pub(crate) struct Continuation {
    pub client: DaemonClient,
    pub events: UnboundedReceiver<ClientEvent>,
    pub attached: Attached,
}

/// Carry on as an attached terminal after `/remote` handed the session to
/// the daemon. The transcript is on screen already, so nothing is replayed.
pub(crate) async fn run_handed_over(continuation: Continuation, input: Input) -> Result<i32> {
    let Continuation {
        client,
        events,
        attached,
    } = continuation;
    run(client, events, attached, None, input, false).await
}

async fn choose_and_attach(
    client: &DaemonClient,
    args: &Args,
    cwd: &Path,
) -> Result<Attached, ClientError> {
    let existing = match &args.session {
        // A partial id names whichever session it starts; the daemon wants
        // the whole id for one it hosts.
        Some(reference) => {
            let id = session_reference(reference);
            let sessions = client.list_sessions(None).await?;
            Some(
                sessions
                    .into_iter()
                    .find(|session| session.session_id.starts_with(&id))
                    .map(|session| session.session_id)
                    .unwrap_or(id),
            )
        }
        None => existing_session(args, &client.list_sessions(Some(cwd)).await?),
    };
    let session_id = match existing {
        Some(id) => id,
        None => {
            client
                .create_session(NewSession {
                    cwd: cwd.to_path_buf(),
                    model: args.model.clone(),
                    provider: args.provider.clone(),
                    thinking_level: args.thinking.clone(),
                    name: args.name.clone(),
                })
                .await?
                .session_id
        }
    };
    client.attach(&session_id).await
}

/// `--session` takes a file or a (partial) id; the daemon wants the id.
fn session_reference(reference: &str) -> String {
    let path = Path::new(reference);
    if path.is_file()
        && let Ok(manager) = super_coding::SessionManager::open(path)
    {
        return manager.session_id().to_string();
    }
    reference.to_string()
}

/// An approval waiting for a numbered answer.
struct OpenApproval {
    id: String,
    choices: Vec<(String, String)>,
}

struct Screen {
    color: bool,
    interactive: bool,
    /// The reply is streaming mid-line.
    mid_line: bool,
    /// The cursor waits after `› `; output starts on the next line.
    at_prompt: bool,
}

impl Screen {
    fn dim(&self, text: &str) -> String {
        if self.color {
            format!("\x1b[2m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn line(&mut self, text: &str) {
        let mut out = std::io::stdout().lock();
        if self.mid_line || self.at_prompt {
            let _ = writeln!(out);
            self.mid_line = false;
            self.at_prompt = false;
        }
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }

    fn meta(&mut self, text: &str) {
        let text = self.dim(text);
        self.line(&text);
    }

    fn stream(&mut self, delta: &str) {
        let mut out = std::io::stdout().lock();
        if self.at_prompt {
            let _ = writeln!(out);
            self.at_prompt = false;
        }
        let _ = write!(out, "{delta}");
        let _ = out.flush();
        self.mid_line = !delta.ends_with('\n');
    }

    fn end_reply(&mut self) {
        if self.mid_line || self.at_prompt {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out);
            let _ = out.flush();
            self.mid_line = false;
            self.at_prompt = false;
        }
    }

    fn prompt(&mut self) {
        if self.interactive && !self.at_prompt {
            self.end_reply();
            let mut out = std::io::stdout().lock();
            let _ = write!(out, "› ");
            let _ = out.flush();
            self.at_prompt = true;
        }
    }

    /// The user pressed enter: the terminal moved to the next line.
    fn entered(&mut self) {
        self.at_prompt = false;
    }
}

const HELP: &str = "enter sends (a steer while it works) · ctrl+c aborts the turn, or detaches when idle · /detach or ctrl+d leaves it running · /stop stops the session";

async fn run(
    client: DaemonClient,
    mut events: UnboundedReceiver<ClientEvent>,
    attached: Attached,
    initial: Option<(String, String)>,
    input: Input,
    intro: bool,
) -> Result<i32> {
    let session = attached.session;
    let interactive = std::io::stdin().is_terminal();
    let mut screen = Screen {
        color: std::io::stdout().is_terminal(),
        interactive,
        mid_line: false,
        at_prompt: false,
    };
    if intro {
        let hello = client.hello().clone();
        let mut header = format!(
            "attached to session {} in {} · SUPER daemon {} (pid {})",
            short_id(&session.session_id),
            session.cwd,
            hello.daemon_version,
            hello.pid
        );
        if let Some(model) = &session.model {
            header.push_str(&format!(" · {model}"));
        }
        screen.meta(&header);
        if interactive {
            screen.meta(HELP);
        }
    }

    let mut working = false;
    if let Ok(Some(state)) = client.execute_ok(Command::GetState {}).await {
        working = state["isStreaming"].as_bool().unwrap_or(false);
    }
    if intro
        && let Ok(Some(data)) = client.execute_ok(Command::GetMessages {}).await
        && let Some(messages) = data["messages"].as_array()
    {
        print_recent(&mut screen, messages, 6);
    }

    let mut own_prompts: VecDeque<String> = VecDeque::new();
    let mut approval: Option<OpenApproval> = None;
    if let Some((shown, message)) = initial {
        screen.line(&format!("› {shown}"));
        send_prompt(&client, &mut screen, &mut own_prompts, message, working).await;
        working = true;
    } else if !working {
        screen.prompt();
    }

    let mut input = input.lines();
    let mut input_open = true;
    let code = loop {
        tokio::select! {
            line = input.next_line(), if input_open => {
                let line = match line {
                    Ok(Some(line)) => {
                        screen.entered();
                        line
                    }
                    Ok(None) | Err(_) => {
                        input_open = false;
                        // Piped input waits for the turn it started.
                        if interactive || !working {
                            break 0;
                        }
                        continue;
                    }
                };
                let text = line.trim().to_string();
                if text.is_empty() {
                    if !working {
                        screen.prompt();
                    }
                    continue;
                }
                if let Some(open) = &approval {
                    match pick_choice(&open.choices, &text) {
                        Some(choice) => {
                            let answer = client
                                .execute_ok(Command::AnswerApproval {
                                    approval_id: open.id.clone(),
                                    choice,
                                })
                                .await;
                            if let Err(error) = answer {
                                screen.meta(&format!("could not answer: {error}"));
                            }
                            approval = None;
                        }
                        None => screen.meta(&format!(
                            "answer with a number from 1 to {}",
                            open.choices.len()
                        )),
                    }
                    continue;
                }
                match text.as_str() {
                    "/detach" | "/exit" | "/quit" => break 0,
                    "/help" => {
                        screen.meta(HELP);
                        screen.prompt();
                    }
                    "/remote" | "/rc" | "/remote status" => {
                        screen.meta(&crate::handover::attached_status(&client, &session.session_id));
                        screen.prompt();
                    }
                    "/abort" => {
                        let _ = client.execute(Command::Abort {}).await;
                    }
                    "/stop" => {
                        match client.stop_session(&session.session_id).await {
                            Ok(()) => screen.meta("stopped; the session can be resumed with `super -c`"),
                            Err(error) => screen.meta(&format!("could not stop: {error}")),
                        }
                        break 0;
                    }
                    _ => {
                        send_prompt(&client, &mut screen, &mut own_prompts, text, working).await;
                        working = true;
                    }
                }
            }
            event = events.recv() => {
                let Some(event) = event else {
                    screen.meta("the SUPER daemon closed the connection");
                    break 1;
                };
                match event {
                    ClientEvent::Daemon(DaemonEvent::SessionEnded { session_id, reason })
                        if session_id == session.session_id =>
                    {
                        screen.meta(&format!(
                            "the session was unloaded ({}); `super -c` resumes it",
                            serde_json::to_value(reason)
                                .ok()
                                .and_then(|value| value.as_str().map(str::to_string))
                                .unwrap_or_default()
                        ));
                        break 0;
                    }
                    ClientEvent::Daemon(DaemonEvent::DaemonShutdown { .. }) => {
                        screen.meta("the SUPER daemon is shutting down");
                        break 0;
                    }
                    ClientEvent::Daemon(_) => {}
                    ClientEvent::Session(event) => {
                        let kind = event.event_type().to_string();
                        render(&mut screen, &event.0, &mut own_prompts, &mut approval);
                        match kind.as_str() {
                            "agent_start" => working = true,
                            "agent_settled" => {
                                working = false;
                                if !input_open {
                                    break 0;
                                }
                                screen.prompt();
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                if working {
                    screen.meta("aborting");
                    let _ = client.execute(Command::Abort {}).await;
                } else {
                    break 0;
                }
            }
        }
    };
    screen.end_reply();
    if !client.is_closed() {
        let _ = client.detach().await;
        if code == 0 {
            screen.meta(&format!(
                "detached from {}; it keeps running in the SUPER daemon and `super` here attaches again",
                short_id(&session.session_id)
            ));
        }
    }
    Ok(code)
}

async fn send_prompt(
    client: &DaemonClient,
    screen: &mut Screen,
    own_prompts: &mut VecDeque<String>,
    message: String,
    working: bool,
) {
    own_prompts.push_back(message.clone());
    if own_prompts.len() > 16 {
        own_prompts.pop_front();
    }
    let command = Command::Prompt {
        message,
        images: Vec::new(),
        streaming_behavior: working.then_some(StreamingBehavior::Steer),
    };
    if let Err(error) = client.execute_ok(command).await {
        screen.meta(&format!("not sent: {error}"));
    }
}

/// Print one session event the way a reader wants to see it.
fn render(
    screen: &mut Screen,
    event: &Value,
    own_prompts: &mut VecDeque<String>,
    approval: &mut Option<OpenApproval>,
) {
    match event["type"].as_str().unwrap_or("") {
        "message_update" => {
            let delta = &event["assistantMessageEvent"];
            if delta["type"] == "text_delta"
                && let Some(text) = delta["delta"].as_str()
            {
                screen.stream(text);
            }
        }
        "message_start" if event["message"]["role"] == "user" => {
            let text = message_text(&event["message"]);
            // What this terminal typed is on screen already.
            if let Some(position) = own_prompts.iter().position(|sent| *sent == text) {
                own_prompts.remove(position);
            } else if !text.is_empty() {
                screen.line(&format!("› {text}"));
            }
        }
        "message_end" if event["message"]["role"] == "assistant" => {
            screen.end_reply();
            if event["message"]["stopReason"] == "error" {
                let error = event["message"]["errorMessage"]
                    .as_str()
                    .unwrap_or("the model request failed");
                screen.line(&format!("error: {error}"));
            }
        }
        "tool_execution_start" => {
            let name = event["toolName"].as_str().unwrap_or("tool");
            screen.meta(&format!("• {}", tool_summary(name, &event["args"])));
        }
        "tool_execution_end" if event["isError"] == true => {
            let name = event["toolName"].as_str().unwrap_or("tool");
            screen.meta(&format!("  {name} failed"));
        }
        "approval_request" => {
            let choices: Vec<(String, String)> = event["choices"]
                .as_array()
                .map(|choices| {
                    choices
                        .iter()
                        .filter_map(|choice| {
                            Some((
                                choice["id"].as_str()?.to_string(),
                                choice["label"]
                                    .as_str()
                                    .or_else(|| choice["id"].as_str())?
                                    .to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            screen.line(&format!(
                "? {}",
                event["title"].as_str().unwrap_or("approval needed")
            ));
            if let Some(detail) = event["detail"].as_str() {
                screen.meta(detail);
            }
            let listed: Vec<String> = choices
                .iter()
                .enumerate()
                .map(|(index, (_, label))| format!("{}) {label}", index + 1))
                .collect();
            screen.line(&format!("  {}", listed.join("  ")));
            *approval = event["approvalId"].as_str().map(|id| OpenApproval {
                id: id.to_string(),
                choices,
            });
        }
        "approval_resolved" => {
            let resolved_here = approval
                .as_ref()
                .is_some_and(|open| event["approvalId"].as_str() == Some(open.id.as_str()));
            if resolved_here {
                *approval = None;
            }
            screen.meta(&format!(
                "{} by {}",
                event["choice"].as_str().unwrap_or("answered"),
                event["by"].as_str().unwrap_or("another client")
            ));
        }
        "compaction_start" => screen.meta("compacting the context"),
        "retry" => screen.meta(&format!(
            "retrying ({}/{})",
            event["attempt"].as_u64().unwrap_or(0),
            event["max"].as_u64().unwrap_or(0)
        )),
        "event_lag" => screen.meta("some output was skipped; the transcript is complete"),
        _ => {}
    }
}

/// The last `count` user and assistant messages, shortened.
fn print_recent(screen: &mut Screen, messages: &[Value], count: usize) {
    let readable: Vec<(bool, String)> = messages
        .iter()
        .filter_map(|message| {
            let user = match message["role"].as_str()? {
                "user" => true,
                "assistant" => false,
                _ => return None,
            };
            let text = message_text(message);
            (!text.trim().is_empty()).then_some((user, text))
        })
        .collect();
    let skip = readable.len().saturating_sub(count);
    for (user, text) in &readable[skip..] {
        let text = shorten(text, 400);
        if *user {
            screen.line(&format!("› {text}"));
        } else {
            screen.line(&text);
        }
    }
}

/// The readable text of a stored message: a user's string or text blocks,
/// an assistant's text blocks.
fn message_text(message: &Value) -> String {
    let text = match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    if message["role"] == "user" {
        crate::modes::interactive::visible_user_text(&text)
    } else {
        text
    }
}

/// `read src/main.rs`, `bash cargo test`, or the name and its arguments.
fn tool_summary(name: &str, args: &Value) -> String {
    let detail = ["path", "file_path", "command", "pattern", "url"]
        .iter()
        .find_map(|key| args[*key].as_str())
        .map(|value| value.lines().next().unwrap_or_default().to_string())
        .unwrap_or_else(|| match args {
            Value::Null => String::new(),
            other => other.to_string(),
        });
    let detail = shorten(&detail, 100);
    if detail.is_empty() {
        name.to_string()
    } else {
        format!("{name} {detail}")
    }
}

fn pick_choice(choices: &[(String, String)], answer: &str) -> Option<String> {
    if let Ok(number) = answer.parse::<usize>() {
        return number
            .checked_sub(1)
            .and_then(|index| choices.get(index))
            .map(|(id, _)| id.clone());
    }
    choices
        .iter()
        .find(|(id, label)| id.eq_ignore_ascii_case(answer) || label.eq_ignore_ascii_case(answer))
        .map(|(id, _)| id.clone())
}

fn short_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}

fn shorten(text: &str, limit: usize) -> String {
    let flat = text.trim();
    if flat.chars().count() <= limit {
        return flat.to_string();
    }
    let cut: String = flat.chars().take(limit).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;
    use serde_json::json;
    use super_sdk::daemon::SessionState;

    fn args(argv: &[&str]) -> Args {
        Args::parse_from(std::iter::once("super").chain(argv.iter().copied()))
    }

    #[test]
    fn a_plain_signed_in_super_attaches() {
        let settings = Settings::default();
        assert!(wanted(&args(&[]), &settings, true));
        assert!(wanted(&args(&["fix", "the", "tests"]), &settings, true));
        assert!(wanted(&args(&["see", "@notes.md"]), &settings, true));
        assert!(wanted(&args(&["-c"]), &settings, true));
        assert!(wanted(&args(&["--model", "mock/mock-1"]), &settings, true));
    }

    #[test]
    fn signed_out_or_opted_out_runs_in_process() {
        let settings = Settings::default();
        assert!(!wanted(&args(&[]), &settings, false));
        assert!(!wanted(&args(&["--no-remote"]), &settings, true));
        assert!(!wanted(&args(&["--no-session"]), &settings, true));
        let off: Settings = serde_json::from_str(r#"{"remote":{"enabled":false}}"#).unwrap();
        assert!(!wanted(&args(&[]), &off, true));
        let no_attach: Settings = serde_json::from_str(r#"{"daemon":{"attach":false}}"#).unwrap();
        assert!(!wanted(&args(&[]), &no_attach, true));
    }

    #[test]
    fn other_modes_and_engine_options_run_in_process() {
        let settings = Settings::default();
        for argv in [
            &["-p", "hi"][..],
            &["--mode", "rpc"],
            &["daemon", "status"],
            &["--tools", "read"],
            &["--skill", "x"],
            &["--approve"],
            &["--resume"],
            &["--fork", "abc"],
            &["--session-dir", "/tmp/x"],
            &["-c", "--model", "mock/mock-1"],
        ] {
            assert!(!wanted(&args(argv), &settings, true), "{argv:?}");
        }
    }

    fn summary(id: &str, hosted: bool) -> SessionSummary {
        SessionSummary {
            session_id: id.into(),
            cwd: "/w".into(),
            workspace: "/w".into(),
            session_file: format!("/s/{id}.jsonl"),
            name: None,
            first_message: None,
            state: if hosted {
                SessionState::Idle
            } else {
                SessionState::Ended
            },
            blocked_reason: None,
            hosted,
            attached_clients: 0,
            model: None,
            thinking_level: None,
            modified_at: "2026-10-30T00:00:00Z".into(),
        }
    }

    #[test]
    fn super_joins_the_running_session_and_continue_takes_the_newest() {
        let newest_stored_then_hosted = [summary("stored01", false), summary("hosted01", true)];
        assert_eq!(
            existing_session(&args(&[]), &newest_stored_then_hosted).as_deref(),
            Some("hosted01")
        );
        assert_eq!(
            existing_session(&args(&["-c"]), &newest_stored_then_hosted).as_deref(),
            Some("stored01")
        );
        assert_eq!(
            existing_session(&args(&[]), &[summary("stored01", false)]),
            None
        );
        assert_eq!(
            existing_session(&args(&["--name", "new"]), &newest_stored_then_hosted),
            None,
            "options for a new session create one"
        );
        assert_eq!(existing_session(&args(&["-c"]), &[]), None);
    }

    #[test]
    fn approvals_are_answered_by_number_or_name() {
        let choices = vec![
            ("approve".to_string(), "Run it".to_string()),
            ("cancel".to_string(), "Cancel".to_string()),
        ];
        assert_eq!(pick_choice(&choices, "1").as_deref(), Some("approve"));
        assert_eq!(pick_choice(&choices, "cancel").as_deref(), Some("cancel"));
        assert_eq!(pick_choice(&choices, "run it").as_deref(), Some("approve"));
        assert_eq!(pick_choice(&choices, "3"), None);
        assert_eq!(pick_choice(&choices, "0"), None);
    }

    #[test]
    fn tools_and_messages_read_short() {
        assert_eq!(
            tool_summary("bash", &json!({"command": "cargo test\necho done"})),
            "bash cargo test"
        );
        assert_eq!(
            tool_summary("read", &json!({"path": "src/main.rs"})),
            "read src/main.rs"
        );
        assert_eq!(tool_summary("todo", &Value::Null), "todo");
        assert_eq!(
            message_text(&json!({"role": "assistant", "content": [
                {"type": "thinking", "thinking": "hmm"},
                {"type": "text", "text": "Done."}
            ]})),
            "Done."
        );
        assert_eq!(
            message_text(&json!({"role": "user", "content": "hi"})),
            "hi"
        );
        assert_eq!(shorten("abcdef", 3), "abc…");
        assert_eq!(short_id("a1b2c3d4-e5f6"), "a1b2c3d4");
    }
}
