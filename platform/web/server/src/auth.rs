use axum::{
    async_trait,
    extract::{FromRequestParts, Query, State},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Json, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use jsonwebtoken::{decode, decode_header, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// OAuth config
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub jwt_secret: String,
    /// Cloudflare Access audience tag (Application Audience / AUD).
    /// When non-empty, the `/auth/cf-access` route is active.
    pub cf_access_aud: String,
    /// URL of the Cloudflare Access public-key endpoint.
    /// Defaults to the standard CF endpoint; overrideable for tests.
    pub cf_certs_url: String,
}

impl OAuthConfig {
    pub fn from_env() -> Self {
        let team_domain = std::env::var("CF_TEAM_DOMAIN").unwrap_or_default();
        let cf_certs_url = if team_domain.is_empty() {
            String::new()
        } else {
            format!("https://{}.cloudflareaccess.com/cdn-cgi/access/certs", team_domain)
        };
        Self {
            client_id:     std::env::var("GOOGLE_CLIENT_ID").unwrap_or_default(),
            client_secret: std::env::var("GOOGLE_CLIENT_SECRET").unwrap_or_default(),
            redirect_uri:  std::env::var("OAUTH_REDIRECT_URI").unwrap_or_default(),
            jwt_secret:    std::env::var("JWT_SECRET").unwrap_or_default(),
            cf_access_aud: std::env::var("CF_ACCESS_AUD").unwrap_or_default(),
            cf_certs_url,
        }
    }
}

// ---------------------------------------------------------------------------
// JWT
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    jti: String, // JWT ID — used for revocation
    exp: u64,
}

pub struct JwtPayload {
    pub user_id: String,
    pub jti: String,
    pub exp: u64,
}

pub fn create_jwt(user_id: &str, secret: &str) -> String {
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + 30 * 24 * 60 * 60; // 30 days

    let claims = Claims {
        sub: user_id.to_string(),
        jti: uuid::Uuid::new_v4().to_string(),
        exp,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap_or_default()
}

pub fn verify_jwt(token: &str, secret: &str) -> Result<JwtPayload, ()> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;

    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|data| JwtPayload {
        user_id: data.claims.sub,
        jti: data.claims.jti,
        exp: data.claims.exp,
    })
    .map_err(|_| ())
}

// ---------------------------------------------------------------------------
// Session cookie helpers
// ---------------------------------------------------------------------------

const COOKIE_NAME: &str = "rb_session";

pub fn session_cookie(token: &str) -> String {
    format!(
        "{}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000",
        COOKIE_NAME, token
    )
}

pub fn clear_session_cookie() -> String {
    format!(
        "{}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        COOKIE_NAME
    )
}

// ---------------------------------------------------------------------------
// AuthUser extractor
// ---------------------------------------------------------------------------

pub struct AuthUser {
    pub user_id: String,
    pub jti: String,
    pub exp: u64,
}

/// Extension type used to ferry the JWT secret into FromRequestParts.
#[derive(Clone)]
pub struct JwtSecretExt(pub String);

/// Extension type used to ferry the DB handle into FromRequestParts.
#[derive(Clone)]
pub struct DbExt(pub crate::db::Database);

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let unauthed = || {
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "unauthorized"})),
            )
                .into_response()
        };

        // Read the Cookie header
        let cookie_header = parts
            .headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // Find rb_session=<value>
        let prefix = format!("{}=", COOKIE_NAME);
        let token = match cookie_header
            .split(';')
            .map(|s| s.trim())
            .find_map(|part| part.strip_prefix(prefix.as_str()))
        {
            Some(t) => t.to_string(),
            None => return Err(unauthed()),
        };

        // Retrieve the JWT secret from request extensions.
        let secret = parts
            .extensions
            .get::<JwtSecretExt>()
            .map(|e| e.0.clone())
            .unwrap_or_default();

        let payload = match verify_jwt(&token, &secret) {
            Ok(p) => p,
            Err(_) => return Err(unauthed()),
        };

        // Check revocation list
        if let Some(db_ext) = parts.extensions.get::<DbExt>() {
            match db_ext.0.is_token_revoked(&payload.jti).await {
                Ok(true) => return Err(unauthed()),
                Err(e) => {
                    tracing::error!("revocation check failed: {e}");
                    return Err(unauthed());
                }
                Ok(false) => {}
            }
        }

        Ok(AuthUser {
            user_id: payload.user_id,
            jti: payload.jti,
            exp: payload.exp,
        })
    }
}

// ---------------------------------------------------------------------------
// OAuth serde helpers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct UserInfo {
    sub: String,
    email: String,
    name: String,
    picture: Option<String>,
}

// ---------------------------------------------------------------------------
// Percent-encode a string (for OAuth URL params)
// ---------------------------------------------------------------------------

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Helper: build a redirect response with an optional Set-Cookie header
// ---------------------------------------------------------------------------

fn redirect_response(location: &str, cookie: Option<String>) -> Response {
    let mut builder = axum::http::Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location);

    if let Some(c) = cookie {
        builder = builder.header(header::SET_COOKIE, c);
    }

    builder
        .body(axum::body::Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Derive the OAuth redirect URI from the incoming request's Host header,
/// falling back to the configured `redirect_uri` if the header is absent.
fn redirect_uri_from_request(headers: &axum::http::HeaderMap, configured: &str) -> String {
    if let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) {
        // Strip optional port to check just the hostname
        let hostname = host.split(':').next().unwrap_or(host);

        // Only build a dynamic URI for named hosts (tunnel domains, localhost).
        // Reject bare IPs — Google OAuth won't accept them as redirect URIs,
        // and the configured OAUTH_REDIRECT_URI is the correct value to use.
        let is_ip = hostname.parse::<std::net::IpAddr>().is_ok();
        if !is_ip {
            let scheme = if hostname == "localhost" { "http" } else { "https" };
            return format!("{}://{}/auth/google/callback", scheme, host);
        }
    }
    configured.to_string()
}

pub async fn google_login(
    State(state): State<Arc<crate::AppState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    // DEV_MODE: skip real OAuth, create a local dev user immediately
    if std::env::var("DEV_MODE").is_ok() {
        let user = match state
            .db
            .upsert_user("dev-user", "dev@localhost", "Dev User", None)
            .await
        {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("DEV_MODE: upsert_user failed: {e}");
                return redirect_response("/?auth_error=1", None);
            }
        };
        let token = create_jwt(&user.id, &state.oauth.jwt_secret);
        return redirect_response("/", Some(session_cookie(&token)));
    }

    let cfg = &state.oauth;
    let redirect_uri = redirect_uri_from_request(&headers, &cfg.redirect_uri);
    let url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth\
         ?client_id={}\
         &redirect_uri={}\
         &response_type=code\
         &scope={}\
         &access_type=offline",
        percent_encode(&cfg.client_id),
        percent_encode(&redirect_uri),
        percent_encode("openid email profile"),
    );
    redirect_response(&url, None)
}

#[derive(Deserialize)]
pub struct CallbackParams {
    code: Option<String>,
}

pub async fn google_callback(
    State(state): State<Arc<crate::AppState>>,
    headers: axum::http::HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Response {
    let code = match params.code {
        Some(c) if !c.is_empty() => c,
        _ => return redirect_response("/?auth_error=1", None),
    };

    let cfg = &state.oauth;
    let redirect_uri = redirect_uri_from_request(&headers, &cfg.redirect_uri);

    // Exchange code for tokens
    let token_res = state
        .http_client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", code.as_str()),
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await;

    let token_body: TokenResponse = match token_res {
        Ok(r) => match r.json().await {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("Failed to parse token response: {e}");
                return redirect_response("/?auth_error=1", None);
            }
        },
        Err(e) => {
            tracing::error!("Token exchange request failed: {e}");
            return redirect_response("/?auth_error=1", None);
        }
    };

    // Fetch user info
    let userinfo_res = state
        .http_client
        .get("https://www.googleapis.com/oauth2/v3/userinfo")
        .bearer_auth(&token_body.access_token)
        .send()
        .await;

    let userinfo: UserInfo = match userinfo_res {
        Ok(r) => match r.json().await {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("Failed to parse userinfo: {e}");
                return redirect_response("/?auth_error=1", None);
            }
        },
        Err(e) => {
            tracing::error!("Userinfo request failed: {e}");
            return redirect_response("/?auth_error=1", None);
        }
    };

    // Upsert user in DB
    let user = match state
        .db
        .upsert_user(
            &userinfo.sub,
            &userinfo.email,
            &userinfo.name,
            userinfo.picture.as_deref(),
        )
        .await
    {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("DB upsert_user failed: {e}");
            return redirect_response("/?auth_error=1", None);
        }
    };

    let token = create_jwt(&user.id, &cfg.jwt_secret);
    redirect_response("/", Some(session_cookie(&token)))
}

/// Check that the Origin header, if present, matches the request Host.
/// Returns Err(403) for cross-origin requests.
pub fn check_origin(headers: &axum::http::HeaderMap) -> Result<(), Response> {
    let origin = match headers.get("origin").and_then(|v| v.to_str().ok()) {
        Some(o) => o,
        None => return Ok(()), // no Origin header — same-origin curl/fetch, allow
    };
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // Strip scheme from origin to compare just host:port
    let origin_host = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");

    if origin_host == host {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "cross-origin request rejected").into_response())
    }
}

pub async fn logout(
    auth: AuthUser,
    State(state): State<Arc<crate::AppState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    if let Err(e) = check_origin(&headers) {
        return e;
    }
    if let Err(e) = state.db.revoke_token(&auth.jti, auth.exp as i64).await {
        tracing::error!("failed to revoke token on logout: {e}");
    }
    redirect_response("/", Some(clear_session_cookie()))
}

// ---------------------------------------------------------------------------
// Cloudflare Access handler
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CfClaims {
    email: String,
    sub: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<JwkKey>,
}

#[derive(Deserialize)]
struct JwkKey {
    kid: Option<String>,
    n: String,
    e: String,
}

/// GET /auth/cf-access
///
/// Called automatically by the client when `CF_ACCESS_AUD` is configured.
/// Cloudflare has already authenticated the user and injected a signed JWT
/// in `Cf-Access-Jwt-Assertion`.  We validate it against the team's public
/// keys, then create/update the local user record and set a session cookie.
pub async fn cf_access_login(
    State(state): State<Arc<crate::AppState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let cfg = &state.oauth;

    // CF path disabled when no audience is configured
    if cfg.cf_access_aud.is_empty() || cfg.cf_certs_url.is_empty() {
        return redirect_response("/?auth_error=1", None);
    }

    let jwt = match headers.get("Cf-Access-Jwt-Assertion").and_then(|v| v.to_str().ok()) {
        Some(j) => j.to_string(),
        None => return redirect_response("/?auth_error=1", None),
    };

    // Decode header to get `kid` for key lookup
    let kid = decode_header(&jwt).ok().and_then(|h| h.kid);

    // Fetch JWKS from Cloudflare (or test server)
    let jwks: Jwks = match state.http_client.get(&cfg.cf_certs_url).send().await {
        Ok(r) => match r.json().await {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("CF Access: failed to parse JWKS: {e}");
                return redirect_response("/?auth_error=1", None);
            }
        },
        Err(e) => {
            tracing::error!("CF Access: failed to fetch JWKS: {e}");
            return redirect_response("/?auth_error=1", None);
        }
    };

    // Find the matching key (by kid if present, else try all)
    let matching_keys: Vec<&JwkKey> = jwks.keys.iter().filter(|k| {
        kid.as_deref().map_or(true, |kid| k.kid.as_deref() == Some(kid))
    }).collect();

    let claims = matching_keys.iter().find_map(|key| {
        let n_bytes = URL_SAFE_NO_PAD.decode(&key.n).ok()?;
        let e_bytes = URL_SAFE_NO_PAD.decode(&key.e).ok()?;
        let decoding_key = DecodingKey::from_rsa_raw_components(&n_bytes, &e_bytes);
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[&cfg.cf_access_aud]);
        validation.leeway = 0;
        decode::<CfClaims>(&jwt, &decoding_key, &validation).ok().map(|t| t.claims)
    });

    let claims = match claims {
        Some(c) => c,
        None => {
            tracing::warn!("CF Access: JWT validation failed");
            return redirect_response("/?auth_error=1", None);
        }
    };

    // Prefer an existing user matched by email (e.g. previously logged in via Google)
    // so that CF Access and Google OAuth resolve to the same account.
    let user = if let Ok(Some(existing)) = state.db.get_user_by_email(&claims.email).await {
        existing
    } else {
        // No existing user — create one using the email local-part as display name.
        let display_name = claims.email.split('@').next().unwrap_or(&claims.email);
        match state.db.upsert_user(&claims.sub, &claims.email, display_name, None).await {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("CF Access: upsert_user failed: {e}");
                return redirect_response("/?auth_error=1", None);
            }
        }
    };

    let token = create_jwt(&user.id, &cfg.jwt_secret);
    redirect_response("/", Some(session_cookie(&token)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use std::sync::Arc;

    // ── redirect_uri_from_request ──────────────────────────────────────────

    fn headers_with_host(host: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("host", host.parse().unwrap());
        h
    }

    #[test]
    fn redirect_uri_uses_host_for_named_domain() {
        let h = headers_with_host("rustyboy.example.com");
        let uri = redirect_uri_from_request(&h, "https://fallback.example.com/auth/google/callback");
        assert_eq!(uri, "https://rustyboy.example.com/auth/google/callback");
    }

    #[test]
    fn redirect_uri_uses_http_for_localhost() {
        let h = headers_with_host("localhost:8080");
        let uri = redirect_uri_from_request(&h, "https://fallback.example.com/auth/google/callback");
        assert_eq!(uri, "http://localhost:8080/auth/google/callback");
    }

    #[test]
    fn redirect_uri_falls_back_for_ipv4() {
        // Private IPs are rejected by Google OAuth; must use the configured URI.
        let h = headers_with_host("192.168.2.254:9002");
        let configured = "https://rustyboy.example.com/auth/google/callback";
        let uri = redirect_uri_from_request(&h, configured);
        assert_eq!(uri, configured);
    }

    #[test]
    fn redirect_uri_falls_back_for_loopback_ip() {
        let h = headers_with_host("127.0.0.1:8080");
        let configured = "https://rustyboy.example.com/auth/google/callback";
        let uri = redirect_uri_from_request(&h, configured);
        assert_eq!(uri, configured);
    }

    #[test]
    fn redirect_uri_uses_configured_when_no_host_header() {
        let h = HeaderMap::new();
        let configured = "https://rustyboy.example.com/auth/google/callback";
        let uri = redirect_uri_from_request(&h, configured);
        assert_eq!(uri, configured);
    }

    // ── CF Access handler ──────────────────────────────────────────────────

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use rsa::traits::PublicKeyParts;
    use rsa::RsaPrivateKey;
    use serde_json::json;
    use tower::ServiceExt;

    async fn test_app(oauth: OAuthConfig) -> axum::Router {
        let db = crate::db::Database::connect(":memory:").await.unwrap();
        let state = Arc::new(crate::AppState {
            roms_dir: std::path::PathBuf::from("/tmp"),
            static_dir: std::path::PathBuf::from("/tmp"),
            db,
            oauth,
            http_client: reqwest::Client::new(),
        });
        crate::build_router(state)
    }

    fn make_cf_jwt(private_key: &RsaPrivateKey, aud: &str, sub: &str, email: &str) -> String {
        #[derive(serde::Serialize)]
        struct Claims<'a> {
            sub: &'a str,
            email: &'a str,
            aud: &'a str,
            exp: u64,
        }
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        let claims = Claims { sub, email, aud, exp };
        let pem = rsa::pkcs1::EncodeRsaPrivateKey::to_pkcs1_pem(private_key, rsa::pkcs1::LineEnding::LF)
            .unwrap();
        let encoding_key = EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-key-1".to_string());
        encode(&header, &claims, &encoding_key).unwrap()
    }

    fn jwks_json(private_key: &RsaPrivateKey) -> serde_json::Value {
        let pub_key = private_key.to_public_key();
        let n = URL_SAFE_NO_PAD.encode(pub_key.n().to_bytes_be());
        let e = URL_SAFE_NO_PAD.encode(pub_key.e().to_bytes_be());
        json!({ "keys": [{ "kid": "test-key-1", "kty": "RSA", "n": n, "e": e }] })
    }

    /// Spawn a tiny HTTP server that returns `body` for every GET, return its base URL.
    async fn mock_jwks_server(body: serde_json::Value) -> String {
        use std::convert::Infallible;
        use axum::response::Json as AxumJson;

        let app = axum::Router::new().route(
            "/certs",
            axum::routing::get(move || {
                let b = body.clone();
                async move { Result::<AxumJson<serde_json::Value>, Infallible>::Ok(AxumJson(b)) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{}/certs", addr)
    }

    #[tokio::test]
    async fn cf_access_login_no_jwt_header_redirects_auth_error() {
        let app = test_app(OAuthConfig {
            client_id: String::new(),
            client_secret: String::new(),
            redirect_uri: String::new(),
            jwt_secret: "secret".to_string(),
            cf_access_aud: "test-aud".to_string(),
            cf_certs_url: "http://localhost:1/certs".to_string(), // unreachable — shouldn't be called
        })
        .await;

        let res = app
            .oneshot(Request::get("/auth/cf-access").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::FOUND);
        let location = res.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("auth_error"), "expected auth_error redirect, got: {location}");
    }

    #[tokio::test]
    async fn cf_access_login_valid_jwt_sets_session_cookie() {
        let mut rng = rand::thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let aud = "test-aud";
        let jwt = make_cf_jwt(&private_key, aud, "cf-sub-1", "user@example.com");
        let jwks_url = mock_jwks_server(jwks_json(&private_key)).await;

        let app = test_app(OAuthConfig {
            client_id: String::new(),
            client_secret: String::new(),
            redirect_uri: String::new(),
            jwt_secret: "secret".to_string(),
            cf_access_aud: aud.to_string(),
            cf_certs_url: jwks_url,
        })
        .await;

        let res = app
            .oneshot(
                Request::get("/auth/cf-access")
                    .header("Cf-Access-Jwt-Assertion", jwt)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::FOUND);
        let location = res.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(location, "/", "should redirect to / on success");
        let cookie = res.headers().get("set-cookie").unwrap().to_str().unwrap();
        assert!(cookie.contains("rb_session="), "should set session cookie");
    }

    #[tokio::test]
    async fn cf_access_login_reuses_existing_google_user() {
        // Pre-create a user via Google (different sub, same email).
        // CF login should issue a session for that same user record.
        let mut rng = rand::thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let aud = "test-aud";
        let email = "shared@example.com";
        let jwt = make_cf_jwt(&private_key, aud, "cf-sub-shared", email);
        let jwks_url = mock_jwks_server(jwks_json(&private_key)).await;

        let db = crate::db::Database::connect(":memory:").await.unwrap();
        // Simulate prior Google login
        let google_user = db
            .upsert_user("google-sub-shared", email, "Shared User", None)
            .await
            .unwrap();

        let state = Arc::new(crate::AppState {
            roms_dir: std::path::PathBuf::from("/tmp"),
            static_dir: std::path::PathBuf::from("/tmp"),
            db: db.clone(),
            oauth: OAuthConfig {
                client_id: String::new(),
                client_secret: String::new(),
                redirect_uri: String::new(),
                jwt_secret: "secret".to_string(),
                cf_access_aud: aud.to_string(),
                cf_certs_url: jwks_url,
            },
            http_client: reqwest::Client::new(),
        });
        let app = crate::build_router(state);

        let res = app
            .oneshot(
                Request::get("/auth/cf-access")
                    .header("Cf-Access-Jwt-Assertion", jwt)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::FOUND);
        let cookie = res.headers().get("set-cookie").unwrap().to_str().unwrap();
        assert!(cookie.contains("rb_session="));

        // Extract the JWT from the cookie and verify it resolves to the Google user
        let token = cookie
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches("rb_session=");
        let payload = verify_jwt(token, "secret").unwrap();
        assert_eq!(
            payload.user_id, google_user.id,
            "CF login should reuse the existing Google user id"
        );

        // Only one user record should exist for this email
        let by_email = db.get_user_by_email(email).await.unwrap().unwrap();
        assert_eq!(by_email.id, google_user.id);
        assert_eq!(by_email.display_name, "Shared User");
    }
}
