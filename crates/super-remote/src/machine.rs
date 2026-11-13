//! Which machine this is. Created on the first signed-in run and kept in
//! `~/.super/agent/machine.json`.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineIdentity {
    pub machine_id: String,
    pub name: String,
    pub platform: String,
}

/// What `host.json`, the file older versions kept the identity in, holds.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FormerIdentity {
    host_id: String,
    name: String,
    platform: String,
}

impl MachineIdentity {
    pub fn default_path() -> Option<PathBuf> {
        dirs::home_dir().map(|home| home.join(".super/agent/machine.json"))
    }

    /// Read the identity, or create one named after the machine. When only
    /// the former `host.json` exists beside `path`, its identity is taken
    /// over once: written to `path`, with `host.json` left where it is.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if let Some(identity) = read(path) {
            return Ok(identity);
        }
        let identity = former(path).unwrap_or_else(|| Self {
            machine_id: crate::ulid::generate(),
            name: machine_name(),
            platform: platform_name().to_string(),
        });
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

fn read(path: &Path) -> Option<MachineIdentity> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<MachineIdentity>(&text)
        .ok()
        .filter(|identity| !identity.machine_id.is_empty())
}

/// The identity in `host.json` beside `path`, if there is one.
fn former(path: &Path) -> Option<MachineIdentity> {
    let text = std::fs::read_to_string(path.with_file_name("host.json")).ok()?;
    let former = serde_json::from_str::<FormerIdentity>(&text).ok()?;
    (!former.host_id.is_empty()).then_some(MachineIdentity {
        machine_id: former.host_id,
        name: former.name,
        platform: former.platform,
    })
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
        .unwrap_or_else(|| "machine".to_string())
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
        let path = dir.path().join("nested/machine.json");
        let first = MachineIdentity::load_or_create(&path).expect("create");
        assert_eq!(first.machine_id.len(), 26);
        assert!(!first.name.is_empty());
        let second = MachineIdentity::load_or_create(&path).expect("reload");
        assert_eq!(first, second);
        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored["machineId"], first.machine_id.as_str());
    }

    #[test]
    fn renaming_persists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("machine.json");
        let mut identity = MachineIdentity::load_or_create(&path).expect("create");
        identity.rename("  studio  ", &path).expect("rename");
        let reloaded = MachineIdentity::load_or_create(&path).expect("reload");
        assert_eq!(reloaded.name, "studio");
        assert_eq!(reloaded.machine_id, identity.machine_id);
    }

    #[test]
    fn the_id_in_host_json_is_taken_over_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let former = dir.path().join("host.json");
        let before = r#"{"hostId":"01JABCDEFGHJKMNPQRSTVWXYZ0","name":"mbp","platform":"macos"}"#;
        std::fs::write(&former, before).unwrap();
        let path = dir.path().join("machine.json");

        let identity = MachineIdentity::load_or_create(&path).expect("take over");
        assert_eq!(identity.machine_id, "01JABCDEFGHJKMNPQRSTVWXYZ0");
        assert_eq!(identity.name, "mbp");
        assert_eq!(identity.platform, "macos");
        assert_eq!(
            std::fs::read_to_string(&former).unwrap(),
            before,
            "host.json stays as it was"
        );

        // From then on machine.json alone counts.
        let mut identity = identity;
        identity.rename("studio", &path).expect("rename");
        std::fs::write(
            &former,
            r#"{"hostId":"01OTHER","name":"x","platform":"linux"}"#,
        )
        .unwrap();
        let reloaded = MachineIdentity::load_or_create(&path).expect("reload");
        assert_eq!(reloaded.machine_id, "01JABCDEFGHJKMNPQRSTVWXYZ0");
        assert_eq!(reloaded.name, "studio");
    }
}
