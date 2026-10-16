//! SUPER platform sign-in: RFC 8628 device flow, token refresh, the
//! `/api/models` catalog, and sign-out.
//!
//! Everything here runs only when the caller asks for it. Without a stored
//! `super` credential no function makes a network request, so a signed-out
//! `super` behaves exactly as before.

use super::OAuthCredential;
use super::device_code::{PollResult, poll};
use crate::model::Model;
use anyhow::{Context as _, Result};
use serde_json::Value;
use std::path::Path;
use std::sync::RwLock;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use url::Url;

/// Provider ID of the SUPER platform in the auth file and model catalog.
pub const PROVIDER: &str = "super";
/// Provider name shown in `/login` and `/model`.
pub const DISPLAY_NAME: &str = "SUPER";
/// Platform base URL when neither the setting nor the variable is set.
pub const DEFAULT_URL: &str = "https://super.flxkpe.io";
/// Environment variable that overrides the `platform.url` setting.
pub const URL_ENV: &str = "SUPER_PLATFORM_URL";
/// Notice shown when the stored sign-in can no longer be refreshed.
pub const SIGN_IN_EXPIRED: &str = "SUPER sign-in expired. /login to restore remote control.";

const CLIENT_ID: &str = "super-cli";
const SCOPE: &str = "super";
const DEVICE_KIND: &str = "cli";
const MODELS_API: &str = "openai-completions";

/// The `platform.url` setting, handed over by the application at startup.
static CONFIGURED_URL: RwLock<Option<String>> = RwLock::new(None);

/// Record the `platform.url` setting for this process. `None` clears it.
pub fn configure_url(url: Option<&str>) {
    let value = url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    *CONFIGURED_URL
        .write()
        .unwrap_or_else(|error| error.into_inner()) = value;
}

/// Pick the platform base URL: the environment variable, then the setting,
/// then the default. A trailing slash is removed.
pub fn resolve_base_url(setting: Option<&str>, environment: Option<&str>) -> String {
    environment
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| setting.map(str::trim).filter(|value| !value.is_empty()))
        .unwrap_or(DEFAULT_URL)
        .trim_end_matches('/')
        .to_string()
}

/// The platform base URL for this process.
pub fn base_url() -> String {
    let configured = CONFIGURED_URL
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let environment = std::env::var(URL_ENV).ok();
    resolve_base_url(configured.as_deref(), environment.as_deref())
}

/// Host of the platform base URL, shown as the `/login` subtitle.
pub fn host_label() -> String {
    let base = base_url();
    Url::parse(&base)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or(base)
}

#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub base_url: String,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        Self {
            base_url: base_url(),
        }
    }
}

impl OAuthConfig {
    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url.trim_end_matches('/'))
    }

    /// Default inference base URL for models that do not carry their own.
    pub fn inference_url(&self) -> String {
        self.url("/v1")
    }
}

#[derive(Debug, Clone)]
pub struct DeviceAuthorization {
    pub device_code: String,
    /// Raw code as issued by the platform.
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub interval: Duration,
    pub expires_in: Duration,
}

impl DeviceAuthorization {
    /// The code in the `XXXX-XXXX` form the platform page expects.
    pub fn display_code(&self) -> String {
        format_user_code(&self.user_code)
    }

    /// URL to open in a browser: the complete form when the platform sent one.
    pub fn browser_url(&self) -> &str {
        self.verification_uri_complete
            .as_deref()
            .unwrap_or(&self.verification_uri)
    }
}

/// Signed-in account from `GET /api/me`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub email: String,
}

/// A completed sign-in: the credential to store and who signed in.
#[derive(Debug, Clone)]
pub struct SignIn {
    pub credential: OAuthCredential,
    pub account: Account,
}

/// Result of one model catalog synchronisation.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncOutcome {
    /// No `super` credential is stored. Nothing was requested or written.
    SignedOut,
    /// The catalog now holds these `super` models.
    Synced(Vec<Model>),
}

/// Result of a sign-out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignOut {
    /// Whether a `super` credential existed before.
    pub had_credential: bool,
    /// The platform could not be told to revoke the device. The local
    /// credential and models are removed regardless.
    pub revoke_error: Option<String>,
}

/// Format a user code as `XXXX-XXXX`. Other lengths are returned trimmed.
pub fn format_user_code(code: &str) -> String {
    let cleaned: String = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_uppercase())
        .collect();
    if cleaned.len() == 8 {
        format!("{}-{}", &cleaned[..4], &cleaned[4..])
    } else {
        code.trim().to_string()
    }
}

/// The name this device registers with: the machine's host name without its
/// domain (`mbp`, not `mbp.local`), like the registry host identity.
pub fn device_name() -> String {
    for variable in ["HOSTNAME", "COMPUTERNAME"] {
        if let Ok(value) = std::env::var(variable)
            && !value.trim().is_empty()
        {
            return short_host_name(&value);
        }
    }
    if let Ok(text) = std::fs::read_to_string("/etc/hostname")
        && !text.trim().is_empty()
    {
        return short_host_name(&text);
    }
    if let Ok(output) = std::process::Command::new("hostname").output()
        && output.status.success()
    {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            return short_host_name(&name);
        }
    }
    "super".into()
}

fn short_host_name(name: &str) -> String {
    let name = name.trim();
    let label = name.split('.').next().unwrap_or(name).trim();
    if label.is_empty() {
        name.to_string()
    } else {
        label.to_string()
    }
}

/// `macos`, `linux`, or `windows` for the platform's device record.
pub fn device_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

/// The stored `super` credential, if the user is signed in.
pub fn stored_credential() -> Option<OAuthCredential> {
    super::stored_oauth(PROVIDER)
}

/// The platform device ID of the stored sign-in.
pub fn stored_device_id() -> Option<String> {
    stored_credential().and_then(|credential| credential.device_id)
}

/// Whether a `super` credential is stored. No network access.
pub fn is_signed_in() -> bool {
    stored_credential().is_some()
}

async fn post_form(
    url: &str,
    fields: &[(&str, &str)],
    cancel: &CancellationToken,
) -> Result<(reqwest::StatusCode, Value)> {
    let response = tokio::select! {
        response = crate::stream::http_client()
            .post(url)
            .header("accept", "application/json")
            .form(fields)
            .send() => response.with_context(|| format!("request {url}"))?,
        _ = cancel.cancelled() => anyhow::bail!("login cancelled"),
    };
    let status = response.status();
    let body = response.json::<Value>().await.unwrap_or(Value::Null);
    Ok((status, body))
}

fn required(body: &Value, name: &str) -> Result<String> {
    body[name]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .with_context(|| format!("SUPER platform response has no {name}"))
}

fn trusted_url(value: &str) -> Result<String> {
    let url = Url::parse(value).context("SUPER platform returned an invalid verification URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("SUPER platform returned an untrusted verification URL");
    }
    Ok(url.into())
}

fn oauth_error(status: reqwest::StatusCode, body: &Value) -> String {
    match body["error"].as_str() {
        Some(error) => match body["error_description"].as_str() {
            Some(description) => format!("{error}: {description}"),
            None => error.to_string(),
        },
        None => format!("HTTP {status}"),
    }
}

/// Build the credential from a token response. `previous` carries the
/// account and device of the sign-in being refreshed.
fn credential(body: &Value, previous: Option<&OAuthCredential>) -> Result<OAuthCredential> {
    let expires_in = body["expires_in"].as_i64().unwrap_or(3600);
    Ok(OAuthCredential {
        kind: "oauth".into(),
        access: required(body, "access_token")?,
        refresh: body["refresh_token"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| previous.map(|previous| previous.refresh.clone()))
            .context("SUPER platform response has no refresh_token")?,
        expires: chrono::Utc::now().timestamp_millis() + expires_in.saturating_mul(1000),
        account_id: previous
            .map(|previous| previous.account_id.clone())
            .unwrap_or_default(),
        client_id: None,
        available_model_ids: None,
        device_id: body["device_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| previous.and_then(|previous| previous.device_id.clone())),
    })
}

/// Ask the platform for a device code.
pub async fn start(
    config: &OAuthConfig,
    cancel: &CancellationToken,
) -> Result<DeviceAuthorization> {
    let device_name = device_name();
    let (status, body) = post_form(
        &config.url("/oauth/device/code"),
        &[
            ("client_id", CLIENT_ID),
            ("scope", SCOPE),
            ("device_name", &device_name),
            ("device_kind", DEVICE_KIND),
            ("device_platform", device_platform()),
        ],
        cancel,
    )
    .await?;
    if !status.is_success() {
        anyhow::bail!(
            "SUPER device authorization failed ({})",
            oauth_error(status, &body)
        );
    }
    Ok(DeviceAuthorization {
        device_code: required(&body, "device_code")?,
        user_code: required(&body, "user_code")?,
        verification_uri: trusted_url(&required(&body, "verification_uri")?)?,
        verification_uri_complete: body["verification_uri_complete"]
            .as_str()
            .and_then(|value| trusted_url(value).ok()),
        interval: Duration::from_secs(body["interval"].as_u64().unwrap_or(5).max(1)),
        expires_in: Duration::from_secs(body["expires_in"].as_u64().unwrap_or(900)),
    })
}

/// Poll until the user approves the device. The credential has no account
/// ID yet; `login` fills it in.
pub async fn finish(
    config: &OAuthConfig,
    device: &DeviceAuthorization,
    cancel: &CancellationToken,
) -> Result<OAuthCredential> {
    poll(device.interval, device.expires_in, true, cancel, || async {
        let (status, body) = post_form(
            &config.url("/oauth/device/token"),
            &[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", &device.device_code),
                ("client_id", CLIENT_ID),
            ],
            cancel,
        )
        .await?;
        if status.is_success() {
            return Ok(PollResult::Complete(credential(&body, None)?));
        }
        Ok(match body["error"].as_str() {
            Some("authorization_pending") => PollResult::Pending,
            Some("slow_down") => {
                PollResult::SlowDown(body["interval"].as_u64().map(Duration::from_secs))
            }
            Some("expired_token") => PollResult::Failed(
                "the SUPER device code expired before it was approved. Run /login again".into(),
            ),
            Some("access_denied") => {
                PollResult::Failed("the SUPER device approval was denied".into())
            }
            _ => PollResult::Failed(format!(
                "SUPER device login failed ({})",
                oauth_error(status, &body)
            )),
        })
    })
    .await
}

/// Who the access token belongs to.
pub async fn me(config: &OAuthConfig, access: &str, cancel: &CancellationToken) -> Result<Account> {
    let url = config.url("/api/me");
    let response = tokio::select! {
        response = crate::stream::http_client()
            .get(&url)
            .bearer_auth(access)
            .header("accept", "application/json")
            .send() => response.with_context(|| format!("request {url}"))?,
        _ = cancel.cancelled() => anyhow::bail!("login cancelled"),
    };
    let status = response.status();
    let body = response.json::<Value>().await.unwrap_or(Value::Null);
    if !status.is_success() {
        anyhow::bail!("SUPER account lookup failed (HTTP {status})");
    }
    Ok(Account {
        id: required(&body["user"], "id")?,
        email: body["user"]["email"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    })
}

/// Wait for approval, then look up the account. The returned credential is
/// ready to store under [`PROVIDER`].
pub async fn login(
    config: &OAuthConfig,
    device: &DeviceAuthorization,
    cancel: &CancellationToken,
) -> Result<SignIn> {
    let mut credential = finish(config, device, cancel).await?;
    let account = me(config, &credential.access, cancel).await?;
    credential.account_id = account.id.clone();
    Ok(SignIn {
        credential,
        account,
    })
}

/// Exchange the refresh token. The platform rotates it, so the result must
/// replace the stored credential.
pub async fn refresh(
    credential_value: &OAuthCredential,
    config: &OAuthConfig,
) -> Result<OAuthCredential> {
    let cancel = CancellationToken::new();
    let (status, body) = post_form(
        &config.url("/oauth/token"),
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &credential_value.refresh),
            ("client_id", CLIENT_ID),
        ],
        &cancel,
    )
    .await?;
    if status.is_client_error() {
        anyhow::bail!("{SIGN_IN_EXPIRED}");
    }
    if !status.is_success() {
        anyhow::bail!(
            "SUPER token refresh failed ({})",
            oauth_error(status, &body)
        );
    }
    credential(&body, Some(credential_value))
}

/// Tell the platform to revoke this sign-in's tokens.
pub async fn revoke(config: &OAuthConfig, credential: &OAuthCredential) -> Result<()> {
    let cancel = CancellationToken::new();
    let (status, body) = post_form(
        &config.url("/oauth/revoke"),
        &[("token", &credential.refresh), ("client_id", CLIENT_ID)],
        &cancel,
    )
    .await?;
    if !status.is_success() {
        anyhow::bail!(
            "SUPER token revocation failed ({})",
            oauth_error(status, &body)
        );
    }
    Ok(())
}

/// Normalise a catalog entry from `/api/models` so it streams through the
/// chat-completions adapter with the SUPER credential.
fn normalize_model(mut model: Model, config: &OAuthConfig) -> Model {
    model.provider = PROVIDER.into();
    if model.api.trim().is_empty() {
        model.api = MODELS_API.into();
    }
    if model.base_url.trim().is_empty() {
        model.base_url = config.inference_url();
    }
    if model.name.trim().is_empty() {
        model.name = model.id.clone();
    }
    if model.reasoning {
        // The gateway accepts `reasoning_effort`; the adapter only sends it
        // to hosts it recognises unless the model says so.
        let compat = model.compat.get_or_insert_default();
        if compat.supports_reasoning_effort.is_none() {
            compat.supports_reasoning_effort = Some(true);
        }
    }
    model
}

/// `GET /api/models`. The inner error is the HTTP status of a rejection.
async fn get_models(
    config: &OAuthConfig,
    access: &str,
    cancel: &CancellationToken,
) -> Result<std::result::Result<Vec<Model>, reqwest::StatusCode>> {
    let url = config.url("/api/models");
    let response = tokio::select! {
        response = crate::stream::http_client()
            .get(&url)
            .bearer_auth(access)
            .header("accept", "application/json")
            .send() => response.with_context(|| format!("request {url}"))?,
        _ = cancel.cancelled() => anyhow::bail!("model refresh cancelled"),
    };
    let status = response.status();
    if !status.is_success() {
        return Ok(Err(status));
    }
    #[derive(serde::Deserialize)]
    struct ModelsResponse {
        #[serde(default)]
        models: Vec<Model>,
    }
    let models: ModelsResponse = response
        .json()
        .await
        .context("parse the SUPER models response")?;
    Ok(Ok(models
        .models
        .into_iter()
        .map(|model| normalize_model(model, config))
        .collect()))
}

/// Fetch the model catalog for a signed-in user. An expired token is
/// refreshed first; a 401 triggers one refresh and one retry. The second
/// value is the refreshed credential the caller must store, if any.
pub async fn fetch_models(
    config: &OAuthConfig,
    credential: &OAuthCredential,
    cancel: &CancellationToken,
) -> Result<(Vec<Model>, Option<OAuthCredential>)> {
    let mut refreshed = None;
    if credential.is_expired() {
        refreshed = Some(refresh(credential, config).await?);
    }
    let access = refreshed
        .as_ref()
        .map_or(credential.access.as_str(), |value| value.access.as_str());
    let status = match get_models(config, access, cancel).await? {
        Ok(models) => return Ok((models, refreshed)),
        Err(status) => status,
    };
    if status != reqwest::StatusCode::UNAUTHORIZED || refreshed.is_some() {
        anyhow::bail!("SUPER models request failed (HTTP {status})");
    }
    let replacement = refresh(credential, config).await?;
    match get_models(config, &replacement.access, cancel).await? {
        Ok(models) => Ok((models, Some(replacement))),
        Err(reqwest::StatusCode::UNAUTHORIZED) => anyhow::bail!("{SIGN_IN_EXPIRED}"),
        Err(status) => anyhow::bail!("SUPER models request failed (HTTP {status})"),
    }
}

/// Write `models` to the catalog at `path` as the `super` provider,
/// replacing whatever that provider held before.
pub fn write_models_at(path: &Path, config: &OAuthConfig, models: &[Model]) -> Result<()> {
    crate::provider_config::replace_provider_models_at(
        path,
        PROVIDER,
        DISPLAY_NAME,
        &config.inference_url(),
        MODELS_API,
        models,
    )
}

/// Remove the `super` provider from the catalog at `path`.
pub fn clear_models_at(path: &Path) -> Result<bool> {
    crate::provider_config::remove_at(path, PROVIDER)
}

/// Refresh the catalog for the credential stored at `auth_path`. Signed out
/// means no request and no change.
pub async fn sync_models_at(
    config: &OAuthConfig,
    auth_path: &Path,
    catalog_path: &Path,
    cancel: &CancellationToken,
) -> Result<SyncOutcome> {
    let Some(credential) = super::stored_oauth_at(auth_path, PROVIDER) else {
        return Ok(SyncOutcome::SignedOut);
    };
    let (models, refreshed) = fetch_models(config, &credential, cancel).await?;
    if let Some(refreshed) = refreshed {
        super::store_oauth_at(auth_path, PROVIDER, refreshed)?;
    }
    write_models_at(catalog_path, config, &models)?;
    Ok(SyncOutcome::Synced(models))
}

/// Refresh the user's catalog from the platform. Signed out means no
/// request and no change; a failure leaves the previous catalog in place.
pub async fn sync_models(config: &OAuthConfig, cancel: &CancellationToken) -> Result<SyncOutcome> {
    let auth_path = super::auth_file_path().context("no home directory")?;
    let catalog_path = crate::provider_config::catalog_path()?;
    sync_models_at(config, &auth_path, &catalog_path, cancel).await
}

/// Store a finished sign-in and load its models into the catalog.
pub async fn complete_sign_in(
    config: &OAuthConfig,
    sign_in: &SignIn,
    cancel: &CancellationToken,
) -> Result<Vec<Model>> {
    super::store_oauth(PROVIDER, sign_in.credential.clone())?;
    match sync_models(config, cancel).await? {
        SyncOutcome::Synced(models) => Ok(models),
        SyncOutcome::SignedOut => Ok(Vec::new()),
    }
}

/// Revoke the device on the platform, forget the credential, and drop the
/// `super` models from the catalog.
pub async fn sign_out(config: &OAuthConfig) -> Result<SignOut> {
    let credential = stored_credential();
    let revoke_error = match &credential {
        Some(credential) => revoke(config, credential)
            .await
            .err()
            .map(|error| format!("{error:#}")),
        None => None,
    };
    let had_credential = super::remove_api_key(PROVIDER)? || credential.is_some();
    let catalog_path = crate::provider_config::catalog_path()?;
    clear_models_at(&catalog_path)?;
    Ok(SignOut {
        had_credential,
        revoke_error,
    })
}

/// After the gateway rejected `rejected_access` with 401: when it is the
/// stored SUPER token, refresh once and return the replacement. `Ok(None)`
/// means the token is not SUPER's and the rejection stands.
pub async fn replacement_token(rejected_access: &str) -> Result<Option<String>> {
    if !super::is_oauth_access_token(PROVIDER, rejected_access) {
        return Ok(None);
    }
    super::refresh_rejected_oauth(PROVIDER, rejected_access).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    /// One request as the mock platform received it.
    #[derive(Debug, Clone)]
    struct Received {
        method: String,
        path: String,
        headers: String,
        body: String,
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> Received {
        let mut raw = Vec::new();
        let mut chunk = [0_u8; 4096];
        let header_end = loop {
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0, "client closed before the request was complete");
            raw.extend_from_slice(&chunk[..count]);
            if let Some(position) = raw.windows(4).position(|part| part == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let head = String::from_utf8(raw[..header_end].to_vec()).unwrap();
        let length = head
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        while raw.len() < header_end + length {
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0, "client closed before the body was complete");
            raw.extend_from_slice(&chunk[..count]);
        }
        let mut request_line = head.lines().next().unwrap().split_whitespace();
        Received {
            method: request_line.next().unwrap().to_string(),
            path: request_line.next().unwrap().to_string(),
            headers: head.to_ascii_lowercase(),
            body: String::from_utf8(raw[header_end..header_end + length].to_vec()).unwrap(),
        }
    }

    async fn respond(socket: &mut tokio::net::TcpStream, status: u16, body: &Value) {
        let body = body.to_string();
        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            _ => "Error",
        };
        socket
            .write_all(
                format!(
                    "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        socket.shutdown().await.unwrap();
    }

    type Handler = dyn Fn(&Received, &[Received]) -> (u16, Value) + Send + Sync;

    struct MockPlatform {
        base_url: String,
        received: Arc<Mutex<Vec<Received>>>,
        server: tokio::task::JoinHandle<()>,
    }

    impl MockPlatform {
        async fn start(handler: Box<Handler>) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let received: Arc<Mutex<Vec<Received>>> = Arc::default();
            let log = Arc::clone(&received);
            let server = tokio::spawn(async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let request = read_request(&mut socket).await;
                    let (status, body) = {
                        let earlier = log.lock().unwrap();
                        handler(&request, &earlier)
                    };
                    log.lock().unwrap().push(request);
                    respond(&mut socket, status, &body).await;
                }
            });
            Self {
                base_url,
                received,
                server,
            }
        }

        fn config(&self) -> OAuthConfig {
            OAuthConfig::with_base_url(&self.base_url)
        }

        fn requests(&self) -> Vec<Received> {
            self.received.lock().unwrap().clone()
        }
    }

    impl Drop for MockPlatform {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    fn count(earlier: &[Received], path: &str) -> usize {
        earlier
            .iter()
            .filter(|request| request.path == path)
            .count()
    }

    fn tokens(access: &str, refresh: &str) -> Value {
        json!({
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": refresh,
            "scope": "super",
            "device_id": "dev-1"
        })
    }

    fn stored(access: &str, expires: i64) -> OAuthCredential {
        OAuthCredential {
            kind: "oauth".into(),
            access: access.into(),
            refresh: "old-refresh".into(),
            expires,
            account_id: "user-1".into(),
            client_id: None,
            available_model_ids: None,
            device_id: Some("dev-1".into()),
        }
    }

    fn far_future() -> i64 {
        chrono::Utc::now().timestamp_millis() + 3_600_000
    }

    fn catalog_model(id: &str) -> Value {
        json!({
            "id": id,
            "name": id.to_uppercase(),
            "api": "openai-completions",
            "provider": "super",
            "baseUrl": "https://gateway.example/v1",
            "reasoning": true,
            "input": ["text", "image"],
            "cost": {"input": 1.25, "output": 10, "cacheRead": 0.125, "cacheWrite": 0},
            "contextWindow": 400000,
            "maxTokens": 128000
        })
    }

    #[test]
    fn device_names_drop_the_domain() {
        assert_eq!(short_host_name("mbp.local"), "mbp");
        assert_eq!(short_host_name(" studio.example.com\n"), "studio");
        assert_eq!(short_host_name("plain"), "plain");
        assert_eq!(short_host_name(".weird"), ".weird");
    }

    #[test]
    fn base_url_prefers_the_variable_then_the_setting_then_the_default() {
        assert_eq!(resolve_base_url(None, None), DEFAULT_URL);
        assert_eq!(
            resolve_base_url(Some("https://staging.example/"), None),
            "https://staging.example"
        );
        assert_eq!(
            resolve_base_url(
                Some("https://staging.example"),
                Some("http://localhost:8080/")
            ),
            "http://localhost:8080"
        );
        assert_eq!(resolve_base_url(Some("  "), Some("")), DEFAULT_URL);
    }

    #[test]
    fn user_codes_are_shown_in_two_groups() {
        assert_eq!(format_user_code("abcdefgh"), "ABCD-EFGH");
        assert_eq!(format_user_code("ABCD-EFGH"), "ABCD-EFGH");
        assert_eq!(format_user_code(" short "), "short");
    }

    #[test]
    fn platform_models_get_the_provider_and_reasoning_effort() {
        let config = OAuthConfig::with_base_url("https://platform.example");
        let mut model: Model = serde_json::from_value(catalog_model("gpt-5")).unwrap();
        model.provider = "something-else".into();
        model.base_url = String::new();
        let model = normalize_model(model, &config);
        assert_eq!(model.provider, PROVIDER);
        assert_eq!(model.base_url, "https://platform.example/v1");
        assert_eq!(model.compat.unwrap().supports_reasoning_effort, Some(true));
        let mut plain: Model = serde_json::from_value(catalog_model("plain")).unwrap();
        plain.reasoning = false;
        assert!(normalize_model(plain, &config).compat.is_none());
    }

    #[tokio::test]
    async fn device_flow_polls_until_approved_and_looks_up_the_account() {
        let platform = MockPlatform::start(Box::new(|request, earlier| {
            match request.path.as_str() {
                "/oauth/device/code" => {
                    assert_eq!(request.method, "POST");
                    assert!(request.body.contains("client_id=super-cli"));
                    assert!(request.body.contains("scope=super"));
                    assert!(request.body.contains("device_kind=cli"));
                    assert!(request.body.contains("device_name="));
                    assert!(request.body.contains("device_platform="));
                    (
                        200,
                        json!({
                            "device_code": "device-secret",
                            "user_code": "ABCDEFGH",
                            "verification_uri": "http://platform.test/device",
                            "verification_uri_complete": "http://platform.test/device?code=ABCDEFGH",
                            "expires_in": 60,
                            "interval": 1
                        }),
                    )
                }
                "/oauth/device/token" => {
                    assert!(request.body.contains("device_code=device-secret"));
                    assert!(request.body.contains(
                        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"
                    ));
                    match count(earlier, "/oauth/device/token") {
                        0 => (400, json!({"error": "authorization_pending"})),
                        1 => (400, json!({"error": "slow_down", "interval": 1})),
                        _ => (200, tokens("access-1", "refresh-1")),
                    }
                }
                "/api/me" => {
                    assert!(request.headers.contains("authorization: bearer access-1"));
                    (
                        200,
                        json!({
                            "user": {"id": "user-42", "email": "felix@example.com", "isAdmin": false},
                            "device": {"id": "dev-1", "kind": "cli"}
                        }),
                    )
                }
                other => panic!("unexpected request to {other}"),
            }
        }))
        .await;
        let config = platform.config();
        let cancel = CancellationToken::new();

        let device = start(&config, &cancel).await.unwrap();
        assert_eq!(device.display_code(), "ABCD-EFGH");
        assert_eq!(device.verification_uri, "http://platform.test/device");
        assert_eq!(
            device.browser_url(),
            "http://platform.test/device?code=ABCDEFGH"
        );

        let sign_in = login(&config, &device, &cancel).await.unwrap();
        assert_eq!(sign_in.account.email, "felix@example.com");
        assert_eq!(sign_in.credential.account_id, "user-42");
        assert_eq!(sign_in.credential.access, "access-1");
        assert_eq!(sign_in.credential.refresh, "refresh-1");
        assert_eq!(sign_in.credential.device_id.as_deref(), Some("dev-1"));
        assert!(!sign_in.credential.is_expired());
        assert_eq!(count(&platform.requests(), "/oauth/device/token"), 3);
    }

    #[tokio::test]
    async fn refresh_rotates_the_refresh_token_and_keeps_the_device() {
        let platform = MockPlatform::start(Box::new(|request, _| {
            assert_eq!(request.path, "/oauth/token");
            assert!(request.body.contains("grant_type=refresh_token"));
            assert!(request.body.contains("refresh_token=old-refresh"));
            assert!(request.body.contains("client_id=super-cli"));
            (200, tokens("access-2", "refresh-2"))
        }))
        .await;
        let refreshed = refresh(&stored("access-1", 0), &platform.config())
            .await
            .unwrap();
        assert_eq!(refreshed.access, "access-2");
        assert_eq!(refreshed.refresh, "refresh-2");
        assert_eq!(refreshed.account_id, "user-1");
        assert_eq!(refreshed.device_id.as_deref(), Some("dev-1"));
        assert!(!refreshed.is_expired());
    }

    #[tokio::test]
    async fn a_rejected_refresh_reports_the_expired_sign_in() {
        let platform =
            MockPlatform::start(Box::new(|_, _| (400, json!({"error": "invalid_grant"})))).await;
        let error = refresh(&stored("access-1", 0), &platform.config())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), SIGN_IN_EXPIRED);
    }

    #[tokio::test]
    async fn model_sync_retries_once_after_a_refresh_on_401_and_replaces_the_catalog() {
        let platform =
            MockPlatform::start(Box::new(|request, earlier| match request.path.as_str() {
                "/api/models" if count(earlier, "/api/models") == 0 => {
                    assert!(request.headers.contains("authorization: bearer access-1"));
                    (
                        401,
                        json!({"error": {"code": "unauthorized", "message": "expired"}}),
                    )
                }
                "/api/models" => {
                    assert!(request.headers.contains("authorization: bearer access-2"));
                    (
                        200,
                        json!({"models": [catalog_model("gpt-5"), catalog_model("claude-x")]}),
                    )
                }
                "/oauth/token" => (200, tokens("access-2", "refresh-2")),
                other => panic!("unexpected request to {other}"),
            }))
            .await;
        let directory = tempfile::tempdir().unwrap();
        let auth_path = directory.path().join("auth.json");
        let catalog_path = directory.path().join("models.json");
        super::super::store_oauth_at(&auth_path, PROVIDER, stored("access-1", far_future()))
            .unwrap();
        std::fs::write(
            &catalog_path,
            json!({
                "future": true,
                "providers": {
                    "ollama": {"baseUrl": "http://localhost:11434/v1", "api": "openai-completions", "apiKey": "ollama", "models": [{"id": "llama"}]},
                    "super": {"baseUrl": "https://old.example/v1", "api": "openai-completions", "models": [{"id": "stale-model"}]}
                }
            })
            .to_string(),
        )
        .unwrap();

        let outcome = sync_models_at(
            &platform.config(),
            &auth_path,
            &catalog_path,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let SyncOutcome::Synced(synced) = outcome else {
            panic!("expected a synced catalog, got {outcome:?}");
        };
        assert_eq!(synced.len(), 2);

        let paths: Vec<String> = platform
            .requests()
            .iter()
            .map(|request| request.path.clone())
            .collect();
        assert_eq!(paths, ["/api/models", "/oauth/token", "/api/models"]);

        let credential = super::super::stored_oauth_at(&auth_path, PROVIDER).unwrap();
        assert_eq!(credential.access, "access-2");
        assert_eq!(credential.refresh, "refresh-2");

        let catalog: Value =
            serde_json::from_str(&std::fs::read_to_string(&catalog_path).unwrap()).unwrap();
        assert_eq!(catalog["future"], true);
        assert_eq!(catalog["providers"]["ollama"]["models"][0]["id"], "llama");
        let models = catalog["providers"]["super"]["models"].as_array().unwrap();
        let ids: Vec<&str> = models
            .iter()
            .map(|model| model["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["gpt-5", "claude-x"]);
        assert_eq!(catalog["providers"]["super"]["name"], DISPLAY_NAME);
        assert_eq!(models[0]["baseUrl"], "https://gateway.example/v1");
        assert_eq!(models[0]["compat"]["supportsReasoningEffort"], true);
        assert!(models[0].get("provider").is_none());

        let registry = crate::Registry::load(Some(&catalog_path));
        let listed: Vec<&str> = registry
            .all()
            .iter()
            .filter(|model| model.provider == PROVIDER)
            .map(|model| model.id.as_str())
            .collect();
        if is_signed_in() {
            assert_eq!(listed, ["gpt-5", "claude-x"]);
        } else {
            // This machine has no SUPER sign-in, so the models stay hidden.
            assert!(listed.is_empty());
        }
    }

    #[tokio::test]
    async fn signed_out_makes_no_request_and_changes_nothing() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let contacted = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&contacted);
        let server = tokio::spawn(async move {
            let _ = listener.accept().await;
            flag.store(true, Ordering::SeqCst);
        });

        let directory = tempfile::tempdir().unwrap();
        let auth_path = directory.path().join("auth.json");
        let catalog_path = directory.path().join("models.json");
        std::fs::write(&auth_path, r#"{"openai":"key"}"#).unwrap();

        let outcome = sync_models_at(
            &OAuthConfig::with_base_url(&base_url),
            &auth_path,
            &catalog_path,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(outcome, SyncOutcome::SignedOut);
        assert!(!catalog_path.exists());
        assert!(!contacted.load(Ordering::SeqCst), "platform was contacted");
        server.abort();
        assert!(
            !contacted.load(Ordering::SeqCst),
            "platform was contacted while signed out"
        );
    }

    #[tokio::test]
    async fn a_failed_refresh_keeps_the_stale_catalog() {
        let platform = MockPlatform::start(Box::new(|request, _| {
            assert_eq!(request.path, "/oauth/token");
            (400, json!({"error": "invalid_grant"}))
        }))
        .await;
        let directory = tempfile::tempdir().unwrap();
        let auth_path = directory.path().join("auth.json");
        let catalog_path = directory.path().join("models.json");
        super::super::store_oauth_at(&auth_path, PROVIDER, stored("access-1", 0)).unwrap();
        let before = json!({"providers": {"super": {"baseUrl": "https://old.example/v1", "api": "openai-completions", "models": [{"id": "stale-model"}]}}}).to_string();
        std::fs::write(&catalog_path, &before).unwrap();

        let error = sync_models_at(
            &platform.config(),
            &auth_path,
            &catalog_path,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), SIGN_IN_EXPIRED);
        assert_eq!(std::fs::read_to_string(&catalog_path).unwrap(), before);
        assert_eq!(
            super::super::stored_oauth_at(&auth_path, PROVIDER)
                .unwrap()
                .access,
            "access-1"
        );
    }
}
