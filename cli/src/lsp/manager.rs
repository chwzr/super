use std::collections::HashMap;
use std::path::Path;

use tracing::{debug, error};

use super::config::load_config;
use super::instance::LspServerInstance;
use super::types::LspServerState;

pub struct LspServerManager {
    servers: HashMap<String, LspServerInstance>,
    extension_map: HashMap<String, Vec<String>>,
    opened_files: HashMap<String, String>,
}

impl LspServerManager {
    pub async fn initialize() -> Result<Self, String> {
        let configs = load_config().await;
        let mut servers: HashMap<String, LspServerInstance> = HashMap::new();
        let mut extension_map: HashMap<String, Vec<String>> = HashMap::new();

        for (name, config) in &configs {
            if config.command.is_empty() {
                error!("Server '{}' has empty command — skipping", name);
                continue;
            }
            if config.extension_to_language.is_empty() {
                error!("Server '{}' has no extensions — skipping", name);
                continue;
            }

            for ext in config.extension_to_language.keys() {
                let normalized = ext.to_lowercase();
                extension_map
                    .entry(normalized)
                    .or_default()
                    .push(name.clone());
            }

            let instance = LspServerInstance::new(name.clone(), config.clone());
            servers.insert(name.clone(), instance);
        }

        debug!(
            "LSP manager initialized with {} servers, {} extensions",
            servers.len(),
            extension_map.len()
        );

        Ok(Self {
            servers,
            extension_map,
            opened_files: HashMap::new(),
        })
    }

    pub async fn shutdown(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        for (name, server) in &mut self.servers {
            if matches!(
                server.state,
                LspServerState::Running | LspServerState::Error
            ) {
                if let Err(e) = server.stop().await {
                    errors.push(format!("{}: {}", name, e));
                }
            }
        }
        self.servers.clear();
        self.extension_map.clear();
        self.opened_files.clear();

        if !errors.is_empty() {
            let msg = format!("LSP shutdown errors: {}", errors.join("; "));
            error!("{}", msg);
            return Err(msg);
        }
        Ok(())
    }

    pub fn get_server_for_file(&self, file_path: &str) -> Option<&LspServerInstance> {
        let ext = get_extension(file_path)?;
        let names = self.extension_map.get(&ext)?;
        let name = names.first()?;
        self.servers.get(name)
    }

    fn get_server_for_file_mut(
        &mut self,
        file_path: &str,
    ) -> Option<&mut LspServerInstance> {
        let ext = get_extension(file_path)?;
        let names = self.extension_map.get(&ext)?;
        let name = names.first()?.clone();
        self.servers.get_mut(&name)
    }

    pub async fn ensure_server_started(
        &mut self,
        file_path: &str,
    ) -> Result<Option<&LspServerInstance>, String> {
        let ext = match get_extension(file_path) {
            Some(e) => e,
            None => return Ok(None),
        };

        let names = match self.extension_map.get(&ext) {
            Some(n) => n,
            None => return Ok(None),
        };

        let name = match names.first() {
            Some(n) => n.clone(),
            None => return Ok(None),
        };

        let needs_start = match self.servers.get(&name) {
            Some(s) => {
                matches!(
                    s.state,
                    LspServerState::Stopped | LspServerState::Error
                )
            }
            None => return Ok(None),
        };

        if needs_start {
            if let Some(server) = self.servers.get_mut(&name) {
                server.start().await?;
            }
        }

        Ok(self.servers.get(&name))
    }

    pub async fn send_request<T: serde::de::DeserializeOwned + Send + 'static>(
        &mut self,
        file_path: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<Option<T>, String> {
        self.ensure_server_started(file_path).await?;
        match self.get_server_for_file_mut(file_path) {
            Some(server) => Ok(Some(server.send_request::<T>(method, params).await?)),
            None => Ok(None),
        }
    }

    pub async fn open_file(
        &mut self,
        file_path: &str,
        content: &str,
    ) -> Result<(), String> {
        let ext = get_extension(file_path);
        let server_name = match ext
            .as_ref()
            .and_then(|e| self.extension_map.get(e))
            .and_then(|names| names.first().cloned())
        {
            Some(name) => name,
            None => return Ok(()),
        };

        self.ensure_server_started(file_path).await?;

        let file_uri = path_to_uri(file_path);

        if self.opened_files.get(&file_uri) == Some(&server_name) {
            return Ok(());
        }

        let language_id = self
            .servers
            .get(&server_name)
            .and_then(|s| {
                ext.as_ref()
                    .and_then(|e| s.config.extension_to_language.get(e).cloned())
            })
            .unwrap_or_else(|| "plaintext".into());

        if let Some(server) = self.servers.get(&server_name) {
            let params = serde_json::json!({
                "textDocument": {
                    "uri": file_uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": content,
                },
            });

            server
                .send_notification("textDocument/didOpen", params)
                .await?;
            self.opened_files
                .insert(file_uri, server_name.clone());
            debug!("LSP: Sent didOpen for {}", file_path);
        }

        Ok(())
    }

    pub async fn change_file(
        &mut self,
        file_path: &str,
        content: &str,
    ) -> Result<(), String> {
        let file_uri = path_to_uri(file_path);

        let server_name = match self.opened_files.get(&file_uri) {
            Some(name) => name.clone(),
            None => return self.open_file(file_path, content).await,
        };

        if let Some(server) = self.servers.get(&server_name) {
            if server.state != LspServerState::Running {
                return self.open_file(file_path, content).await;
            }

            let params = serde_json::json!({
                "textDocument": {
                    "uri": file_uri,
                    "version": 1,
                },
                "contentChanges": [
                    { "text": content }
                ],
            });

            server
                .send_notification("textDocument/didChange", params)
                .await?;
            debug!("LSP: Sent didChange for {}", file_path);
        }

        Ok(())
    }

    pub async fn save_file(&mut self, file_path: &str) -> Result<(), String> {
        let file_uri = path_to_uri(file_path);

        let ext = get_extension(file_path);
        let server_name = match ext
            .as_ref()
            .and_then(|e| self.extension_map.get(e))
            .and_then(|names| names.first().cloned())
        {
            Some(name) => name,
            None => return Ok(()),
        };

        if let Some(server) = self.servers.get(&server_name) {
            if server.state != LspServerState::Running {
                return Ok(());
            }

            let params = serde_json::json!({
                "textDocument": { "uri": file_uri },
            });

            server
                .send_notification("textDocument/didSave", params)
                .await?;
            debug!("LSP: Sent didSave for {}", file_path);
        }

        Ok(())
    }

    pub fn is_file_open(&self, file_path: &str) -> bool {
        let file_uri = path_to_uri(file_path);
        self.opened_files.contains_key(&file_uri)
    }

    pub fn get_all_servers(&self) -> &HashMap<String, LspServerInstance> {
        &self.servers
    }
}

fn get_extension(file_path: &str) -> Option<String> {
    Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e).to_lowercase())
}

fn path_to_uri(file_path: &str) -> String {
    let abs = std::fs::canonicalize(file_path)
        .unwrap_or_else(|_| Path::new(file_path).to_path_buf());
    format!("file://{}", abs.to_string_lossy().replace('\\', "/"))
}
