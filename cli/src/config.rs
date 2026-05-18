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
    let mut config = if path.exists() {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        CliConfig::default()
    };
    config.model =
        crate::providers::resolve_slug(&config.provider, &config.model_class).to_string();
    config
}

pub fn save_config(config: &CliConfig) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let content = serde_json::to_string_pretty(config).unwrap_or_default();
    std::fs::write(&path, content).ok();
}
