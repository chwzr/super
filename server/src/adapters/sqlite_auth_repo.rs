use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection};
use shared::AuthError;
use uuid::Uuid;

use crate::domain::auth::ports::{ApiKey, AuthRepository, AuthorizationCode, RefreshToken, User};

pub struct SqliteAuthRepo {
    conn: std::sync::Mutex<Connection>,
}

impl SqliteAuthRepo {
    pub fn new(path: &str) -> Result<Self, AuthError> {
        let conn = Connection::open(path).map_err(|e| AuthError::Internal(e.to_string()))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                email TEXT UNIQUE NOT NULL,
                password_hash TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS api_keys (
                user_id TEXT NOT NULL REFERENCES users(id),
                openrouter_key_id TEXT NOT NULL,
                openrouter_key_value TEXT NOT NULL,
                created_at TEXT NOT NULL,
                revoked_at TEXT
            );
            CREATE TABLE IF NOT EXISTS auth_codes (
                code TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id),
                code_challenge TEXT NOT NULL,
                expires_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS refresh_tokens (
                token_hash TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id),
                expires_at TEXT NOT NULL
            );",
        )
        .map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
        })
    }
}

#[async_trait]
impl AuthRepository for SqliteAuthRepo {
    async fn create_user(&self, email: &str, password_hash: &str) -> Result<User, AuthError> {
        let id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO users (id, email, password_hash, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id.to_string(), email, password_hash, now],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                AuthError::UserAlreadyExists
            } else {
                AuthError::Internal(e.to_string())
            }
        })?;
        Ok(User {
            id,
            email: email.to_string(),
            password_hash: password_hash.to_string(),
            created_at: Utc::now(),
        })
    }

    async fn find_user_by_email(&self, email: &str) -> Result<Option<User>, AuthError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, email, password_hash, created_at FROM users WHERE email = ?1")
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let mut rows = stmt
            .query_map(params![email], |row| {
                Ok(User {
                    id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                    email: row.get(1)?,
                    password_hash: row.get(2)?,
                    created_at: chrono::DateTime::parse_from_rfc3339(
                        &row.get::<_, String>(3)?,
                    )
                    .unwrap()
                    .with_timezone(&Utc),
                })
            })
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(rows.next().transpose().map_err(|e| AuthError::Internal(e.to_string()))?)
    }

    async fn find_user_by_id(&self, id: &Uuid) -> Result<Option<User>, AuthError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, email, password_hash, created_at FROM users WHERE id = ?1")
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let mut rows = stmt
            .query_map(params![id.to_string()], |row| {
                Ok(User {
                    id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                    email: row.get(1)?,
                    password_hash: row.get(2)?,
                    created_at: chrono::DateTime::parse_from_rfc3339(
                        &row.get::<_, String>(3)?,
                    )
                    .unwrap()
                    .with_timezone(&Utc),
                })
            })
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(rows.next().transpose().map_err(|e| AuthError::Internal(e.to_string()))?)
    }

    async fn store_api_key(
        &self,
        user_id: &Uuid,
        key_id: &str,
        key_value: &str,
    ) -> Result<(), AuthError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO api_keys (user_id, openrouter_key_id, openrouter_key_value, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![user_id.to_string(), key_id, key_value, now],
        )
        .map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn get_active_api_key(&self, user_id: &Uuid) -> Result<Option<ApiKey>, AuthError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT openrouter_key_id, openrouter_key_value, created_at, revoked_at FROM api_keys WHERE user_id = ?1 AND revoked_at IS NULL ORDER BY created_at DESC LIMIT 1")
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let mut rows = stmt
            .query_map(params![user_id.to_string()], |row| {
                Ok(ApiKey {
                    openrouter_key_id: row.get(0)?,
                    openrouter_key_value: row.get(1)?,
                    _created_at: chrono::DateTime::parse_from_rfc3339(
                        &row.get::<_, String>(2)?,
                    )
                    .unwrap()
                    .with_timezone(&Utc),
                    _revoked_at: row.get::<_, Option<String>>(3)?.map(|s| {
                        chrono::DateTime::parse_from_rfc3339(&s)
                            .unwrap()
                            .with_timezone(&Utc)
                    }),
                })
            })
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(rows.next().transpose().map_err(|e| AuthError::Internal(e.to_string()))?)
    }

    async fn revoke_api_key(&self, user_id: &Uuid, key_id: &str) -> Result<(), AuthError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE api_keys SET revoked_at = ?1 WHERE user_id = ?2 AND openrouter_key_id = ?3",
            params![now, user_id.to_string(), key_id],
        )
        .map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn store_authorization_code(
        &self,
        code: &AuthorizationCode,
    ) -> Result<(), AuthError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO auth_codes (code, user_id, code_challenge, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![code.code, code.user_id.to_string(), code.code_challenge, code.expires_at.to_rfc3339()],
        )
        .map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn consume_authorization_code(
        &self,
        code: &str,
    ) -> Result<Option<AuthorizationCode>, AuthError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT code, user_id, code_challenge, expires_at FROM auth_codes WHERE code = ?1",
            )
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let result = stmt
            .query_map(params![code], |row| {
                Ok(AuthorizationCode {
                    code: row.get(0)?,
                    user_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap(),
                    code_challenge: row.get(2)?,
                    expires_at: chrono::DateTime::parse_from_rfc3339(
                        &row.get::<_, String>(3)?,
                    )
                    .unwrap()
                    .with_timezone(&Utc),
                })
            })
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let auth_code = result.last().transpose().map_err(|e| AuthError::Internal(e.to_string()))?;
        if auth_code.is_some() {
            conn.execute(
                "DELETE FROM auth_codes WHERE code = ?1",
                params![code],
            )
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        }
        Ok(auth_code)
    }

    async fn store_refresh_token(&self, token: &RefreshToken) -> Result<(), AuthError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO refresh_tokens (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
            params![token.token_hash, token.user_id.to_string(), token.expires_at.to_rfc3339()],
        )
        .map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn consume_refresh_token(
        &self,
        token_hash: &str,
    ) -> Result<Option<RefreshToken>, AuthError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT token_hash, user_id, expires_at FROM refresh_tokens WHERE token_hash = ?1",
            )
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let result = stmt
            .query_map(params![token_hash], |row| {
                Ok(RefreshToken {
                    token_hash: row.get(0)?,
                    user_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap(),
                    expires_at: chrono::DateTime::parse_from_rfc3339(
                        &row.get::<_, String>(2)?,
                    )
                    .unwrap()
                    .with_timezone(&Utc),
                })
            })
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let token = result.last().transpose().map_err(|e| AuthError::Internal(e.to_string()))?;
        if token.is_some() {
            conn.execute(
                "DELETE FROM refresh_tokens WHERE token_hash = ?1",
                params![token_hash],
            )
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        }
        Ok(token)
    }
}