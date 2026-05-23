use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::{debug, error};

use lsp_types::{
    CallHierarchyClientCapabilities, ClientCapabilities, DocumentSymbolClientCapabilities,
    GeneralClientCapabilities, GotoCapability, HoverClientCapabilities, InitializeParams,
    MarkupKind, PositionEncodingKind, ReferenceClientCapabilities, TextDocumentClientCapabilities,
    TextDocumentSyncClientCapabilities, Uri, WorkspaceClientCapabilities,
};

use super::client::LspClient;
use super::types::{LspServerConfig, LspServerState};

const MAX_TRANSIENT_RETRIES: u32 = 3;
const RETRY_BASE_DELAY_MS: u64 = 500;

pub struct LspServerInstance {
    pub name: String,
    pub config: LspServerConfig,
    pub state: LspServerState,
    pub start_time: Option<Instant>,
    pub last_error: Option<String>,
    pub restart_count: u32,
    crash_recovery_count: u32,
    client: Arc<Mutex<LspClient>>,
}

impl LspServerInstance {
    pub fn new(name: String, config: LspServerConfig) -> Self {
        let client = Arc::new(Mutex::new(LspClient::new(name.clone())));
        Self {
            name,
            config,
            state: LspServerState::Stopped,
            start_time: None,
            last_error: None,
            restart_count: 0,
            crash_recovery_count: 0,
            client,
        }
    }

    pub async fn start(&mut self) -> Result<(), String> {
        if matches!(self.state, LspServerState::Running | LspServerState::Starting) {
            return Ok(());
        }

        // Check crash recovery cap
        if matches!(self.state, LspServerState::Error)
            && self.crash_recovery_count > self.config.max_restarts
        {
            let msg = format!(
                "LSP server '{}' exceeded max crash recovery attempts ({})",
                self.name, self.config.max_restarts
            );
            error!("{}", msg);
            return Err(msg);
        }

        self.state = LspServerState::Starting;

        let name = self.name.clone();
        let command = self.config.command.clone();
        let args = self.config.args.clone();
        let env = self.config.env.clone();
        let workspace_folder = self
            .config
            .workspace_folder
            .clone()
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        let init_options = self.config.initialization_options.clone();
        let _startup_timeout_ms = self.config.startup_timeout_ms;

        let client = self.client.clone();

        let result = tokio::task::spawn_blocking(move || {
            let mut c = client.lock().unwrap();

            // Build crash callback that logs when process exits unexpectedly.
            // The Instance will detect Error state on the next health check.
            let crash_name = name.clone();
            let crash_cb: super::client::CrashCallback =
                Arc::new(move |msg: String| {
                    error!(
                        "LSP server '{}' crash detected via callback: {}",
                        crash_name, msg
                    );
                });

            c.start(
                &command,
                &args,
                env.as_ref(),
                Some(&workspace_folder),
                Some(crash_cb),
            )?;

            let init_params = build_init_params(&workspace_folder, init_options);
            c.initialize(init_params)?;

            Ok::<_, String>(Instant::now())
        })
        .await
        .map_err(|e| format!("spawn_blocking join error: {}", e))?;

        match result {
            Ok(start_time) => {
                self.state = LspServerState::Running;
                self.start_time = Some(start_time);
                self.crash_recovery_count = 0;
                debug!("LSP server '{}' started", self.name);
                Ok(())
            }
            Err(e) => {
                // Attempt cleanup of the failed client
                let cleanup_client = self.client.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    cleanup_client.lock().unwrap().stop()
                })
                .await;
                self.state = LspServerState::Error;
                self.last_error = Some(e.clone());
                error!("LSP server '{}' failed to start: {}", self.name, e);
                Err(e)
            }
        }
    }

    pub async fn stop(&mut self) -> Result<(), String> {
        if matches!(self.state, LspServerState::Stopped | LspServerState::Stopping) {
            return Ok(());
        }

        self.state = LspServerState::Stopping;

        let client = self.client.clone();
        let result = tokio::task::spawn_blocking(move || client.lock().unwrap().stop()).await;

        match result {
            Ok(Ok(())) | Err(_) => {
                self.state = LspServerState::Stopped;
                debug!("LSP server '{}' stopped", self.name);
                Ok(())
            }
            Ok(Err(e)) => {
                self.state = LspServerState::Error;
                self.last_error = Some(e.clone());
                error!("LSP server '{}' stop error: {}", self.name, e);
                Err(e)
            }
        }
    }

    pub fn is_healthy(&self) -> bool {
        self.state == LspServerState::Running
            && {
                let c = self.client.lock().unwrap();
                c.is_initialized
            }
    }

    pub async fn send_request<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T, String> {
        if !self.is_healthy() {
            return Err(format!(
                "LSP server '{}' not healthy (state: {:?})",
                self.name, self.state
            ));
        }

        let mut last_error = String::new();
        for attempt in 0..=MAX_TRANSIENT_RETRIES {
            let client = self.client.clone();
            let method_owned = method.to_string();
            let params_clone = params.clone();

            let result = tokio::task::spawn_blocking(move || {
                let mut c = client.lock().unwrap();
                c.send_request::<T>(&method_owned, params_clone)
            })
            .await
            .map_err(|e| format!("spawn_blocking join: {}", e))?;

            match result {
                Ok(val) => return Ok(val),
                Err(e) => {
                    last_error = e;

                    // Check if transient ContentModified error (-32801)
                    let is_content_modified = last_error.contains("\"code\": -32801")
                        || last_error.contains("\"code\":-32801");

                    if is_content_modified && attempt < MAX_TRANSIENT_RETRIES {
                        let delay = RETRY_BASE_DELAY_MS * 2u64.pow(attempt);
                        debug!(
                            "LSP ContentModified for '{}', retrying in {}ms (attempt {}/{})",
                            self.name,
                            delay,
                            attempt + 1,
                            MAX_TRANSIENT_RETRIES
                        );
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        continue;
                    }
                    break;
                }
            }
        }

        Err(last_error)
    }

    pub async fn send_notification(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), String> {
        if !self.is_healthy() {
            return Err(format!(
                "LSP server '{}' not healthy for notification",
                self.name
            ));
        }

        let client = self.client.clone();
        let method_owned = method.to_string();

        tokio::task::spawn_blocking(move || {
            let mut c = client.lock().unwrap();
            c.send_notification(&method_owned, params)
        })
        .await
        .map_err(|e| format!("spawn_blocking join: {}", e))?
    }
}

#[allow(deprecated)]
fn build_init_params(
    workspace_folder: &str,
    initialization_options: Option<serde_json::Value>,
) -> InitializeParams {
    let workspace_uri = format!("file://{}", workspace_folder);

    InitializeParams {
        process_id: Some(std::process::id()),
        root_path: Some(workspace_folder.to_string()),
        root_uri: Some(
            workspace_uri
                .parse::<Uri>()
                .unwrap_or_else(|_| Uri::from_str("file:///").unwrap()),
        ),
        initialization_options,
        capabilities: ClientCapabilities {
            workspace: Some(WorkspaceClientCapabilities {
                configuration: Some(false),
                workspace_folders: Some(false),
                ..Default::default()
            }),
            text_document: Some(TextDocumentClientCapabilities {
                synchronization: Some(TextDocumentSyncClientCapabilities {
                    dynamic_registration: Some(false),
                    will_save: Some(false),
                    will_save_wait_until: Some(false),
                    did_save: Some(true),
                }),
                publish_diagnostics: None, // out of scope v1
                hover: Some(HoverClientCapabilities {
                    dynamic_registration: Some(false),
                    content_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
                }),
                definition: Some(GotoCapability {
                    dynamic_registration: Some(false),
                    link_support: Some(true),
                }),
                references: Some(ReferenceClientCapabilities {
                    dynamic_registration: Some(false),
                }),
                document_symbol: Some(DocumentSymbolClientCapabilities {
                    dynamic_registration: Some(false),
                    hierarchical_document_symbol_support: Some(true),
                    ..Default::default()
                }),
                call_hierarchy: Some(CallHierarchyClientCapabilities {
                    dynamic_registration: Some(false),
                }),
                ..Default::default()
            }),
            general: Some(GeneralClientCapabilities {
                position_encodings: Some(vec![PositionEncodingKind::UTF16]),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}
