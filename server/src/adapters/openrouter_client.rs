use async_trait::async_trait;
use serde::Deserialize;
use shared::AuthError;

use crate::domain::auth::ports::{OpenRouterKey, OpenRouterProvider};

pub struct OpenRouterClient {
    management_key: String,
    http: reqwest::Client,
}

impl OpenRouterClient {
    pub fn new(management_key: String) -> Self {
        Self {
            management_key,
            http: reqwest::Client::new(),
        }
    }
}

#[derive(Deserialize)]
struct OpenRouterKeyResponse {
    key: String,
    name: Option<String>,
    label: Option<String>,
    #[serde(default, rename = "limit")]
    _limit: Option<f64>,
    #[serde(default, rename = "disabled")]
    _disabled: bool,
}

#[derive(Deserialize)]
struct OpenRouterKeysResponse {
    data: Vec<OpenRouterKeyResponse>,
}

#[derive(Deserialize)]
struct OpenRouterKeyUsageData {
    usage: f64,
    limit: Option<f64>,
}

#[derive(Deserialize)]
struct OpenRouterKeyUsageResponse {
    data: OpenRouterKeyUsageData,
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
                "workspace_id": "e6d70ee1-c01c-4e5c-8af0-2838765bad95"
            }))
            .send()
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AuthError::Internal(format!(
                "OpenRouter key creation failed: {body}"
            )));
        }

        let key: OpenRouterKeyResponse = resp
            .json()
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        Ok(OpenRouterKey {
            id: label.to_string(),
            key: key.key,
            _label: label.to_string(),
            _limit_usd: limit_usd,
        })
    }

    async fn revoke_key(&self, key_label: &str) -> Result<(), AuthError> {
        let resp = self
            .http
            .get("https://openrouter.ai/api/v1/keys?workspace_id=e6d70ee1-c01c-4e5c-8af0-2838765bad95")
            .bearer_auth(&self.management_key)
            .send()
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        let keys: OpenRouterKeysResponse = resp
            .json()
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        for key in keys.data {
            if key.label.as_deref() == Some(key_label) || key.name.as_deref() == Some(key_label) {
                self.http
                    .delete(format!("https://openrouter.ai/api/v1/keys/{}", key.key))
                    .bearer_auth(&self.management_key)
                    .send()
                    .await
                    .map_err(|e| AuthError::Internal(e.to_string()))?;
            }
        }
        Ok(())
    }

    async fn fetch_key_usage(&self, user_key: &str) -> Result<(f64, f64), AuthError> {
        let resp = self
            .http
            .get("https://openrouter.ai/api/v1/key")
            .bearer_auth(user_key)
            .send()
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(AuthError::Internal("OpenRouter usage fetch failed".into()));
        }

        let usage: OpenRouterKeyUsageResponse = resp
            .json()
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        // None means no spending cap; use f64::MAX as sentinel for "unlimited".
        Ok((usage.data.usage, usage.data.limit.unwrap_or(f64::MAX)))
    }
}
