use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Extension, Request};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

use crate::auth::permissions::PermissionValidator;
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

    // Skip authentication for public endpoints
    if path.starts_with("/auth/login")
        || path.starts_with("/auth/challenge")
        || path.starts_with("/auth/token")
        || path == "/health"
        || path == "/providers"
    {
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

            // Create permission validator
            let validator = PermissionValidator::new();

            // Determine required permissions for this request
            let mut required_permissions = validator.determine_required_permissions(&request);

            // For JSON-RPC requests, ensure execute permission
            if path.starts_with("/jsonrpc") {
                if !required_permissions
                    .iter()
                    .any(|p| p.starts_with("jsonrpc:"))
                {
                    required_permissions.push("jsonrpc:execute".to_string());
                }
            }

            // For admin API requests, ensure admin permission
            if path.starts_with("/admin-api") {
                if !required_permissions.iter().any(|p| p.starts_with("admin:")) {
                    required_permissions.push("admin:access".to_string());
                }
            }

            // Validate user's permissions
            let has_permission =
                validator.validate_permissions(&auth_response.permissions, &required_permissions);

            if !has_permission {
                tracing::warn!(
                    "Permission denied for {} {} - required: {:?}, had: {:?}",
                    method,
                    path,
                    required_permissions,
                    auth_response.permissions
                );
                return Err(StatusCode::FORBIDDEN);
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

/// Determine required permissions for a given path and method
fn determine_required_permissions(path: &str, method: &Method) -> Vec<String> {
    // This is a simplified example - you should implement your own permission mapping logic
    let mut required = Vec::new();

    // Extract resource type and ID from path
    let parts: Vec<&str> = path.split('/').collect();

    match parts.get(1) {
        Some(&"applications") => {
            required.push("application".to_string());

            // Add specific permissions based on method
            match *method {
                Method::GET => required.push("application:list".to_string()),
                Method::POST => required.push("application:install".to_string()),
                Method::DELETE => required.push("application:uninstall".to_string()),
                _ => {}
            }

            // If specific application ID is in path
            if let Some(app_id) = parts.get(2) {
                required.push(format!("application[{}]", app_id));
            }
        }
        Some(&"blobs") => {
            required.push("blob".to_string());

            match *method {
                Method::POST => required.push("blob:add".to_string()),
                Method::DELETE => required.push("blob:remove".to_string()),
                _ => {}
            }

            if let Some(blob_id) = parts.get(2) {
                required.push(format!("blob[{}]", blob_id));
            }
        }
        Some(&"contexts") => {
            required.push("context".to_string());

            match *method {
                Method::GET => required.push("context:list".to_string()),
                Method::POST => required.push("context:create".to_string()),
                Method::DELETE => required.push("context:delete".to_string()),
                _ => {}
            }

            if let Some(context_id) = parts.get(2) {
                required.push(format!("context[{}]", context_id));
            }
        }
        _ => {}
    }

    required
}

/// Validate if the user has the required permissions
fn validate_permissions(user_permissions: &[String], required_permissions: &[String]) -> bool {
    // For each required permission
    required_permissions.iter().any(|required| {
        // Check if user has this exact permission or a parent permission
        user_permissions.iter().any(|user_perm| {
            // Exact match
            if user_perm == required {
                return true;
            }

            // Check if user has parent permission
            // e.g. if required is "application:list[app1]", check if user has "application:list" or "application"
            let parts: Vec<&str> = required.split(&[':', '[', ']']).collect();
            if parts.len() > 1 {
                let parent = parts[0].to_string();
                if user_perm == &parent {
                    return true;
                }

                let parent_with_action = format!("{}:{}", parts[0], parts[1]);
                if user_perm == &parent_with_action {
                    return true;
                }
            }

            false
        })
    })
}
