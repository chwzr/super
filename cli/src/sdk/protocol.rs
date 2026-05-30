use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Wire protocol message types matching Claude Code's SDK surface
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SdkMessage {
    #[serde(rename = "init")]
    Init {
        session_id: String,
        tools: Vec<String>,
    },
    #[serde(rename = "user")]
    User { content: String },
    #[serde(rename = "assistant")]
    Assistant { content: Vec<ContentBlock> },
    #[serde(rename = "tool_use")]
    ToolUse {
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        name: String,
        content: String,
        is_error: bool,
    },
    #[serde(rename = "result")]
    Result {
        success: bool,
        content: String,
        usage: UsageInfo,
    },
    #[serde(rename = "system")]
    System { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageInfo {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// Control protocol messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ControlMessage {
    #[serde(rename = "initialize")]
    Initialize { tools: Vec<String>, model: String },
    #[serde(rename = "interrupt")]
    Interrupt,
    #[serde(rename = "can_use_tool")]
    CanUseTool {
        tool_name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "set_permission_mode")]
    SetPermissionMode { mode: String },
    #[serde(rename = "set_model")]
    SetModel { model: String },
    #[serde(rename = "get_settings")]
    GetSettings,
    #[serde(rename = "apply_flag_settings")]
    ApplyFlagSettings { flags: serde_json::Value },
}

/// SDK session for programmatic use
pub struct SdkSession {
    pub session_id: String,
}

impl SdkSession {
    pub fn new() -> Self {
        Self {
            session_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub fn init_message(&self, tools: Vec<String>) -> SdkMessage {
        SdkMessage::Init {
            session_id: self.session_id.clone(),
            tools,
        }
    }

    pub fn format_output(&self, messages: &[SdkMessage]) -> String {
        serde_json::to_string_pretty(messages).unwrap_or_default()
    }
}

impl Default for SdkSession {
    fn default() -> Self {
        Self::new()
    }
}

/// Layer 1 — raw Anthropic-shaped stream events (verbatim wire format).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: MessageMeta },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: ContentBlockStream,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: BlockDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDeltaInfo,
        usage: AnthropicUsage,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    /// Anthropic also sends `ping` events for keep-alive. We accept them silently.
    #[serde(rename = "ping")]
    Ping,
}

/// Content blocks as they appear on the wire during streaming (text/thinking start
/// with empty strings; tool_use starts with empty input).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlockStream {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
}

/// Content blocks in finalized assistant/user messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlockFinal {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BlockDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageMeta {
    pub id: String,
    pub model: String,
    pub role: String,
    #[serde(default)]
    pub content: Vec<ContentBlockFinal>,
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
    pub usage: AnthropicUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDeltaInfo {
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
}

/// Anthropic-shaped usage struct with all fields optional/defaulted so partial
/// usage deltas parse cleanly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnthropicUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
}

/// Layer 2 — envelopes carried on the session bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BusMessage {
    #[serde(rename = "user")]
    User {
        message: UserPayload,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "assistant")]
    Assistant {
        message: AssistantPayload,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "stream_event")]
    StreamEvent {
        event: crate::sdk::protocol::StreamEvent,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "tool_progress")]
    ToolProgress {
        tool_use_id: String,
        tool_name: String,
        elapsed_seconds: f32,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "system")]
    SystemEvent {
        subtype: SystemSubtype,
        message: String,
        #[serde(default)]
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "result")]
    Result {
        stop_reason: Option<String>,
        usage: AnthropicUsage,
        total_cost_usd: f64,
        duration_ms: u64,
        num_turns: u32,
        #[serde(default)]
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "render_event")]
    RenderEvent {
        tool_use_id: String,
        slot: shared::RenderSlot,
        spec: shared::RenderSpec,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "interaction_requested")]
    InteractionRequested {
        tool_use_id: String,
        spec: shared::RenderSpec,
        response_schema: serde_json::Value,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "interaction_response")]
    InteractionResponse {
        tool_use_id: String,
        payload: serde_json::Value,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
    #[serde(rename = "interaction_denied")]
    InteractionDenied {
        tool_use_id: String,
        parent_tool_use_id: Option<String>,
        uuid: Uuid,
        session_id: String,
    },
}

impl BusMessage {
    pub fn session_id(&self) -> &str {
        match self {
            BusMessage::User { session_id, .. }
            | BusMessage::Assistant { session_id, .. }
            | BusMessage::StreamEvent { session_id, .. }
            | BusMessage::ToolProgress { session_id, .. }
            | BusMessage::SystemEvent { session_id, .. }
            | BusMessage::Result { session_id, .. }
            | BusMessage::RenderEvent { session_id, .. }
            | BusMessage::InteractionRequested { session_id, .. }
            | BusMessage::InteractionResponse { session_id, .. }
            | BusMessage::InteractionDenied { session_id, .. } => session_id.as_str(),
        }
    }

    pub fn parent_tool_use_id(&self) -> Option<&str> {
        match self {
            BusMessage::User {
                parent_tool_use_id, ..
            }
            | BusMessage::Assistant {
                parent_tool_use_id, ..
            }
            | BusMessage::StreamEvent {
                parent_tool_use_id, ..
            }
            | BusMessage::ToolProgress {
                parent_tool_use_id, ..
            }
            | BusMessage::SystemEvent {
                parent_tool_use_id, ..
            }
            | BusMessage::Result {
                parent_tool_use_id, ..
            }
            | BusMessage::RenderEvent {
                parent_tool_use_id, ..
            }
            | BusMessage::InteractionRequested {
                parent_tool_use_id, ..
            }
            | BusMessage::InteractionResponse {
                parent_tool_use_id, ..
            }
            | BusMessage::InteractionDenied {
                parent_tool_use_id, ..
            } => parent_tool_use_id.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPayload {
    pub role: String, // always "user"
    pub content: Vec<ContentBlockFinal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantPayload {
    pub id: String,
    pub model: String,
    pub role: String, // always "assistant"
    pub content: Vec<ContentBlockFinal>,
    pub stop_reason: Option<String>,
    pub usage: AnthropicUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemSubtype {
    CompactBoundary,
    PostTurnSummary,
    ApiRetry,
    PermissionRequest,
    Notice,
    AsyncAgentDone,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_event_text_delta_roundtrip() {
        let json =
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#;
        let parsed: StreamEvent = serde_json::from_str(json).unwrap();
        match parsed {
            StreamEvent::ContentBlockDelta {
                index,
                delta: BlockDelta::TextDelta { text },
            } => {
                assert_eq!(index, 0);
                assert_eq!(text, "hi");
            }
            _ => panic!("wrong variant: {parsed:?}"),
        }
    }

    #[test]
    fn message_start_parses() {
        let json = r#"{"type":"message_start","message":{"id":"msg_1","model":"anthropic/claude-sonnet-4-5","role":"assistant","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":0}}}"#;
        let parsed: StreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(parsed, StreamEvent::MessageStart { .. }));
    }

    #[test]
    fn tool_use_input_json_delta_parses() {
        let json = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\":\"PLAN"}}"#;
        let parsed: StreamEvent = serde_json::from_str(json).unwrap();
        match parsed {
            StreamEvent::ContentBlockDelta {
                delta: BlockDelta::InputJsonDelta { partial_json },
                ..
            } => {
                assert!(partial_json.starts_with("{\"file_path\""));
            }
            _ => panic!("wrong variant: {parsed:?}"),
        }
    }

    #[test]
    fn ping_event_parses() {
        let json = r#"{"type":"ping"}"#;
        let parsed: StreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(parsed, StreamEvent::Ping));
    }

    #[test]
    fn system_event_carries_parent_tool_use_id() {
        let msg = BusMessage::SystemEvent {
            subtype: SystemSubtype::AsyncAgentDone,
            message: "agent-1 finished: ok".into(),
            parent_tool_use_id: Some("tu_parent".into()),
            uuid: Uuid::new_v4(),
            session_id: "agent-1".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"parent_tool_use_id\":\"tu_parent\""));
        assert!(json.contains("\"subtype\":\"async_agent_done\""));
        let back: BusMessage = serde_json::from_str(&json).unwrap();
        match back {
            BusMessage::SystemEvent {
                subtype,
                parent_tool_use_id,
                ..
            } => {
                assert!(matches!(subtype, SystemSubtype::AsyncAgentDone));
                assert_eq!(parent_tool_use_id.as_deref(), Some("tu_parent"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn bus_message_accessors() {
        let msg = BusMessage::Result {
            stop_reason: None,
            usage: AnthropicUsage::default(),
            total_cost_usd: 0.0,
            duration_ms: 0,
            num_turns: 1,
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s-root".into(),
        };
        assert_eq!(msg.session_id(), "s-root");
        assert_eq!(msg.parent_tool_use_id(), None);

        let msg = BusMessage::SystemEvent {
            subtype: SystemSubtype::Notice,
            message: "x".into(),
            parent_tool_use_id: Some("tu_p".into()),
            uuid: Uuid::new_v4(),
            session_id: "s-child".into(),
        };
        assert_eq!(msg.session_id(), "s-child");
        assert_eq!(msg.parent_tool_use_id(), Some("tu_p"));
    }

    #[test]
    fn result_message_carries_parent_tool_use_id() {
        let msg = BusMessage::Result {
            stop_reason: Some("end_turn".into()),
            usage: AnthropicUsage::default(),
            total_cost_usd: 0.01,
            duration_ms: 1234,
            num_turns: 3,
            parent_tool_use_id: Some("tu_parent".into()),
            uuid: Uuid::new_v4(),
            session_id: "agent-1".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"parent_tool_use_id\":\"tu_parent\""));
        let back: BusMessage = serde_json::from_str(&json).unwrap();
        match back {
            BusMessage::Result {
                parent_tool_use_id,
                num_turns,
                ..
            } => {
                assert_eq!(parent_tool_use_id.as_deref(), Some("tu_parent"));
                assert_eq!(num_turns, 3);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn result_message_accessor_returns_parent_tool_use_id() {
        let msg = BusMessage::Result {
            stop_reason: None,
            usage: AnthropicUsage::default(),
            total_cost_usd: 0.0,
            duration_ms: 0,
            num_turns: 1,
            parent_tool_use_id: Some("tu_x".into()),
            uuid: Uuid::new_v4(),
            session_id: "child".into(),
        };
        assert_eq!(msg.parent_tool_use_id(), Some("tu_x"));
        assert_eq!(msg.session_id(), "child");
    }

    #[test]
    fn bus_message_assistant_roundtrip() {
        let msg = BusMessage::Assistant {
            message: AssistantPayload {
                id: "m1".into(),
                model: "anthropic/claude-sonnet-4-5".into(),
                role: "assistant".into(),
                content: vec![ContentBlockFinal::Text { text: "hi".into() }],
                stop_reason: Some("end_turn".into()),
                usage: AnthropicUsage::default(),
            },
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: BusMessage = serde_json::from_str(&json).unwrap();
        match back {
            BusMessage::Assistant {
                message,
                session_id,
                ..
            } => {
                assert_eq!(session_id, "s1");
                assert_eq!(message.id, "m1");
                assert_eq!(message.content.len(), 1);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}

#[cfg(test)]
mod render_event_tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn render_event_serializes_with_type_tag() {
        let msg = BusMessage::RenderEvent {
            tool_use_id: "tu_1".into(),
            slot: shared::RenderSlot::Message,
            spec: shared::RenderSpec::Nothing,
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"render_event""#));
        assert!(json.contains(r#""tool_use_id":"tu_1""#));
        assert!(json.contains(r#""kind":"nothing""#));
    }

    #[test]
    fn render_event_round_trips() {
        let msg = BusMessage::RenderEvent {
            tool_use_id: "tu_2".into(),
            slot: shared::RenderSlot::Message,
            spec: shared::RenderSpec::Status {
                state: shared::StatusState::Success,
                message: Some("done".into()),
            },
            parent_tool_use_id: None,
            uuid: Uuid::new_v4(),
            session_id: "s1".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: BusMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, BusMessage::RenderEvent { .. }));
    }

    #[test]
    fn render_event_carries_slot() {
        let msg = BusMessage::RenderEvent {
            tool_use_id: "tu_1".into(),
            slot: shared::RenderSlot::Result,
            spec: shared::RenderSpec::Nothing,
            parent_tool_use_id: None,
            uuid: uuid::Uuid::new_v4(),
            session_id: "s".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""slot":"result""#), "got: {json}");
        let back: BusMessage = serde_json::from_str(&json).unwrap();
        match back {
            BusMessage::RenderEvent { slot, .. } => {
                assert_eq!(slot, shared::RenderSlot::Result);
            }
            _ => panic!("wrong variant"),
        }
    }
}
