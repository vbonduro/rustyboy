/// Cloudflare Access authentication tests.
///
/// These tests spin up a tiny in-process JWKS server so we never hit the real
/// Cloudflare certs endpoint.  Each test:
///   1. Generates a fresh RSA-2048 keypair.
///   2. Starts a local axum server that serves the public key as a JWKS.
///   3. Builds the app with `cf_certs_url` pointing at that local server.
///   4. Signs a CF-style JWT with the private key and sends it as the
///      `Cf-Access-Jwt-Assertion` header.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Json,
    routing::get,
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::rngs::OsRng;
use rsa::{
    pkcs1v15::SigningKey,
    signature::{RandomizedSigner, SignatureEncoding},
    traits::PublicKeyParts,
    RsaPrivateKey,
};
use rustyboy_web_server::{AppState, auth::OAuthConfig, build_router};
use serde_json::json;
use rsa::sha2::Sha256;
use std::{net::SocketAddr, sync::Arc};
use tempfile::TempDir;
use tower::ServiceExt;

// ── Helpers ──────────────────────────────────────────────────────────────────

struct TestKeys {
    private_key: RsaPrivateKey,
    kid: String,
}

impl TestKeys {
    fn generate() -> Self {
        let private_key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        Self { private_key, kid: "test-key-1".to_string() }
    }

    /// Build a minimal JWKS JSON document with the public key.
    fn jwks(&self) -> serde_json::Value {
        let pub_key = self.private_key.to_public_key();
        let n = URL_SAFE_NO_PAD.encode(pub_key.n().to_bytes_be());
        let e = URL_SAFE_NO_PAD.encode(pub_key.e().to_bytes_be());
        json!({
            "keys": [{
                "kty": "RSA",
                "use": "sig",
                "alg": "RS256",
                "kid": self.kid,
                "n": n,
                "e": e,
            }]
        })
    }

    /// Sign a minimal CF Access JWT (RS256, header `kid` matches).
    fn sign_jwt(&self, email: &str, aud: &str, exp_offset_secs: i64) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let exp = now + exp_offset_secs;

        let header = json!({"alg": "RS256", "typ": "JWT", "kid": self.kid});
        let claims = json!({
            "sub": email,
            "email": email,
            "aud": [aud],
            "iss": "https://test.cloudflareaccess.com",
            "iat": now,
            "exp": exp,
        });

        let header_b64  = URL_SAFE_NO_PAD.encode(header.to_string());
        let claims_b64  = URL_SAFE_NO_PAD.encode(claims.to_string());
        let signing_input = format!("{}.{}", header_b64, claims_b64);

        let signing_key: SigningKey<Sha256> = SigningKey::new(self.private_key.clone());
        let sig = signing_key
            .sign_with_rng(&mut OsRng, signing_input.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_bytes());

        format!("{}.{}", signing_input, sig_b64)
    }
}

/// Spawn a local HTTP server that serves `jwks` at `/cdn-cgi/access/certs`.
/// Returns the base URL (e.g. `http://127.0.0.1:PORT`).
async fn spawn_jwks_server(jwks: serde_json::Value) -> String {
    let router = Router::new().route(
        "/cdn-cgi/access/certs",
        get(move || {
            let jwks = jwks.clone();
            async move { Json(jwks) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://127.0.0.1:{}", addr.port())
}

async fn make_app(cf_aud: Option<&str>, certs_base_url: Option<&str>) -> (axum::Router, TempDir, TempDir) {
    let roms_dir   = TempDir::new().unwrap();
    let static_dir = TempDir::new().unwrap();
    let db = rustyboy_web_server::db_connect(":memory:").await.unwrap();

    let certs_url = certs_base_url
        .map(|base| format!("{}/cdn-cgi/access/certs", base));

    let state = Arc::new(AppState {
        roms_dir:   roms_dir.path().to_path_buf(),
        static_dir: static_dir.path().to_path_buf(),
        db,
        oauth: OAuthConfig {
            client_id:     String::new(),
            client_secret: String::new(),
            redirect_uri:  String::new(),
            jwt_secret:    "test-secret".to_string(),
            cf_access_aud: cf_aud.unwrap_or("").to_string(),
            cf_certs_url:  certs_url.unwrap_or_default(),
        },
        http_client: reqwest::Client::new(),
    });

    (build_router(state), roms_dir, static_dir)
}

fn has_session_cookie(response: &axum::http::Response<Body>) -> bool {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .any(|v| v.to_str().unwrap_or("").starts_with("rb_session="))
}

fn redirect_location(response: &axum::http::Response<Body>) -> Option<String> {
    response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// A valid CF JWT → server sets a session cookie and redirects to `/`.
#[tokio::test]
async fn cf_valid_jwt_sets_session_cookie() {
    let keys     = TestKeys::generate();
    let base_url = spawn_jwks_server(keys.jwks()).await;
    let aud      = "test-audience-tag";
    let jwt      = keys.sign_jwt("alice@example.com", aud, 3600);

    let (app, _r, _s) = make_app(Some(aud), Some(&base_url)).await;

    let req = Request::builder()
        .uri("/auth/cf-access")
        .header("Cf-Access-Jwt-Assertion", jwt)
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FOUND);
    assert!(has_session_cookie(&res), "expected rb_session cookie");
    assert_eq!(redirect_location(&res).as_deref(), Some("/"));
}

/// CF_ACCESS_AUD is configured but the request has no CF header → no cookie,
/// request continues to normal (unauthenticated) flow.
#[tokio::test]
async fn cf_missing_header_no_cookie() {
    let keys     = TestKeys::generate();
    let base_url = spawn_jwks_server(keys.jwks()).await;
    let aud      = "test-audience-tag";

    let (app, _r, _s) = make_app(Some(aud), Some(&base_url)).await;

    let req = Request::builder()
        .uri("/auth/cf-access")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    // No CF header → redirect to error (not authed)
    assert!(!has_session_cookie(&res), "must not set session cookie without CF header");
}

/// A CF JWT with a bad signature → rejected, redirected to auth error.
#[tokio::test]
async fn cf_tampered_jwt_rejected() {
    let keys     = TestKeys::generate();
    let base_url = spawn_jwks_server(keys.jwks()).await;
    let aud      = "test-audience-tag";

    // Sign with a *different* key so the signature won't verify
    let other_keys = TestKeys::generate();
    let bad_jwt    = other_keys.sign_jwt("evil@example.com", aud, 3600);

    let (app, _r, _s) = make_app(Some(aud), Some(&base_url)).await;

    let req = Request::builder()
        .uri("/auth/cf-access")
        .header("Cf-Access-Jwt-Assertion", bad_jwt)
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert!(!has_session_cookie(&res), "tampered JWT must not set session cookie");
    assert_eq!(res.status(), StatusCode::FOUND);
    assert!(
        redirect_location(&res).as_deref().unwrap_or("").contains("auth_error"),
        "expected auth_error redirect"
    );
}

/// An expired CF JWT → rejected.
#[tokio::test]
async fn cf_expired_jwt_rejected() {
    let keys     = TestKeys::generate();
    let base_url = spawn_jwks_server(keys.jwks()).await;
    let aud      = "test-audience-tag";
    let jwt      = keys.sign_jwt("alice@example.com", aud, -3600); // expired an hour ago

    let (app, _r, _s) = make_app(Some(aud), Some(&base_url)).await;

    let req = Request::builder()
        .uri("/auth/cf-access")
        .header("Cf-Access-Jwt-Assertion", jwt)
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert!(!has_session_cookie(&res), "expired JWT must not set session cookie");
    assert!(
        redirect_location(&res).as_deref().unwrap_or("").contains("auth_error"),
        "expected auth_error redirect"
    );
}

/// Wrong audience in JWT → rejected.
#[tokio::test]
async fn cf_wrong_audience_rejected() {
    let keys     = TestKeys::generate();
    let base_url = spawn_jwks_server(keys.jwks()).await;
    let aud      = "correct-audience";
    let jwt      = keys.sign_jwt("alice@example.com", "wrong-audience", 3600);

    let (app, _r, _s) = make_app(Some(aud), Some(&base_url)).await;

    let req = Request::builder()
        .uri("/auth/cf-access")
        .header("Cf-Access-Jwt-Assertion", jwt)
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert!(!has_session_cookie(&res), "wrong audience must not set session cookie");
    assert!(
        redirect_location(&res).as_deref().unwrap_or("").contains("auth_error"),
        "expected auth_error redirect"
    );
}

/// When CF_ACCESS_AUD is empty/unset, the CF header is ignored entirely.
/// The request should proceed without a session cookie (falls through to
/// normal auth).
#[tokio::test]
async fn cf_disabled_when_no_aud_configured() {
    let keys = TestKeys::generate();
    // Point at a real certs URL but aud is empty → CF path should not activate
    let base_url = spawn_jwks_server(keys.jwks()).await;
    let jwt      = keys.sign_jwt("alice@example.com", "any-aud", 3600);

    let (app, _r, _s) = make_app(None, Some(&base_url)).await;

    let req = Request::builder()
        .uri("/auth/cf-access")
        .header("Cf-Access-Jwt-Assertion", jwt)
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert!(!has_session_cookie(&res), "CF path must not activate when aud is not configured");
}

/// After CF auth succeeds, /api/me returns the user's data.
#[tokio::test]
async fn cf_valid_jwt_user_visible_in_api_me() {
    let keys     = TestKeys::generate();
    let base_url = spawn_jwks_server(keys.jwks()).await;
    let aud      = "test-audience-tag";
    let jwt      = keys.sign_jwt("alice@example.com", aud, 3600);

    let (app, _r, _s) = make_app(Some(aud), Some(&base_url)).await;

    // First request: authenticate via CF
    let auth_req = Request::builder()
        .uri("/auth/cf-access")
        .header("Cf-Access-Jwt-Assertion", jwt)
        .body(Body::empty())
        .unwrap();
    let auth_res = app.clone().oneshot(auth_req).await.unwrap();
    assert_eq!(auth_res.status(), StatusCode::FOUND);

    // Extract the session cookie
    let cookie = auth_res
        .headers()
        .get_all("set-cookie")
        .iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            if s.starts_with("rb_session=") { Some(s.split(';').next()?.to_string()) } else { None }
        })
        .expect("no session cookie");

    // Second request: use the cookie to call /api/me
    let me_req = Request::builder()
        .uri("/api/me")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let me_res = app.oneshot(me_req).await.unwrap();
    assert_eq!(me_res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(me_res.into_body(), usize::MAX).await.unwrap();
    let user: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(user["email"], "alice@example.com");
}
