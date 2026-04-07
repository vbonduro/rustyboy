pub mod auth;
pub mod config;
pub mod db;

use auth::{AuthUser, DbExt, JwtSecretExt};
use axum::{
    Router,
    body::Bytes,
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post},
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
        .route("/api/battery-saves/:rom_name", get(get_battery_save).put(put_battery_save))
        .route("/api/save-states", get(list_roms_with_saves))
        .route("/api/save-states/:rom_name", get(list_save_states).post(post_save_state))
        .route("/api/save-states/:rom_name/latest", get(get_latest_save_state))
        .route("/api/save-states/by-id/:id/data", get(get_save_state_data))
        .route("/api/save-states/by-id/:id", delete(delete_save_state))
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

async fn get_battery_save(
    auth: AuthUser,
    Path(rom_name): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.db.get_battery_save(&auth.user_id, &rom_name).await {
        Ok(Some(bs)) => (
            StatusCode::OK,
            [("content-type", "application/octet-stream")],
            bs.data,
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn put_battery_save(
    auth: AuthUser,
    Path(rom_name): Path<String>,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> impl IntoResponse {
    if body.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match state.db.upsert_battery_save(&auth.user_id, &rom_name, body.to_vec()).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── Save state handlers ────────────────────────────────────────────────────

/// GET /api/save-states — list roms the user has saves for
async fn list_roms_with_saves(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.db.list_roms_with_saves(&auth.user_id).await {
        Ok(rows) => {
            let items: Vec<_> = rows
                .into_iter()
                .map(|(rom_name, last_saved)| serde_json::json!({ "rom_name": rom_name, "last_saved": last_saved }))
                .collect();
            Json(items).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// GET /api/save-states/:rom_name — list save slots for a game
async fn list_save_states(
    auth: AuthUser,
    Path(rom_name): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.db.list_save_states(&auth.user_id, &rom_name).await {
        Ok(saves) => {
            let items: Vec<_> = saves
                .into_iter()
                .map(|s| serde_json::json!({
                    "id": s.id,
                    "slot_name": s.slot_name,
                    "created_at": s.created_at,
                    "updated_at": s.updated_at,
                }))
                .collect();
            Json(items).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// GET /api/save-states/:rom_name/latest — get metadata for most recent save
async fn get_latest_save_state(
    auth: AuthUser,
    Path(rom_name): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.db.get_latest_save_state(&auth.user_id, &rom_name).await {
        Ok(Some(s)) => Json(serde_json::json!({
            "id": s.id,
            "slot_name": s.slot_name,
            "created_at": s.created_at,
            "updated_at": s.updated_at,
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// POST /api/save-states/:rom_name — upload a new save state blob
async fn post_save_state(
    auth: AuthUser,
    Path(rom_name): Path<String>,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> impl IntoResponse {
    if body.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    // Auto-generate slot name from current unix timestamp
    let slot_name = now_unix_secs().to_string();
    match state.db.upsert_save_state(&auth.user_id, &rom_name, &slot_name, body.to_vec()).await {
        Ok(s) => {
            // Keep only the 5 most recent saves per user+rom; silently ignore prune errors
            let _ = state.db.prune_save_states(&auth.user_id, &rom_name, 5).await;
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "id": s.id,
                    "slot_name": s.slot_name,
                    "created_at": s.created_at,
                    "updated_at": s.updated_at,
                })),
            )
                .into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// GET /api/save-states/by-id/:id/data — download save state blob
async fn get_save_state_data(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.db.get_save_state(&id).await {
        Ok(Some(s)) if s.user_id == auth.user_id => (
            StatusCode::OK,
            [("content-type", "application/octet-stream")],
            s.data,
        )
            .into_response(),
        Ok(Some(_)) => StatusCode::FORBIDDEN.into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// DELETE /api/save-states/by-id/:id — delete a save state
async fn delete_save_state(
    auth: AuthUser,
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.db.get_save_state(&id).await {
        Ok(Some(s)) if s.user_id == auth.user_id => {
            match state.db.delete_save_state(&id).await {
                Ok(_) => StatusCode::NO_CONTENT.into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Ok(Some(_)) => StatusCode::FORBIDDEN.into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
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
