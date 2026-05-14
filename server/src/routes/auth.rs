use std::sync::Arc;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::Html,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use shared::{AuthorizeRequest, RefreshRequest, RegisterRequest, TokenResponse, UserProfile};

use crate::domain::auth::service::AuthService;

#[derive(Clone)]
struct AppState {
    service: Arc<AuthService>,
}

#[derive(Serialize)]
struct UsageResponse {
    used_usd: f64,
    limit_usd: f64,
    remaining_usd: f64,
}

pub fn routes_with_state(service: Arc<AuthService>) -> Router {
    Router::new()
        .route("/register", post(register))
        .route("/authorize", post(authorize))
        .route("/refresh", post(refresh))
        .route("/me", get(me))
        .route("/key", post(rotate_key))
        .route("/usage", get(usage))
        .route("/login", get(login_page))
        .route("/login", post(login))
        .with_state(AppState { service })
}

async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<UserProfile>, (StatusCode, String)> {
    state.service.register(&req).await.map(Json).map_err(|e| {
        let status = match &e {
            shared::AuthError::UserAlreadyExists => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, e.to_string())
    })
}

async fn authorize(
    State(state): State<AppState>,
    Json(req): Json<AuthorizeRequest>,
) -> Result<Json<TokenResponse>, (StatusCode, String)> {
    state
        .service
        .authorize(&req.code, &req.code_verifier)
        .await
        .map(Json)
        .map_err(|e| {
            if matches!(e, shared::AuthError::InvalidToken | shared::AuthError::TokenExpired) {
                (StatusCode::UNAUTHORIZED, e.to_string())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })
}

async fn refresh(
    State(state): State<AppState>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<TokenResponse>, (StatusCode, String)> {
    state
        .service
        .refresh(&req.refresh_token)
        .await
        .map(Json)
        .map_err(|e| {
            if matches!(e, shared::AuthError::InvalidToken | shared::AuthError::TokenExpired) {
                (StatusCode::UNAUTHORIZED, e.to_string())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })
}

async fn me(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<UserProfile>, (StatusCode, String)> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or((StatusCode::UNAUTHORIZED, "missing authorization header".into()))?;
    state.service.get_profile(token).await.map(Json).map_err(|e| {
        if matches!(e, shared::AuthError::InvalidToken) {
            (StatusCode::UNAUTHORIZED, e.to_string())
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    })
}

async fn rotate_key(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<UserProfile>, (StatusCode, String)> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or((StatusCode::UNAUTHORIZED, "missing authorization header".into()))?;
    state
        .service
        .rotate_key(token)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn usage(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<UsageResponse>, (StatusCode, String)> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or((StatusCode::UNAUTHORIZED, "missing authorization header".into()))?;

    let (used, limit, remaining) = state
        .service
        .get_key_usage(token)
        .await
        .map_err(|e| {
            if matches!(e, shared::AuthError::InvalidToken | shared::AuthError::TokenExpired) {
                (StatusCode::UNAUTHORIZED, e.to_string())
            } else {
                (StatusCode::BAD_GATEWAY, e.to_string())
            }
        })?;

    Ok(Json(UsageResponse {
        used_usd: used,
        limit_usd: limit,
        remaining_usd: remaining,
    }))
}

async fn login_page() -> Html<&'static str> {
    Html(LOGIN_HTML)
}

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
    code_challenge: String,
    redirect_uri: Option<String>,
}

async fn login(
    State(state): State<AppState>,
    axum::Form(form): axum::Form<LoginForm>,
) -> Result<axum::response::Redirect, (StatusCode, String)> {
    let code = state
        .service
        .login(&form.email, &form.password, &form.code_challenge)
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;
    let redirect = form
        .redirect_uri
        .unwrap_or_else(|| "http://localhost:0/callback".into());

    let separator = if redirect.contains('?') { '&' } else { '?' };
    Ok(axum::response::Redirect::to(&format!(
        "{redirect}{separator}code={code}"
    )))
}

const LOGIN_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Super Login</title>
    <style>
        body { font-family: system-ui; display: flex; justify-content: center; align-items: center; min-height: 100vh; margin: 0; background: #0d1117; color: #c9d1d9; }
        form { background: #161b22; padding: 2rem; border-radius: 8px; border: 1px solid #30363d; width: 320px; }
        h1 { text-align: center; margin: 0 0 1.5rem; font-size: 1.25rem; }
        input { width: 100%; padding: 8px 12px; margin: 8px 0; border: 1px solid #30363d; border-radius: 6px; background: #0d1117; color: #c9d1d9; font-size: 14px; box-sizing: border-box; }
        button { width: 100%; padding: 8px; margin-top: 12px; background: #238636; color: white; border: none; border-radius: 6px; font-size: 14px; cursor: pointer; }
        button:hover { background: #2ea043; }
    </style>
</head>
<body>
    <form method="post">
        <h1>Super Login</h1>
        <input name="email" type="email" placeholder="Email" required />
        <input name="password" type="password" placeholder="Password" required />
        <input name="code_challenge" type="hidden" id="cc" />
        <input name="redirect_uri" type="hidden" id="ru" />
        <button type="submit">Log in</button>
    </form>
    <script>
        const params = new URLSearchParams(window.location.search);
        document.getElementById('cc').value = params.get('code_challenge') || '';
        document.getElementById('ru').value = params.get('redirect_uri') || '';
    </script>
</body>
</html>"#;
