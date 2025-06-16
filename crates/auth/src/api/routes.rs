use std::sync::Arc;

use axum::middleware::from_fn;
use axum::routing::{delete, get, post, put};
use axum::{Extension, Router};
use tower_http::cors::CorsLayer;

use super::handlers::client_keys::generate_client_key_handler;
use crate::api::handlers::auth::{
    callback_handler, challenge_handler, login_handler, refresh_token_handler,
    revoke_token_handler, token_handler, validate_handler,
};
use crate::api::handlers::client_keys::{delete_client_handler, list_clients_handler};
use crate::api::handlers::permissions::{
    get_key_permissions_handler, update_key_permissions_handler,
};
use crate::api::handlers::root_keys::{create_key_handler, delete_key_handler, list_keys_handler};
use crate::api::handlers::{
    asset_handler, health_handler, identity_handler, metrics_handler, providers_handler,
};
use crate::auth::middleware::forward_auth_middleware;
use crate::auth::security::{create_body_limit_layer, create_security_headers, RateLimitLayer};
use crate::config::AuthConfig;
use crate::server::AppState;

/// Creates and configures the router with all routes and middleware
pub fn create_router(state: Arc<AppState>, config: &AuthConfig) -> Router {
    // Configure CORS layer
    let cors_layer = if config.cors.allow_all_origins {
        CorsLayer::permissive()
    } else {
        let mut layer = CorsLayer::new();

        // Add allowed origins
        for origin in &config.cors.allowed_origins {
            layer = layer.allow_origin(origin.parse::<axum::http::HeaderValue>().unwrap());
        }

        // Add allowed methods
        let methods: Vec<axum::http::Method> = config
            .cors
            .allowed_methods
            .iter()
            .filter_map(|m| m.parse().ok())
            .collect();
        layer = layer.allow_methods(methods);

        // Add allowed headers
        layer = layer.allow_headers(
            config
                .cors
                .allowed_headers
                .iter()
                .filter_map(|h| h.parse::<axum::http::HeaderName>().ok())
                .collect::<Vec<_>>(),
        );

        layer
    };

    // 1. Public routes (no JWT validation required)
    let public_routes = Router::new()
        // Auth UI/Frontend
        .route("/login", get(login_handler))
        .route("/assets/*path", get(asset_handler)) // Handle static assets
        .route("/favicon.ico", get(asset_handler)) // Handle favicon explicitly
        // Public Auth API endpoints
        .route("/token", post(token_handler))
        .route("/challenge", get(challenge_handler))
        .route("/callback", get(callback_handler))
        .route("/providers", get(providers_handler))
        .route("/health", get(health_handler))
        .route("/refresh", post(refresh_token_handler))
        .route("/validate", get(validate_handler).post(validate_handler));

    // 2. Protected routes (require JWT validation)
    let protected_routes = Router::new()
        // Token operations
        .route("/revoke", post(revoke_token_handler))
        // Root key management
        .route("/keys", get(list_keys_handler))
        .route("/keys", post(create_key_handler))
        .route("/keys/:key_id", delete(delete_key_handler))
        // Client key management
        .route("/keys/clients", get(list_clients_handler))
        .route("/client-key", post(generate_client_key_handler))
        .route(
            "/keys/:key_id/clients/:client_id",
            delete(delete_client_handler),
        )
        // Permission management for both root and client keys
        .route(
            "/keys/:key_id/permissions",
            get(get_key_permissions_handler).put(update_key_permissions_handler),
        )
        // Protected system endpoints
        .route("/identity", get(identity_handler))
        .route("/metrics", get(metrics_handler))
        // Add authentication middleware to all protected routes
        .layer(from_fn(forward_auth_middleware));

    // Create the base router with all routes
    let mut router = Router::new()
        .nest("/public", public_routes)
        .nest("/private", protected_routes)
        .layer(Extension(Arc::clone(&state)));

    // Add security layers from outermost to innermost

    // 1. Add CORS layer first (outermost)
    router = router.layer(cors_layer);

    // 2. Add security headers if enabled
    if config.security.headers.enabled {
        for header_layer in create_security_headers(&config.security.headers) {
            router = router.layer(header_layer);
        }
    }

    // 3. Add rate limiting
    router = router.layer(RateLimitLayer::new(config.security.rate_limit.clone()));

    // 4. Add body size limiting (innermost)
    router = router.layer(create_body_limit_layer(config.security.max_body_size));

    router
}
