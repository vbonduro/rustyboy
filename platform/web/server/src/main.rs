use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
};
use std::{path::PathBuf, sync::Arc};
use tower_http::services::ServeDir;

#[derive(Clone)]
struct AppState {
    roms_dir: PathBuf,
    static_dir: PathBuf,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let roms_dir = PathBuf::from(
        std::env::var("ROMS_DIR").unwrap_or_else(|_| "/roms".to_string()),
    );
    let static_dir = PathBuf::from(
        std::env::var("STATIC_DIR").unwrap_or_else(|_| "/static".to_string()),
    );

    let state = Arc::new(AppState {
        roms_dir,
        static_dir: static_dir.clone(),
    });

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/api/roms", get(list_roms))
        .route("/roms/{name}", get(serve_rom))
        .nest_service("/static", ServeDir::new(&static_dir))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    tracing::info!("Listening on http://0.0.0.0:8080");
    axum::serve(listener, app).await.unwrap();
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

async fn serve_rom(
    Path(name): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // Reject path traversal attempts
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = state.roms_dir.join(&name);
    match tokio::fs::read(path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
