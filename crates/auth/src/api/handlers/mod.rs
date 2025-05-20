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
use axum::http::{header, Uri};
use axum::extract::Path;
use rust_embed::RustEmbed;

use crate::server::AppState;

/// Embed the contents of the auth frontend build directory into the binary
#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct AuthUiStaticFiles;

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

/// Asset handler for serving static files from the React build
pub async fn asset_handler(Path(path): Path<String>) -> impl IntoResponse {
    serve_embedded_file(&path).await
}

/// Serves embedded static files or falls back to `index.html` for SPA routing.
async fn serve_embedded_file(path: &str) -> impl IntoResponse {
    // Clean up the path, removing any leading slashes and /auth prefix
    let clean_path = path
        .trim_start_matches('/')
        .trim_start_matches("auth/");

    // For empty paths or root requests, serve index.html
    let path_to_serve = if clean_path.is_empty() {
        "index.html"
    } else {
        clean_path
    };

    // Special case for favicon.ico at root
    let path_to_serve = if path_to_serve == "favicon.ico" {
        "assets/favicon.ico"
    } else {
        path_to_serve
    };

    // Debug logging
    tracing::debug!("Attempting to serve file: {}", path_to_serve);

    // Attempt to serve the requested file
    if let Some(file) = AuthUiStaticFiles::get(path_to_serve) {
        // Get the file extension and determine content type
        let content_type = if path_to_serve.ends_with(".js") {
            // For ES modules, we need to set the correct MIME type
            if path_to_serve.contains(".esm.") || path_to_serve.contains(".module.") {
                "text/javascript; charset=utf-8"
            } else {
                "application/javascript; charset=utf-8"
            }
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

        return (StatusCode::OK, headers, file.data.into_owned()).into_response();
    }

    // Debug logging for not found files
    tracing::debug!("File not found: {}", path_to_serve);

    // Fallback to index.html for SPA routing if it's not a direct asset request
    if !path_to_serve.starts_with("assets/") && path_to_serve != "index.html" {
        if let Some(index_file) = AuthUiStaticFiles::get("index.html") {
            // Convert the file content to a string
            let html_content = String::from_utf8_lossy(&index_file.data);
            
            // Replace the asset paths to use the /auth prefix
            let modified_html = html_content
                .replace("=\"/assets/", "=\"/auth/assets/")
                .replace("=\"/favicon.ico", "=\"/auth/favicon.ico");

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
