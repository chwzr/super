//! SUPER platform sign-in shared by the command line and the interactive
//! mode: the device flow, the model catalog, and sign-out.
//!
//! Remote control builds on the same credential: read it with
//! `super_ai::auth::super_platform::stored_credential()` and the device with
//! `stored_device_id()`; `base_url()` is the platform the user signed in to.

use anyhow::Result;
use super_ai::auth::super_platform as platform;
use tokio_util::sync::CancellationToken;

pub use platform::{DISPLAY_NAME, DeviceAuthorization, PROVIDER, SIGN_IN_EXPIRED, SignOut};

/// A finished sign-in, ready to announce.
#[derive(Debug, Clone)]
pub struct SignInSummary {
    pub email: String,
    pub models: Vec<super_ai::Model>,
}

/// Hand the `platform.url` setting to the auth module, so refreshes and
/// catalog requests reach the same platform the user signed in to.
pub fn configure(settings: &super_coding::Settings) {
    platform::configure_url(settings.platform.url.as_deref());
}

/// The `/login` subtitle: the platform host.
pub fn login_subtitle() -> String {
    platform::host_label()
}

/// The models clause of the sign-in line, such as `12 models`.
pub fn models_part(count: usize) -> String {
    match count {
        1 => "1 model".into(),
        count => format!("{count} models"),
    }
}

/// `Signed in to SUPER as <email> · <part> · <part>`. Remote control adds
/// its own part when it is on.
pub fn signed_in_message(email: &str, parts: &[String]) -> String {
    let mut message = format!("Signed in to {DISPLAY_NAME} as {email}");
    for part in parts.iter().filter(|part| !part.trim().is_empty()) {
        message.push_str(" · ");
        message.push_str(part.trim());
    }
    message
}

/// Run the device flow. `show_device` is called once with the code to show
/// and the URL to open; the call then waits for approval, stores the
/// credential, and loads the model catalog.
pub async fn sign_in(
    cancel: &CancellationToken,
    show_device: impl FnOnce(&DeviceAuthorization),
) -> Result<SignInSummary> {
    let config = platform::OAuthConfig::default();
    let device = platform::start(&config, cancel).await?;
    show_device(&device);
    let sign_in = platform::login(&config, &device, cancel).await?;
    let models = platform::complete_sign_in(&config, &sign_in, cancel).await?;
    Ok(SignInSummary {
        email: sign_in.account.email,
        models,
    })
}

/// Refresh the catalog for a signed-in user. Signed out, this makes no
/// request and returns no models.
pub async fn refresh_models(cancel: &CancellationToken) -> Result<Vec<super_ai::Model>> {
    match platform::sync_models(&platform::OAuthConfig::default(), cancel).await? {
        platform::SyncOutcome::Synced(models) => Ok(models),
        platform::SyncOutcome::SignedOut => Ok(Vec::new()),
    }
}

/// Start a catalog refresh in the background when signed in. Nothing runs,
/// and nothing is requested, when signed out.
pub fn spawn_background_refresh() {
    if !platform::is_signed_in() {
        return;
    }
    tokio::spawn(async move {
        let _ = refresh_models(&CancellationToken::new()).await;
    });
}

/// Revoke the device, forget the credential, and drop the models.
pub async fn sign_out() -> Result<SignOut> {
    platform::sign_out(&platform::OAuthConfig::default()).await
}

/// `super login super`: the device flow on the terminal.
pub async fn sign_in_cli(headless: bool, cancel: &CancellationToken) -> Result<()> {
    let summary = sign_in(cancel, |device| {
        println!("Open {}", device.verification_uri);
        println!("Enter code: {}", device.display_code());
        if !headless && !crate::auth_flow::open_browser(device.browser_url()) {
            eprintln!("The browser did not open. Open the URL manually.");
        }
    })
    .await?;
    println!(
        "{}",
        signed_in_message(&summary.email, &[models_part(summary.models.len())])
    );
    Ok(())
}

/// `super logout super`.
pub async fn sign_out_cli() -> Result<()> {
    let outcome = sign_out().await?;
    if let Some(error) = outcome.revoke_error {
        eprintln!("warning: the device was not revoked on the platform: {error}");
    }
    if outcome.had_credential {
        println!("Signed out of {DISPLAY_NAME}. The {PROVIDER} models were removed.");
    } else {
        println!("No saved credentials for {PROVIDER}.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sign_in_line_is_built_from_parts() {
        assert_eq!(
            signed_in_message("felix@example.com", &[models_part(12)]),
            "Signed in to SUPER as felix@example.com · 12 models"
        );
        assert_eq!(
            signed_in_message(
                "felix@example.com",
                &["remote control on".into(), models_part(1)]
            ),
            "Signed in to SUPER as felix@example.com · remote control on · 1 model"
        );
        assert_eq!(
            signed_in_message("felix@example.com", &[String::new()]),
            "Signed in to SUPER as felix@example.com"
        );
    }
}
