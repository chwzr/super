//! How this `super` was installed, so a message can name the command that
//! updates it: the version skew error of the daemon client and `super update`.

use std::path::{Path, PathBuf};

/// The command that upgrades a Homebrew install.
pub const HOMEBREW_UPGRADE_COMMAND: &str = "brew upgrade chwzr/tap/super";
/// The command that installs the latest release with cargo.
pub const CARGO_INSTALL_COMMAND: &str = "cargo install --git https://github.com/chwzr/super super";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// Inside (or a symlink into) `SUPER.app/Contents/Helpers`: the macOS app
    /// ships and updates it.
    App,
    /// A Homebrew cellar.
    Homebrew,
    /// `install.sh` or `install.ps1` put it in `~/.local/bin` (or
    /// `SUPER_INSTALL_DIR`); `super update` replaces it.
    Script,
    /// `cargo install` put it in `~/.cargo/bin` (or `$CARGO_HOME/bin`).
    Cargo,
    /// A build in a checkout's `target/` directory.
    Source,
    /// Anywhere else. `super update` replaces it in place.
    Unknown,
}

/// The places [`InstallKind::classify`] compares the executable against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallPaths {
    /// The executable as the system reports it (may be a symlink).
    pub exe: PathBuf,
    /// The same path with every symlink resolved.
    pub canonical: Option<PathBuf>,
    /// `SUPER_INSTALL_DIR`, else `~/.local/bin`.
    pub script_dir: Option<PathBuf>,
    /// `$CARGO_HOME/bin`, else `~/.cargo/bin`.
    pub cargo_bin: Option<PathBuf>,
}

impl InstallPaths {
    /// The paths of the running executable.
    pub fn current() -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        let canonical = std::fs::canonicalize(&exe).ok();
        let home = dirs::home_dir();
        let script_dir = std::env::var_os("SUPER_INSTALL_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".local/bin")));
        let cargo_bin = std::env::var_os("CARGO_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".cargo")))
            .map(|cargo| cargo.join("bin"));
        Some(Self {
            exe,
            canonical,
            script_dir,
            cargo_bin,
        })
    }
}

impl InstallKind {
    /// How the running executable was installed.
    pub fn current() -> Self {
        InstallPaths::current()
            .map(|paths| Self::classify(&paths))
            .unwrap_or(Self::Unknown)
    }

    pub fn classify(paths: &InstallPaths) -> Self {
        let candidates: Vec<&Path> = std::iter::once(paths.exe.as_path())
            .chain(paths.canonical.as_deref())
            .collect();
        if candidates.iter().any(|path| in_app_bundle(path)) {
            return Self::App;
        }
        if candidates.iter().any(|path| in_homebrew_cellar(path)) {
            return Self::Homebrew;
        }
        let in_dir = |dir: &Option<PathBuf>| {
            dir.as_deref().is_some_and(|dir| {
                candidates
                    .iter()
                    .any(|path| path.parent().is_some_and(|parent| same_dir(parent, dir)))
            })
        };
        if in_dir(&paths.cargo_bin) {
            return Self::Cargo;
        }
        if in_dir(&paths.script_dir) {
            return Self::Script;
        }
        if candidates.iter().any(|path| in_target_dir(path)) {
            return Self::Source;
        }
        Self::Unknown
    }

    /// What to do to update this install, as the end of a sentence:
    /// "…: run `brew upgrade chwzr/tap/super`".
    pub fn update_hint(self) -> String {
        match self {
            Self::App => "managed by the SUPER app: update the app".into(),
            Self::Homebrew => format!("run `{HOMEBREW_UPGRADE_COMMAND}`"),
            Self::Script | Self::Unknown => "run `super update`".into(),
            Self::Cargo => format!("run `{CARGO_INSTALL_COMMAND}`"),
            Self::Source => "rebuild it from an up-to-date checkout".into(),
        }
    }
}

/// `…/<Name>.app/Contents/Helpers/…`.
fn in_app_bundle(path: &Path) -> bool {
    path.ancestors().any(|dir| {
        dir.file_name().is_some_and(|name| name == "Helpers")
            && dir.parent().is_some_and(|contents| {
                contents.file_name().is_some_and(|name| name == "Contents")
                    && contents
                        .parent()
                        .and_then(Path::file_name)
                        .is_some_and(|bundle| bundle.to_string_lossy().ends_with(".app"))
            })
    })
}

/// `…/Cellar/super/<version>/bin/super`.
fn in_homebrew_cellar(path: &Path) -> bool {
    path.ancestors().any(|dir| dir.ends_with("Cellar/super"))
}

/// `…/target/<profile>/super`.
fn in_target_dir(path: &Path) -> bool {
    path.parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .is_some_and(|name| name == "target")
}

fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(exe: &str, canonical: Option<&str>) -> InstallPaths {
        InstallPaths {
            exe: PathBuf::from(exe),
            canonical: canonical.map(PathBuf::from),
            script_dir: Some(PathBuf::from("/home/felix/.local/bin")),
            cargo_bin: Some(PathBuf::from("/home/felix/.cargo/bin")),
        }
    }

    #[test]
    fn a_symlink_into_the_app_bundle_is_the_app() {
        let kind = InstallKind::classify(&paths(
            "/usr/local/bin/super",
            Some("/Applications/SUPER.app/Contents/Helpers/super"),
        ));
        assert_eq!(kind, InstallKind::App);
        assert_eq!(
            InstallKind::classify(&paths(
                "/Applications/SUPER.app/Contents/Helpers/super",
                None
            )),
            InstallKind::App
        );
    }

    #[test]
    fn the_cellar_is_homebrew() {
        let kind = InstallKind::classify(&paths(
            "/opt/homebrew/bin/super",
            Some("/opt/homebrew/Cellar/super/0.0.41/bin/super"),
        ));
        assert_eq!(kind, InstallKind::Homebrew);
    }

    #[test]
    fn install_dirs_and_builds_are_told_apart() {
        assert_eq!(
            InstallKind::classify(&paths("/home/felix/.local/bin/super", None)),
            InstallKind::Script
        );
        assert_eq!(
            InstallKind::classify(&paths("/home/felix/.cargo/bin/super", None)),
            InstallKind::Cargo
        );
        assert_eq!(
            InstallKind::classify(&paths("/home/felix/dev/super/target/release/super", None)),
            InstallKind::Source
        );
        assert_eq!(
            InstallKind::classify(&paths("/usr/bin/super", None)),
            InstallKind::Unknown
        );
    }

    #[test]
    fn every_kind_names_its_update() {
        assert_eq!(
            InstallKind::App.update_hint(),
            "managed by the SUPER app: update the app"
        );
        assert_eq!(
            InstallKind::Homebrew.update_hint(),
            "run `brew upgrade chwzr/tap/super`"
        );
        assert_eq!(InstallKind::Script.update_hint(), "run `super update`");
        assert_eq!(InstallKind::Unknown.update_hint(), "run `super update`");
        assert!(InstallKind::Cargo.update_hint().contains("cargo install"));
        assert!(InstallKind::Source.update_hint().contains("rebuild"));
    }
}
