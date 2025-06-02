use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, error, info, warn};
use validator::Validate;

use crate::api::handlers::AuthUiStaticFiles;
use crate::auth::validation::ValidatedJson;
use crate::server::AppState;
use crate::AuthError;

// Common response type used by all helper functions
type ApiResponse = (StatusCode, Json<serde_json::Value>);

// Helper functions for common response patterns
pub fn unauthorized_response(message: &str) -> ApiResponse {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": message })),
    )
}

pub fn internal_error_response(message: &str) -> ApiResponse {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": message })),
    )
}

pub fn success_response<T: Serialize>(data: T) -> ApiResponse {
    (StatusCode::OK, Json(serde_json::json!(data)))
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

            // Replace the asset paths to use the /auth prefix
            let modified_html = html_content
                .replace("=\"/assets/", "=\"/auth/assets/")
                .replace("=\"/favicon.ico", "=\"/auth/favicon.ico");

            return (
                StatusCode::OK,
                [("Content-Type", "text/html")],
                modified_html.into_bytes(),
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
    /// Token type
    token_type: String,
    /// Expires in seconds
    expires_in: u64,
    /// Client ID
    client_id: String,
    /// Error information (if any)
    error: Option<String>,
}

impl TokenResponse {
    /// Create a new success token response
    pub fn new(
        access_token: String,
        refresh_token: String,
        client_id: String,
        expires_in: u64,
    ) -> Self {
        Self {
            access_token,
            refresh_token,
            token_type: "Bearer".to_string(),
            expires_in,
            client_id,
            error: None,
        }
    }

    /// Create an error token response
    fn error(msg: &str) -> Self {
        Self {
            access_token: String::new(),
            refresh_token: String::new(),
            token_type: String::new(),
            expires_in: 0,
            client_id: String::new(),
            error: Some(msg.to_string()),
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
    ValidatedJson(token_request): ValidatedJson<TokenRequest>,
) -> impl IntoResponse {
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
            return unauthorized_response(&format!("Authentication failed: {}", err));
        }
    };

    // Ensure authentication was successful
    if !auth_response.is_valid {
        return unauthorized_response("Authentication failed: Invalid credentials");
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
            let response = TokenResponse::new(
                access_token,
                refresh_token,
                key_id,
                state.0.config.jwt.access_token_expiry,
            );
            success_response(response)
        }
        Err(err) => {
            error!("Failed to generate tokens: {}", err);
            internal_error_response("Failed to generate tokens")
        }
    }
}

/// Refresh token request
#[derive(Debug, Deserialize, Validate)]
pub struct RefreshTokenRequest {
    /// Refresh token
    #[validate(length(min = 1, message = "Refresh token is required"))]
    refresh_token: String,
}

/// Refresh token handler
///
/// This endpoint refreshes an access token using a refresh token.
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
    // First verify the refresh token to get the claims
    let claims = match state
        .0
        .token_generator
        .verify_token(&request.refresh_token)
        .await
    {
        Ok(claims) => claims,
        Err(err) => {
            debug!("Failed to verify refresh token: {}", err);
            let error_response = TokenResponse::error("Invalid refresh token");
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!(error_response)),
            );
        }
    };

    // Extract client_id from claims
    let client_id = claims.sub;

    // Generate new token pair
    match state
        .0
        .token_generator
        .refresh_token_pair(&request.refresh_token)
        .await
    {
        Ok((access_token, refresh_token)) => {
            let response = TokenResponse::new(
                access_token,
                refresh_token,
                client_id,
                state.0.config.jwt.access_token_expiry,
            );
            success_response(response)
        }
        Err(err) => {
            debug!("Failed to refresh token: {}", err);
            let error_response = TokenResponse::error("Invalid refresh token");
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!(error_response)),
            )
        }
    }
}

/// Validation handler
///
/// This endpoint validates a request and returns authentication information.
/// It's used by reverse proxies for forward authentication.
///
/// # Arguments
///
/// * `state` - The application state
/// * `request` - The request to validate
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn validate_handler(
    state: Extension<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    println!("validate_handler headers: {:?}", headers);
    // Validate the request using the headers
    match state
        .0
        .auth_service
        .verify_token_from_headers(&headers)
        .await
    {
        Ok(auth_response) => {
            if !auth_response.is_valid {
                return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
            }

            // Return success with appropriate headers
            let mut response_headers = HeaderMap::new();

            // Add key_id header directly since it's no longer optional
            if let Ok(header_value) = auth_response.key_id.parse() {
                response_headers.insert("X-Auth-User", header_value);
            }

            // Add permissions as a comma-separated list
            if !auth_response.permissions.is_empty() {
                let permissions = auth_response.permissions.join(",");
                if let Ok(header_value) = permissions.parse() {
                    response_headers.insert("X-Auth-Permissions", header_value);
                }
            }

            // Convert to Response to match error case
            (StatusCode::OK, response_headers, "").into_response()
        }
        Err(_) => (StatusCode::UNAUTHORIZED, HeaderMap::new(), "Unauthorized").into_response(),
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
        Ok(response) => success_response(response),
        Err(err) => {
            error!("Failed to generate challenge: {}", err);
            internal_error_response("Failed to generate challenge")
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
    ValidatedJson(request): ValidatedJson<RevokeTokenRequest>,
) -> impl IntoResponse {
    match state
        .0
        .token_generator
        .revoke_client_tokens(&request.client_id)
        .await
    {
        Ok(_) => {
            // Log successful revocation
            debug!(
                "Successfully revoked tokens for client {}",
                request.client_id
            );

            success_response(serde_json::json!({
                    "success": true,
                    "message": "Tokens revoked successfully"
            }))
        }
        Err(err) => {
            // Log error
            error!(
                "Failed to revoke tokens for client {}: {}",
                request.client_id, err
            );

            let status_code = match err {
                AuthError::AuthenticationFailed(_) => StatusCode::NOT_FOUND,
                AuthError::StorageError(_) => StatusCode::INTERNAL_SERVER_ERROR,
                _ => StatusCode::BAD_REQUEST,
            };

            (
                status_code,
                Json(serde_json::json!({
                    "error": format!("Failed to revoke tokens: {}", err)
                })),
            )
        }
    }
}
