use std::sync::Arc;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use shared::{AuthError, RegisterRequest, TokenResponse, UserProfile};
use uuid::Uuid;

use crate::domain::auth::ports::{AuthRepository, AuthorizationCode, OpenRouterProvider, RefreshToken};

const JWT_SECRET: &str = "CHANGE_ME_IN_PRODUCTION_USE_ENV_VAR";
const DEFAULT_KEY_LIMIT_USD: u32 = 20;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
    iat: usize,
}

pub struct AuthService {
    repo: Arc<dyn AuthRepository>,
    openrouter: Arc<dyn OpenRouterProvider>,
}

impl AuthService {
    pub fn new(
        repo: Arc<dyn AuthRepository>,
        openrouter: Arc<dyn OpenRouterProvider>,
    ) -> Self {
        Self { repo, openrouter }
    }

    fn hash_password(password: &str) -> Result<String, AuthError> {
        bcrypt::hash(password, bcrypt::DEFAULT_COST)
            .map_err(|e| AuthError::Internal(e.to_string()))
    }

    fn verify_password(password: &str, hash: &str) -> Result<bool, AuthError> {
        bcrypt::verify(password, hash).map_err(|e| AuthError::Internal(e.to_string()))
    }

    fn generate_code() -> String {
        let bytes: [u8; 32] = rand::thread_rng().gen();
        URL_SAFE_NO_PAD.encode(bytes)
    }

    fn generate_token() -> String {
        let mut bytes = vec![0u8; 48];
        rand::thread_rng().fill(bytes.as_mut_slice());
        URL_SAFE_NO_PAD.encode(bytes)
    }

    fn generate_jwt(&self, user_id: &Uuid) -> Result<String, AuthError> {
        let now = Utc::now();
        let claims = Claims {
            sub: user_id.to_string(),
            iat: now.timestamp() as usize,
            exp: (now + Duration::hours(1)).timestamp() as usize,
        };
        jsonwebtoken::encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(JWT_SECRET.as_bytes()),
        )
        .map_err(|e| AuthError::Internal(e.to_string()))
    }

    fn verify_jwt(&self, token: &str) -> Result<Uuid, AuthError> {
        let data = jsonwebtoken::decode::<Claims>(
            token,
            &DecodingKey::from_secret(JWT_SECRET.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| AuthError::InvalidToken)?;
        Uuid::parse_str(&data.claims.sub).map_err(|_| AuthError::InvalidToken)
    }

    pub async fn register(&self, req: &RegisterRequest) -> Result<UserProfile, AuthError> {
        let password_hash = Self::hash_password(&req.password)?;
        let user = self
            .repo
            .create_user(&req.email, &password_hash)
            .await?;

        // Provision OpenRouter key at registration time
        let label = format!("super-user-{}", user.id);
        let key = self
            .openrouter
            .create_key(&label, DEFAULT_KEY_LIMIT_USD)
            .await?;
        self.repo
            .store_api_key(&user.id, &key.id, &key.key)
            .await?;

        Ok(UserProfile {
            id: user.id,
            email: user.email,
            openrouter_api_key: key.key,
            created_at: user.created_at,
        })
    }

    pub async fn login(
        &self,
        email: &str,
        password: &str,
        code_challenge: &str,
    ) -> Result<String, AuthError> {
        let user = self
            .repo
            .find_user_by_email(email)
            .await?
            .ok_or(AuthError::InvalidCredentials)?;

        if !Self::verify_password(password, &user.password_hash)? {
            return Err(AuthError::InvalidCredentials);
        }

        let code = Self::generate_code();
        let auth_code = AuthorizationCode {
            code: code.clone(),
            user_id: user.id,
            code_challenge: code_challenge.to_string(),
            expires_at: Utc::now() + Duration::minutes(5),
        };
        self.repo.store_authorization_code(&auth_code).await?;
        Ok(code)
    }

    pub async fn authorize(
        &self,
        code: &str,
        code_verifier: &str,
    ) -> Result<TokenResponse, AuthError> {
        let auth_code = self
            .repo
            .consume_authorization_code(code)
            .await?
            .ok_or(AuthError::InvalidToken)?;

        if auth_code.expires_at < Utc::now() {
            return Err(AuthError::TokenExpired);
        }

        // Verify PKCE: SHA256(code_verifier) must match code_challenge
        let mut hasher = Sha256::new();
        hasher.update(code_verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());
        if challenge != auth_code.code_challenge {
            return Err(AuthError::InvalidToken);
        }

        let access_token = self.generate_jwt(&auth_code.user_id)?;
        let refresh_token_value = Self::generate_token();

        let mut hasher = Sha256::new();
        hasher.update(refresh_token_value.as_bytes());
        let token_hash = URL_SAFE_NO_PAD.encode(hasher.finalize());

        self.repo
            .store_refresh_token(&RefreshToken {
                token_hash: token_hash.clone(),
                user_id: auth_code.user_id,
                expires_at: Utc::now() + Duration::days(30),
            })
            .await?;

        Ok(TokenResponse {
            access_token,
            refresh_token: refresh_token_value,
            expires_in: 3600,
        })
    }

    pub async fn refresh(
        &self,
        refresh_token: &str,
    ) -> Result<TokenResponse, AuthError> {
        let mut hasher = Sha256::new();
        hasher.update(refresh_token.as_bytes());
        let token_hash = URL_SAFE_NO_PAD.encode(hasher.finalize());

        let token = self
            .repo
            .consume_refresh_token(&token_hash)
            .await?
            .ok_or(AuthError::InvalidToken)?;

        if token.expires_at < Utc::now() {
            return Err(AuthError::TokenExpired);
        }

        let access_token = self.generate_jwt(&token.user_id)?;
        let new_refresh = Self::generate_token();

        let mut hasher = Sha256::new();
        hasher.update(new_refresh.as_bytes());
        let new_hash = URL_SAFE_NO_PAD.encode(hasher.finalize());

        self.repo
            .store_refresh_token(&RefreshToken {
                token_hash: new_hash,
                user_id: token.user_id,
                expires_at: Utc::now() + Duration::days(30),
            })
            .await?;

        Ok(TokenResponse {
            access_token,
            refresh_token: new_refresh,
            expires_in: 3600,
        })
    }

    pub async fn get_profile(&self, access_token: &str) -> Result<UserProfile, AuthError> {
        let user_id = self.verify_jwt(access_token)?;
        let user = self
            .repo
            .find_user_by_id(&user_id)
            .await?
            .ok_or(AuthError::UserNotFound)?;
        let api_key = self
            .repo
            .get_active_api_key(&user_id)
            .await?
            .ok_or(AuthError::Internal("no API key provisioned".into()))?;
        Ok(UserProfile {
            id: user.id,
            email: user.email,
            openrouter_api_key: api_key.openrouter_key_value,
            created_at: user.created_at,
        })
    }

    pub async fn rotate_key(&self, access_token: &str) -> Result<UserProfile, AuthError> {
        let user_id = self.verify_jwt(access_token)?;
        let user = self
            .repo
            .find_user_by_id(&user_id)
            .await?
            .ok_or(AuthError::UserNotFound)?;

        // Revoke old key
        if let Some(old_key) = self.repo.get_active_api_key(&user_id).await? {
            self.openrouter
                .revoke_key(&old_key.openrouter_key_id)
                .await?;
            self.repo
                .revoke_api_key(&user_id, &old_key.openrouter_key_id)
                .await?;
        }

        // Create new key
        let label = format!("super-user-{}", user_id);
        let key = self
            .openrouter
            .create_key(&label, DEFAULT_KEY_LIMIT_USD)
            .await?;
        self.repo
            .store_api_key(&user_id, &key.id, &key.key)
            .await?;

        Ok(UserProfile {
            id: user.id,
            email: user.email,
            openrouter_api_key: key.key,
            created_at: user.created_at,
        })
    }
}
