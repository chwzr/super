use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "super", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
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
            super_cli::bootstrap::run().await;
        }
    }
}
