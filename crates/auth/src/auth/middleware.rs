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
    // Skip authentication for login and token endpoints
    let path = request.uri().path();
    if path.starts_with("/auth/") {
        return Ok(next.run(request).await);
    }

    // Extract headers for token validation
    let headers = request.headers().clone();

    // Validate the request using the headers
    match state.auth_service.verify_token_from_headers(&headers).await {
        Ok(auth_response) => {
            if !auth_response.is_valid {
                return Err(StatusCode::UNAUTHORIZED);
            }

            // Continue with normal request
            let mut response = next.run(request).await;

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
        Err(_) => Err(StatusCode::UNAUTHORIZED),
    }
} 