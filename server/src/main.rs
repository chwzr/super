mod adapters;
mod domain;
mod routes;

use axum::Router;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

use adapters::openrouter_client::OpenRouterClient;
use adapters::sqlite_auth_repo::SqliteAuthRepo;
use domain::auth::service::AuthService;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let repo = Arc::new(SqliteAuthRepo::new("super.db").expect("failed to open database"));
    let openrouter = Arc::new(OpenRouterClient::new(
        std::env::var("OPENROUTER_MANAGEMENT_KEY").expect("OPENROUTER_MANAGEMENT_KEY not set"),
    ));
    let service = Arc::new(AuthService::new(repo, openrouter));

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .nest("/auth", routes::auth::routes_with_state(service.clone()))
        .layer(cors);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    tracing::info!("server listening on :3000");
    axum::serve(listener, app).await.unwrap();
}
