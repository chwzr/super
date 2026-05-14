use serde::{Deserialize, Serialize};

/// Wire protocol message types matching Claude Code's SDK surface
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SdkMessage {
    #[serde(rename = "init")]
    Init { session_id: String, tools: Vec<String> },
    #[serde(rename = "user")]
    User { content: String },
    #[serde(rename = "assistant")]
    Assistant { content: Vec<ContentBlock> },
    #[serde(rename = "tool_use")]
    ToolUse { name: String, input: serde_json::Value },
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
    Initialize {
        tools: Vec<String>,
        model: String,
    },
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