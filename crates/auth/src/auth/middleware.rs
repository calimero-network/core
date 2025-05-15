use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Extension, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

use crate::server::AppState;

/// Forward authentication middleware for reverse proxy
///
/// This middleware is used by reverse proxies to validate authentication.
/// If the request is authenticated, it adds the user ID and permissions to the response headers.
///
/// # Arguments
///
/// * `request` - The request to check
/// * `next` - The next middleware
///
/// # Returns
///
/// * `Result<Response, StatusCode>` - The response or error
pub async fn forward_auth_middleware(
    state: Extension<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // Extract request details for logging
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let start_time = std::time::Instant::now();

    // Skip authentication for login and token endpoints
    if path.starts_with("/auth/") {
        tracing::debug!("Skipping auth for {} {}", method, path);
        let response = next.run(request).await;
        let duration = start_time.elapsed();
        tracing::debug!("Request {} {} completed in {:?}", method, path, duration);
        return Ok(response);
    }

    // Extract headers for token validation
    let headers = request.headers().clone();

    // Validate the request using the headers
    match state.auth_service.verify_token_from_headers(&headers).await {
        Ok(auth_response) => {
            // Log successful authentication
            if let Some(key_id) = auth_response.key_id.as_ref() {
                tracing::debug!(
                    "Successful authentication for {} {} by user {}",
                    method,
                    path,
                    key_id
                );
            }

            // Continue with normal request
            let mut response = next.run(request).await;
            let duration = start_time.elapsed();
            tracing::debug!("Request {} {} completed in {:?}", method, path, duration);

            // Add authentication headers
            if let Some(key_id) = auth_response.key_id.as_ref() {
                response
                    .headers_mut()
                    .insert("X-Auth-User", key_id.parse().unwrap());
            }

            // Add permissions
            if !auth_response.permissions.is_empty() {
                response.headers_mut().insert(
                    "X-Auth-Permissions",
                    auth_response.permissions.join(",").parse().unwrap(),
                );
            }

            Ok(response)
        }
        Err(err) => {
            let duration = start_time.elapsed();
            tracing::warn!(
                "Authentication failed for {} {}: {} (took {:?})",
                method,
                path,
                err,
                duration
            );
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}
