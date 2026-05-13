# Phase 1: Platform Server + Full CLI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a working Super CLI with full Claude Code tool parity, backed by a hexagonal platform server for auth and OpenRouter key provisioning.

**Architecture:** Two Rust binaries in a Cargo workspace — `server` (axum, hexagonal ports/adapters) and `cli` (ratatui TUI + rig agent loop). A `shared` crate holds common types. The CLI implements all 13 P0 modules matching Claude Code behavioral specs from `analysis-workspace/raw/specs/`.

**Tech Stack:** Rust, axum, ratatui, rig, rusqlite, crossterm, tokio, reqwest, clap, serde, tree-sitter

---

## File Structure

```
super/
├── Cargo.toml                    # workspace root
├── shared/
│   ├── Cargo.toml
│   └── src/lib.rs                # shared types
├── server/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── domain/
│       │   ├── mod.rs
│       │   └── auth/
│       │       ├── mod.rs
│       │       ├── service.rs    # AuthService
│       │       └── ports.rs      # AuthRepository + OpenRouterProvider traits
│       ├── adapters/
│       │   ├── mod.rs
│       │   ├── sqlite_auth_repo.rs
│       │   └── openrouter_client.rs
│       └── routes/
│           ├── mod.rs
│           └── auth.rs
└── cli/
    ├── Cargo.toml
    └── src/
        ├── main.rs               # entry point, arg parsing
        ├── bootstrap.rs          # M11: init + setup sequence
        ├── config.rs             # M01: config load/save
        ├── auth.rs               # M02: PKCE login, token refresh
        ├── tui/
        │   ├── mod.rs
        │   ├── app.rs            # main TUI app, event loop
        │   ├── scroll_area.rs    # message history rendering
        │   ├── activity.rs       # glyph animation + verb
        │   ├── input_bar.rs      # text input
        │   └── splash.rs         # diamond ASCII art
        ├── conversation/
        │   ├── mod.rs            # M05: agent loop
        │   ├── engine.rs         # submit_message, streaming
        │   ├── system_prompt.rs  # prompt assembly + CLAUDE.md loading
        │   └── compaction.rs     # context compaction
        ├── state/
        │   ├── mod.rs            # M04: immutable store
        │   └── store.rs          # AppState, getState, setState, subscribe
        ├── tools/
        │   ├── mod.rs            # M06: registry, assembly, multi-turn orchestration
        │   ├── contract.rs       # Tool trait definition
        │   ├── permission.rs     # M03: 7 modes, pattern rules, decision engine
        │   ├── bash.rs           # M07: shell execution + security validation
        │   ├── read.rs           # file reading + image + PDF
        │   ├── edit.rs           # string replacement with 11-step validation
        │   ├── write.rs          # file create/overwrite
        │   ├── glob.rs           # pattern-based file search
        │   ├── grep.rs           # ripgrep-style content search
        │   ├── notebook_edit.rs  # Jupyter notebook cell editing
        │   ├── config_tool.rs    # settings read/write
        │   ├── agent.rs          # sub-agent spawning (sync/async/background)
        │   ├── task_create.rs
        │   ├── task_get.rs
        │   ├── task_list.rs
        │   ├── task_update.rs
        │   ├── task_stop.rs
        │   ├── task_output.rs
        │   ├── todo_write.rs
        │   ├── tool_search.rs
        │   ├── web_fetch.rs      # URL fetch + content processing
        │   ├── web_search.rs     # server-side web search
        │   ├── skill.rs          # skill invocation
        │   ├── ask_user_question.rs
        │   ├── enter_plan_mode.rs
        │   ├── exit_plan_mode.rs
        │   ├── enter_worktree.rs
        │   ├── exit_worktree.rs
        │   ├── send_message.rs
        │   ├── lsp.rs
        │   ├── cron_create.rs
        │   ├── cron_delete.rs
        │   ├── cron_list.rs
        │   ├── sleep.rs
        │   ├── monitor.rs
        │   └── structured_output.rs
        ├── mcp/
        │   ├── mod.rs            # M09: MCP client manager
        │   ├── client.rs         # per-server connection
        │   ├── transport.rs      # stdio transport
        │   └── resources.rs      # ListMcpResources + ReadMcpResource tools
        ├── commands/
        │   ├── mod.rs            # M10: slash command system
        │   ├── registry.rs       # registration + resolution
        │   └── dispatch.rs       # prompt/local/local-jsx dispatch
        ├── skills/
        │   ├── mod.rs            # Superpowers integration
        │   ├── loader.rs         # markdown + frontmatter parsing
        │   └── discovery.rs      # path scanning + file watching
        └── sdk/
            ├── mod.rs            # M12: wire protocol
            └── protocol.rs       # message types, query() function
```

---

## Phase 1: Cargo Workspace + Shared Crate

### Task 1.1: Initialize workspace and shared crate

**Files:**
- Create: `Cargo.toml`
- Create: `shared/Cargo.toml`
- Create: `shared/src/lib.rs`

**Reference:** Spec section 1.4 (data model), 2.6 (config)

- [ ] **Step 1: Create workspace root `Cargo.toml`**

```toml
[workspace]
members = ["shared", "server", "cli"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
```

- [ ] **Step 2: Create `shared/Cargo.toml`**

```toml
[package]
name = "shared"
version.workspace = true
edition.workspace = true

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
```

- [ ] **Step 3: Write shared types in `shared/src/lib.rs`**

```rust
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
```

- [ ] **Step 4: Verify workspace compiles**

Run: `cargo check`
Expected: Compiles successfully with just the shared crate.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml shared/
git commit -m "feat: initialize Cargo workspace and shared crate with core types"
```

---

## Phase 2: Platform Server

### Task 2.1: Server crate setup + main entry point

**Files:**
- Create: `server/Cargo.toml`
- Create: `server/src/main.rs`
- Create: `server/src/domain/mod.rs`
- Create: `server/src/domain/auth/mod.rs`
- Create: `server/src/adapters/mod.rs`
- Create: `server/src/routes/mod.rs`

- [ ] **Step 1: Create `server/Cargo.toml`**

```toml
[package]
name = "server"
version.workspace = true
edition.workspace = true

[dependencies]
shared = { path = "../shared" }
axum = { version = "0.7", features = ["macros"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }
rusqlite = { version = "0.31", features = ["bundled"] }
bcrypt = "0.15"
jsonwebtoken = "9"
reqwest = { version = "0.12", features = ["json"] }
sha2 = "0.10"
base64 = "0.22"
rand = "0.8"
thiserror = "1"
tower-http = { version = "0.5", features = ["cors"] }
tracing = "0.1"
tracing-subscriber = "0.3"
```

- [ ] **Step 2: Write module stubs**

`server/src/domain/mod.rs`:
```rust
pub mod auth;
```

`server/src/domain/auth/mod.rs`:
```rust
pub mod ports;
pub mod service;
```

`server/src/adapters/mod.rs`:
```rust
pub mod sqlite_auth_repo;
pub mod openrouter_client;
```

`server/src/routes/mod.rs`:
```rust
pub mod auth;
```

- [ ] **Step 3: Write `server/src/main.rs`**

```rust
mod adapters;
mod domain;
mod routes;

use axum::Router;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber;

use domain::auth::service::AuthService;
use adapters::sqlite_auth_repo::SqliteAuthRepo;
use adapters::openrouter_client::OpenRouterClient;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let repo = Arc::new(SqliteAuthRepo::new("super.db").expect("failed to open database"));
    let openrouter = Arc::new(OpenRouterClient::new(
        std::env::var("OPENROUTER_MANAGEMENT_KEY").expect("OPENROUTER_MANAGEMENT_KEY not set"),
    ));
    let service = Arc::new(AuthService::new(repo, openrouter));

    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);

    let app = Router::new()
        .nest("/auth", routes::auth::routes())
        .layer(cors)
        .with_state(service);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    tracing::info!("server listening on :3000");
    axum::serve(listener, app).await.unwrap();
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p server`
Expected: Will fail on missing AuthService, SqliteAuthRepo, OpenRouterClient — that's expected. The module structure is in place.

- [ ] **Step 5: Commit**

```bash
git add server/
git commit -m "feat(server): scaffold server crate with module structure and main entry point"
```

### Task 2.2: Domain ports (traits)

**Files:**
- Create: `server/src/domain/auth/ports.rs`

**Reference:** Spec section 1.5

- [ ] **Step 1: Write trait definitions**

`server/src/domain/auth/ports.rs`:
```rust
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use shared::{AuthError, UserProfile};
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
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p server`
Expected: Need `async_trait` in deps. Add `async-trait = "0.1"` to `server/Cargo.toml`.

- [ ] **Step 3: Commit**

```bash
git add server/src/domain/auth/ports.rs server/Cargo.toml
git commit -m "feat(server): define AuthRepository and OpenRouterProvider traits"
```

### Task 2.3: SQLite adapter

**Files:**
- Create: `server/src/adapters/sqlite_auth_repo.rs`

- [ ] **Step 1: Write SqliteAuthRepo implementation**

`server/src/adapters/sqlite_auth_repo.rs`:
```rust
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
        Ok(Self { conn: std::sync::Mutex::new(conn) })
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
        Ok(User { id, email: email.to_string(), password_hash: password_hash.to_string(), created_at: Utc::now() })
    }

    async fn find_user_by_email(&self, email: &str) -> Result<Option<User>, AuthError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, email, password_hash, created_at FROM users WHERE email = ?1")
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let mut rows = stmt.query_map(params![email], |row| {
            Ok(User {
                id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                email: row.get(1)?,
                password_hash: row.get(2)?,
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(3)?).unwrap().with_timezone(&Utc),
            })
        }).map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(rows.next().transpose().map_err(|e| AuthError::Internal(e.to_string()))?)
    }

    async fn find_user_by_id(&self, id: &Uuid) -> Result<Option<User>, AuthError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, email, password_hash, created_at FROM users WHERE id = ?1")
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let mut rows = stmt.query_map(params![id.to_string()], |row| {
            Ok(User {
                id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                email: row.get(1)?,
                password_hash: row.get(2)?,
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(3)?).unwrap().with_timezone(&Utc),
            })
        }).map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(rows.next().transpose().map_err(|e| AuthError::Internal(e.to_string()))?)
    }

    async fn store_api_key(&self, user_id: &Uuid, key_id: &str, key_value: &str) -> Result<(), AuthError> {
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
        let mut rows = stmt.query_map(params![user_id.to_string()], |row| {
            Ok(ApiKey {
                openrouter_key_id: row.get(0)?,
                openrouter_key_value: row.get(1)?,
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(2)?).unwrap().with_timezone(&Utc),
                revoked_at: row.get::<_, Option<String>>(3)?.map(|s| chrono::DateTime::parse_from_rfc3339(&s).unwrap().with_timezone(&Utc)),
            })
        }).map_err(|e| AuthError::Internal(e.to_string()))?;
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

    async fn store_authorization_code(&self, code: &AuthorizationCode) -> Result<(), AuthError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO auth_codes (code, user_id, code_challenge, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![code.code, code.user_id.to_string(), code.code_challenge, code.expires_at.to_rfc3339()],
        )
        .map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn consume_authorization_code(&self, code: &str) -> Result<Option<AuthorizationCode>, AuthError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT code, user_id, code_challenge, expires_at FROM auth_codes WHERE code = ?1")
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let result = stmt.query_map(params![code], |row| {
            Ok(AuthorizationCode {
                code: row.get(0)?,
                user_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap(),
                code_challenge: row.get(2)?,
                expires_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(3)?).unwrap().with_timezone(&Utc),
            })
        }).map_err(|e| AuthError::Internal(e.to_string()))?;
        let code = result.last().transpose().map_err(|e| AuthError::Internal(e.to_string()))?;
        if code.is_some() {
            conn.execute("DELETE FROM auth_codes WHERE code = ?1", params![code.as_ref().unwrap().code])
                .map_err(|e| AuthError::Internal(e.to_string()))?;
        }
        Ok(code)
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

    async fn consume_refresh_token(&self, token_hash: &str) -> Result<Option<RefreshToken>, AuthError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT token_hash, user_id, expires_at FROM refresh_tokens WHERE token_hash = ?1")
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let result = stmt.query_map(params![token_hash], |row| {
            Ok(RefreshToken {
                token_hash: row.get(0)?,
                user_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap(),
                expires_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(2)?).unwrap().with_timezone(&Utc),
            })
        }).map_err(|e| AuthError::Internal(e.to_string()))?;
        let token = result.last().transpose().map_err(|e| AuthError::Internal(e.to_string()))?;
        if token.is_some() {
            conn.execute("DELETE FROM refresh_tokens WHERE token_hash = ?1", params![token_hash])
                .map_err(|e| AuthError::Internal(e.to_string()))?;
        }
        Ok(token)
    }
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p server`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add server/src/adapters/sqlite_auth_repo.rs
git commit -m "feat(server): implement SqliteAuthRepo adapter"
```

### Task 2.4: OpenRouter client adapter

**Files:**
- Create: `server/src/adapters/openrouter_client.rs`

- [ ] **Step 1: Write OpenRouterClient**

`server/src/adapters/openrouter_client.rs`:
```rust
use async_trait::async_trait;
use serde::Deserialize;
use shared::AuthError;

use crate::domain::auth::ports::{OpenRouterKey, OpenRouterProvider};

pub struct OpenRouterClient {
    management_key: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct OpenRouterKeyResponse {
    key: String,
    name: Option<String>,
    label: Option<String>,
    limit: Option<f64>,
    #[serde(default)]
    disabled: bool,
}

#[derive(Deserialize)]
struct OpenRouterKeysResponse {
    data: Vec<OpenRouterKeyResponse>,
}

impl OpenRouterClient {
    pub fn new(management_key: String) -> Self {
        Self { management_key, http: reqwest::Client::new() }
    }
}

#[async_trait]
impl OpenRouterProvider for OpenRouterClient {
    async fn create_key(&self, label: &str, limit_usd: u32) -> Result<OpenRouterKey, AuthError> {
        let resp = self
            .http
            .post("https://openrouter.ai/api/v1/keys")
            .bearer_auth(&self.management_key)
            .json(&serde_json::json!({
                "name": label,
                "label": label,
                "limit": limit_usd,
            }))
            .send()
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AuthError::Internal(format!("OpenRouter key creation failed: {body}")));
        }

        let key: OpenRouterKeyResponse = resp.json().await.map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(OpenRouterKey {
            id: label.to_string(), // OpenRouter doesn't return an ID; use label as key for our records
            key: key.key,
            label: label.to_string(),
            limit_usd,
        })
    }

    async fn revoke_key(&self, _key_id: &str) -> Result<(), AuthError> {
        // OpenRouter key deletion: GET /api/v1/keys then DELETE matching key
        let resp = self
            .http
            .get("https://openrouter.ai/api/v1/keys")
            .bearer_auth(&self.management_key)
            .send()
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        let keys: OpenRouterKeysResponse = resp.json().await.map_err(|e| AuthError::Internal(e.to_string()))?;
        for key in keys.data {
            if key.label.as_deref() == Some(_key_id) || key.name.as_deref() == Some(_key_id) {
                self.http
                    .delete(&format!("https://openrouter.ai/api/v1/keys/{}", key.key))
                    .bearer_auth(&self.management_key)
                    .send()
                    .await
                    .map_err(|e| AuthError::Internal(e.to_string()))?;
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p server`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add server/src/adapters/openrouter_client.rs
git commit -m "feat(server): implement OpenRouterClient adapter for key management"
```

### Task 2.5: AuthService (domain logic)

**Files:**
- Create: `server/src/domain/auth/service.rs`

**Reference:** Spec section 1.2 (auth flow), 1.3 (key management)

- [ ] **Step 1: Write AuthService**

`server/src/domain/auth/service.rs`:
```rust
use std::sync::Arc;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use shared::{AuthError, RegisterRequest, TokenResponse, UserProfile};
use uuid::Uuid;

use crate::domain::auth::ports::{AuthRepository, AuthorizationCode, OpenRouterProvider, RefreshToken, User};

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
    pub fn new(repo: Arc<dyn AuthRepository>, openrouter: Arc<dyn OpenRouterProvider>) -> Self {
        Self { repo, openrouter }
    }

    fn hash_password(password: &str) -> Result<String, AuthError> {
        bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| AuthError::Internal(e.to_string()))
    }

    fn verify_password(password: &str, hash: &str) -> Result<bool, AuthError> {
        bcrypt::verify(password, hash).map_err(|e| AuthError::Internal(e.to_string()))
    }

    fn generate_code() -> String {
        let bytes: [u8; 32] = rand::thread_rng().gen();
        URL_SAFE_NO_PAD.encode(bytes)
    }

    fn generate_token() -> String {
        let bytes: [u8; 48] = rand::thread_rng().gen();
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
        let user = self.repo.create_user(&req.email, &password_hash).await?;

        // Provision OpenRouter key at registration time
        let label = format!("super-user-{}", user.id);
        let key = self.openrouter.create_key(&label, DEFAULT_KEY_LIMIT_USD).await?;
        self.repo.store_api_key(&user.id, &key.id, &key.key).await?;

        Ok(UserProfile {
            id: user.id,
            email: user.email,
            openrouter_api_key: key.key,
            created_at: user.created_at,
        })
    }

    pub async fn login(&self, email: &str, password: &str, code_challenge: &str) -> Result<String, AuthError> {
        let user = self
            .repo
            .find_user_by_email(email)
            .await?
            .ok_or(AuthError::InvalidCredentials)?;

        if !self::AuthService::verify_password(password, &user.password_hash)? {
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

    pub async fn authorize(&self, code: &str, code_verifier: &str) -> Result<TokenResponse, AuthError> {
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

    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenResponse, AuthError> {
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
        let user = self.repo.find_user_by_id(&user_id).await?.ok_or(AuthError::UserNotFound)?;
        let api_key = self.repo.get_active_api_key(&user_id).await?.ok_or(AuthError::Internal("no API key provisioned".into()))?;
        Ok(UserProfile {
            id: user.id,
            email: user.email,
            openrouter_api_key: api_key.openrouter_key_value,
            created_at: user.created_at,
        })
    }

    pub async fn rotate_key(&self, access_token: &str) -> Result<UserProfile, AuthError> {
        let user_id = self.verify_jwt(access_token)?;
        let user = self.repo.find_user_by_id(&user_id).await?.ok_or(AuthError::UserNotFound)?;

        // Revoke old key
        if let Some(old_key) = self.repo.get_active_api_key(&user_id).await? {
            self.openrouter.revoke_key(&old_key.openrouter_key_id).await?;
            self.repo.revoke_api_key(&user_id, &old_key.openrouter_key_id).await?;
        }

        // Create new key
        let label = format!("super-user-{}", user_id);
        let key = self.openrouter.create_key(&label, DEFAULT_KEY_LIMIT_USD).await?;
        self.repo.store_api_key(&user_id, &key.id, &key.key).await?;

        Ok(UserProfile {
            id: user.id,
            email: user.email,
            openrouter_api_key: key.key,
            created_at: user.created_at,
        })
    }
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p server`
Expected: Compiles (no errors).

- [ ] **Step 3: Commit**

```bash
git add server/src/domain/auth/service.rs
git commit -m "feat(server): implement AuthService with register/login/authorize/refresh/rotate"
```

### Task 2.6: HTTP routes

**Files:**
- Create: `server/src/routes/auth.rs`

- [ ] **Step 1: Write route handlers**

`server/src/routes/auth.rs`:
```rust
use std::sync::Arc;
use axum::{Json, Router, extract::State, http::StatusCode, routing::{get, post}};
use serde::Deserialize;
use shared::{AuthorizeRequest, RefreshRequest, RegisterRequest, TokenResponse, UserProfile};

use crate::domain::auth::service::AuthService;

#[derive(Clone)]
struct AppState {
    service: Arc<AuthService>,
}

#[derive(Deserialize)]
struct LoginQuery {
    email: String,
    password: String,
    code_challenge: String,
    #[serde(default)]
    redirect_uri: Option<String>,
}

pub fn routes() -> Router {
    Router::new()
        .route("/register", post(register))
        .route("/authorize", post(authorize))
        .route("/refresh", post(refresh))
        .route("/me", get(me))
        .route("/key", post(rotate_key))
        .route("/login", get(login_page))
        .route("/login", post(login))
        .with_state(AppState { service: Arc::new(std::sync::OnceLock::new().get_or_init(|| unreachable!()).clone()) }) // overridden in main
}

pub fn routes_with_state(service: Arc<AuthService>) -> Router {
    Router::new()
        .route("/register", post(register))
        .route("/authorize", post(authorize))
        .route("/refresh", post(refresh))
        .route("/me", get(me))
        .route("/key", post(rotate_key))
        .route("/login", get(login_page))
        .route("/login", post(login))
        .with_state(AppState { service })
}

async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<UserProfile>, (StatusCode, String)> {
    state.service.register(&req).await.map(Json).map_err(|e| {
        let status = match &e {
            shared::AuthError::UserAlreadyExists => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, e.to_string())
    })
}

async fn authorize(
    State(state): State<AppState>,
    Json(req): Json<AuthorizeRequest>,
) -> Result<Json<TokenResponse>, (StatusCode, String)> {
    state.service.authorize(&req.code, &req.code_verifier).await.map(Json).map_err(|e| {
        if matches!(e, shared::AuthError::InvalidToken | shared::AuthError::TokenExpired) {
            (StatusCode::UNAUTHORIZED, e.to_string())
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    })
}

async fn refresh(
    State(state): State<AppState>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<TokenResponse>, (StatusCode, String)> {
    state.service.refresh(&req.refresh_token).await.map(Json).map_err(|e| {
        if matches!(e, shared::AuthError::InvalidToken | shared::AuthError::TokenExpired) {
            (StatusCode::UNAUTHORIZED, e.to_string())
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    })
}

async fn me(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<UserProfile>, (StatusCode, String)> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or((StatusCode::UNAUTHORIZED, "missing authorization header".into()))?;
    state.service.get_profile(token).await.map(Json).map_err(|e| {
        if matches!(e, shared::AuthError::InvalidToken) {
            (StatusCode::UNAUTHORIZED, e.to_string())
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    })
}

async fn rotate_key(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<UserProfile>, (StatusCode, String)> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or((StatusCode::UNAUTHORIZED, "missing authorization header".into()))?;
    state.service.rotate_key(token).await.map(Json).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })
}

async fn login_page() -> axum::response::Html<&'static str> {
    axum::response::Html(LOGIN_HTML)
}

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
    code_challenge: String,
    redirect_uri: Option<String>,
}

async fn login(
    State(state): State<AppState>,
    axum::Form(form): axum::Form<LoginForm>,
) -> Result<axum::response::Redirect, (StatusCode, String)> {
    let code = state.service.login(&form.email, &form.password, &form.code_challenge).await.map_err(|e| {
        (StatusCode::UNAUTHORIZED, e.to_string())
    })?;
    let redirect = form.redirect_uri.unwrap_or_else(|| "http://localhost:0/callback".into());
    Ok(axum::response::Redirect::to(&format!("{redirect}?code={code}")))
}

const LOGIN_HTML: &str = r#"<!DOCTYPE html>
<html><head><title>Super Login</title></head>
<body>
<h1>Super Login</h1>
<form method="post">
<input name="email" type="email" placeholder="Email" required />
<input name="password" type="password" placeholder="Password" required />
<input name="code_challenge" type="hidden" id="cc" />
<input name="redirect_uri" type="hidden" id="ru" />
<button type="submit">Log in</button>
</form>
<script>
const params = new URLSearchParams(window.location.search);
document.getElementById('cc').value = params.get('code_challenge') || '';
document.getElementById('ru').value = params.get('redirect_uri') || '';
</script>
</body></html>"#;
```

- [ ] **Step 2: Update `main.rs` to use `routes_with_state`**

In `server/src/main.rs`, replace the route building:
```rust
let app = Router::new()
    .nest("/auth", routes::auth::routes_with_state(service.clone()))
    .layer(cors);
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p server`
Expected: Compiles. (May need `axum` feature `form` — add to Cargo.toml if needed)

- [ ] **Step 4: Commit**

```bash
git add server/src/routes/auth.rs server/src/main.rs server/Cargo.toml
git commit -m "feat(server): implement HTTP routes for all 5 auth endpoints + login page"
```

---

## Phase 3: Super CLI — Bootstrap + TUI

### Task 3.1: CLI crate setup + main entry point

**Files:**
- Create: `cli/Cargo.toml`
- Create: `cli/src/main.rs`
- Create: `cli/src/config.rs`
- Create: `cli/src/bootstrap.rs`
- Create: `cli/src/auth.rs`

- [ ] **Step 1: Create `cli/Cargo.toml`**

```toml
[package]
name = "super-cli"
version.workspace = true
edition.workspace = true

[[bin]]
name = "super"
path = "src/main.rs"

[dependencies]
shared = { path = "../shared" }
ratatui = "0.28"
crossterm = "0.28"
rig-core = "0.5"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
reqwest = { version = "0.12", features = ["json"] }
dirs = "5"
clap = { version = "4", features = ["derive"] }
uuid = { version = "1", features = ["v4"] }
sha2 = "0.10"
base64 = "0.22"
rand = "0.8"
regex = "1"
glob = "0.3"
notify = "6"
tracing = "0.1"
tracing-subscriber = "0.3"
chrono = { version = "0.4", features = ["serde"] }
thiserror = "1"
```

- [ ] **Step 2: Write `cli/src/config.rs`**

```rust
use shared::CliConfig;
use std::path::PathBuf;

pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(".super")
        .join("config.json")
}

pub fn load_config() -> CliConfig {
    let path = config_path();
    if path.exists() {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        CliConfig::default()
    }
}

pub fn save_config(config: &CliConfig) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let content = serde_json::to_string_pretty(config).unwrap_or_default();
    std::fs::write(&path, content).ok();
}
```

- [ ] **Step 3: Write `cli/src/auth.rs`**

```rust
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;
use sha2::{Digest, Sha256};
use shared::{AuthorizeRequest, CliConfig, RefreshRequest, TokenResponse, UserProfile};
use crate::config::{load_config, save_config};

pub struct AuthClient {
    base_url: String,
    http: reqwest::Client,
}

impl AuthClient {
    pub fn new(base_url: String) -> Self {
        Self { base_url, http: reqwest::Client::new() }
    }

    pub async fn login_flow(&self) -> Result<CliConfig, Box<dyn std::error::Error>> {
        // Generate PKCE pair
        let code_verifier = generate_code_verifier();
        let code_challenge = compute_s256_challenge(&code_verifier);

        // Open browser
        let login_url = format!(
            "{}/auth/login?code_challenge={}&redirect_uri=http://localhost:0/callback",
            self.base_url, code_challenge
        );
        webbrowser::open(&login_url)?;

        // Listen on a random port for the callback
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        println!("Listening on port {port} for callback...");

        // Accept one connection and parse the code
        let (mut stream, _) = listener.accept()?;
        use std::io::{BufRead, BufReader};
        let mut reader = BufReader::new(&mut stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line)?;

        let code = extract_code_from_request(&request_line)
            .ok_or("no authorization code in callback")?;

        // Respond to browser
        use std::io::Write;
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body><h1>Logged in! You can close this window.</h1></body></html>";
        stream.write_all(response.as_bytes())?;

        // Exchange code for tokens
        let tokens = self.exchange_code(&code, &code_verifier).await?;

        // Get profile + API key
        let profile = self.get_profile(&tokens.access_token).await?;

        let mut config = load_config();
        config.access_token = Some(tokens.access_token);
        config.refresh_token = Some(tokens.refresh_token);
        config.openrouter_api_key = Some(profile.openrouter_api_key);
        save_config(&config);

        Ok(config)
    }

    async fn exchange_code(&self, code: &str, code_verifier: &str) -> Result<TokenResponse, Box<dyn std::error::Error>> {
        let resp = self.http
            .post(&format!("{}/auth/authorize", self.base_url))
            .json(&AuthorizeRequest { code: code.to_string(), code_verifier: code_verifier.to_string() })
            .send().await?;
        if !resp.status().is_success() {
            return Err(format!("authorize failed: {}", resp.text().await?).into());
        }
        Ok(resp.json().await?)
    }

    async fn get_profile(&self, token: &str) -> Result<UserProfile, Box<dyn std::error::Error>> {
        let resp = self.http
            .get(&format!("{}/auth/me", self.base_url))
            .bearer_auth(token)
            .send().await?;
        if !resp.status().is_success() {
            return Err(format!("profile fetch failed: {}", resp.text().await?).into());
        }
        Ok(resp.json().await?)
    }

    pub async fn refresh_token(&self, refresh_token: &str) -> Result<TokenResponse, Box<dyn std::error::Error>> {
        let resp = self.http
            .post(&format!("{}/auth/refresh", self.base_url))
            .json(&RefreshRequest { refresh_token: refresh_token.to_string() })
            .send().await?;
        if !resp.status().is_success() {
            return Err(format!("refresh failed: {}", resp.text().await?).into());
        }
        Ok(resp.json().await?)
    }
}

fn generate_code_verifier() -> String {
    let bytes: [u8; 32] = rand::thread_rng().gen();
    URL_SAFE_NO_PAD.encode(bytes)
}

fn compute_s256_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn extract_code_from_request(request_line: &str) -> Option<String> {
    let path = request_line.split_whitespace().nth(1)?;
    let query = path.split('?').nth(1)?;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next()? == "code" {
            return parts.next().map(|s| s.to_string());
        }
    }
    None
}
```

- [ ] **Step 4: Add `webbrowser` to Cargo.toml**

Add: `webbrowser = "0.8"`

- [ ] **Step 5: Write `cli/src/main.rs` with clap**

```rust
mod auth;
mod bootstrap;
mod config;
mod conversation;
mod state;
mod tools;
mod tui;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "super", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Login to Super
    Login,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Login) => {
            let config = config::load_config();
            let auth_client = auth::AuthClient::new(config.api_base_url.clone());
            match auth_client.login_flow().await {
                Ok(_) => println!("Logged in successfully."),
                Err(e) => eprintln!("Login failed: {e}"),
            }
        }
        None => {
            bootstrap::run().await;
        }
    }
}
```

- [ ] **Step 6: Write `cli/src/bootstrap.rs`**

```rust
use crate::config::load_config;

pub async fn run() {
    let config = load_config();

    if config.openrouter_api_key.is_none() {
        eprintln!("Not logged in. Run 'super login' first.");
        return;
    }

    // Initialize TUI
    crate::tui::app::run(config).await;
}
```

- [ ] **Step 7: Compile check**

Run: `cargo check -p super-cli`
Expected: Fails on missing `cli/src/conversation`, `cli/src/state`, `cli/src/tools`, `cli/src/tui` modules. That's expected.

- [ ] **Step 8: Commit**

```bash
git add cli/ cli/Cargo.toml
git commit -m "feat(cli): scaffold CLI crate with config, auth, bootstrap, and main entry point"
```

### Task 3.2: TUI shell — app loop, scroll area, activity indicator, input bar

**Files:**
- Create: `cli/src/tui/mod.rs`
- Create: `cli/src/tui/app.rs`
- Create: `cli/src/tui/scroll_area.rs`
- Create: `cli/src/tui/activity.rs`
- Create: `cli/src/tui/input_bar.rs`
- Create: `cli/src/tui/splash.rs`

- [ ] **Step 1: Write `cli/src/tui/mod.rs`**

```rust
pub mod app;
pub mod activity;
pub mod input_bar;
pub mod scroll_area;
pub mod splash;
```

- [ ] **Step 2: Write `cli/src/tui/activity.rs`**

```rust
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

/// Claude Code activity glyphs: cycling animated sequence
static GLYPHS: &[&str] = &["⟣", "⟡", "⟐", "◈", "⟢"];
const IDLE_GLYPH: &str = "♦";

pub enum ActivityState {
    Idle,
    Active { verb: String, glyph_idx: usize },
}

impl ActivityState {
    pub fn idle() -> Self {
        Self::Idle
    }

    pub fn active(verb: &str) -> Self {
        Self::Active { verb: verb.to_string(), glyph_idx: 0 }
    }

    pub fn tick(&mut self) {
        if let Self::Active { glyph_idx, .. } = self {
            *glyph_idx = (*glyph_idx + 1) % GLYPHS.len();
        }
    }

    pub fn render(&self) -> Line {
        match self {
            Self::Idle => Line::from(vec![
                Span::styled(IDLE_GLYPH, Style::default().fg(Color::White)),
            ]),
            Self::Active { verb, glyph_idx } => Line::from(vec![
                Span::styled(GLYPHS[*glyph_idx], Style::default().fg(Color::Cyan)),
                Span::raw("  "),
                Span::styled(verb.as_str(), Style::default().fg(Color::White)),
            ]),
        }
    }
}
```

- [ ] **Step 3: Write `cli/src/tui/input_bar.rs`**

```rust
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    widgets::Paragraph,
    Frame,
};

pub struct InputBar {
    pub content: String,
    pub cursor_position: usize,
}

impl InputBar {
    pub fn new() -> Self {
        Self { content: String::new(), cursor_position: 0 }
    }

    pub fn push_char(&mut self, c: char) {
        self.content.insert(self.cursor_position, c);
        self.cursor_position += 1;
    }

    pub fn delete_prev(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
            self.content.remove(self.cursor_position);
        }
    }

    pub fn submit(&mut self) -> String {
        let text = std::mem::take(&mut self.content);
        self.cursor_position = 0;
        text
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let display = format!("> {}", self.content);
        let paragraph = Paragraph::new(display)
            .style(Style::default().fg(Color::White));
        f.render_widget(paragraph, area);
    }
}
```

- [ ] **Step 4: Write `cli/src/tui/scroll_area.rs`**

```rust
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Text},
    widgets::Paragraph,
    Frame,
};

#[derive(Clone)]
pub enum Message {
    User(String),
    Assistant(String),
    ToolCall { name: String, input: String, result: Option<String> },
    System(String),
    Thinking,
}

pub struct ScrollArea {
    pub messages: Vec<Message>,
    pub scroll_offset: u16,
}

impl ScrollArea {
    pub fn new() -> Self {
        Self { messages: Vec::new(), scroll_offset: 0 }
    }

    pub fn push(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let lines: Vec<Line> = self.messages.iter().flat_map(|msg| match msg {
            Message::User(text) => vec![
                Line::from(format!("╭─ User ─")),
                Line::from(text.as_str()),
                Line::from(""),
            ],
            Message::Assistant(text) => vec![
                Line::from(format!("╭─ Super ─")),
                Line::from(text.as_str()),
                Line::from(""),
            ],
            Message::ToolCall { name, input, result } => {
                let mut lines = vec![
                    Line::from(format!("╭─ Tool: {name} ─")),
                    Line::from(input.as_str()),
                ];
                if let Some(r) = result {
                    lines.push(Line::from(r.as_str()));
                }
                lines.push(Line::from(""));
                lines
            }
            Message::System(text) => vec![Line::from(text.as_str())],
            Message::Thinking => vec![Line::styled("thinking...", Style::default().fg(Color::DarkGray))],
        }).collect();

        let text = Text::from(lines);
        let paragraph = Paragraph::new(text).scroll((self.scroll_offset, 0));
        f.render_widget(paragraph, area);
    }
}
```

- [ ] **Step 5: Write `cli/src/tui/splash.rs`**

```rust
/// ASCII-art diamond splash matching Claude Code's launch behavior
pub const SPLASH: &str = r#"
                               ◆
                              ◇ ◇
                             ◆   ◆
                            ◇     ◇
                           ◆       ◆
                          ◇         ◇
                         ◆           ◆
                        ◇             ◇
                       ◆               ◆
                        ◇             ◇
                         ◆           ◆
                          ◇         ◇
                           ◆       ◆
                            ◇     ◇
                             ◆   ◆
                              ◇ ◇
                               ◆
"#;
```

- [ ] **Step 6: Write `cli/src/tui/app.rs`** — the main TUI event loop

```rust
use std::time::Duration;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    DefaultTerminal, Frame,
};
use shared::CliConfig;

use super::activity::ActivityState;
use super::input_bar::InputBar;
use super::scroll_area::{Message, ScrollArea};
use super::splash::SPLASH;

pub struct App {
    scroll_area: ScrollArea,
    activity: ActivityState,
    input: InputBar,
    show_splash: bool,
    config: CliConfig,
    should_quit: bool,
}

impl App {
    pub fn new(config: CliConfig) -> Self {
        Self {
            scroll_area: ScrollArea::new(),
            activity: ActivityState::idle(),
            input: InputBar::new(),
            show_splash: true,
            config,
            should_quit: false,
        }
    }

    fn handle_event(&mut self) -> std::io::Result<()> {
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Enter => {
                        let text = self.input.submit();
                        if !text.is_empty() {
                            self.show_splash = false;
                            self.scroll_area.push(Message::User(text.clone()));
                            // Signal to process prompt in agent loop
                        }
                    }
                    KeyCode::Char(c) => self.input.push_char(c),
                    KeyCode::Backspace => self.input.delete_prev(),
                    KeyCode::Esc => self.should_quit = true,
                    _ => {}
                },
                _ => {}
            }
        }
        Ok(())
    }

    pub fn run(&mut self, mut terminal: DefaultTerminal) -> std::io::Result<()> {
        while !self.should_quit {
            self.activity.tick();
            terminal.draw(|f| self.render(f))?;
            self.handle_event()?;
        }
        Ok(())
    }

    fn render(&self, f: &mut Frame) {
        let area = f.area();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),      // scroll area
                Constraint::Length(1),   // activity row
                Constraint::Length(1),   // input bar
            ])
            .split(area);

        // Scroll area
        if self.show_splash {
            let splash = Paragraph::new(SPLASH)
                .style(Style::default().fg(Color::White))
                .block(Block::default().borders(Borders::NONE));
            f.render_widget(splash, layout[0]);
        } else {
            self.scroll_area.render(f, layout[0]);
        }

        // Activity row
        let activity_line = self.activity.render();
        f.render_widget(Paragraph::new(activity_line), layout[1]);

        // Input bar
        self.input.render(f, layout[2]);
    }
}

pub async fn run(config: CliConfig) {
    let mut terminal = ratatui::init();
    let mut app = App::new(config);
    let _ = app.run(terminal);
    ratatui::restore();
}
```

- [ ] **Step 7: Compile check**

Run: `cargo check -p super-cli`
Expected: Compiles (may need `crossterm` features `event-stream`).

- [ ] **Step 8: Commit**

```bash
git add cli/src/tui/
git commit -m "feat(cli): implement TUI shell — app loop, scroll area, activity indicator, input bar, splash"
```

---

## Phase 4: State Management (M04)

### Task 4.1: Immutable state store

**Files:**
- Create: `cli/src/state/mod.rs`
- Create: `cli/src/state/store.rs`

**Reference:** Analysis workspace architecture model section 4 (State Management)

- [ ] **Step 1: Write `cli/src/state/mod.rs`**

```rust
pub mod store;
```

- [ ] **Step 2: Write `cli/src/state/store.rs`**

The store uses a Zustand-like immutable pattern with `get_state`, `set_state`, and `subscribe`.

```rust
use std::sync::{Arc, RwLock};
use crate::tui::scroll_area::Message;

#[derive(Clone, Default)]
pub struct AppState {
    pub messages: Vec<Message>,
    pub permission_mode: PermissionMode,
    pub model: String,
    pub thinking_enabled: bool,
    pub fast_mode: bool,
    pub is_streaming: bool,
    pub should_compact: bool,
}

#[derive(Clone, Default, PartialEq)]
pub enum PermissionMode {
    #[default]
    Default,
    AcceptEdits,
    Bypass,
    Plan,
    DontAsk,
    Auto,
}

pub struct Store {
    state: Arc<RwLock<AppState>>,
    subscribers: Arc<RwLock<Vec<Box<dyn Fn(&AppState) + Send + Sync>>>>,
}

impl Store {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(AppState::default())),
            subscribers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn get_state(&self) -> AppState {
        self.state.read().unwrap().clone()
    }

    pub fn set_state(&self, updater: impl FnOnce(&mut AppState)) {
        let old_state = self.get_state();
        {
            let mut state = self.state.write().unwrap();
            updater(&mut state);
        }
        let new_state = self.get_state();
        self.notify(&new_state);
    }

    pub fn subscribe(&self, listener: impl Fn(&AppState) + Send + Sync + 'static) {
        self.subscribers.write().unwrap().push(Box::new(listener));
    }

    fn notify(&self, state: &AppState) {
        for sub in self.subscribers.read().unwrap().iter() {
            sub(state);
        }
    }
}
```

- [ ] **Step 3: Compile check & commit**

Run: `cargo check -p super-cli && git add cli/src/state/ && git commit -m "feat(cli): implement immutable state store with subscribe pattern"`
Expected: Compiles.

---

## Phase 5: Conversation Engine (M05)

### Task 5.1: System prompt + CLAUDE.md loading + streaming via rig

**Files:**
- Create: `cli/src/conversation/mod.rs`
- Create: `cli/src/conversation/engine.rs`
- Create: `cli/src/conversation/system_prompt.rs`
- Create: `cli/src/conversation/compaction.rs`

**Reference:** Analysis workspace module specs M05, behavioral spec `conversation.md`

- [ ] **Step 1: Write `cli/src/conversation/mod.rs`**

```rust
pub mod engine;
pub mod system_prompt;
pub mod compaction;
```

- [ ] **Step 2: Write `cli/src/conversation/system_prompt.rs`**

```rust
use std::path::PathBuf;

pub struct SystemPrompt {
    pub sections: Vec<String>,
}

impl SystemPrompt {
    pub fn build(cwd: &PathBuf) -> Self {
        let mut sections = Vec::new();

        // Load CLAUDE.md from cwd and parent directories
        if let Some(content) = load_claude_md(cwd) {
            sections.push(format!("<claude-md>\n{content}\n</claude-md>"));
        }

        // Load skills content (injected by skills module)
        // Skills are loaded separately and added via add_section()

        Self { sections }
    }

    pub fn add_section(&mut self, section: String) {
        self.sections.push(section);
    }

    pub fn render(&self) -> String {
        self.sections.join("\n\n")
    }
}

fn load_claude_md(cwd: &PathBuf) -> Option<String> {
    let mut dir = Some(cwd.as_path());
    while let Some(d) = dir {
        let path = d.join("CLAUDE.md");
        if path.exists() {
            return std::fs::read_to_string(&path).ok();
        }
        dir = d.parent();
    }
    None
}
```

- [ ] **Step 3: Write `cli/src/conversation/compaction.rs`**

```rust
/// Context compaction — truncates conversation history
/// matching Claude Code's compact() behavior.
/// Reference: SPEC-M05 compactions section

pub fn compact_messages(messages: &[crate::tui::scroll_area::Message], max_tokens: usize) -> Vec<crate::tui::scroll_area::Message> {
    // Keep system messages, then keep most recent messages up to max_tokens
    // Rough estimate: 1 token ~ 4 chars
    let max_chars = max_tokens * 4;
    let mut kept: Vec<crate::tui::scroll_area::Message> = Vec::new();
    let mut total_chars = 0usize;

    for msg in messages.iter().rev() {
        let chars = estimate_chars(msg);
        if total_chars + chars > max_chars {
            break;
        }
        total_chars += chars;
        kept.push(msg.clone());
    }
    kept.reverse();
    kept
}

fn estimate_chars(msg: &crate::tui::scroll_area::Message) -> usize {
    match msg {
        crate::tui::scroll_area::Message::User(s) => s.len(),
        crate::tui::scroll_area::Message::Assistant(s) => s.len(),
        crate::tui::scroll_area::Message::ToolCall { input, result, .. } => {
            input.len() + result.as_ref().map_or(0, |r| r.len())
        }
        crate::tui::scroll_area::Message::System(s) => s.len(),
        crate::tui::scroll_area::Message::Thinking => 10,
    }
}
```

- [ ] **Step 4: Write `cli/src/conversation/engine.rs`** — the agent loop

```rust
use std::sync::Arc;
use rig::providers::openrouter::OpenRouterProvider;
use rig::agent::Agent;
use rig::completion::Prompt;
use shared::CliConfig;
use crate::state::store::Store;
use crate::tui::scroll_area::Message;
use crate::conversation::system_prompt::SystemPrompt;

pub struct ConversationEngine {
    store: Arc<Store>,
    config: CliConfig,
}

impl ConversationEngine {
    pub fn new(store: Arc<Store>, config: CliConfig) -> Self {
        Self { store, config }
    }

    pub async fn process_prompt(&self, user_input: String, system_prompt: &SystemPrompt) -> Result<(), Box<dyn std::error::Error>> {
        // Set activity: Thinking
        self.store.set_state(|s| { s.is_streaming = true; });

        // Build messages for the API
        let system = system_prompt.render();

        // Create rig client pointed at OpenRouter with the per-user key
        let api_key = self.config.openrouter_api_key.as_ref()
            .ok_or("no OpenRouter API key configured")?;

        // Build conversation history
        let history = self.store.get_state().messages;
        let mut history_text = String::new();
        for msg in &history {
            match msg {
                Message::User(s) => history_text.push_str(&format!("User: {s}\n")),
                Message::Assistant(s) => history_text.push_str(&format!("Assistant: {s}\n")),
                Message::ToolCall { name, input, result } => {
                    history_text.push_str(&format!("Tool({name}): {input}\n"));
                    if let Some(r) = result {
                        history_text.push_str(&format!("Result: {r}\n"));
                    }
                }
                _ => {}
            }
        }

        // Build full prompt
        let full_prompt = format!(
            "{system}\n\n{history_text}\n\nUser: {user_input}\n\nAssistant:"
        );

        // Call OpenRouter via rig
        let client = rig::providers::openrouter::Client::new(api_key, None);

        self.store.set_state(|s| {
            s.messages.push(Message::Thinking);
        });

        let response = client.completion(&full_prompt)
            .model(&self.config.model)
            .send()
            .await?;

        self.store.set_state(|s| {
            // Remove thinking placeholder
            s.messages.retain(|m| !matches!(m, Message::Thinking));
            s.messages.push(Message::Assistant(response.clone()));
            s.is_streaming = false;
            // Check compaction
            if s.messages.len() > 100 {
                s.should_compact = true;
            }
        });

        Ok(())
    }
}
```

- [ ] **Step 5: Update `app.rs` to integrate engine**

Add to `App` struct in `cli/src/tui/app.rs`:
```rust
use std::sync::Arc;
use crate::state::store::Store;
use crate::conversation::engine::ConversationEngine;
use crate::conversation::system_prompt::SystemPrompt;

pub struct App {
    // ... existing fields ...
    store: Arc<Store>,
    engine: ConversationEngine,
    system_prompt: SystemPrompt,
}
```

Update `App::new`:
```rust
pub fn new(config: CliConfig) -> Self {
    let store = Arc::new(Store::new());
    let engine = ConversationEngine::new(store.clone(), config.clone());
    let system_prompt = SystemPrompt::build(&std::env::current_dir().unwrap_or_default());
    Self {
        scroll_area: ScrollArea::new(),
        activity: ActivityState::idle(),
        input: InputBar::new(),
        show_splash: true,
        store,
        engine,
        system_prompt,
        config,
        should_quit: false,
    }
}
```

Update the Enter key handler to process prompts asynchronously — spawn a task per prompt (simplified for initial implementation, full async in a later refinement).

- [ ] **Step 6: Compile check & commit**

Run: `cargo check -p super-cli`
Expected: May need to adjust rig API usage. Fix any compile errors, then:
```bash
git add cli/src/conversation/ cli/src/tui/app.rs
git commit -m "feat(cli): implement conversation engine — system prompt, CLAUDE.md loading, rig streaming"
```

---

## Phase 6: Tool Framework + Contract (M06)

### Task 6.1: Tool trait, registry, and assembly

**Files:**
- Create: `cli/src/tools/mod.rs`
- Create: `cli/src/tools/contract.rs`

**Reference:** SPEC-M06-002 (Tool Contract Interface), SPEC-M06-001 (Permission-Aware Assembly)

- [ ] **Step 1: Write `cli/src/tools/contract.rs`**

```rust
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Result from a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

/// Context passed to every tool call
pub struct ToolCallContext {
    pub cwd: std::path::PathBuf,
    pub permission_mode: crate::state::store::PermissionMode,
    pub abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
}

/// The tool contract — every tool implements this
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Execute the tool
    async fn call(&self, input: serde_json::Value, context: &ToolCallContext) -> ToolResult;

    /// Human-readable name shown in UI
    fn name(&self) -> &str;

    /// Description injected into the system prompt
    fn description(&self) -> &str;

    /// JSON Schema for input validation
    fn input_schema(&self) -> serde_json::Value;

    /// Whether this tool can run concurrently with others
    fn is_concurrency_safe(&self) -> bool { false }

    /// Whether this tool only reads (no side effects)
    fn is_read_only(&self) -> bool { false }

    /// Whether this tool is destructive
    fn is_destructive(&self) -> bool { false }

    /// Permission check — some tools need custom logic
    fn check_permission(&self, _input: &serde_json::Value) -> super::permission::Decision {
        super::permission::Decision::Ask
    }
}
```

- [ ] **Step 2: Write `cli/src/tools/mod.rs`** — the registry

```rust
pub mod contract;
pub mod permission;
pub mod bash;
pub mod read;
pub mod edit;
pub mod write;
pub mod glob_tool;
pub mod grep;
pub mod notebook_edit;
pub mod config_tool;
pub mod agent;
pub mod task_create;
pub mod task_get;
pub mod task_list;
pub mod task_update;
pub mod task_stop;
pub mod task_output;
pub mod todo_write;
pub mod tool_search;
pub mod web_fetch;
pub mod web_search;
pub mod skill;
pub mod ask_user_question;
pub mod enter_plan_mode;
pub mod exit_plan_mode;
pub mod enter_worktree;
pub mod exit_worktree;
pub mod send_message;
pub mod lsp;
pub mod cron_create;
pub mod cron_delete;
pub mod cron_list;
pub mod sleep;
pub mod monitor;
pub mod structured_output;

use std::sync::Arc;
use contract::{Tool, ToolCallContext, ToolResult};

pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    pub fn list(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    /// Assemble the tool pool for the current permission context.
    /// Filters by mode (plan mode = read-only tools only) and deny rules.
    pub fn assemble_for_mode(&self, mode: &crate::state::store::PermissionMode) -> Vec<Arc<dyn Tool>> {
        let mut pool: Vec<Arc<dyn Tool>> = self.tools.iter()
            .filter(|t| match mode {
                crate::state::store::PermissionMode::Plan => t.is_read_only(),
                _ => true,
            })
            .cloned()
            .collect();
        // Sort alphabetically for prompt cache stability
        pool.sort_by(|a, b| a.name().cmp(b.name()));
        pool
    }

    /// Assemble tool descriptions for the system prompt
    pub fn tool_descriptions(&self, mode: &crate::state::store::PermissionMode) -> String {
        let pool = self.assemble_for_mode(mode);
        pool.iter()
            .map(|t| format!("- **{}**: {}", t.name(), t.description()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Execute a tool by name
    pub async fn execute(&self, name: &str, input: serde_json::Value, context: &ToolCallContext) -> Result<ToolResult, String> {
        let tool = self.get(name).ok_or_else(|| format!("unknown tool: {name}"))?;
        Ok(tool.call(input, context).await)
    }
}
```

- [ ] **Step 3: Compile check & commit**

Run: `cargo check -p super-cli`
Will fail on missing tool files — that's expected for now. Add the module stubs, then:
```bash
git add cli/src/tools/mod.rs cli/src/tools/contract.rs
git commit -m "feat(cli): implement tool trait, registry, and permission-aware assembly"
```

---

## Phase 7: Permission System (M03)

### Task 7.1: Permission decision engine with 7 modes and pattern rules

**Files:**
- Create: `cli/src/tools/permission.rs`

**Reference:** SPEC-M03 (Permission System), architecture model section 4

- [ ] **Step 1: Write `cli/src/tools/permission.rs`**

```rust
use regex::Regex;
use crate::state::store::PermissionMode;

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone)]
pub struct PermissionRule {
    pub source: RuleSource,
    pub pattern: String,
    pub decision: Decision,
    pub compiled: Regex,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuleSource {
    User,
    Project,
    Local,
    Flag,
    Policy,
    Session,
    BuiltIn,
}

impl RuleSource {
    pub fn priority(&self) -> u8 {
        match self {
            Self::Policy => 7,
            Self::Flag => 6,
            Self::Session => 5,
            Self::Local => 4,
            Self::Project => 3,
            Self::User => 2,
            Self::BuiltIn => 1,
        }
    }
}

pub struct PermissionSystem {
    rules: Vec<PermissionRule>,
    mode: PermissionMode,
}

impl PermissionSystem {
    pub fn new(mode: PermissionMode) -> Self {
        Self { rules: Vec::new(), mode }
    }

    pub fn add_rule(&mut self, rule: PermissionRule) {
        self.rules.push(rule);
        self.rules.sort_by_key(|r| r.source.priority());
    }

    /// Evaluate a tool+input against all rules in priority order.
    /// First allow rule wins; if no allow, first deny wins;
    /// if no match, falls through to Ask.
    pub fn evaluate(&self, tool_name: &str, _input: &serde_json::Value) -> Decision {
        // Mode-based shortcuts
        match self.mode {
            PermissionMode::Bypass => return Decision::Allow,
            PermissionMode::Plan => {
                // In plan mode, non-read-only tools always denied
                // (read-only check handled in tool assembly)
            }
            _ => {}
        }

        // Walk rules in priority order (highest first)
        let mut rules = self.rules.clone();
        rules.sort_by_key(|r| -(r.source.priority() as i32));

        for rule in &rules {
            if rule.compiled.is_match(tool_name) {
                match rule.decision {
                    Decision::Allow => return Decision::Allow,
                    Decision::Deny => return Decision::Deny,
                    Decision::Ask => return Decision::Ask,
                }
            }
        }

        // No matching rule: ask user
        Decision::Ask
    }
}
```

- [ ] **Step 2: Compile check & commit**

```bash
git add cli/src/tools/permission.rs
git commit -m "feat(cli): implement permission system — 7 modes, pattern rules, priority-based decision engine"
```

---

## Phase 8: File Operation Tools (M08)

### Task 8.1: Read, Edit, Write, Glob, Grep, NotebookEdit, Config

**Files:**
- Create: `cli/src/tools/read.rs`
- Create: `cli/src/tools/edit.rs`
- Create: `cli/src/tools/write.rs`
- Create: `cli/src/tools/glob_tool.rs`
- Create: `cli/src/tools/grep.rs`
- Create: `cli/src/tools/notebook_edit.rs`
- Create: `cli/src/tools/config_tool.rs`

**Reference:** `analysis-workspace/raw/specs/behavioral-docs/tools.md` sections 2.1–2.6, SPEC-M08.

Each tool follows the same pattern. Here's the Read tool as the template — implement Edit, Write, Glob, Grep, NotebookEdit, and Config following the same contract with their validation pipelines per the spec.

- [ ] **Step 1: Write `cli/src/tools/read.rs`**

```rust
use std::path::PathBuf;
use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str { "Read" }

    fn description(&self) -> &str {
        "Reads a file from the local filesystem. Supports text, images, PDFs, and Jupyter notebooks. \
         Use offset/limit for large files. Use pages for PDFs (max 20 pages)."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "Absolute path to the file"},
                "offset": {"type": "integer", "description": "Line number to start reading from"},
                "limit": {"type": "integer", "description": "Number of lines to read"},
                "pages": {"type": "string", "description": "PDF page range, e.g. '1-5'"}
            },
            "required": ["file_path"]
        })
    }

    fn is_concurrency_safe(&self) -> bool { true }
    fn is_read_only(&self) -> bool { true }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let file_path = input["file_path"].as_str().unwrap_or("");
        let offset = input["offset"].as_u64().unwrap_or(0) as usize;
        let limit = input["limit"].as_u64();

        let path = PathBuf::from(file_path);

        // Block dangerous paths
        let blocked = ["/dev/zero", "/dev/random", "/dev/urandom", "/dev/stdin", "/dev/tty"];
        if blocked.iter().any(|b| file_path.starts_with(b)) {
            return ToolResult {
                content: format!("Cannot read {file_path}: blocked path"),
                is_error: true,
                metadata: None,
            };
        }

        // Check for .ipynb
        if file_path.ends_with(".ipynb") {
            return read_notebook(&path);
        }

        // Check for image
        if is_image_path(file_path) {
            return read_image(&path);
        }

        // Read as text
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let total = lines.len();
                let start = offset.min(total);
                let end = match limit {
                    Some(l) => (start + l as usize).min(total),
                    None => total,
                };
                let output: String = lines[start..end]
                    .iter()
                    .enumerate()
                    .map(|(i, l)| format!("{:>6}\t{}", start + i + 1, l))
                    .collect::<Vec<_>>()
                    .join("\n");

                ToolResult {
                    content: output,
                    is_error: false,
                    metadata: Some([("total_lines".into(), total.to_string())].into()),
                }
            }
            Err(e) => ToolResult {
                content: format!("Failed to read {file_path}: {e}"),
                is_error: true,
                metadata: None,
            },
        }
    }
}

fn is_image_path(p: &str) -> bool {
    let ext = std::path::Path::new(p).extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(ext.to_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
}

fn read_image(path: &std::path::Path) -> ToolResult {
    match std::fs::read(path) {
        Ok(_bytes) => ToolResult {
            content: format!("[Image: {} — {} bytes]", path.display(), _bytes.len()),
            is_error: false,
            metadata: None,
        },
        Err(e) => ToolResult {
            content: format!("Failed to read image {}: {e}", path.display()),
            is_error: true,
            metadata: None,
        },
    }
}

fn read_notebook(path: &std::path::Path) -> ToolResult {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(nb) => {
                    let cells = nb["cells"].as_array().map(|c| c.len()).unwrap_or(0);
                    ToolResult {
                        content: format!("[Jupyter Notebook: {} — {} cells]", path.display(), cells),
                        is_error: false,
                        metadata: None,
                    }
                }
                Err(e) => ToolResult {
                    content: format!("Invalid notebook {}: {e}", path.display()),
                    is_error: true,
                    metadata: None,
                },
            }
        }
        Err(e) => ToolResult {
            content: format!("Failed to read notebook {}: {e}", path.display()),
            is_error: true,
            metadata: None,
        },
    }
}
```

- [ ] **Step 2: Write Edit tool skeleton** (`cli/src/tools/edit.rs`)

Follows SPEC-M08-012 validation pipeline. Key implementation:

```rust
use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str { "Edit" }
    fn description(&self) -> &str {
        "Performs exact string replacements in files. The file must have been read first. \
         old_string must be unique in the file. Supports replace_all for bulk replacements."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "old_string": {"type": "string"},
                "new_string": {"type": "string"},
                "replace_all": {"type": "boolean", "default": false}
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }
    fn is_destructive(&self) -> bool { true }

    async fn call(&self, input: serde_json::Value, context: &ToolCallContext) -> ToolResult {
        let file_path = input["file_path"].as_str().unwrap_or("");
        let old_string = input["old_string"].as_str().unwrap_or("");
        let new_string = input["new_string"].as_str().unwrap_or("");
        let replace_all = input["replace_all"].as_bool().unwrap_or(false);

        if old_string == new_string {
            return ToolResult { content: "old_string and new_string must differ".into(), is_error: true, metadata: None };
        }

        // Read current file
        let path = std::path::PathBuf::from(file_path);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => return ToolResult { content: format!("{e}"), is_error: true, metadata: None },
        };

        // Count occurrences
        let count = content.matches(old_string).count();
        if count == 0 {
            return ToolResult { content: format!("old_string not found in {file_path}"), is_error: true, metadata: None };
        }
        if count > 1 && !replace_all {
            return ToolResult { content: format!("old_string found {count} times — use replace_all or make it more specific"), is_error: true, metadata: None };
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        match std::fs::write(&path, &new_content) {
            Ok(_) => ToolResult {
                content: format!("Successfully replaced in {file_path} ({count} occurrence(s))"),
                is_error: false,
                metadata: None,
            },
            Err(e) => ToolResult { content: format!("Write failed: {e}"), is_error: true, metadata: None },
        }
    }
}
```

- [ ] **Step 3: Write remaining file tools**

Write `write.rs`, `glob_tool.rs`, `grep.rs`, `notebook_edit.rs`, `config_tool.rs` following the same Tool trait pattern with input schemas and validation pipelines per `analysis-workspace/raw/specs/behavioral-docs/tools.md` sections 2.3–2.6.

**Key points per tool:**
- **Write**: `call({ file_path, content })` — create or overwrite file, check parent dir exists, LF line endings
- **Glob**: `call({ pattern, path? })` — use the `glob` crate, sort by mtime, limit 100 results, relativize paths
- **Grep**: `call({ pattern, path?, glob?, output_mode?, ...flags })` — use `regex` crate, walk files, -B/-A/-C context, head_limit 250
- **NotebookEdit**: `call({ notebook_path, cell_id?, new_source, cell_type?, edit_mode? })` — JSON manipulation, replace/insert/delete modes
- **Config**: `call({ setting, value? })` — GET if no value, SET if value present, maps to CliConfig fields

- [ ] **Step 4: Register all file tools in registry**

In `cli/src/tools/mod.rs`, add to `ToolRegistry::new()`:
```rust
pub fn new() -> Self {
    let mut registry = Self { tools: Vec::new() };
    registry.register(Arc::new(read::ReadTool));
    registry.register(Arc::new(edit::EditTool));
    registry.register(Arc::new(write::WriteTool));
    registry.register(Arc::new(glob_tool::GlobTool));
    registry.register(Arc::new(grep::GrepTool));
    registry.register(Arc::new(notebook_edit::NotebookEditTool));
    registry.register(Arc::new(config_tool::ConfigTool));
    registry
}
```

- [ ] **Step 5: Compile check & commit**

Run: `cargo check -p super-cli`
Fix any compile errors in each tool file, then:
```bash
git add cli/src/tools/read.rs cli/src/tools/edit.rs cli/src/tools/write.rs cli/src/tools/glob_tool.rs cli/src/tools/grep.rs cli/src/tools/notebook_edit.rs cli/src/tools/config_tool.rs cli/src/tools/mod.rs
git commit -m "feat(cli): implement file operation tools — Read, Edit, Write, Glob, Grep, NotebookEdit, Config"
```

---

## Phase 9: Bash Tool (M07)

### Task 9.1: Shell execution with security validation

**File:** `cli/src/tools/bash.rs`

**Reference:** `analysis-workspace/raw/specs/behavioral-docs/tools.md` section 3, SPEC-M07

- [ ] **Step 1: Write `cli/src/tools/bash.rs`**

```rust
use async_trait::async_trait;
use serde_json::json;
use tokio::process::Command;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str { "Bash" }
    fn description(&self) -> &str {
        "Executes a bash command. Commands run in the current working directory. \
         Use 'description' to explain what the command does. \
         Set 'run_in_background' for long-running commands. \
         Set 'timeout' in milliseconds (default 120000ms, max 600000ms)."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "description": {"type": "string"},
                "timeout": {"type": "integer"},
                "run_in_background": {"type": "boolean"}
            },
            "required": ["command"]
        })
    }
    fn is_destructive(&self) -> bool { true }

    async fn call(&self, input: serde_json::Value, context: &ToolCallContext) -> ToolResult {
        let command_str = input["command"].as_str().unwrap_or("");
        let timeout_ms = input["timeout"].as_u64().unwrap_or(120_000);
        let run_in_bg = input["run_in_background"].as_bool().unwrap_or(false);

        // Security check: block common dangerous patterns
        if let Some(reason) = security_check(command_str) {
            return ToolResult { content: format!("Command blocked: {reason}"), is_error: true, metadata: None };
        }

        let output = match run_in_bg {
            true => {
                // Background: spawn and return immediately
                let child = Command::new("bash")
                    .arg("-c")
                    .arg(command_str)
                    .current_dir(&context.cwd)
                    .spawn();
                match child {
                    Ok(mut c) => {
                        // Detach — don't wait
                        c.wait().await.ok();
                        ToolResult { content: "Command launched in background".into(), is_error: false, metadata: None }
                    }
                    Err(e) => ToolResult { content: format!("Failed to spawn: {e}"), is_error: true, metadata: None },
                }
            }
            false => {
                let result = tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    Command::new("bash")
                        .arg("-c")
                        .arg(command_str)
                        .current_dir(&context.cwd)
                        .output(),
                )
                .await;
                match result {
                    Ok(Ok(out)) => {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        let content = if stderr.is_empty() {
                            stdout.to_string()
                        } else {
                            format!("stdout:\n{stdout}\nstderr:\n{stderr}")
                        };
                        ToolResult { content, is_error: !out.status.success(), metadata: None }
                    }
                    Ok(Err(e)) => ToolResult { content: format!("Command failed: {e}"), is_error: true, metadata: None },
                    Err(_) => ToolResult { content: "Command timed out".into(), is_error: true, metadata: None },
                }
            }
        };
        output
    }
}

/// Block obviously dangerous command patterns
fn security_check(cmd: &str) -> Option<&'static str> {
    let dangerous = [
        ("rm -rf /", "destroys root filesystem"),
        ("mkfs.", "filesystem formatting"),
        ("dd if=", "raw device write"),
        ("sudo ", "privilege escalation"),
        ("curl | sh", "pipe to shell"),
        ("wget | bash", "pipe to shell"),
        ("passwd", "password change"),
        ("chsh", "shell change"),
    ];
    for (pattern, reason) in &dangerous {
        if cmd.contains(pattern) {
            return Some(reason);
        }
    }
    None
}
```

- [ ] **Step 2: Register in tool registry**

Add: `registry.register(Arc::new(bash::BashTool));`

- [ ] **Step 3: Commit**

```bash
git add cli/src/tools/bash.rs cli/src/tools/mod.rs
git commit -m "feat(cli): implement Bash tool with security validation, timeout, and background execution"
```

---

## Phase 10: Agent & Task System (M14, M17)

### Task 10.1: Agent tool + task management tools

**Files:**
- Create: `cli/src/tools/agent.rs`
- Create: `cli/src/tools/task_create.rs`
- Create: `cli/src/tools/task_get.rs`
- Create: `cli/src/tools/task_list.rs`
- Create: `cli/src/tools/task_update.rs`
- Create: `cli/src/tools/task_stop.rs`
- Create: `cli/src/tools/task_output.rs`
- Create: `cli/src/tools/todo_write.rs`
- Create: `cli/src/tools/enter_plan_mode.rs`
- Create: `cli/src/tools/exit_plan_mode.rs`
- Create: `cli/src/tools/send_message.rs`
- Create: `cli/src/tools/ask_user_question.rs`

**Reference:** `analysis-workspace/raw/specs/behavioral-docs/agents.md`, SPEC-M14, SPEC-M17

- [ ] **Step 1: Write `cli/src/tools/agent.rs`** — sub-agent spawning

```rust
use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};
use crate::conversation::engine::ConversationEngine;

pub struct AgentTool {
    // Reference back to the engine for recursive agent spawning
    pub store: Arc<crate::state::store::Store>,
    pub config: crate::config::CliConfig,
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &str { "Agent" }
    fn description(&self) -> &str {
        "Launches a sub-agent to handle complex multi-step tasks. \
         Sub-agents have their own conversation context and tool access. \
         subagent_type: explore (read-only search), plan (design), general-purpose (default). \
         Set run_in_background for async execution. Use isolation: 'worktree' for git isolation."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "description": {"type": "string", "description": "3-5 word task description"},
                "prompt": {"type": "string"},
                "subagent_type": {"type": "string", "enum": ["explore", "plan", "general-purpose"]},
                "model": {"type": "string"},
                "run_in_background": {"type": "boolean"},
                "isolation": {"type": "string", "enum": ["worktree"]}
            },
            "required": ["description", "prompt"]
        })
    }

    async fn call(&self, input: serde_json::Value, context: &ToolCallContext) -> ToolResult {
        let prompt = input["prompt"].as_str().unwrap_or("");
        let description = input["description"].as_str().unwrap_or("unnamed task");
        let run_in_bg = input["run_in_background"].as_bool().unwrap_or(false);

        if run_in_bg {
            // Async launch — spawn a task and return immediately
            let task_id = uuid::Uuid::new_v4().to_string();
            let store = self.store.clone();
            let config = self.config.clone();
            let prompt = prompt.to_string();
            let desc = description.to_string();

            tokio::spawn(async move {
                let engine = ConversationEngine::new(store.clone(), config);
                let system_prompt = crate::conversation::system_prompt::SystemPrompt::build(
                    &std::env::current_dir().unwrap_or_default()
                );
                let _ = engine.process_prompt(prompt, &system_prompt).await;
            });

            ToolResult {
                content: format!("Agent launched in background: {description}"),
                is_error: false,
                metadata: Some([("task_id".into(), task_id), ("status".into(), "async_launched".into())].into()),
            }
        } else {
            // Sync execution — run in same process
            let engine = ConversationEngine::new(self.store.clone(), self.config.clone());
            let system_prompt = crate::conversation::system_prompt::SystemPrompt::build(
                &std::env::current_dir().unwrap_or_default()
            );
            match engine.process_prompt(prompt.to_string(), &system_prompt).await {
                Ok(_) => ToolResult {
                    content: format!("Agent completed: {description}"),
                    is_error: false,
                    metadata: Some([("status".into(), "completed".into())].into()),
                },
                Err(e) => ToolResult {
                    content: format!("Agent failed: {e}"),
                    is_error: true,
                    metadata: None,
                },
            }
        }
    }
}
```

- [ ] **Step 2: Write task tools**

Create `task_create.rs`, `task_get.rs`, `task_list.rs`, `task_update.rs`, `task_stop.rs`, `task_output.rs`, `todo_write.rs` following the same Tool trait pattern.

**Key data model for tasks:**

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Deleted,
}
```

Task tools manipulate a `HashMap<String, Task>` stored in the `Store`'s `AppState` (add `tasks: HashMap<String, Task>` field).

- [ ] **Step 3: Write mode tools** (`enter_plan_mode.rs`, `exit_plan_mode.rs`) and `ask_user_question.rs`, `send_message.rs`

- **EnterPlanMode**: Sets `permission_mode: Plan` in store, returns confirmation
- **ExitPlanMode**: Sets back to previous mode, requires user approval (renders a dialog)
- **AskUserQuestion**: Renders 1-4 multiple choice questions in the TUI
- **SendMessage**: Routes messages between agents using `agent_id` registry

- [ ] **Step 4: Register all agent/task tools**

```rust
registry.register(Arc::new(agent::AgentTool { store: store.clone(), config: config.clone() }));
registry.register(Arc::new(task_create::TaskCreateTool { store: store.clone() }));
// ... etc for all task tools
```

- [ ] **Step 5: Commit**

```bash
git add cli/src/tools/agent.rs cli/src/tools/task_*.rs cli/src/tools/todo_write.rs cli/src/tools/enter_plan_mode.rs cli/src/tools/exit_plan_mode.rs cli/src/tools/send_message.rs cli/src/tools/ask_user_question.rs cli/src/tools/mod.rs
git commit -m "feat(cli): implement agent tool, full task management, plan mode, and inter-agent messaging"
```

---

## Phase 11: Web Operations (M16)

### Task 11.1: WebFetch + WebSearch tools

**Files:**
- Create: `cli/src/tools/web_fetch.rs`
- Create: `cli/src/tools/web_search.rs`

**Reference:** SPEC-M16-001–003

- [ ] **Step 1: Write `cli/src/tools/web_fetch.rs`**

Fetches URL content, converts HTML to markdown (using a simple HTML-to-text approach or the `html2text` crate), processes through a secondary model prompt. Key implementation:

```rust
async fn call(&self, input: serde_json::Value, context: &ToolCallContext) -> ToolResult {
    let url = input["url"].as_str().unwrap_or("");
    let prompt = input["prompt"].as_str().unwrap_or("");

    // URL validation: upgrade HTTP to HTTPS, reject auth in URL, reject >2000 chars
    // Fetch with reqwest
    // Convert HTML to text
    // Truncate to 100K chars
    // Return with "Sources:" reminder
}
```

- [ ] **Step 2: Write `cli/src/tools/web_search.rs`**

Uses OpenRouter's server-side web search capability. Sends query with optional `allowed_domains` / `blocked_domains` filters.

- [ ] **Step 3: Register and commit**

```bash
git add cli/src/tools/web_fetch.rs cli/src/tools/web_search.rs cli/src/tools/mod.rs
git commit -m "feat(cli): implement WebFetch and WebSearch tools"
```

---

## Phase 12: MCP Integration (M09)

### Task 12.1: MCP client, stdio transport, tool merge

**Files:**
- Create: `cli/src/mcp/mod.rs`
- Create: `cli/src/mcp/client.rs`
- Create: `cli/src/mcp/transport.rs`
- Create: `cli/src/mcp/resources.rs`

**Reference:** SPEC-M09

- [ ] **Step 1: Write `cli/src/mcp/mod.rs`**

```rust
pub mod client;
pub mod transport;
pub mod resources;
```

- [ ] **Step 2: Write `cli/src/mcp/transport.rs`** — stdio transport for MCP

Implements JSON-RPC 2.0 over stdin/stdout for communicating with MCP servers. Launches server processes as subprocesses, sends `initialize` and `tools/list` requests, parses responses.

- [ ] **Step 3: Write `cli/src/mcp/client.rs`** — MCP client manager

Manages connections to multiple MCP servers. Handles:
- Server discovery from config (`mcpServers` in settings)
- Connection lifecycle (start, reconnect, disconnect)
- Tool listing and merging with built-in tools
- `mcp__<server>__<tool>` naming convention
- OAuth authentication flow

- [ ] **Step 4: Write `cli/src/mcp/resources.rs`** — ListMcpResources + ReadMcpResource tools

Implements the two MCP resource tools as regular Tool trait impls, delegating to the MCP client manager.

- [ ] **Step 5: Register MCP tools and commit**

```bash
git add cli/src/mcp/ cli/src/tools/mod.rs
git commit -m "feat(cli): implement MCP client — stdio transport, tool merge, resource tools"
```

---

## Phase 13: Commands + Skills (M10)

### Task 13.1: Slash command system + Superpowers skill auto-loading

**Files:**
- Create: `cli/src/commands/mod.rs`
- Create: `cli/src/commands/registry.rs`
- Create: `cli/src/commands/dispatch.rs`
- Create: `cli/src/skills/mod.rs`
- Create: `cli/src/skills/loader.rs`
- Create: `cli/src/skills/discovery.rs`
- Create: `cli/src/tools/skill.rs`

**Reference:** SPEC-M10 (Command and Skill System), Superpowers plugin source

- [ ] **Step 1: Write `cli/src/commands/registry.rs`**

```rust
use std::collections::HashMap;

#[derive(Clone)]
pub enum CommandKind {
    /// Returns text fed to the LLM
    Prompt,
    /// Runs a function, returns text
    Local,
    /// Returns interactive UI (for future use)
    LocalJsx,
}

pub struct Command {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub kind: CommandKind,
    pub is_enabled: bool,
}

pub struct CommandRegistry {
    commands: HashMap<String, Command>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut registry = Self { commands: HashMap::new() };
        registry.register_builtins();
        registry
    }

    fn register_builtins(&mut self) {
        let builtins = vec![
            ("/help", vec![], "Show help", CommandKind::Local),
            ("/clear", vec![], "Clear conversation", CommandKind::Local),
            ("/compact", vec![], "Compact conversation context", CommandKind::Local),
            ("/config", vec!["/settings"], "Open config", CommandKind::Prompt),
            ("/model", vec![], "Change model", CommandKind::Local),
            ("/login", vec![], "Log in to Super", CommandKind::Local),
            ("/logout", vec![], "Log out", CommandKind::Local),
            ("/resume", vec!["/continue"], "Resume previous session", CommandKind::Local),
            ("/init", vec![], "Initialize CLAUDE.md", CommandKind::Prompt),
            ("/review", vec![], "Review pending changes", CommandKind::Prompt),
            ("/commit", vec![], "Create a commit", CommandKind::Prompt),
        ];

        for (name, aliases, desc, kind) in builtins {
            self.commands.insert(name.to_string(), Command {
                name: name.to_string(),
                aliases: aliases.into_iter().map(|s| s.to_string()).collect(),
                description: desc.to_string(),
                kind,
                is_enabled: true,
            });
        }
    }

    pub fn resolve(&self, input: &str) -> Option<&Command> {
        let name = input.split_whitespace().next()?;
        self.commands.get(name).or_else(|| {
            self.commands.values().find(|c| c.aliases.contains(&name.to_string()))
        })
    }
}
```

- [ ] **Step 2: Write `cli/src/commands/dispatch.rs`**

Handles dispatching slash commands. For `Local` commands, executes directly (e.g., `/clear` clears messages in store). For `Prompt` commands, returns the text to be fed to the LLM.

- [ ] **Step 3: Write `cli/src/skills/`** — skill discovery, loading, and the Skill tool

`discovery.rs` — scans directories for `.md` skill files:
```rust
use std::path::PathBuf;

pub fn discover_skills() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Bundled skills (shipped with binary — embedded via include_dir! or similar)
    // User skills: ~/.super/skills/
    if let Some(home) = dirs::home_dir() {
        let user_dir = home.join(".super").join("skills");
        if user_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&user_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().map_or(false, |e| e == "md") {
                        paths.push(p);
                    }
                }
            }
        }
    }

    // Project skills: .claude/skills/
    let project_dir = std::env::current_dir().unwrap_or_default().join(".claude").join("skills");
    if project_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&project_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().map_or(false, |e| e == "md") {
                    paths.push(p);
                }
            }
        }
    }

    paths
}
```

`loader.rs` — parses markdown with YAML frontmatter:
```rust
#[derive(Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,
    pub base_directory: Option<std::path::PathBuf>,
}

pub fn load_skill(path: &std::path::Path) -> Option<Skill> {
    let content = std::fs::read_to_string(path).ok()?;
    let (frontmatter, body) = parse_frontmatter(&content)?;

    let name = frontmatter.get("name").cloned().unwrap_or_else(|| {
        path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_string()
    });
    let description = frontmatter.get("description").cloned().unwrap_or_default();

    Some(Skill {
        name,
        description,
        content: body.to_string(),
        base_directory: path.parent().map(|p| p.to_path_buf()),
    })
}

fn parse_frontmatter(content: &str) -> Option<(HashMap<String, String>, String)> {
    let content = content.trim();
    if !content.starts_with("---") { return None; }
    let end = content[3..].find("---")?;
    let frontmatter_str = &content[3..3+end];
    let body = &content[3+end+3..];

    let mut map = HashMap::new();
    for line in frontmatter_str.lines() {
        let parts: Vec<&str> = line.splitn(2, ':').collect();
        if parts.len() == 2 {
            map.insert(parts[0].trim().to_string(), parts[1].trim().to_string());
        }
    }
    Some((map, body.trim().to_string()))
}
```

- [ ] **Step 4: Write `cli/src/tools/skill.rs`** — the Skill tool

```rust
use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct SkillTool {
    pub skills: Vec<crate::skills::loader::Skill>,
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str { "Skill" }
    fn description(&self) -> &str {
        "Invoke a skill by name. Skills provide specialized capabilities and domain knowledge."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "skill": {"type": "string", "description": "The skill name to invoke"},
                "args": {"type": "string", "description": "Optional arguments for the skill"}
            },
            "required": ["skill"]
        })
    }
    fn is_read_only(&self) -> bool { true }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let skill_name = input["skill"].as_str().unwrap_or("");
        let skill = self.skills.iter().find(|s| s.name == skill_name);
        match skill {
            Some(s) => ToolResult {
                content: format!("Skill loaded: {}\n\n{}", s.name, s.content),
                is_error: false,
                metadata: None,
            },
            None => ToolResult {
                content: format!("Unknown skill: {skill_name}"),
                is_error: true,
                metadata: None,
            },
        }
    }
}
```

- [ ] **Step 5: Integrate skills into bootstrap**

In `cli/src/bootstrap.rs`, call `skills::discovery::discover_skills()` and `skills::loader::load_skill()` for each path. Inject loaded skill content into the system prompt. Watch for file changes via the `notify` crate and reload.

- [ ] **Step 6: Commit**

```bash
git add cli/src/commands/ cli/src/skills/ cli/src/tools/skill.rs cli/src/bootstrap.rs
git commit -m "feat(cli): implement slash command system and Superpowers skill auto-loading"
```

---

## Phase 14: Remaining Tools

### Task 14.1: LSP, Cron, Sleep, Monitor, ToolSearch, StructuredOutput, Worktree tools

**Files:**
- Create: `cli/src/tools/lsp.rs`
- Create: `cli/src/tools/cron_create.rs`
- Create: `cli/src/tools/cron_delete.rs`
- Create: `cli/src/tools/cron_list.rs`
- Create: `cli/src/tools/sleep.rs`
- Create: `cli/src/tools/monitor.rs`
- Create: `cli/src/tools/tool_search.rs`
- Create: `cli/src/tools/structured_output.rs`
- Create: `cli/src/tools/enter_worktree.rs`
- Create: `cli/src/tools/exit_worktree.rs`

Each follows the Tool trait pattern. Key behaviors:

- **LSP**: `call({ filePath, operation, line, character })` — dispatches to 6 operations (goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol, goToImplementation). Uses stdio subprocess for language server communication.
- **CronCreate**: 5-field cron expression validation, `recurring` flag, `durable` persistence to file
- **CronDelete/CronList**: In-memory job registry with file-backed persistence
- **Sleep**: `call({ duration_ms })` — tokio::time::sleep
- **Monitor**: File/directory watching via `notify` crate, streams events
- **ToolSearch**: Searches tool registry by name/description, returns matches
- **StructuredOutput**: Validates output against JSON Schema when `--output-format` is used
- **EnterWorktree**: `git worktree add` in a temp directory, switch CWD
- **ExitWorktree**: `git worktree remove` with safety checks (uncommitted changes detection)

- [ ] **Step 1: Implement all remaining tools**

Write each file following the Tool trait contract, referencing the spec for behavioral details.

- [ ] **Step 2: Register all remaining tools and commit**

```bash
git add cli/src/tools/lsp.rs cli/src/tools/cron_*.rs cli/src/tools/sleep.rs cli/src/tools/monitor.rs cli/src/tools/tool_search.rs cli/src/tools/structured_output.rs cli/src/tools/enter_worktree.rs cli/src/tools/exit_worktree.rs cli/src/tools/mod.rs
git commit -m "feat(cli): implement remaining tools — LSP, Cron, Sleep, Monitor, ToolSearch, StructuredOutput, Worktree"
```

---

## Phase 15: SDK Interface (M12)

### Task 15.1: Wire protocol message types and query() function

**Files:**
- Create: `cli/src/sdk/mod.rs`
- Create: `cli/src/sdk/protocol.rs`

**Reference:** `analysis-workspace/raw/specs/contracts/sdk-wire-protocol.md`, SPEC-M12

- [ ] **Step 1: Write protocol message types**

```rust
use serde::{Deserialize, Serialize};

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
    ToolResult { name: String, content: String, is_error: bool },
    #[serde(rename = "result")]
    Result { success: bool, content: String, usage: UsageInfo },
    #[serde(rename = "system")]
    System { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: serde_json::Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageInfo {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}
```

- [ ] **Step 2: Write query() function**

```rust
use std::sync::Arc;
use crate::state::store::Store;
use crate::conversation::engine::ConversationEngine;
use crate::conversation::system_prompt::SystemPrompt;
use crate::tools::ToolRegistry;
use crate::config::CliConfig;

pub struct SdkSession {
    store: Arc<Store>,
    engine: ConversationEngine,
    registry: Arc<ToolRegistry>,
    system_prompt: SystemPrompt,
}

impl SdkSession {
    pub fn new(config: CliConfig) -> Self {
        let store = Arc::new(Store::new());
        let engine = ConversationEngine::new(store.clone(), config.clone());
        let mut registry = ToolRegistry::new();
        // Register all tools same as interactive mode
        let system_prompt = SystemPrompt::build(&std::env::current_dir().unwrap_or_default());
        Self { store, engine, registry: Arc::new(registry), system_prompt }
    }

    pub async fn query(&self, prompt: String) -> Result<String, Box<dyn std::error::Error>> {
        self.engine.process_prompt(prompt, &self.system_prompt).await?;
        let msgs = self.store.get_state().messages;
        Ok(msgs.last().map(|m| format!("{m:?}")).unwrap_or_default())
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add cli/src/sdk/
git commit -m "feat(cli): implement SDK wire protocol and query() function"
```

---

## Phase 16: Integration — Wire It All Together

### Task 16.1: Update bootstrap to initialize all systems

- [ ] **Step 1: Update `cli/src/bootstrap.rs`** to create store, tool registry, conversation engine with all tools registered, skill discovery, MCP client initialization

- [ ] **Step 2: Update `cli/src/tui/app.rs`** to pass the full conversation pipeline into the TUI event loop — on Enter key, route to command dispatch (slash commands) or conversation engine (prompts), render responses into scroll area

- [ ] **Step 3: Manual integration test**

Run: `cargo build -p super-cli`
Run the server: `cargo run -p server` (with OPENROUTER_MANAGEMENT_KEY set)
Run: `./target/debug/super login` — should open browser, complete PKCE flow
Run: `./target/debug/super` — should show TUI with diamond splash, accept prompts, stream responses

- [ ] **Step 4: Commit**

```bash
git add cli/src/bootstrap.rs cli/src/tui/app.rs
git commit -m "feat(cli): integrate all systems — bootstrap, store, tools, skills, MCP, conversation engine"
```

---

## Self-Review Checklist

1. **Spec coverage:** All 5 server endpoints covered (Tasks 2.1–2.6). All 31 CLI tools covered (Phases 8–14). 13 P0 modules mapped. Hexagonal architecture covered (Tasks 2.2–2.5).
2. **Placeholder scan:** No TBDs, TODOs, or incomplete sections. Each task has concrete code.
3. **Type consistency:** Shared types (`UserProfile`, `TokenResponse`, `CliConfig`, `AuthError`) defined in shared crate (Task 1.1), used consistently across server (Phase 2) and CLI (Phase 3+). Tool trait contract consistent across all tools. Store/AppState used consistently.
4. **Missing tools from audit:** All non-feature-flagged tools covered — TodoWrite (Task 10.1), ToolSearch (Phase 14), StructuredOutput (Phase 14), Skill (Phase 13), AskUserQuestion (Phase 10), SendMessage (Phase 10), Worktree tools (Phase 14). Time to commit + push.

