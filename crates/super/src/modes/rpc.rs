//! RPC mode: expose the agent over newline-delimited JSON so any language can
//! drive it.
//!
//! Two transports are available. Without `--rpc-listen` the process reads
//! commands from its standard input and writes responses and events to its
//! standard output, which is what a parent process spawning `super` wants.
//! With `--rpc-listen <ADDR>` it instead accepts WebSocket connections on that
//! address, which is what a browser page wants.
//!
//! The protocol itself is documented in `docs/rpc.md` and defined once in
//! `crates/super-sdk/src/protocol.rs`.

use crate::args::Args;
use anyhow::Result;
use std::path::PathBuf;
use super_sdk::options::{SessionOptions, SessionSource};

/// Translate command-line arguments into SDK session options.
pub fn options_from_args(args: &Args) -> Result<SessionOptions> {
    let cwd = std::env::current_dir()?;
    let session = if args.no_session {
        SessionSource::InMemory
    } else if let Some(reference) = &args.session {
        SessionSource::Open(PathBuf::from(reference))
    } else if let Some(reference) = &args.fork {
        SessionSource::Fork(PathBuf::from(reference))
    } else if args.continue_recent {
        SessionSource::ContinueRecent
    } else {
        SessionSource::Create
    };

    Ok(SessionOptions {
        cwd,
        model: args.model.clone(),
        provider: args.provider.clone(),
        thinking_level: args
            .thinking
            .as_deref()
            .and_then(super_ai::ThinkingLevel::parse),
        api_key: args.api_key.clone(),
        models_file: std::env::var_os("SUPER_MODELS_FILE").map(PathBuf::from),
        tools: args.tools.as_ref().map(|_| Args::split_csv(&args.tools)),
        exclude_tools: Args::split_csv(&args.exclude_tools),
        no_tools: args.no_tools,
        system_prompt: args.system_prompt.clone(),
        append_system_prompt: args.append_system_prompt.clone(),
        session,
        session_dir: args.session_dir.clone().map(PathBuf::from),
        session_name: args.name.clone(),
        trust_project_files: args.approve && !args.no_approve,
        no_context_files: args.no_context_files,
        ..Default::default()
    })
}

pub async fn run(args: &Args) -> Result<i32> {
    let session = super_sdk::Session::create(options_from_args(args)?).await?;

    // A signed-in headless session is mirrored like a terminal one, so a
    // desktop app can drive it locally while phones follow along. Only then
    // do workflow approvals wait for an answer: a plain RPC client keeps the
    // auto-approve behaviour it always had.
    let settings = session.agent_session().settings();
    let remote = if crate::remote::wanted(args, &settings) {
        install_approval_gate(&session);
        let (events, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let crate::remote::RemoteEvent::Failed(reason) = event {
                    eprintln!("super remote: unavailable: {reason}");
                }
            }
        });
        let session_file = session.state().session_file;
        let control =
            crate::remote::RemoteControl::start(session.clone(), session_file, &settings, events);
        let record = crate::remote::session_record(
            super_remote::SessionState::Idle,
            None,
            session.agent_session(),
            args,
            true,
        );
        control.publish(record);
        Some((control, session.clone()))
    } else {
        None
    };
    let presence = remote.as_ref().map(|(control, sdk)| {
        // Keep the registry current while the stdio client drives the session.
        let args = args.clone();
        let sdk = sdk.clone();
        let publish = control_publisher(control);
        tokio::spawn(async move {
            let mut events = sdk.events();
            loop {
                let Some(event) = events.recv().await else {
                    break;
                };
                let state = match event.0["type"].as_str() {
                    Some("agent_start") => super_remote::SessionState::Working,
                    Some("agent_settled") | Some("agent_end") => super_remote::SessionState::Idle,
                    _ => continue,
                };
                publish(crate::remote::session_record(
                    state,
                    None,
                    sdk.agent_session(),
                    &args,
                    true,
                ));
            }
        })
    });

    match &args.rpc_listen {
        Some(address) => {
            let address: std::net::SocketAddr = address.parse().map_err(|error| {
                anyhow::anyhow!("invalid --rpc-listen address '{address}': {error}")
            })?;
            super_sdk::rpc::serve_websocket(session, address).await?;
        }
        None => super_sdk::rpc::serve_stdio(session).await?,
    }
    if let Some(task) = presence {
        task.abort();
    }
    if let Some((control, _)) = remote {
        control.stop().await;
    }
    Ok(0)
}

/// A publisher that outlives the borrow of the control.
fn control_publisher(
    control: &crate::remote::RemoteControl,
) -> impl Fn(super_remote::SessionRecord) + Send + 'static {
    let publish = control.publisher();
    move |record| publish(record)
}

/// Workflows ask for approval; without a terminal the question goes to the
/// RPC and remote clients as an `approval_request` event.
fn install_approval_gate(session: &std::sync::Arc<super_sdk::Session>) {
    let sdk = session.clone();
    session
        .agent_session()
        .set_workflow_approver(std::sync::Arc::new(move |plan| {
            let sdk = sdk.clone();
            Box::pin(async move {
                let (_, answer) = sdk.open_approval(super_sdk::ApprovalRequest {
                    kind: super_sdk::ApprovalKind::Workflow,
                    title: format!("Run workflow {}?", plan.name),
                    detail: Some(plan.agent_estimate()),
                    choices: vec![
                        super_sdk::ApprovalChoice::new("approve", "Run it"),
                        super_sdk::ApprovalChoice::new("cancel", "Cancel"),
                    ],
                });
                match answer.await {
                    Ok(answer) if answer.choice == "approve" => {
                        super_coding::workflows::ApprovalDecision::Approve
                    }
                    _ => super_coding::workflows::ApprovalDecision::Cancel,
                }
            })
        }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn no_session_means_nothing_is_written_to_disk() {
        let args = Args::parse_from(["super", "--mode", "rpc", "--no-session"]);
        let options = options_from_args(&args).expect("options");
        assert_eq!(options.session, SessionSource::InMemory);
    }

    #[test]
    fn a_session_is_persisted_by_default() {
        let args = Args::parse_from(["super", "--mode", "rpc"]);
        let options = options_from_args(&args).expect("options");
        assert_eq!(options.session, SessionSource::Create);
    }

    #[test]
    fn tool_flags_are_carried_across() {
        let args = Args::parse_from([
            "super",
            "--mode",
            "rpc",
            "--tools",
            "read,bash",
            "--exclude-tools",
            "bash",
        ]);
        let options = options_from_args(&args).expect("options");
        assert_eq!(
            options.tools.as_deref(),
            Some(&["read".to_string(), "bash".to_string()][..])
        );
        assert_eq!(options.exclude_tools, ["bash"]);
    }
}
