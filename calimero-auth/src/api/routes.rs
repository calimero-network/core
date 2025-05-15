use axum::{
    middleware,
    routing::{get, post},
    Router,
};

use crate::auth::middleware::auth_middleware;
use crate::api::handlers::auth;
use crate::api::handlers::identity;
use crate::api::handlers::keys;

/// Create the API router with all routes
///
/// # Returns
///
/// * `Router` - The configured router
pub fn create_router() -> Router {
    // Public routes that don't require authentication
    let public_routes = Router::new()
        // Authentication endpoints
        .route("/auth/login", get(auth::login_page))
        .route("/auth/callback", get(auth::callback_handler))
        .route("/auth/token", post(auth::token_handler))
        // Identity endpoint
        .route("/identity", get(identity::get_identity));

    // Protected routes that require authentication
    let protected_routes = Router::new()
        // Key management endpoints
        .route("/keys", get(keys::list_keys))
        .route("/keys", post(keys::create_key))
        .route("/keys/:key_id", get(keys::get_key))
        .route("/keys/:key_id", post(keys::update_key))
        .route("/keys/:key_id", delete(keys::delete_key))
        // Apply auth middleware to all protected routes
        .route_layer(middleware::from_fn(auth_middleware));

    // Combine all routes
    Router::new()
        .merge(public_routes)
        .merge(protected_routes)
} 