pub mod auth;
pub mod clients;
pub mod keys;
pub mod permissions;

use std::sync::Arc;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::server::AppState;

/// Identity handler
///
/// This endpoint returns information about the authentication mode and service identity.
///
/// # Arguments
///
/// * `state` - The application state
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn identity_handler(state: Extension<Arc<AppState>>) -> impl IntoResponse {
    let response = json!({
        "service": "calimero-auth",
        "version": env!("CARGO_PKG_VERSION"),
        "authentication_mode": "forward",
        "providers": state.auth_service.providers().iter().map(|p| p.name()).collect::<Vec<_>>(),
    });

    (StatusCode::OK, Json(response))
}

/// Metrics handler
///
/// This endpoint returns metrics about the authentication service.
///
/// # Arguments
///
/// * `state` - The application state
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn metrics_handler(state: Extension<Arc<AppState>>) -> impl IntoResponse {
    let metrics = state.metrics.get_metrics().await;

    (StatusCode::OK, Json(metrics))
}

/// Health check handler
///
/// This endpoint returns the health status of the authentication service.
///
/// # Arguments
///
/// * `state` - The application state
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn health_handler(state: Extension<Arc<AppState>>) -> impl IntoResponse {
    // Check the connection to the storage backend
    let storage_ok = state.storage.exists("health-check").await.is_ok();

    let status = if storage_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let response = json!({
        "status": if status == StatusCode::OK { "healthy" } else { "unhealthy" },
        "storage": storage_ok,
        "uptime_seconds": state.metrics.get_uptime_seconds(),
    });

    (status, Json(response))
}

/// Providers information handler
///
/// This endpoint returns information about available authentication providers.
///
/// # Arguments
///
/// * `state` - The application state
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn providers_handler(state: Extension<Arc<AppState>>) -> impl IntoResponse {
    let providers = state
        .auth_service
        .providers()
        .iter()
        .map(|provider| {
            json!({
                "name": provider.name(),
                "type": provider.provider_type(),
                "description": provider.description(),
                "configured": provider.is_configured(),
                "config": provider.get_config_options(),
            })
        })
        .collect::<Vec<_>>();

    let response = json!({
        "providers": providers,
        "count": providers.len(),
    });

    (StatusCode::OK, Json(response))
}
