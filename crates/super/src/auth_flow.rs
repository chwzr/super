//! Browser authentication shared by CLI and interactive mode.

use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub async fn login_browser(
    provider: &str,
    cancel: &CancellationToken,
    show_url: impl FnOnce(&str),
) -> Result<()> {
    let credential = match provider {
        "openai" => {
            let id = super_coding::settings::Settings::get_or_create_device_id()?;
            super_ai::auth::openai_codex::login_browser(
                &super_ai::auth::openai_codex::OAuthConfig::chatgpt(Some(id)),
                cancel,
                show_url,
            )
            .await?
        }
        "openai-codex" => {
            super_ai::auth::openai_codex::login_browser(&Default::default(), cancel, show_url)
                .await?
        }
        "anthropic" => {
            super_ai::auth::anthropic::login_browser(&Default::default(), cancel, show_url).await?
        }
        "cursor" => {
            super_ai::auth::cursor::login_browser(&Default::default(), cancel, show_url).await?
        }
        "openrouter" => {
            super_ai::auth::openrouter::login_browser(&Default::default(), cancel, show_url).await?
        }
        "radius" => {
            super_ai::auth::radius::login_browser(&Default::default(), cancel, show_url).await?
        }
        _ => anyhow::bail!("{provider} does not provide browser authentication"),
    };
    super_ai::auth::store_oauth(provider, credential)
}

pub fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "windows")]
    if std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_TTY").is_some()
        || std::env::var("SESSIONNAME").is_ok_and(|name| name.eq_ignore_ascii_case("Services"))
    {
        return false;
    }
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return false;
        }
        std::process::Command::new("xdg-open")
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        // Avoid cmd.exe: it treats '&' in authentication URLs as command separators.
        let mut command = std::process::Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return false;

    command.arg(url).spawn().is_ok()
}
