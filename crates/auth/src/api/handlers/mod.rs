pub mod auth;
pub mod client_keys;
pub mod permissions;
pub mod root_keys;

use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use rust_embed::RustEmbed;
use serde_json::json;
use tracing::info;

use crate::api::handlers::auth::success_response;
use crate::server::AppState;

/// Embed the contents of the auth frontend build directory into the binary
#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct AuthUiStaticFiles;

/// Re-export authentication flow handlers
pub use auth::{
    callback_handler,  // OAuth callback handling
    challenge_handler, // Step 4-5: Generate challenge for signing
    login_handler,     // Step 7-8: Verify signature and create root key
    refresh_token_handler,
    revoke_token_handler,
    token_handler,    // Step 12-13: Create client key and JWT
    validate_handler, // Forward auth validation
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
    let providers = state
        .0
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

    success_response(response, None)
}

/// Asset handler for serving static files from the React build
pub async fn asset_handler(Path(path): Path<String>) -> impl IntoResponse {
    serve_embedded_file(&path).await
}

/// Serves embedded static files or falls back to `index.html` for SPA routing.
async fn serve_embedded_file(path: &str) -> impl IntoResponse {
    let path_to_serve = path.trim_start_matches('/');
    
    // Add assets/ prefix for JS and CSS files if not already present
    let path_to_serve = if (path_to_serve.ends_with(".js") || path_to_serve.ends_with(".css")) 
        && !path_to_serve.starts_with("assets/") {
        format!("assets/{}", path_to_serve)
    } else {
        path_to_serve.to_string()
    };

    info!("Serving file: {}", path_to_serve);

    // Attempt to serve the requested file
    if let Some(file) = AuthUiStaticFiles::get(&path_to_serve) {
        // Get the file extension and determine content type
        let content_type = if path_to_serve.ends_with(".js") {
            "application/javascript; charset=utf-8"
        } else if path_to_serve.ends_with(".esm") {
            "text/javascript; charset=utf-8"
        } else if path_to_serve.ends_with(".css") {
            "text/css; charset=utf-8"
        } else if path_to_serve.ends_with(".html") {
            "text/html; charset=utf-8"
        } else if path_to_serve.ends_with(".ico") {
            "image/x-icon"
        } else if path_to_serve.ends_with(".png") {
            "image/png"
        } else if path_to_serve.ends_with(".svg") {
            "image/svg+xml"
        } else {
            "application/octet-stream"
        };

        info!("Content type: {}", content_type);

        // Set appropriate headers
        let headers = [
            (header::CONTENT_TYPE, content_type),
            // Add cache control for static assets
            (
                header::CACHE_CONTROL,
                if path_to_serve.starts_with("assets/") {
                    "public, max-age=31536000" // Cache for 1 year
                } else {
                    "no-cache" // Don't cache HTML
                },
            ),
        ];

        return (StatusCode::OK, headers, file.data).into_response();
    }

    info!("File not found: {}", path_to_serve);

    // Fallback to index.html for SPA routing if it's not a direct asset request
    if !path_to_serve.starts_with("assets/") && path_to_serve != "index.html" {
        if let Some(index_file) = AuthUiStaticFiles::get("index.html") {
            // Convert the file content to a string
            let html_content = String::from_utf8_lossy(&index_file.data);

            // Replace the asset paths to use the /public prefix
            let modified_html = html_content
                .replace("=\"/assets/", "=\"/public/assets/")
                .replace("=\"/favicon.ico", "=\"/public/favicon.ico");

            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                modified_html.into_bytes(),
            )
                .into_response();
        }
    }

    // Return 404 if the file is not found and we can't fallback to index.html
    (StatusCode::NOT_FOUND, "File not found").into_response()
}
