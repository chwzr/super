//! The daemon protocol: what a client of `super daemon` says over the local
//! socket besides the session commands in [`crate::protocol`].
//!
//! Like [`crate::protocol`] this module is pure serde, so every SDK surface
//! and the WebAssembly build see one definition. The user-facing description
//! is `docs/daemon.md`.

use crate::protocol::{ProtocolError, Request, decode_request};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol versions this build speaks, as `[min, max]`.
///
/// Additive changes (new commands, new optional fields, new events) do not
/// bump the version. Renaming or removing a command, or changing what one
/// means, bumps `max`; `min` follows one release later.
pub const PROTOCOL_VERSIONS: [u32; 2] = [1, 1];

/// What a client sees of a session's life. `Ended` means "not loaded in the
/// daemon right now", never "gone": every ended session can be resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Idle,
    Working,
    Blocked,
    Ended,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Ended => "ended",
        }
    }
}

/// Which kind of program a client is. Shown in `super daemon status` and in
/// the log; `hello.name` is what other clients see as the origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Tui,
    Cli,
    Macos,
    Ios,
    Other,
}

impl ClientKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tui => "tui",
            Self::Cli => "cli",
            Self::Macos => "macos",
            Self::Ios => "ios",
            Self::Other => "other",
        }
    }
}

/// Why a hosted session was unloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Stopped,
    Idle,
    Shutdown,
}

/// One session as the daemon lists it: hosted or stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_id: String,
    pub cwd: String,
    /// The same as `cwd` until workspaces mean more than a directory.
    pub workspace: String,
    pub session_file: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub first_message: Option<String>,
    pub state: SessionState,
    #[serde(default)]
    pub blocked_reason: Option<String>,
    pub hosted: bool,
    pub attached_clients: usize,
    /// `provider/model`; null for a stored session.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking_level: Option<String>,
    /// RFC 3339, seconds, UTC.
    pub modified_at: String,
}

/// A directory with sessions, derived from the sessions' working directories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSummary {
    pub path: String,
    pub name: String,
    pub session_count: usize,
    pub latest_state: SessionState,
    pub latest_modified_at: String,
}

/// One entry of `list_directories`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
}

/// The daemon's answer to `hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloReply {
    pub protocol_version: u32,
    pub daemon_version: String,
    pub host_id: String,
    pub host_name: String,
    pub platform: String,
    pub pid: u32,
    pub started_at: String,
}

/// The daemon's answer to `status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    pub daemon_version: String,
    pub protocol_versions: [u32; 2],
    pub host_id: String,
    pub host_name: String,
    pub platform: String,
    pub pid: u32,
    pub started_at: String,
    pub uptime_seconds: u64,
    pub hosted_sessions: usize,
    pub stored_sessions: usize,
    pub clients: usize,
    pub socket_path: String,
    pub session_dir: String,
}

/// Every daemon-level command. Session commands are [`crate::protocol::Command`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonCommand {
    #[serde(rename_all = "camelCase")]
    Hello {
        client: ClientKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        version: String,
        protocol_versions: [u32; 2],
    },
    Status {},
    #[serde(rename_all = "camelCase")]
    ListSessions {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    ListWorkspaces {},
    ListDirectories {
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    CreateSession {
        cwd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking_level: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ResumeSession {
        session_id: String,
    },
    #[serde(rename_all = "camelCase")]
    Attach {
        session_id: String,
    },
    Detach {},
    #[serde(rename_all = "camelCase")]
    StopSession {
        session_id: String,
    },
    Shutdown {},
}

impl DaemonCommand {
    /// The snake_case name echoed as `command` on the response.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::Status {} => "status",
            Self::ListSessions { .. } => "list_sessions",
            Self::ListWorkspaces {} => "list_workspaces",
            Self::ListDirectories { .. } => "list_directories",
            Self::CreateSession { .. } => "create_session",
            Self::ResumeSession { .. } => "resume_session",
            Self::Attach { .. } => "attach",
            Self::Detach {} => "detach",
            Self::StopSession { .. } => "stop_session",
            Self::Shutdown {} => "shutdown",
        }
    }
}

/// One decoded daemon-level line: the command plus the client's optional id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(flatten)]
    pub command: DaemonCommand,
}

/// Events the daemon sends to every connection, attached or not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    SessionCreated {
        session: SessionSummary,
    },
    SessionUpdated {
        session: SessionSummary,
    },
    #[serde(rename_all = "camelCase")]
    SessionEnded {
        session_id: String,
        reason: EndReason,
    },
    DaemonShutdown {
        reason: String,
    },
}

impl DaemonEvent {
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::SessionCreated { .. } => "session_created",
            Self::SessionUpdated { .. } => "session_updated",
            Self::SessionEnded { .. } => "session_ended",
            Self::DaemonShutdown { .. } => "daemon_shutdown",
        }
    }

    /// One JSON line without the trailing newline.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// A line a client sent, sorted into the two command sets.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientLine {
    Daemon(DaemonRequest),
    Session(Request),
}

/// Decode one client line. Daemon-level types win; anything else goes to the
/// session decoder, whose error names an unknown type.
pub fn decode_client_line(line: &str) -> Result<ClientLine, ProtocolError> {
    let value: Value = serde_json::from_str(line)?;
    if value.get("type").and_then(Value::as_str).is_none() {
        return Err(ProtocolError::MissingType);
    }
    match serde_json::from_value::<DaemonRequest>(value) {
        Ok(request) => Ok(ClientLine::Daemon(request)),
        Err(error) if error.to_string().contains("unknown variant") => {
            decode_request(line).map(ClientLine::Session)
        }
        Err(error) => Err(ProtocolError::Json(error)),
    }
}

/// Pick the highest version both sides speak, or explain why there is none.
pub fn negotiate(client: [u32; 2], daemon: [u32; 2]) -> Result<u32, String> {
    let low = client[0].max(daemon[0]);
    let high = client[1].min(daemon[1]);
    if low <= high {
        Ok(high)
    } else {
        Err(format!(
            "this SUPER daemon speaks protocol {}; this client speaks {}",
            describe_range(daemon),
            describe_range(client)
        ))
    }
}

/// `1` or `2 to 3`.
pub fn describe_range(range: [u32; 2]) -> String {
    if range[0] == range[1] {
        range[0].to_string()
    } else {
        format!("{} to {}", range[0], range[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hello_round_trips_in_camel_case() {
        let line = r#"{"id":"1","type":"hello","client":"tui","name":"Terminal","version":"0.0.38","protocolVersions":[1,1]}"#;
        let ClientLine::Daemon(request) = decode_client_line(line).expect("decodes") else {
            panic!("hello is a daemon command");
        };
        assert_eq!(request.id.as_deref(), Some("1"));
        assert_eq!(
            request.command,
            DaemonCommand::Hello {
                client: ClientKind::Tui,
                name: Some("Terminal".into()),
                version: "0.0.38".into(),
                protocol_versions: [1, 1],
            }
        );
        let encoded = serde_json::to_value(&request.command).unwrap();
        assert_eq!(encoded["type"], "hello");
        assert_eq!(encoded["protocolVersions"], json!([1, 1]));
    }

    #[test]
    fn a_session_command_is_routed_to_the_session_decoder() {
        let line = r#"{"id":"7","type":"prompt","message":"hi"}"#;
        match decode_client_line(line).expect("decodes") {
            ClientLine::Session(request) => {
                assert_eq!(request.id.as_deref(), Some("7"));
                assert_eq!(request.command.name(), "prompt");
            }
            ClientLine::Daemon(_) => panic!("prompt is a session command"),
        }
    }

    #[test]
    fn an_unknown_type_names_itself() {
        let error = decode_client_line(r#"{"type":"teleport"}"#).expect_err("unknown");
        assert!(error.to_string().contains("teleport"), "{error}");
        assert!(matches!(
            decode_client_line(r#"{"id":"1"}"#),
            Err(ProtocolError::MissingType)
        ));
    }

    #[test]
    fn negotiation_picks_the_highest_shared_version() {
        assert_eq!(negotiate([1, 2], [1, 1]), Ok(1));
        assert_eq!(negotiate([1, 3], [2, 5]), Ok(3));
        assert_eq!(negotiate([1, 1], [1, 1]), Ok(1));
    }

    #[test]
    fn no_overlap_names_both_ranges() {
        let error = negotiate([1, 1], [2, 3]).expect_err("no overlap");
        assert_eq!(
            error,
            "this SUPER daemon speaks protocol 2 to 3; this client speaks 1"
        );
        assert_eq!(describe_range([4, 4]), "4");
    }

    #[test]
    fn events_carry_their_reason_and_summary() {
        let ended = DaemonEvent::SessionEnded {
            session_id: "a1b2".into(),
            reason: EndReason::Idle,
        };
        let value: Value = serde_json::from_str(&ended.to_line()).unwrap();
        assert_eq!(value["type"], "session_ended");
        assert_eq!(value["sessionId"], "a1b2");
        assert_eq!(value["reason"], "idle");
        assert_eq!(ended.event_type(), "session_ended");
        assert_eq!(SessionState::Blocked.as_str(), "blocked");
        assert_eq!(
            serde_json::to_value(SessionState::Ended).unwrap(),
            json!("ended")
        );
    }
}
