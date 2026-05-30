#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
    )
)]

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "super", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Resume a specific session by ID, or the most recent session if no ID given
    #[arg(long, num_args = 0..=1)]
    resume: Option<Option<String>>,
}

#[derive(Subcommand)]
enum Commands {
    /// Login to Super
    Login,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Login) => {
            let config = super_cli::config::load_config();
            let auth_client = super_cli::auth::AuthClient::new(config.api_base_url.clone());
            match auth_client.login_flow().await {
                Ok(_) => println!("Logged in successfully."),
                Err(e) => eprintln!("Login failed: {e}"),
            }
        }
        None => {
            super_cli::bootstrap::run(cli.resume).await;
        }
    }
}
