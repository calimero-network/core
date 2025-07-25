use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Extension, Request};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use tracing::{debug, info, warn};

use crate::auth::permissions::PermissionValidator;
use crate::server::AppState;
use crate::AuthError;

/// Authentication middleware for protected routes
///
/// This middleware validates JWT tokens and enforces permissions for protected API endpoints.
/// It adds user ID and permissions headers to successful responses for downstream use.
///
/// # Arguments
///
/// * `request` - The request to check
/// * `next` - The next middleware
///
/// # Returns
///
/// * `Result<Response, (StatusCode, HeaderMap)>` - The response or error with headers
pub async fn auth_middleware(
    state: Extension<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, (StatusCode, HeaderMap)> {
    // Extract request details for logging
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let start_time = std::time::Instant::now();

    // Skip authentication for public endpoints
    if path.starts_with("/public") {
        info!("Skipping auth for public endpoint {} {}", method, path);
        let response = next.run(request).await;
        let duration = start_time.elapsed();
        debug!("Request {} {} completed in {:?}", method, path, duration);
        return Ok(response);
    }

    let headers = request.headers().clone();

    match state.auth_service.verify_token_from_headers(&headers).await {
        Ok(auth_response) => {
            debug!(
                "Successful authentication for {} {} by user {}",
                method, path, auth_response.key_id
            );

            let validator = PermissionValidator::new();

            let required_permissions = validator.determine_required_permissions(&request);

            let has_permission =
                validator.validate_permissions(&auth_response.permissions, &required_permissions);

            if !has_permission {
                warn!(
                    "Permission denied for {} {} - required: {:?}, had: {:?}",
                    method, path, required_permissions, auth_response.permissions
                );
                let mut headers = HeaderMap::new();
                headers.insert("X-Auth-Error", "permission_denied".parse().unwrap());
                return Err((StatusCode::FORBIDDEN, headers));
            }

            let mut response = next.run(request).await;
            let duration = start_time.elapsed();
            debug!("Request {} {} completed in {:?}", method, path, duration);

            response
                .headers_mut()
                .insert("X-Auth-User", auth_response.key_id.parse().unwrap());

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
            let mut headers = HeaderMap::new();

            match err {
                AuthError::InvalidToken(msg) if msg.contains("expired") => {
                    warn!(
                        "Token expired for {} {} (took {:?})",
                        method, path, duration
                    );
                    headers.insert("X-Auth-Error", "token_expired".parse().unwrap());
                    Err((StatusCode::UNAUTHORIZED, headers))
                }
                AuthError::InvalidToken(msg) if msg.contains("revoked") => {
                    warn!(
                        "Token revoked for {} {} (took {:?})",
                        method, path, duration
                    );
                    headers.insert("X-Auth-Error", "token_revoked".parse().unwrap());
                    Err((StatusCode::FORBIDDEN, headers))
                }
                AuthError::InvalidRequest(_) => {
                    warn!(
                        "Invalid request for {} {}: {} (took {:?})",
                        method, path, err, duration
                    );
                    headers.insert("X-Auth-Error", "invalid_request".parse().unwrap());
                    Err((StatusCode::BAD_REQUEST, headers))
                }
                _ => {
                    warn!(
                        "Authentication failed for {} {}: {} (took {:?})",
                        method, path, err, duration
                    );
                    headers.insert("X-Auth-Error", "invalid_token".parse().unwrap());
                    Err((StatusCode::UNAUTHORIZED, headers))
                }
            }
        }
    }
}
