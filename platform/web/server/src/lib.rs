pub mod auth;
pub mod config;
pub mod db;

use auth::{AuthUser, DbExt, JwtSecretExt};
use axum::{
    Router,
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use std::{path::PathBuf, sync::Arc};
use tower_http::services::ServeDir;

#[derive(Clone)]
pub struct AppState {
    pub roms_dir: PathBuf,
    pub static_dir: PathBuf,
    pub db: db::Database,
    pub oauth: auth::OAuthConfig,
    pub http_client: reqwest::Client,
}

pub async fn db_connect(path: &str) -> Result<db::Database, sqlx::Error> {
    db::Database::connect(path).await
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let static_dir = state.static_dir.clone();
    Router::new()
        .route("/", get(serve_index))
        .route("/api/roms", get(list_roms))
        .route("/api/me", get(api_me))
        .route("/api/auth-method", get(api_auth_method))
        .route("/roms/:name", get(serve_rom))
        .route("/auth/google", get(auth::google_login))
        .route("/auth/google/callback", get(auth::google_callback))
        .route("/auth/cf-access", get(auth::cf_access_login))
        .route("/auth/logout", post(auth::logout))
        .route("/dev/log", post(dev_log))
        .nest_service("/static", ServeDir::new(&static_dir))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            inject_auth_extensions,
        ))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

/// Middleware: injects the JWT secret and DB handle as request extensions so
/// that the `AuthUser` extractor can verify and check revocation of session
/// cookies without holding a reference to `AppState` directly.
async fn inject_auth_extensions(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    req.extensions_mut()
        .insert(JwtSecretExt(state.oauth.jwt_secret.clone()));
    req.extensions_mut()
        .insert(DbExt(state.db.clone()));
    next.run(req).await
}

async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert("cache-control",              HeaderValue::from_static("no-store"));
    h.insert("x-content-type-options",     HeaderValue::from_static("nosniff"));
    h.insert("x-frame-options",            HeaderValue::from_static("DENY"));
    h.insert("cross-origin-embedder-policy",  HeaderValue::from_static("require-corp"));
    h.insert("cross-origin-opener-policy",    HeaderValue::from_static("same-origin"));
    h.insert("cross-origin-resource-policy",  HeaderValue::from_static("same-origin"));
    h.insert("permissions-policy",            HeaderValue::from_static("camera=(), microphone=(), geolocation=()"));
    h.insert("content-security-policy",       HeaderValue::from_static(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:; connect-src 'self'; worker-src 'self'; frame-ancestors 'none'; form-action 'none'; base-uri 'none'"
    ));
    res
}

async fn serve_index(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let path = state.static_dir.join("index.html");
    match tokio::fs::read(path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "text/html; charset=utf-8")],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn list_roms(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut names: Vec<String> = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(&state.roms_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let s = name.to_string_lossy().into_owned();
            if s.ends_with(".gb") || s.ends_with(".gbc") {
                names.push(s);
            }
        }
    }
    names.sort();
    Json(names)
}

async fn api_me(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.db.get_user_by_id(&auth.user_id).await {
        Ok(Some(user)) => Json(serde_json::json!({
            "id": user.id,
            "display_name": user.display_name,
            "email": user.email,
            "avatar_url": user.avatar_url,
        }))
        .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn api_auth_method(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if state.oauth.dev_mode {
        return Json(serde_json::json!({ "methods": ["dev"] }));
    }
    let mut methods = Vec::new();
    if !state.oauth.cf_access_aud.is_empty() {
        methods.push("cf");
    }
    if !state.oauth.client_id.is_empty() {
        methods.push("google");
    }
    if methods.is_empty() {
        methods.push("google"); // fallback
    }
    Json(serde_json::json!({ "methods": methods }))
}

async fn dev_log(body: String) -> StatusCode {
    tracing::info!("[client] {}", body.trim());
    StatusCode::NO_CONTENT
}

async fn serve_rom(
    Path(name): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // Reject path traversal attempts
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = state.roms_dir.join(&name);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
