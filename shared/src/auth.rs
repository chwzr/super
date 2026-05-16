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
fn default_provider() -> String { "anthropic".to_string() }
fn default_model_class() -> String { "sonnet".to_string() }
fn default_messages_base_url() -> String {
    "https://openrouter.ai/api".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub api_base_url: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_model_class")]
    pub model_class: String,
    #[serde(skip)]
    pub model: String,
    #[serde(default)]
    pub permissions: serde_json::Value,
    #[serde(default)]
    pub settings: serde_json::Value,
    #[serde(default = "default_messages_base_url")]
    pub api_messages_base_url: String,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            access_token: None,
            refresh_token: None,
            openrouter_api_key: None,
            api_base_url: "http://localhost:3000".to_string(),
            provider: default_provider(),
            model_class: default_model_class(),
            model: String::new(), // populated by load_config, not serialized
            permissions: serde_json::json!({}),
            settings: serde_json::json!({}),
            api_messages_base_url: default_messages_base_url(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let c = CliConfig::default();
        assert_eq!(c.provider, "anthropic");
        assert_eq!(c.model_class, "sonnet");
        assert_eq!(c.model, ""); // populated by load_config, not default()
    }

    #[test]
    fn config_round_trip_persists_provider_and_class() {
        let c = CliConfig {
            provider: "z-ai".to_string(),
            model_class: "haiku".to_string(),
            model: "z-ai/glm-4.7-flash".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        // model field must NOT appear in the JSON output
        assert!(!json.contains("\"model\":"), "model field must not be serialized; json was: {json}");
        assert!(json.contains("\"provider\":\"z-ai\""));
        assert!(json.contains("\"model_class\":\"haiku\""));
        // Deserialize back
        let c2: CliConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.provider, "z-ai");
        assert_eq!(c2.model_class, "haiku");
        assert_eq!(c2.model, ""); // skip field not restored from JSON
    }

    #[test]
    fn config_missing_provider_defaults_to_anthropic() {
        let json = r#"{"api_base_url":"http://localhost:3000","api_messages_base_url":"https://openrouter.ai/api"}"#;
        let c: CliConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.provider, "anthropic");
        assert_eq!(c.model_class, "sonnet");
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
