//! Remote control for SUPER sessions.
//!
//! A signed-in `super` mirrors every RPC event of a session to an S2 stream,
//! tails a second stream for RPC commands, and announces the session in a
//! registry stream. Phones, desktops and the web read those streams through
//! the SUPER platform. See `super-platform/docs/ARCHITECTURE.md` for the
//! stream layout this crate implements.
//!
//! Nothing in this crate runs unless something calls [`Remote::start`], so a
//! signed-out `super` never touches the network.

pub mod commands;
#[cfg(feature = "fake")]
pub mod fake;
pub mod host;
mod log;
pub mod registry;
pub mod s2;
pub mod transport;
mod ulid;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub use host::HostIdentity;
pub use registry::{RegistryWriter, SessionRecord, SessionState};
pub use s2::S2Client;
pub use transport::{RemoteNotice, RemoteStatus, SessionTransport};

/// What the transport needs from the sign-in: where the platform is, what
/// this device is called, and a fresh bearer token on demand.
pub trait RemoteCredentials: Send + Sync {
    /// Platform base URL without a trailing slash, for example
    /// `https://super.flxkpe.io`.
    fn platform_url(&self) -> String;
    /// The name this device registered with, shown to other controllers.
    fn device_name(&self) -> String;
    /// A valid access token, refreshed when needed.
    fn access_token(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + '_>>;
}

/// `GET /api/remote/config` from the platform.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConfig {
    pub s2: S2Config,
    pub device_id: String,
    pub device_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct S2Config {
    /// For example `https://super.flxkpe.io/s2/v1`.
    pub base_url: String,
    pub basin: String,
}

impl RemoteConfig {
    /// Ask the platform where this user's streams live.
    pub async fn fetch(
        http: &reqwest::Client,
        credentials: &dyn RemoteCredentials,
    ) -> anyhow::Result<Self> {
        let token = credentials.access_token().await?;
        let url = format!("{}/api/remote/config", credentials.platform_url());
        let response = http
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?;
        Ok(response.json().await?)
    }
}

/// Everything one process shares across its sessions: the platform
/// configuration, the HTTP client and the host identity.
pub struct Remote {
    pub config: RemoteConfig,
    pub host: HostIdentity,
    pub client: Arc<S2Client>,
    pub credentials: Arc<dyn RemoteCredentials>,
}

impl Remote {
    /// Resolve the configuration and build the shared S2 client. Call this
    /// off the UI thread; it makes one HTTP request.
    pub async fn start(
        credentials: Arc<dyn RemoteCredentials>,
        host: HostIdentity,
    ) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("super-remote/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let config = RemoteConfig::fetch(&http, credentials.as_ref()).await?;
        log::line(format!(
            "remote start: platform {} basin {} device {} host {} ({})",
            credentials.platform_url(),
            config.s2.basin,
            config.device_id,
            host.host_id,
            host.name
        ));
        let client = Arc::new(S2Client::new(
            http,
            config.s2.base_url.clone(),
            credentials.clone(),
        ));
        Ok(Self {
            config,
            host,
            client,
            credentials,
        })
    }

    /// Stream names for one session on this host.
    pub fn session_streams(&self, session_id: &str) -> transport::SessionStreams {
        transport::SessionStreams::new(&self.host.host_id, session_id)
    }
}

/// Unix milliseconds now.
pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
