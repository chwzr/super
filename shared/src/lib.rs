use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Auth types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizeRequest {
    pub code: String,
    pub code_verifier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: Uuid,
    pub email: String,
    pub openrouter_api_key: String,
    pub created_at: DateTime<Utc>,
}

// CLI config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub api_base_url: String,
    pub model: String,
    #[serde(default)]
    pub permissions: serde_json::Value,
    #[serde(default)]
    pub settings: serde_json::Value,
    #[serde(default = "default_messages_base_url")]
    pub api_messages_base_url: String,
}

fn default_messages_base_url() -> String {
    "https://openrouter.ai/api".to_string()
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            access_token: None,
            refresh_token: None,
            openrouter_api_key: None,
            api_base_url: "http://localhost:3000".to_string(),
            model: "anthropic/claude-sonnet-4-6".to_string(),
            permissions: serde_json::json!({}),
            settings: serde_json::json!({}),
            api_messages_base_url: default_messages_base_url(),
        }
    }
}

// Error type shared across server
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("user already exists")]
    UserAlreadyExists,
    #[error("user not found")]
    UserNotFound,
    #[error("invalid token")]
    InvalidToken,
    #[error("token expired")]
    TokenExpired,
    #[error("internal error: {0}")]
    Internal(String),
}
