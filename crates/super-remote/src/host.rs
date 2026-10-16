//! Which machine this is. Created on the first signed-in run and kept in
//! `~/.super/agent/host.json`.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostIdentity {
    pub host_id: String,
    pub name: String,
    pub platform: String,
}

impl HostIdentity {
    pub fn default_path() -> Option<PathBuf> {
        dirs::home_dir().map(|home| home.join(".super/agent/host.json"))
    }

    /// Read the identity, or create one named after the machine.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(identity) = serde_json::from_str::<Self>(&text)
            && !identity.host_id.is_empty()
        {
            return Ok(identity);
        }
        let identity = Self {
            host_id: crate::ulid::generate(),
            name: machine_name(),
            platform: platform_name().to_string(),
        };
        identity.save(path)?;
        Ok(identity)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
    }

    pub fn rename(&mut self, name: &str, path: &Path) -> Result<()> {
        self.name = name.trim().to_string();
        self.save(path)
    }
}

fn machine_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .map(|value| {
            value
                .split('.')
                .next()
                .unwrap_or(&value)
                .trim_end_matches(".local")
                .to_string()
        })
        .unwrap_or_else(|| "host".to_string())
}

pub fn platform_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "windows",
        other => {
            if other.is_empty() {
                "unknown"
            } else {
                "other"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_is_created_once_and_then_reused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/host.json");
        let first = HostIdentity::load_or_create(&path).expect("create");
        assert_eq!(first.host_id.len(), 26);
        assert!(!first.name.is_empty());
        let second = HostIdentity::load_or_create(&path).expect("reload");
        assert_eq!(first, second);
    }

    #[test]
    fn renaming_persists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("host.json");
        let mut identity = HostIdentity::load_or_create(&path).expect("create");
        identity.rename("  studio  ", &path).expect("rename");
        let reloaded = HostIdentity::load_or_create(&path).expect("reload");
        assert_eq!(reloaded.name, "studio");
        assert_eq!(reloaded.host_id, identity.host_id);
    }
}
