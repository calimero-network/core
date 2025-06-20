use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, error, info, warn};
use validator::Validate;

use crate::api::handlers::AuthUiStaticFiles;
use crate::auth::validation::{sanitize_identifier, sanitize_string, ValidatedJson};
use crate::server::AppState;

// Common response type used by all helper functions
type ApiResponse = (StatusCode, HeaderMap, Json<serde_json::Value>);

pub fn success_response<T: Serialize>(data: T, headers: Option<HeaderMap>) -> ApiResponse {
    (
        StatusCode::OK,
        headers.unwrap_or_default(),
        Json(serde_json::json!({
            "data": data,
            "error": null
        })),
    )
}

pub fn error_response(
    status: StatusCode,
    error: impl Into<String>,
    headers: Option<HeaderMap>,
) -> ApiResponse {
    (
        status,
        headers.unwrap_or_default(),
        Json(serde_json::json!({
            "data": null,
            "error": error.into()
        })),
    )
}

/// Login request handler
///
/// This endpoint serves the login page.
pub async fn login_handler(
    state: Extension<Arc<AppState>>,
    Query(_params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    // Get enabled providers
    let enabled_providers = state.0.auth_service.providers();

    // If we have any providers available
    if !enabled_providers.is_empty() {
        info!("Loading authentication UI");

        // Get the index.html file from embedded assets
        if let Some(file) = AuthUiStaticFiles::get("index.html") {
            // Convert the file content to a string
            let html_content = String::from_utf8_lossy(&file.data);

            // The assets are already prefixed with /public/ from the Vite build
            return (
                StatusCode::OK,
                [("Content-Type", "text/html")],
                html_content.into_owned().into_bytes(),
            );
        }

        error!("Failed to load authentication UI - index.html not found");
    }

    warn!("No authentication providers available");
    // Fall back to a simple error message if no provider is available
    let html = "<html><body><h1>No authentication provider is available</h1></body></html>";
    (
        StatusCode::OK,
        [("Content-Type", "text/html")],
        html.as_bytes().to_vec(),
    )
}

/// Base token request with common fields
#[derive(Debug, Deserialize, Validate)]
pub struct BaseTokenRequest {
    /// Authentication method
    #[validate(length(min = 1, message = "Authentication method is required"))]
    pub auth_method: String,

    /// Public key
    #[validate(length(min = 1, message = "Public key is required"))]
    pub public_key: String,

    /// Client name
    #[validate(length(min = 1, message = "Client name is required"))]
    pub client_name: String,

    /// Permissions requested
    pub permissions: Option<Vec<String>>,

    /// Timestamp
    pub timestamp: u64,

    /// Provider-specific data as raw JSON
    pub provider_data: Value,
}

/// Token request that includes provider-specific data
pub type TokenRequest = BaseTokenRequest;

/// Token response
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    /// Access token
    access_token: String,
    /// Refresh token
    refresh_token: String,
    /// Error message
    error: Option<String>,
}

impl TokenResponse {
    /// Create a new success token response
    pub fn new(access_token: String, refresh_token: String) -> Self {
        Self {
            access_token,
            refresh_token,
            error: None,
        }
    }
}

/// Token handler
///
/// This endpoint generates JWT tokens for authenticated clients.
///
/// # Arguments
///
/// * `state` - The application state
/// * `request` - The token request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn token_handler(
    state: Extension<Arc<AppState>>,
    ValidatedJson(mut token_request): ValidatedJson<TokenRequest>,
) -> impl IntoResponse {
    info!("token_handler");
    
    // Sanitize string inputs to prevent injection attacks
    token_request.auth_method = sanitize_identifier(&token_request.auth_method);
    token_request.public_key = sanitize_string(&token_request.public_key);
    token_request.client_name = sanitize_string(&token_request.client_name);
    
    // Validate sanitized inputs are not empty
    if token_request.auth_method.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Authentication method must contain valid characters",
            None,
        );
    }
    
    if token_request.public_key.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Public key cannot be empty after sanitization",
            None,
        );
    }
    
    if token_request.client_name.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Client name cannot be empty after sanitization",
            None,
        );
    }
    
    // Authenticate directly using the token request
    let auth_response = match state
        .0
        .auth_service
        .authenticate_token_request(&token_request)
        .await
    {
        Ok(response) => response,
        Err(err) => {
            error!("Authentication failed: {}", err);
            return error_response(
                StatusCode::UNAUTHORIZED,
                format!("Authentication failed: {}", err),
                None,
            );
        }
    };

    // Ensure authentication was successful
    if !auth_response.is_valid {
        return error_response(
            StatusCode::UNAUTHORIZED,
            "Authentication failed: Invalid credentials",
            None,
        );
    }

    let key_id = auth_response.key_id;

    // Generate tokens using the validated permissions from auth_response
    match state
        .0
        .token_generator
        .generate_token_pair(key_id.clone(), auth_response.permissions)
        .await
    {
        Ok((access_token, refresh_token)) => {
            let response = TokenResponse::new(access_token, refresh_token);
            success_response(response, None)
        }
        Err(err) => {
            error!("Failed to generate tokens: {}", err);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to generate tokens",
                None,
            )
        }
    }
}

/// Refresh token request
#[derive(Debug, Deserialize, Validate)]
pub struct RefreshTokenRequest {
    /// Access token
    #[validate(length(min = 1, message = "Access token is required"))]
    access_token: String,
    /// Refresh token
    #[validate(length(min = 1, message = "Refresh token is required"))]
    refresh_token: String,
}

/// Refresh token handler
///
/// This endpoint refreshes an access token using a refresh token.
/// It supports both root and client tokens, handling them appropriately.
///
/// # Arguments
///
/// * `state` - The application state
/// * `request` - The refresh token request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn refresh_token_handler(
    state: Extension<Arc<AppState>>,
    ValidatedJson(request): ValidatedJson<RefreshTokenRequest>,
) -> impl IntoResponse {
    match state
        .0
        .token_generator
        .verify_token(&request.access_token)
        .await
    {
        Ok(_) => {
            return error_response(StatusCode::UNAUTHORIZED, "Access token still valid", None);
        }
        Err(err) => {
            if !err.to_string().contains("expired") {
                return error_response(
                    StatusCode::UNAUTHORIZED,
                    format!("Invalid access token: {}", err),
                    None,
                );
            }
        }
    };

    match state
        .0
        .token_generator
        .refresh_token_pair(&request.refresh_token)
        .await
    {
        Ok((access_token, refresh_token)) => {
            let response = TokenResponse::new(access_token, refresh_token);
            success_response(response, None)
        }
        Err(err) => {
            error!("Failed to refresh token: {}", err);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to refresh token: {}", err),
                None,
            )
        }
    }
}

/// Forward authentication validation handler
///
/// This endpoint is designed for reverse proxies (nginx, Traefik, etc.) to validate
/// authentication before forwarding requests to backend services. It validates JWT tokens
/// and returns user information via response headers.
///
/// # Arguments
///
/// * `state` - The application state
/// * `headers` - The request headers
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn validate_handler(
    state: Extension<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Check if Authorization header exists and starts with "Bearer "
    if !headers
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .map(|h| h.starts_with("Bearer "))
        .unwrap_or(false)
    {
        let mut error_headers = HeaderMap::new();
        error_headers.insert("X-Auth-Error", "missing_token".parse().unwrap());
        return error_response(
            StatusCode::UNAUTHORIZED,
            "No Bearer token provided",
            Some(error_headers),
        );
    }

    // Validate the request using the headers
    match state
        .0
        .auth_service
        .verify_token_from_headers(&headers)
        .await
    {
        Ok(auth_response) => {
            if !auth_response.is_valid {
                return error_response(StatusCode::UNAUTHORIZED, "Unauthorized", None);
            }

            // Create response headers
            let mut response_headers = HeaderMap::new();

            // Add user ID header
            response_headers.insert("X-Auth-User", auth_response.key_id.parse().unwrap());

            // Add permissions as a comma-separated list
            if !auth_response.permissions.is_empty() {
                response_headers.insert(
                    "X-Auth-Permissions",
                    auth_response.permissions.join(",").parse().unwrap(),
                );
            }

            success_response("", Some(response_headers))
        }
        Err(err) => {
            let mut error_headers = HeaderMap::new();
            // Add error type header for better client handling
            if err.to_string().contains("expired") {
                error_headers.insert("X-Auth-Error", "token_expired".parse().unwrap());
            } else if err.to_string().contains("revoked") {
                error_headers.insert("X-Auth-Error", "token_revoked".parse().unwrap());
            } else {
                error_headers.insert("X-Auth-Error", "invalid_token".parse().unwrap());
            }

            error_response(
                StatusCode::UNAUTHORIZED,
                format!("Invalid token: {}", err),
                Some(error_headers),
            )
        }
    }
}

/// OAuth callback handler
///
/// This endpoint handles callbacks from OAuth providers.
///
/// # Arguments
///
/// * `state` - The application state
///
/// # Returns
///
/// * `impl IntoResponse` - The response
// TODO: Implement OAuth callback handling
pub async fn callback_handler(_state: Extension<Arc<AppState>>) -> impl IntoResponse {
    // This is a placeholder implementation
    // In a real implementation, you would:
    // 1. Extract the code from the request
    // 2. Exchange it for tokens
    // 3. Validate the tokens
    // 4. Create or lookup the root key
    // 5. Create a client key
    // 6. Generate tokens
    // 7. Redirect to the original URL with the tokens

    (
        StatusCode::OK,
        "OAuth callback - implement with your OAuth provider",
    )
}

/// Challenge response
#[derive(Debug, Serialize)]
pub struct ChallengeResponse {
    /// Challenge token to be signed
    pub challenge: String,
    /// Server-generated nonce (base64 encoded)
    pub nonce: String,
}

/// Challenge handler
///
/// This endpoint generates a challenge token for authentication.
///
/// # Arguments
///
/// * `state` - The application state
///
/// # Returns
///
/// * `impl IntoResponse` - The response containing the challenge token
pub async fn challenge_handler(state: Extension<Arc<AppState>>) -> impl IntoResponse {
    match state.0.token_generator.generate_challenge().await {
        Ok(response) => success_response(response, None),
        Err(err) => {
            error!("Failed to generate challenge: {}", err);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to generate challenge",
                None,
            )
        }
    }
}

/// Revoke token request
#[derive(Debug, Deserialize, Validate)]
pub struct RevokeTokenRequest {
    /// Client ID to revoke
    #[validate(length(min = 1, message = "Client ID cannot be empty"))]
    client_id: String,
}

/// Revoke token handler
///
/// This endpoint revokes a client's tokens.
///
/// # Arguments
///
/// * `state` - The application state
/// * `request` - The revoke token request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn revoke_token_handler(
    state: Extension<Arc<AppState>>,
    ValidatedJson(mut request): ValidatedJson<RevokeTokenRequest>,
) -> impl IntoResponse {
    // Sanitize client ID to prevent injection attacks
    request.client_id = sanitize_identifier(&request.client_id);
    
    if request.client_id.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Client ID must contain valid characters",
            None,
        );
    }
    match state
        .0
        .token_generator
        .revoke_client_tokens(&request.client_id)
        .await
    {
        Ok(_) => {
            debug!(
                "Successfully revoked tokens for client {}",
                request.client_id
            );

            success_response(
                serde_json::json!({
                        "success": true,
                        "message": "Tokens revoked successfully"
                }),
                None,
            )
        }
        Err(err) => {
            error!(
                "Failed to revoke tokens for client {}: {}",
                request.client_id, err
            );

            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to revoke tokens: {}", err),
                None,
            )
        }
    }
}
