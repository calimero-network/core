//! Drives the real `create_router` so the permission check sees the path the
//! middleware actually receives under `.nest("/admin", ...)`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
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

async fn router_and_token(perms: &[&str]) -> (axum::Router, String) {
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let secrets = Arc::new(SecretManager::new(Arc::clone(&storage)));
    secrets.initialize().await.unwrap();
    let config = default_config();
    let tokens = TokenManager::new(config.jwt.clone(), Arc::clone(&storage), secrets);
    let keys = KeyManager::new(Arc::clone(&storage));

    let perms: Vec<String> = perms.iter().map(|p| (*p).to_owned()).collect();
    let key = Key::new_root_key_with_permissions(
        "pk-low".to_owned(),
        "user_password".to_owned(),
        perms.clone(),
        None,
    );
    let _ = keys.set_key("low", &key).await.unwrap();
    let victim = Key::new_root_key_with_permissions(
        "pk-victim".to_owned(),
        "user_password".to_owned(),
        vec!["admin".to_owned(), "context:list".to_owned()],
        None,
    );
    let _ = keys.set_key("victim", &victim).await.unwrap();
    let (access, _) = tokens
        .generate_token_pair("low".to_owned(), perms, None, None)
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
    });
    (create_router(state, &config), access)
}

async fn status(perms: &[&str], method: Method, uri: &str, body: &str) -> StatusCode {
    let (router, token) = router_and_token(perms).await;
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap();
    router.oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn admin_route_requires_admin_permission_post_keys() {
    let body = r#"{"public_key":"x","auth_method":"user_password","provider_data":{}}"#;
    let got = status(&["keys:create"], Method::POST, "/admin/keys", body).await;
    assert_eq!(
        got,
        StatusCode::FORBIDDEN,
        "keys:create token reached POST /admin/keys"
    );
}

#[tokio::test]
async fn admin_route_requires_admin_permission_put_key_permissions() {
    let body = r#"{"remove":["admin"]}"#;
    let got = status(
        &["context:list"],
        Method::PUT,
        "/admin/keys/victim/permissions",
        body,
    )
    .await;
    assert_eq!(
        got,
        StatusCode::FORBIDDEN,
        "context:list token reached PUT /admin/keys/victim/permissions"
    );
}

#[tokio::test]
async fn admin_route_requires_keys_delete_for_revoke() {
    let got = status(
        &["context:list"],
        Method::POST,
        "/admin/revoke",
        r#"{"client_id":"victim"}"#,
    )
    .await;
    assert_eq!(
        got,
        StatusCode::FORBIDDEN,
        "context:list token reached POST /admin/revoke"
    );
}

#[tokio::test]
async fn admin_token_passes_the_admin_route_gate() {
    let got = status(
        &["admin"],
        Method::PUT,
        "/admin/keys/victim/permissions",
        r#"{"remove":["context:list"]}"#,
    )
    .await;
    assert_eq!(
        got,
        StatusCode::OK,
        "control: an admin token reaches the handler"
    );
}
