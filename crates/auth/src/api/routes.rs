use std::sync::Arc;

use axum::middleware::from_fn;
use axum::routing::{delete, get, post, put};
use axum::{Extension, Router};
use tower_http::cors::CorsLayer;

use crate::api::handlers::auth::{
    callback_handler, challenge_handler, login_handler, refresh_token_handler,
    revoke_token_handler, token_handler, validate_handler,
};
use crate::api::handlers::clients::{
    create_client_handler, delete_client_handler, list_clients_handler,
};
use crate::api::handlers::keys::{create_key_handler, delete_key_handler, list_keys_handler};
use crate::api::handlers::permissions::{
    get_key_permissions_handler, list_permissions_handler, update_key_permissions_handler,
};
use crate::api::handlers::{health_handler, identity_handler, metrics_handler, providers_handler};
use crate::api::handlers::asset_handler;
use crate::auth::middleware::forward_auth_middleware;
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
        // Root key management
        .route("/auth/keys", get(list_keys_handler))
        .route("/auth/keys", post(create_key_handler))
        .route("/auth/keys/:key_id", delete(delete_key_handler))
        // Client key management
        .route("/auth/keys/:key_id/clients", get(list_clients_handler))
        .route("/auth/keys/:key_id/clients", post(create_client_handler))
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

    // Merge both routers, apply CORS, and add state
    public_routes
        .merge(authenticated_routes)
        .layer(cors_layer)
        .layer(Extension(Arc::clone(&state)))
}
