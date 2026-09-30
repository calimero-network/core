//! `/auth/mock-token` exists only in debug builds.
#![cfg(debug_assertions)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use mero_auth::api::routes::create_router;
use mero_auth::auth::rate_limit::LoginRateLimiter;
use mero_auth::auth::token::TokenManager;
use mero_auth::embedded::default_config;
use mero_auth::secrets::SecretManager;
use mero_auth::server::AppState;
use mero_auth::storage::{KeyManager, MemoryStorage, Storage};
use mero_auth::utils::AuthMetrics;
use mero_auth::AuthService;
use tower::ServiceExt;

async fn mock_token(origin: Option<&str>) -> StatusCode {
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let secrets = Arc::new(SecretManager::new(Arc::clone(&storage)));
    secrets.initialize().await.unwrap();
    let mut config = default_config();
    config.development.enable_mock_auth = true;
    config.development.mock_auth_require_header = false;
    let tokens = TokenManager::new(config.jwt.clone(), Arc::clone(&storage), secrets);
    let state = Arc::new(AppState {
        auth_service: AuthService::new(vec![], tokens.clone()),
        key_manager: KeyManager::new(Arc::clone(&storage)),
        storage,
        token_generator: tokens,
        config: config.clone(),
        metrics: AuthMetrics::new(),
        login_rate_limiter: Arc::new(LoginRateLimiter::default()),
    });
    let mut request =
        Request::post("/auth/mock-token").header(header::CONTENT_TYPE, "application/json");
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    create_router(state, &config)
        .oneshot(request.body(Body::from(r#"{"client_name":"ci"}"#)).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn the_mock_token_endpoint_serves_scripts_but_no_browser_page() {
    assert_eq!(mock_token(None).await, StatusCode::OK);
    assert_eq!(
        mock_token(Some("https://evil.example")).await,
        StatusCode::FORBIDDEN
    );
}
