use async_trait::async_trait;
use chrono::{DateTime, Utc};
use shared::AuthError;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ApiKey {
    pub openrouter_key_id: String,
    pub openrouter_key_value: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

pub struct AuthorizationCode {
    pub code: String,
    pub user_id: Uuid,
    pub code_challenge: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct OpenRouterKey {
    pub id: String,
    pub key: String,
    pub label: String,
    pub limit_usd: u32,
}

pub struct RefreshToken {
    pub token_hash: String,
    pub user_id: Uuid,
    pub expires_at: DateTime<Utc>,
}

#[async_trait]
pub trait AuthRepository: Send + Sync {
    async fn create_user(&self, email: &str, password_hash: &str) -> Result<User, AuthError>;
    async fn find_user_by_email(&self, email: &str) -> Result<Option<User>, AuthError>;
    async fn find_user_by_id(&self, id: &Uuid) -> Result<Option<User>, AuthError>;
    async fn store_api_key(&self, user_id: &Uuid, key_id: &str, key_value: &str) -> Result<(), AuthError>;
    async fn get_active_api_key(&self, user_id: &Uuid) -> Result<Option<ApiKey>, AuthError>;
    async fn revoke_api_key(&self, user_id: &Uuid, key_id: &str) -> Result<(), AuthError>;
    async fn store_authorization_code(&self, code: &AuthorizationCode) -> Result<(), AuthError>;
    async fn consume_authorization_code(&self, code: &str) -> Result<Option<AuthorizationCode>, AuthError>;
    async fn store_refresh_token(&self, token: &RefreshToken) -> Result<(), AuthError>;
    async fn consume_refresh_token(&self, token_hash: &str) -> Result<Option<RefreshToken>, AuthError>;
}

#[async_trait]
pub trait OpenRouterProvider: Send + Sync {
    async fn create_key(&self, label: &str, limit_usd: u32) -> Result<OpenRouterKey, AuthError>;
    async fn revoke_key(&self, key_id: &str) -> Result<(), AuthError>;
    /// Fetch usage for a user-level key. Returns (used_usd, limit_usd).
    async fn fetch_key_usage(&self, user_key: &str) -> Result<(f64, f64), AuthError>;
}
