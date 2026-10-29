//! The `RemoteCredentials` every signed-in `super` process uses: the stored
//! platform sign-in in `~/.super/agent/auth.json`, refreshed on demand.

use crate::RemoteCredentials;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

pub const PROVIDER_ID: &str = super_ai::auth::super_platform::PROVIDER;

/// Credentials backed by the stored `super` OAuth credential. Token refresh
/// goes through the same path every provider uses, under the auth-file lock.
pub struct PlatformCredentials {
    url: String,
    device_name: String,
}

impl PlatformCredentials {
    pub fn new(url: String, device_name: String) -> Self {
        Self { url, device_name }
    }
}

impl RemoteCredentials for PlatformCredentials {
    fn platform_url(&self) -> String {
        self.url.clone()
    }

    fn device_name(&self) -> String {
        self.device_name.clone()
    }

    fn access_token(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + '_>> {
        Box::pin(async {
            super_ai::auth::resolve_api_key_async(PROVIDER_ID, &BTreeMap::new())
                .await?
                .ok_or_else(|| anyhow::anyhow!("not signed in to SUPER"))
        })
    }
}

/// Is there a stored sign-in for the SUPER platform? No network access.
pub fn is_signed_in() -> bool {
    super_ai::auth::super_platform::is_signed_in()
}

/// Where the platform lives: `SUPER_PLATFORM_URL`, then the setting, then
/// the default.
pub fn resolve_platform_url(setting: Option<&str>) -> String {
    super_ai::auth::super_platform::resolve_base_url(
        setting,
        std::env::var("SUPER_PLATFORM_URL").ok().as_deref(),
    )
}

/// Hand the `platform.url` setting to the auth module, so token refreshes
/// reach the platform the user signed in to.
pub fn configure_platform_url(setting: Option<&str>) {
    super_ai::auth::super_platform::configure_url(setting);
}
