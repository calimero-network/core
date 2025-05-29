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
    get_key_permissions_handler, list_permissions_handler, update_key_permissions_handler,
};
use crate::api::handlers::root_keys::{create_key_handler, delete_key_handler, list_keys_handler};
use crate::api::handlers::{
    asset_handler, health_handler, identity_handler, metrics_handler, providers_handler,
};
use crate::auth::middleware::forward_auth_middleware;
// use crate::auth::security::{
//     create_body_limit_layer, create_rate_limit_layer,
//     create_security_headers,
// };
use crate::config::AuthConfig;
use crate::server::AppState;

/// Creates and configures the router with all routes and middleware
pub fn create_router(state: Arc<AppState>, config: &AuthConfig) -> Router {
    // Configure the CORS layer
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

    // Create public routes (no authentication required)
    let public_routes = Router::new()
        // Public authentication endpoints
        .route("/auth/login", get(login_handler))
        .route("/auth/login/*path", get(asset_handler))
        .route("/auth/challenge", get(challenge_handler))
        .route("/auth/token", post(token_handler))
        // Health and provider information (public for UI access)
        .route("/health", get(health_handler))
        .route("/providers", get(providers_handler));

    // Create authenticated routes
    let authenticated_routes = Router::new()
        // Protected auth endpoints
        .route("/auth/refresh", post(refresh_token_handler))
        .route("/auth/revoke", post(revoke_token_handler))
        .route("/auth/validate", post(validate_handler))
        .route("/auth/callback", get(callback_handler))
        .route("/auth/client-key", post(generate_client_key_handler))
        // Root key management
        .route("/auth/keys", get(list_keys_handler))
        .route("/auth/keys", post(create_key_handler))
        .route("/auth/keys/:key_id", delete(delete_key_handler))
        // Client key management
        .route("/auth/keys/:key_id/clients", get(list_clients_handler))
        .route(
            "/auth/keys/:key_id/clients/:client_id",
            delete(delete_client_handler),
        )
        // Permission management
        .route("/auth/permissions", get(list_permissions_handler))
        .route(
            "/auth/keys/:key_id/permissions",
            get(get_key_permissions_handler),
        )
        .route(
            "/auth/keys/:key_id/permissions",
            put(update_key_permissions_handler),
        )
        // Identity endpoint for development detection
        .route("/identity", get(identity_handler))
        // Metrics endpoint (should be protected)
        .route("/metrics", get(metrics_handler))
        // Apply authentication middleware only to protected routes
        .layer(from_fn(forward_auth_middleware));

    //TODO: FINISH security headers and rate limit
    // Start with the base router
    let router = Router::new()
        .merge(public_routes)
        .merge(authenticated_routes)
        // .layer(create_body_limit_layer())
        .layer(cors_layer)
        .layer(Extension(Arc::clone(&state)));

    // Add rate limit layer
    // let rate_limit = create_rate_limit_layer();
    // router = router.layer(rate_limit);

    // Add security headers
    // for header_layer in create_security_headers() {
    //     router = router.layer(header_layer);
    // }

    router
}
