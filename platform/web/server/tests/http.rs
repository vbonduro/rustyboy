use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use rustyboy_web_server::{AppState, build_router};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

async fn test_app(roms: &[&str]) -> (axum::Router, TempDir, TempDir) {
    let roms_dir = TempDir::new().unwrap();
    let static_dir = TempDir::new().unwrap();

    for rom in roms {
        std::fs::write(roms_dir.path().join(rom), b"fake rom data").unwrap();
    }

    let db = rustyboy_web_server::db_connect(":memory:").await.unwrap();

    let state = Arc::new(AppState {
        roms_dir: roms_dir.path().to_path_buf(),
        static_dir: static_dir.path().to_path_buf(),
        db,
        oauth: rustyboy_web_server::auth::OAuthConfig::from_env(),
        http_client: reqwest::Client::new(),
    });

    let router = build_router(state);
    (router, roms_dir, static_dir)
}

#[tokio::test]
async fn test_list_roms_empty() {
    let (app, _roms, _static) = test_app(&[]).await;
    let req = Request::builder()
        .uri("/api/roms")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"[]");
}

#[tokio::test]
async fn test_list_roms_with_gb_files() {
    let (app, _roms, _static) = test_app(&["a.gb", "b.gbc"]).await;
    let req = Request::builder()
        .uri("/api/roms")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let names: Vec<String> = serde_json::from_slice(&body).unwrap();
    assert_eq!(names, vec!["a.gb", "b.gbc"]);
}

#[tokio::test]
async fn test_list_roms_ignores_non_gb() {
    let (app, _roms, _static) = test_app(&["game.gb", "readme.txt"]).await;
    let req = Request::builder()
        .uri("/api/roms")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let names: Vec<String> = serde_json::from_slice(&body).unwrap();
    assert_eq!(names, vec!["game.gb"]);
}

#[tokio::test]
async fn test_serve_rom_not_found() {
    let (app, _roms, _static) = test_app(&[]).await;
    let req = Request::builder()
        .uri("/roms/missing.gb")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_serve_rom_path_traversal() {
    let (app, _roms, _static) = test_app(&[]).await;
    // URL-encoded path traversal: /roms/..%2Fetc%2Fpasswd
    let req = Request::builder()
        .uri("/roms/..%2Fetc%2Fpasswd")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_index_not_found_when_no_static() {
    let (app, _roms, _static) = test_app(&[]).await;
    // static_dir is empty (no index.html), so / should return 404
    let req = Request::builder()
        .uri("/")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
