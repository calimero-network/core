//! A proxy's `GET /auth/validate` takes a token from the forwarded URI's query only on
//! the streaming routes a browser cannot set a header on, as the embedded guard does.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use mero_auth::api::routes::create_router;
use mero_auth::auth::rate_limit::LoginRateLimiter;
use mero_auth::auth::token::TokenManager;
use mero_auth::embedded::default_config;
use mero_auth::secrets::SecretManager;
use mero_auth::server::AppState;
use mero_auth::storage::{Key, KeyManager, MemoryStorage, Storage};
use mero_auth::utils::AuthMetrics;
use mero_auth::AuthService;
use tower::ServiceExt;

async fn router_and_token() -> (axum::Router, String) {
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let secrets = Arc::new(SecretManager::new(Arc::clone(&storage)));
    secrets.initialize().await.unwrap();
    let config = default_config();
    let tokens = TokenManager::new(config.jwt.clone(), Arc::clone(&storage), secrets);
    let keys = KeyManager::new(Arc::clone(&storage));
    let perms = vec!["admin".to_owned()];
    let key = Key::new_root_key_with_permissions(
        "pk-owner".to_owned(),
        "user_password".to_owned(),
        perms.clone(),
        None,
    );
    let _ = keys.set_key("owner", &key).await.unwrap();
    let (access, _) = tokens
        .generate_token_pair("owner".to_owned(), perms, None, None)
        .await
        .unwrap();
    let state = Arc::new(AppState {
        auth_service: AuthService::new(vec![], tokens.clone()),
        storage,
        key_manager: keys,
        token_generator: tokens,
        config: config.clone(),
        metrics: AuthMetrics::new(),
        login_rate_limiter: Arc::new(LoginRateLimiter::default()),
        account_rate_limiter: Arc::new(LoginRateLimiter::account_ceiling()),
    });
    (create_router(state, &config), access)
}

/// The proxy's probe for `method uri`, with the caller's `Authorization` header if any.
async fn probe_with(
    method: &str,
    uri: &str,
    authorization: Option<&str>,
) -> (StatusCode, Option<String>) {
    let (router, token) = router_and_token().await;
    let uri = uri.replace("{token}", &token);
    let mut request = Request::get("/auth/validate")
        .header("X-Forwarded-Uri", uri)
        .header("X-Forwarded-Method", method)
        .header(header::HOST, "localhost");
    if let Some(authorization) = authorization {
        request = request.header(header::AUTHORIZATION, authorization);
    }
    let request = request.body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();
    let error = response
        .headers()
        .get("X-Auth-Error")
        .map(|value| value.to_str().unwrap().to_owned());
    (response.status(), error)
}

async fn probe(method: &str, uri: &str) -> (StatusCode, Option<String>) {
    probe_with(method, uri, None).await
}

#[tokio::test]
async fn a_query_token_does_not_authenticate_a_forwarded_route_that_takes_a_header() {
    for (method, uri) in [
        ("GET", "/admin-api/contexts?token={token}"),
        ("DELETE", "/admin-api/contexts/ctx-1?token={token}"),
        ("GET", "/admin-api/contexts?all=1&token={token}"),
        ("POST", "/jsonrpc?token={token}"),
        ("POST", "/admin/client-key?token={token}"),
        ("GET", "/wss?token={token}"),
        ("GET", "/ws/x?token={token}"),
        ("GET", "/node1/ws?token={token}"),
        ("GET", "/admin-api/sse?token={token}"),
        ("GET", "/sse/../admin-api/contexts?token={token}"),
        ("GET", "/sse/session/../../admin-api/contexts?token={token}"),
        ("GET", "/sse/%2e%2e/admin-api/contexts?token={token}"),
    ] {
        assert_eq!(
            probe(method, uri).await,
            (StatusCode::UNAUTHORIZED, Some("missing_token".to_owned())),
            "{method} {uri}",
        );
    }
}

#[tokio::test]
async fn a_query_token_still_authenticates_the_streaming_routes() {
    for (method, uri) in [
        ("GET", "/ws?token={token}"),
        ("GET", "/sse?token={token}"),
        ("GET", "/sse?a=1&token={token}"),
        ("GET", "/sse/session/abc?token={token}"),
        ("POST", "/sse/subscription?token={token}"),
    ] {
        assert_eq!(
            probe(method, uri).await,
            (StatusCode::OK, None),
            "{method} {uri}"
        );
    }
}

#[tokio::test]
async fn an_authorization_header_is_the_only_token_even_when_it_is_not_a_bearer() {
    for authorization in ["Basic dXNlcjpwdw==", "bearer x", "Token x"] {
        let (status, _) = probe_with("GET", "/ws?token={token}", Some(authorization)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{authorization}");
    }
}

#[tokio::test]
async fn a_bearer_header_wins_over_a_url_token() {
    let (router, token) = router_and_token().await;
    let request = Request::get("/auth/validate")
        .header("X-Forwarded-Uri", "/ws?token=garbage")
        .header("X-Forwarded-Method", "GET")
        .header(header::HOST, "localhost")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        router.oneshot(request).await.unwrap().status(),
        StatusCode::OK
    );
}
