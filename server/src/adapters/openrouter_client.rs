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
    limit: Option<f64>,
    #[serde(default)]
    disabled: bool,
}

#[derive(Deserialize)]
struct OpenRouterKeysResponse {
    data: Vec<OpenRouterKeyResponse>,
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
            label: label.to_string(),
            limit_usd,
        })
    }

    async fn revoke_key(&self, key_label: &str) -> Result<(), AuthError> {
        let resp = self
            .http
            .get("https://openrouter.ai/api/v1/keys")
            .bearer_auth(&self.management_key)
            .send()
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        let keys: OpenRouterKeysResponse = resp
            .json()
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        for key in keys.data {
            if key.label.as_deref() == Some(key_label)
                || key.name.as_deref() == Some(key_label)
            {
                self.http
                    .delete(&format!(
                        "https://openrouter.ai/api/v1/keys/{}",
                        key.key
                    ))
                    .bearer_auth(&self.management_key)
                    .send()
                    .await
                    .map_err(|e| AuthError::Internal(e.to_string()))?;
            }
        }
        Ok(())
    }
}