/// Security tests covering:
///   1. SameSite=Strict on session cookies
///   2. Token revocation on logout (blocklist)
///   3. CSRF origin check on state-changing POST routes
use axum::{body::Body, http::{Request, StatusCode}};
use rustyboy_web_server::{AppState, auth::OAuthConfig, build_router, db_connect};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

// ── Test helpers ──────────────────────────────────────────────────────────────

/// Build a fresh app and return (app, cookie) sharing the same in-memory DB.
/// The cookie is a valid rb_session obtained via DEV_MODE login.
async fn authed_app() -> (axum::Router, String, TempDir, TempDir) {
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
        },
        http_client: reqwest::Client::new(),
    });

    // Login via DEV_MODE on the same state so the DB is shared
    unsafe { std::env::set_var("DEV_MODE", "1"); }
    let login_res = build_router(state.clone())
        .oneshot(Request::builder().uri("/auth/google").body(Body::empty()).unwrap())
        .await
        .unwrap();
    unsafe { std::env::remove_var("DEV_MODE"); }

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
    (app, cookie, roms_dir, static_dir)
}

fn cookie_header_value(res: &axum::http::Response<Body>, name: &str) -> Option<String> {
    res.headers()
        .get_all("set-cookie")
        .iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            if s.starts_with(&format!("{}=", name)) { Some(s.to_string()) } else { None }
        })
}

// ── Fix 1: SameSite=Strict ────────────────────────────────────────────────────

#[tokio::test]
async fn session_cookie_is_samesite_strict() {
    let (_, cookie_str, _, _) = authed_app().await;
    // The cookie string from set-cookie includes all attributes
    // Re-fetch the full set-cookie value by doing a fresh login
    let roms_dir   = TempDir::new().unwrap();
    let static_dir = TempDir::new().unwrap();
    let db = db_connect(":memory:").await.unwrap();
    let state = Arc::new(AppState {
        roms_dir: roms_dir.path().to_path_buf(),
        static_dir: static_dir.path().to_path_buf(),
        db,
        oauth: OAuthConfig {
            client_id: String::new(), client_secret: String::new(),
            redirect_uri: String::new(), jwt_secret: "s".to_string(),
            cf_access_aud: String::new(), cf_certs_url: String::new(),
        },
        http_client: reqwest::Client::new(),
    });
    unsafe { std::env::set_var("DEV_MODE", "1"); }
    let res = build_router(state)
        .oneshot(Request::builder().uri("/auth/google").body(Body::empty()).unwrap())
        .await.unwrap();
    unsafe { std::env::remove_var("DEV_MODE"); }

    let cookie = cookie_header_value(&res, "rb_session").expect("no rb_session cookie");
    assert!(cookie.contains("SameSite=Strict"), "expected SameSite=Strict, got: {cookie}");
    assert!(!cookie.contains("SameSite=Lax"), "SameSite=Lax must not be present, got: {cookie}");
    let _ = cookie_str;
}

#[tokio::test]
async fn logout_clear_cookie_is_samesite_strict() {
    let (app, cookie, _, _) = authed_app().await;

    let res = app.oneshot(
        Request::builder()
            .method("POST").uri("/auth/logout")
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();

    let set_cookie = cookie_header_value(&res, "rb_session").expect("no rb_session on logout");
    assert!(set_cookie.contains("SameSite=Strict"), "logout clear-cookie must be SameSite=Strict, got: {set_cookie}");
}

// ── Fix 2: Token revocation ───────────────────────────────────────────────────

#[tokio::test]
async fn token_revoked_after_logout() {
    let (app, cookie, _, _) = authed_app().await;

    // Confirm /api/me works before logout
    let res = app.clone().oneshot(
        Request::builder().uri("/api/me").header("cookie", &cookie).body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "/api/me must work before logout");

    // Logout (no Origin header = same-origin, allowed)
    let res = app.clone().oneshot(
        Request::builder().method("POST").uri("/auth/logout")
            .header("cookie", &cookie).body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::FOUND);

    // Old token must now be rejected
    let res = app.oneshot(
        Request::builder().uri("/api/me").header("cookie", &cookie).body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "old token must be rejected after logout");
}

#[tokio::test]
async fn valid_token_not_revoked() {
    let (app, cookie, _, _) = authed_app().await;
    let res = app.oneshot(
        Request::builder().uri("/api/me").header("cookie", &cookie).body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

// ── Fix 3: CSRF origin check ──────────────────────────────────────────────────

#[tokio::test]
async fn logout_rejects_cross_origin() {
    let (app, cookie, _, _) = authed_app().await;

    let res = app.oneshot(
        Request::builder()
            .method("POST").uri("/auth/logout")
            .header("cookie", &cookie)
            .header("origin", "https://evil.example.com")
            .body(Body::empty()).unwrap()
    ).await.unwrap();

    assert_eq!(res.status(), StatusCode::FORBIDDEN, "cross-origin logout must be rejected");
}

#[tokio::test]
async fn logout_allows_no_origin_header() {
    let (app, cookie, _, _) = authed_app().await;

    let res = app.oneshot(
        Request::builder()
            .method("POST").uri("/auth/logout")
            .header("cookie", &cookie)
            .body(Body::empty()).unwrap()
    ).await.unwrap();

    assert_eq!(res.status(), StatusCode::FOUND);
}

#[tokio::test]
async fn logout_allows_same_origin() {
    let (app, cookie, _, _) = authed_app().await;

    let res = app.oneshot(
        Request::builder()
            .method("POST").uri("/auth/logout")
            .header("cookie", &cookie)
            .header("host", "rustyboy.example.com")
            .header("origin", "https://rustyboy.example.com")
            .body(Body::empty()).unwrap()
    ).await.unwrap();

    assert_eq!(res.status(), StatusCode::FOUND);
}
