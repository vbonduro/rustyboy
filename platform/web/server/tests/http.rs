use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use rustyboy_web_server::{AppState, auth::OAuthConfig, build_router, db_connect};
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

/// Build an app with DEV_MODE auth and return (app, session_cookie).
async fn authed_app() -> (axum::Router, String) {
    let roms_dir   = TempDir::new().unwrap();
    let static_dir = TempDir::new().unwrap();
    let db = db_connect(":memory:").await.unwrap();

    let state = Arc::new(AppState {
        roms_dir:   roms_dir.path().to_path_buf(),
        static_dir: static_dir.path().to_path_buf(),
        db,
        oauth: OAuthConfig {
            client_id:     String::new(),
            client_secret: String::new(),
            redirect_uri:  String::new(),
            jwt_secret:    "test-secret".to_string(),
            cf_access_aud: String::new(),
            cf_certs_url:  String::new(),
            dev_mode:      true,
        },
        http_client: reqwest::Client::new(),
    });

    let login_res = build_router(state.clone())
        .oneshot(Request::builder().uri("/auth/google").body(Body::empty()).unwrap())
        .await
        .unwrap();

    let cookie = login_res
        .headers()
        .get_all("set-cookie")
        .iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            if s.starts_with("rb_session=") { Some(s.split(';').next()?.to_string()) } else { None }
        })
        .expect("no rb_session cookie after dev login");

    let app = build_router(state);
    // TempDirs intentionally kept alive for the duration via leaking — they're in-memory anyway
    std::mem::forget(roms_dir);
    std::mem::forget(static_dir);
    (app, cookie)
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

// ── Battery save endpoint tests ───────────────────────────────────────────────

#[tokio::test]
async fn test_get_battery_save_unauthenticated() {
    let (app, _roms, _static) = test_app(&[]).await;
    let res = app.oneshot(
        Request::builder()
            .uri("/api/battery-saves/pokemon.gb")
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_put_battery_save_unauthenticated() {
    let (app, _roms, _static) = test_app(&[]).await;
    let res = app.oneshot(
        Request::builder()
            .method("PUT").uri("/api/battery-saves/pokemon.gb")
            .header("content-type", "application/octet-stream")
            .body(Body::from(vec![1u8, 2, 3])).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_get_battery_save_not_found() {
    let (app, cookie) = authed_app().await;
    let res = app.oneshot(
        Request::builder()
            .uri("/api/battery-saves/pokemon.gb")
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_put_battery_save_empty_body_rejected() {
    let (app, cookie) = authed_app().await;
    let res = app.oneshot(
        Request::builder()
            .method("PUT").uri("/api/battery-saves/pokemon.gb")
            .header("cookie", &cookie)
            .header("content-type", "application/octet-stream")
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_put_then_get_battery_save() {
    let (app, cookie) = authed_app().await;
    let sram = vec![0xDE, 0xAD, 0xBE, 0xEF];

    // Upload
    let put_res = app.clone().oneshot(
        Request::builder()
            .method("PUT").uri("/api/battery-saves/pokemon.gb")
            .header("cookie", &cookie)
            .header("content-type", "application/octet-stream")
            .body(Body::from(sram.clone())).unwrap()
    ).await.unwrap();
    assert_eq!(put_res.status(), StatusCode::NO_CONTENT);

    // Download
    let get_res = app.oneshot(
        Request::builder()
            .uri("/api/battery-saves/pokemon.gb")
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(get_res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), sram.as_slice());
}

#[tokio::test]
async fn test_put_battery_save_overwrites() {
    let (app, cookie) = authed_app().await;

    // First upload
    app.clone().oneshot(
        Request::builder()
            .method("PUT").uri("/api/battery-saves/zelda.gb")
            .header("cookie", &cookie)
            .header("content-type", "application/octet-stream")
            .body(Body::from(vec![1u8, 2, 3])).unwrap()
    ).await.unwrap();

    // Second upload — should overwrite
    let put_res = app.clone().oneshot(
        Request::builder()
            .method("PUT").uri("/api/battery-saves/zelda.gb")
            .header("cookie", &cookie)
            .header("content-type", "application/octet-stream")
            .body(Body::from(vec![9u8, 8, 7])).unwrap()
    ).await.unwrap();
    assert_eq!(put_res.status(), StatusCode::NO_CONTENT);

    let get_res = app.oneshot(
        Request::builder()
            .uri("/api/battery-saves/zelda.gb")
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    let body = axum::body::to_bytes(get_res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), &[9u8, 8, 7]);
}

// ── Save state endpoint tests ─────────────────────────────────────────────────

#[tokio::test]
async fn test_list_save_states_unauthenticated() {
    let (app, _roms, _static) = test_app(&[]).await;
    let res = app.oneshot(
        Request::builder()
            .uri("/api/save-states/tetris.gb")
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_post_save_state_unauthenticated() {
    let (app, _roms, _static) = test_app(&[]).await;
    let res = app.oneshot(
        Request::builder()
            .method("POST").uri("/api/save-states/tetris.gb")
            .header("content-type", "application/octet-stream")
            .body(Body::from(vec![1u8, 2, 3])).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_list_save_states_empty() {
    let (app, cookie) = authed_app().await;
    let res = app.oneshot(
        Request::builder()
            .uri("/api/save-states/tetris.gb")
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let slots: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert!(slots.is_empty());
}

#[tokio::test]
async fn test_post_save_state_empty_body_rejected() {
    let (app, cookie) = authed_app().await;
    let res = app.oneshot(
        Request::builder()
            .method("POST").uri("/api/save-states/tetris.gb")
            .header("cookie", &cookie)
            .header("content-type", "application/octet-stream")
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_post_then_list_save_state() {
    let (app, cookie) = authed_app().await;
    let blob = vec![0x01u8, 0x02, 0x03, 0x04];

    let post_res = app.clone().oneshot(
        Request::builder()
            .method("POST").uri("/api/save-states/tetris.gb")
            .header("cookie", &cookie)
            .header("content-type", "application/octet-stream")
            .body(Body::from(blob.clone())).unwrap()
    ).await.unwrap();
    assert_eq!(post_res.status(), StatusCode::CREATED);

    let list_res = app.oneshot(
        Request::builder()
            .uri("/api/save-states/tetris.gb")
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(list_res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(list_res.into_body(), usize::MAX).await.unwrap();
    let slots: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(slots.len(), 1);
    assert!(slots[0]["id"].as_str().is_some());
    assert!(slots[0]["updated_at"].as_i64().is_some());
}

#[tokio::test]
async fn test_get_save_state_data() {
    let (app, cookie) = authed_app().await;
    let blob = vec![0xDE, 0xAD, 0xBE, 0xEF];

    // Upload
    let post_res = app.clone().oneshot(
        Request::builder()
            .method("POST").uri("/api/save-states/mario.gb")
            .header("cookie", &cookie)
            .header("content-type", "application/octet-stream")
            .body(Body::from(blob.clone())).unwrap()
    ).await.unwrap();
    assert_eq!(post_res.status(), StatusCode::CREATED);
    let post_body = axum::body::to_bytes(post_res.into_body(), usize::MAX).await.unwrap();
    let meta: serde_json::Value = serde_json::from_slice(&post_body).unwrap();
    let id = meta["id"].as_str().unwrap().to_string();

    // Download
    let get_res = app.oneshot(
        Request::builder()
            .uri(format!("/api/save-states/by-id/{id}/data"))
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let data = axum::body::to_bytes(get_res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(data.as_ref(), blob.as_slice());
}

#[tokio::test]
async fn test_get_save_state_data_not_found() {
    let (app, cookie) = authed_app().await;
    let res = app.oneshot(
        Request::builder()
            .uri("/api/save-states/by-id/00000000-0000-0000-0000-000000000000/data")
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_delete_save_state() {
    let (app, cookie) = authed_app().await;
    let blob = vec![1u8, 2, 3];

    // Upload
    let post_res = app.clone().oneshot(
        Request::builder()
            .method("POST").uri("/api/save-states/wario.gb")
            .header("cookie", &cookie)
            .header("content-type", "application/octet-stream")
            .body(Body::from(blob)).unwrap()
    ).await.unwrap();
    let post_body = axum::body::to_bytes(post_res.into_body(), usize::MAX).await.unwrap();
    let meta: serde_json::Value = serde_json::from_slice(&post_body).unwrap();
    let id = meta["id"].as_str().unwrap().to_string();

    // Delete
    let del_res = app.clone().oneshot(
        Request::builder()
            .method("DELETE").uri(format!("/api/save-states/by-id/{id}"))
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(del_res.status(), StatusCode::NO_CONTENT);

    // Verify gone
    let get_res = app.oneshot(
        Request::builder()
            .uri(format!("/api/save-states/by-id/{id}/data"))
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_latest_save_state() {
    let (app, cookie) = authed_app().await;

    // Upload twice — second should be latest
    app.clone().oneshot(
        Request::builder()
            .method("POST").uri("/api/save-states/link.gb")
            .header("cookie", &cookie)
            .header("content-type", "application/octet-stream")
            .body(Body::from(vec![1u8])).unwrap()
    ).await.unwrap();

    let post2 = app.clone().oneshot(
        Request::builder()
            .method("POST").uri("/api/save-states/link.gb")
            .header("cookie", &cookie)
            .header("content-type", "application/octet-stream")
            .body(Body::from(vec![2u8])).unwrap()
    ).await.unwrap();
    let post2_body = axum::body::to_bytes(post2.into_body(), usize::MAX).await.unwrap();
    let meta2: serde_json::Value = serde_json::from_slice(&post2_body).unwrap();
    let id2 = meta2["id"].as_str().unwrap().to_string();

    let latest_res = app.oneshot(
        Request::builder()
            .uri("/api/save-states/link.gb/latest")
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(latest_res.status(), StatusCode::OK);
    let latest_body = axum::body::to_bytes(latest_res.into_body(), usize::MAX).await.unwrap();
    let latest: serde_json::Value = serde_json::from_slice(&latest_body).unwrap();
    // The second upload has a unique slot_name (timestamp); both may share the same
    // second, but the id of the last insert should match.
    assert_eq!(latest["id"].as_str().unwrap(), id2);
}

#[tokio::test]
async fn test_latest_save_state_not_found() {
    let (app, cookie) = authed_app().await;
    let res = app.oneshot(
        Request::builder()
            .uri("/api/save-states/none.gb/latest")
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_list_roms_with_saves() {
    let (app, cookie) = authed_app().await;

    // Upload saves for two different games
    for rom in &["tetris.gb", "mario.gb"] {
        app.clone().oneshot(
            Request::builder()
                .method("POST").uri(format!("/api/save-states/{rom}"))
                .header("cookie", &cookie)
                .header("content-type", "application/octet-stream")
                .body(Body::from(vec![1u8])).unwrap()
        ).await.unwrap();
    }

    let res = app.oneshot(
        Request::builder()
            .uri("/api/save-states")
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let roms: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(roms.len(), 2);
    let names: Vec<&str> = roms.iter().map(|r| r["rom_name"].as_str().unwrap()).collect();
    assert!(names.contains(&"tetris.gb"));
    assert!(names.contains(&"mario.gb"));
}

#[tokio::test]
async fn test_save_state_forbidden_for_other_user() {
    // Two separate apps = two separate users with separate DBs
    let (app1, cookie1) = authed_app().await;
    let (app2, cookie2) = authed_app().await;

    // User 1 uploads a save
    let post_res = app1.oneshot(
        Request::builder()
            .method("POST").uri("/api/save-states/tetris.gb")
            .header("cookie", &cookie1)
            .header("content-type", "application/octet-stream")
            .body(Body::from(vec![1u8, 2, 3])).unwrap()
    ).await.unwrap();
    let post_body = axum::body::to_bytes(post_res.into_body(), usize::MAX).await.unwrap();
    let meta: serde_json::Value = serde_json::from_slice(&post_body).unwrap();
    let id = meta["id"].as_str().unwrap().to_string();

    // User 2 tries to access it — different DB so it won't exist (NOT_FOUND, not FORBIDDEN)
    let res = app2.oneshot(
        Request::builder()
            .uri(format!("/api/save-states/by-id/{id}/data"))
            .header("cookie", &cookie2)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_battery_saves_are_per_user() {
    // Two separate authed apps (different in-memory DBs) = different users.
    // Each should only see their own save.
    let (app1, cookie1) = authed_app().await;
    let (app2, cookie2) = authed_app().await;

    app1.clone().oneshot(
        Request::builder()
            .method("PUT").uri("/api/battery-saves/pokemon.gb")
            .header("cookie", &cookie1)
            .header("content-type", "application/octet-stream")
            .body(Body::from(vec![0xAAu8])).unwrap()
    ).await.unwrap();

    // app2 has a separate DB — its user hasn't uploaded anything
    let res = app2.oneshot(
        Request::builder()
            .uri("/api/battery-saves/pokemon.gb")
            .header("cookie", &cookie2)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
