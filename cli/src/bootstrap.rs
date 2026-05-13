use crate::config::load_config;

pub async fn run() {
    let config = load_config();

    if config.openrouter_api_key.is_none() {
        eprintln!("Not logged in. Run 'super login' first.");
        return;
    }

    crate::tui::app::run(config).await;
}
