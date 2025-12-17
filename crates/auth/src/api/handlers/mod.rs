pub mod auth;
pub mod client_keys;
pub mod permissions;
pub mod root_keys;

use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use rust_embed::RustEmbed;
use serde_json::json;
use tracing;

use crate::api::handlers::auth::success_response;
use crate::server::AppState;

/// Embed the contents of the auth frontend build directory into the binary
#[derive(RustEmbed)]
#[folder = "$CALIMERO_AUTH_FRONTEND_PATH"]
struct AuthUiStaticFiles;

/// Re-export authentication flow handlers
pub use auth::{
    callback_handler, challenge_handler, login_handler, mock_token_handler, refresh_token_handler,
    revoke_token_handler, token_handler, validate_handler,
};
/// Re-export client key management handlers
pub use client_keys::{delete_client_handler, list_clients_handler};
/// Re-export key management handlers
pub use root_keys::{create_key_handler, delete_key_handler, list_keys_handler};

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
        "providers": state.0.auth_service.providers().iter().map(|p| p.name()).collect::<Vec<_>>(),
    });

    success_response(response, None)
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
    let metrics = state.0.metrics.get_metrics().await;
    success_response(metrics, None)
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
    let storage_ok = state.0.storage.exists("health-check").await.is_ok();

    let response = json!({
        "status": if storage_ok { "healthy" } else { "unhealthy" },
        "storage": storage_ok,
        "uptime_seconds": state.0.metrics.get_uptime_seconds(),
    });

    success_response(response, None)
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
    let mut providers = Vec::new();

    // Check each provider's configuration status
    // Each provider implements its own logic for what "configured" means
    for provider in state.0.auth_service.providers() {
        let is_configured = match provider.is_configured_with_users().await {
            Ok(configured) => configured,
            Err(e) => {
                tracing::error!(
                    provider = provider.name(),
                    error = %e,
                    "Failed to check if provider is configured with users, falling back to is_configured()"
                );
                provider.is_configured()
            }
        };

        providers.push(json!({
            "name": provider.name(),
            "type": provider.provider_type(),
            "description": provider.description(),
            "configured": is_configured,
            "config": provider.get_config_options(),
        }));
    }

    let response = json!({
        "providers": providers,
        "count": providers.len(),
    });

    success_response(response, None)
}

/// Serves embedded frontend files for auth UI root path
pub async fn frontend_handler() -> impl IntoResponse {
    serve_embedded_file("index.html").await
}

/// Asset handler for serving static files from the frontend build
pub async fn asset_handler(Path(path): Path<String>) -> impl IntoResponse {
    // Handle favicon.ico directly
    if path == "favicon.ico" {
        return serve_embedded_file("favicon.ico").await;
    }

    // For all other assets, prepend "assets/" to match the embedded file structure
    let asset_path = format!("assets/{path}");
    serve_embedded_file(&asset_path).await
}

/// Serves embedded static files or falls back to `index.html` for SPA routing.
async fn serve_embedded_file(path: &str) -> impl IntoResponse {
    use axum::body::Body;
    use axum::http::Response;

    let path = path.trim_start_matches('/');

    // Use "index.html" for empty paths (root requests)
    let path = if path.is_empty() { "index.html" } else { path };

    // Attempt to serve the requested file
    if let Some(file) = AuthUiStaticFiles::get(path) {
        return match Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", file.metadata.mimetype())
            .body(Body::from(file.data.into_owned()))
        {
            Ok(response) => response.into_response(),
            Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Failed to serve file").into_response(),
        };
    }

    // Fallback to index.html for SPA routing if the file wasn't found and it's not already "index.html"
    if path != "index.html" {
        if let Some(index_file) = AuthUiStaticFiles::get("index.html") {
            return match Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", index_file.metadata.mimetype())
                .body(Body::from(index_file.data.into_owned()))
            {
                Ok(response) => response.into_response(),
                Err(_) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, "Failed to serve file").into_response()
                }
            };
        }
    }

    // Return 404 if the file is not found and we can't fallback to index.html
    (StatusCode::NOT_FOUND, "File not found").into_response()
}
