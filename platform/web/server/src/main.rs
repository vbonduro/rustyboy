use rustyboy_web_server::{AppState, auth::OAuthConfig, build_router, config, db_connect};
use std::{path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() {
    // Load bundled secrets file before anything else reads env vars
    config::load_secrets_file();

    tracing_subscriber::fmt::init();

    let roms_dir = PathBuf::from(
        std::env::var("ROMS_DIR").unwrap_or_else(|_| "/roms".to_string()),
    );
    let static_dir = PathBuf::from(
        std::env::var("STATIC_DIR").unwrap_or_else(|_| "/static".to_string()),
    );
    let db_path = std::env::var("DB_PATH").unwrap_or_else(|_| "/data/rustyboy.db".to_string());

    let db = db_connect(&db_path)
        .await
        .expect("Failed to connect to database");

    let team_domain = std::env::var("CF_TEAM_DOMAIN").unwrap_or_default();
    let cf_certs_url = if team_domain.is_empty() {
        String::new()
    } else {
        format!("https://{}.cloudflareaccess.com/cdn-cgi/access/certs", team_domain)
    };

    let oauth = OAuthConfig {
        client_id:     config::get_secret("GOOGLE_CLIENT_ID"),
        client_secret: config::get_secret("GOOGLE_CLIENT_SECRET"),
        redirect_uri:  config::get_secret("OAUTH_REDIRECT_URI"),
        jwt_secret:    config::get_secret("JWT_SECRET"),
        cf_access_aud: config::get_secret("CF_ACCESS_AUD"),
        cf_certs_url,
        dev_mode:      std::env::var("DEV_MODE").is_ok(),
    };

    tracing::info!(
        "auth config: dev_mode={} google_client_id={} redirect_uri={:?} cf_access_aud={} cf_certs_url={}",
        oauth.dev_mode,
        if oauth.client_id.is_empty() { "(not set)" } else { "(set)" },
        if oauth.redirect_uri.is_empty() { "(not set — will derive from Host header)" } else { &oauth.redirect_uri },
        if oauth.cf_access_aud.is_empty() { "(not set)" } else { "(set)" },
        if oauth.cf_certs_url.is_empty() { "(not set)" } else { "(set)" },
    );
    let http_client = reqwest::Client::new();

    let state = Arc::new(AppState {
        roms_dir,
        static_dir,
        db,
        oauth,
        http_client,
    });

    let app = build_router(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await.unwrap();
    tracing::info!("Listening on http://0.0.0.0:{}", port);
    axum::serve(listener, app).await.unwrap();
}
