use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::{debug, warn};

use super::types::LspServerConfig;

/// Load merged LSP server configs: hardcoded defaults + user override.
pub async fn load_config() -> HashMap<String, LspServerConfig> {
    let mut merged = hardcoded_defaults();

    // Merge user override from ~/.super/lsp.json
    if let Some(user_override) = load_user_config() {
        for (name, maybe_config) in user_override {
            if maybe_config.is_null() {
                merged.remove(&name);
                debug!("User disabled LSP server: {}", name);
            } else if let Ok(config) = serde_json::from_value::<LspServerConfig>(maybe_config) {
                debug!("User override for LSP server: {}", name);
                merged.insert(name, config);
            } else {
                warn!("Invalid LSP config for server: {}", name);
            }
        }
    }

    // Discover which servers are installed
    let available = discover_binaries(merged).await;
    debug!(
        "LSP servers available on PATH: {:?}",
        available.keys().collect::<Vec<_>>()
    );
    available
}

fn hardcoded_defaults() -> HashMap<String, LspServerConfig> {
    let mut map = HashMap::new();

    map.insert(
        "rust-analyzer".into(),
        LspServerConfig {
            command: "rust-analyzer".into(),
            args: vec![],
            extension_to_language: {
                let mut m = HashMap::new();
                m.insert(".rs".into(), "rust".into());
                m
            },
            env: None,
            initialization_options: None,
            workspace_folder: None,
            startup_timeout_ms: 30_000,
            max_restarts: 3,
        },
    );

    map.insert(
        "typescript-language-server".into(),
        LspServerConfig {
            command: "typescript-language-server".into(),
            args: vec!["--stdio".into()],
            extension_to_language: {
                let mut m = HashMap::new();
                m.insert(".ts".into(), "typescript".into());
                m.insert(".tsx".into(), "typescriptreact".into());
                m.insert(".js".into(), "javascript".into());
                m.insert(".jsx".into(), "javascriptreact".into());
                m
            },
            env: None,
            initialization_options: None,
            workspace_folder: None,
            startup_timeout_ms: 30_000,
            max_restarts: 3,
        },
    );

    map.insert(
        "ruff".into(),
        LspServerConfig {
            command: "ruff".into(),
            args: vec!["server".into()],
            extension_to_language: {
                let mut m = HashMap::new();
                m.insert(".py".into(), "python".into());
                m.insert(".pyi".into(), "python".into());
                m
            },
            env: None,
            initialization_options: None,
            workspace_folder: None,
            startup_timeout_ms: 15_000,
            max_restarts: 3,
        },
    );

    map
}

fn load_user_config() -> Option<HashMap<String, serde_json::Value>> {
    let path = user_lsp_config_path()?;
    let content = fs::read_to_string(&path)
        .map_err(|e| {
            debug!("No user LSP config at {}: {}", path.display(), e);
        })
        .ok()?;
    serde_json::from_str(&content)
        .map_err(|e| {
            warn!("Failed to parse {}: {}", path.display(), e);
        })
        .ok()
}

fn user_lsp_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".super").join("lsp.json"))
}

/// Filter configs to only servers whose binary is on PATH.
async fn discover_binaries(
    configs: HashMap<String, LspServerConfig>,
) -> HashMap<String, LspServerConfig> {
    let mut available = HashMap::new();
    for (name, config) in configs {
        match which::which_global(&config.command) {
            Ok(path) => {
                debug!("Found LSP server '{}' at {}", name, path.display());
                // Insert config with resolved absolute path
                let mut c = config;
                c.command = path.to_string_lossy().into_owned();
                available.insert(name, c);
            }
            Err(_) => {
                debug!(
                    "LSP server '{}' ({}) not found on PATH — skipping",
                    name, config.command
                );
            }
        }
    }
    available
}
