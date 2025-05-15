pub mod auth;
pub mod clients;
pub mod keys;
pub mod permissions;

use std::sync::Arc;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;

use crate::server::AppState;

/// Identity response
#[derive(Debug, Serialize)]
struct IdentityResponse {
    /// Node ID
    node_id: String,
    /// Version
    version: String,
    /// Authentication mode
    authentication_mode: String,
}

/// Identity handler
///
/// This endpoint returns information about the node identity.
/// It's used by clients to detect authentication mode.
///
/// # Arguments
///
/// * `state` - The application state (optional)
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn identity_handler(state: Option<Extension<Arc<AppState>>>) -> impl IntoResponse {
    // Determine the authentication mode based on the number of providers (or standalone mode)
    let auth_mode = match &state {
        Some(state) if !state.0.auth_service.providers().is_empty() => "forward",
        _ => "none",
    };

    // Create a node ID using a timestamp instead of UUID
    let node_id = match &state {
        Some(state) if !state.0.config.node_url.is_empty() => state.0.config.node_url.clone(),
        _ => format!("auth-node-{}", chrono::Utc::now().timestamp()),
    };

    let response = IdentityResponse {
        node_id,
        version: env!("CARGO_PKG_VERSION").to_string(),
        authentication_mode: auth_mode.to_string(),
    };

    (StatusCode::OK, Json(response))
} 