//! `GET /auth/providers` publishes the node's `trusted_callback_origins`, which
//! the login page (auth-frontend) adds to the origins it hands tokens to. A
//! Vercel preview of an app runs on an origin the app registry does not
//! declare, so without this its login is refused with "Login callback
//! destination is not allowed." and the node's owner cannot allow it.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use mero_auth::api::routes::create_router;
use mero_auth::auth::rate_limit::LoginRateLimiter;
use mero_auth::auth::token::TokenManager;
use mero_auth::config::AuthConfig;
use mero_auth::embedded::default_config;
use mero_auth::secrets::SecretManager;
use mero_auth::server::AppState;
use mero_auth::storage::{KeyManager, MemoryStorage, Storage};
use mero_auth::utils::AuthMetrics;
use mero_auth::AuthService;
use serde_json::Value;
use tower::ServiceExt;

async fn providers(config: AuthConfig) -> Value {
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let secrets = Arc::new(SecretManager::new(Arc::clone(&storage)));
    secrets.initialize().await.unwrap();
    let tokens = TokenManager::new(config.jwt.clone(), Arc::clone(&storage), secrets);
    let state = Arc::new(AppState {
        auth_service: AuthService::new(vec![], tokens.clone()),
        key_manager: KeyManager::new(Arc::clone(&storage)),
        storage,
        token_generator: tokens,
        config: config.clone(),
        metrics: AuthMetrics::new(),
        login_rate_limiter: Arc::new(LoginRateLimiter::default()),
        account_rate_limiter: Arc::new(LoginRateLimiter::account_ceiling()),
    });
    let response = create_router(state, &config)
        .oneshot(Request::get("/auth/providers").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn origins(body: &Value) -> Vec<String> {
    let data = body.get("data").unwrap_or(body);
    serde_json::from_value(data["trusted_callback_origins"].clone()).unwrap()
}

#[tokio::test]
async fn a_node_with_no_list_publishes_an_empty_one() {
    assert!(origins(&providers(default_config()).await).is_empty());
}

#[tokio::test]
async fn the_configured_origins_are_published() {
    let mut config = default_config();
    config.development.trusted_callback_origins = vec![
        "https://mero-crm-git-feature-calimero-network.vercel.app".to_owned(),
        "http://localhost:5173".to_owned(),
    ];
    assert_eq!(
        origins(&providers(config).await),
        [
            "https://mero-crm-git-feature-calimero-network.vercel.app",
            "http://localhost:5173",
        ]
    );
}

#[tokio::test]
async fn anything_but_an_exact_http_origin_is_left_out() {
    let mut config = default_config();
    config.development.trusted_callback_origins = vec![
        "https://ok.example/".to_owned(),
        " https://ok.example ".to_owned(),
        "https://*.vercel.app".to_owned(),
        "https://with.path.example/login".to_owned(),
        "https://q.example/?a=1".to_owned(),
        "https://user:pw@creds.example".to_owned(),
        "javascript:alert(1)".to_owned(),
        "file:///etc/passwd".to_owned(),
        "not a url".to_owned(),
        "HTTPS://Upper.Example:443".to_owned(),
    ];
    assert_eq!(
        origins(&providers(config).await),
        ["https://ok.example", "https://upper.example"]
    );
}

#[test]
fn an_old_config_without_the_field_still_parses() {
    let parsed: mero_auth::config::DevelopmentConfig =
        toml::from_str("enable_mock_auth = false\nmock_auth_require_header = true\n").unwrap();
    assert!(parsed.trusted_callback_origins.is_empty());
}

#[test]
fn an_empty_list_is_not_written_back_into_config_toml() {
    let written = toml::to_string(&default_config().development).unwrap();
    assert!(!written.contains("trusted_callback_origins"), "{written}");
}
