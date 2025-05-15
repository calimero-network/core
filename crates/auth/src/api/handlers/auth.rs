use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Extension, Query, Request};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use crate::server::AppState;
use crate::utils::{generate_random_challenge, ChallengeRequest, ChallengeResponse};
use crate::AuthError;

/// Login request handler
///
/// This endpoint serves the login page.
pub async fn login_handler(
    state: Extension<Arc<AppState>>,
    Query(_params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    // Read the main login template
    let mut html = include_str!("../../../templates/login.html").to_string();

    // Get enabled providers
    let enabled_providers = state.0.auth_service.providers();

    // Variables to store provider-specific content
    let mut provider_scripts = String::new();
    let mut provider_buttons = String::new();
    let mut provider_init = String::new();

    // Check if NEAR wallet provider is available
    if let Some(near_wallet) = enabled_providers.iter().find(|p| p.name() == "near_wallet") {
        if near_wallet.is_configured() {
            // Read the NEAR wallet template
            let near_template = include_str!("../../../templates/providers/near_wallet.html");

            // Extract the provider script
            if let Some(script_start) = near_template.find("<script id=\"near-wallet-script\">") {
                if let Some(script_end) = near_template[script_start..].find("</script>") {
                    let script = &near_template[script_start..script_start + script_end + 9]; // +9 to include </script>
                    provider_scripts.push_str(script);
                }
            }

            // Extract the provider button
            if let Some(button_start) = near_template.find("<button id=\"near-login\"") {
                if let Some(result_end) = near_template[button_start..].find("</div>") {
                    let button = &near_template[button_start..button_start + result_end + 6]; // +6 to include </div>
                    provider_buttons.push_str(button);
                }
            }

            // Extract the provider initialization
            if let Some(init_start) = near_template.find("<script id=\"near-wallet-init\">") {
                if let Some(init_end) = near_template[init_start..].find("</script>") {
                    let init = &near_template[init_start..init_start + init_end + 9]; // +9 to include </script>

                    // Replace configuration placeholders with actual values
                    let config = &state.0.config.near;
                    let init = init
                        .replace("{{NEAR_NETWORK}}", &config.network)
                        .replace("{{NEAR_RPC_URL}}", &config.rpc_url)
                        .replace("{{NEAR_WALLET_URL}}", &config.wallet_url)
                        .replace(
                            "{{NEAR_HELPER_URL}}",
                            &config.helper_url.clone().unwrap_or_default(),
                        );

                    provider_init.push_str(&init);
                }
            }
        }
    }

    // Add other providers here in the future...

    // Replace placeholders in the main template
    html = html
        .replace("<!-- PROVIDER_SCRIPTS -->", &provider_scripts)
        .replace("<!-- PROVIDER_BUTTONS -->", &provider_buttons)
        .replace("<!-- PROVIDER_INIT -->", &provider_init);

    (StatusCode::OK, [("Content-Type", "text/html")], html)
}

/// Token request
#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    /// Authentication method
    pub auth_method: String,
    /// Public key
    pub public_key: String,
    /// Wallet address (if applicable)
    pub wallet_address: Option<String>,
    /// Client name
    pub client_name: String,
    /// Permissions requested
    pub permissions: Option<Vec<String>>,
    /// Timestamp
    pub timestamp: u64,
    /// Signature
    pub signature: String,
    /// Message that was signed (only for NEAR wallet)
    pub message: Option<String>,
}

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
    Json(token_request): Json<TokenRequest>,
) -> impl IntoResponse {
    // Special handling for NEAR wallet authentication
    if token_request.auth_method == "near_wallet" {
        // Find the NEAR wallet provider
        let near_provider = state
            .0
            .auth_service
            .providers()
            .iter()
            .find(|p| p.name() == "near_wallet")
            .ok_or_else(|| {
                AuthError::AuthenticationFailed("NEAR wallet provider not found".to_string())
            });

        if let Ok(provider) = near_provider {
            // Create headers to pass to the provider
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-near-account-id",
                token_request
                    .wallet_address
                    .clone()
                    .unwrap_or_default()
                    .parse()
                    .unwrap(),
            );
            headers.insert(
                "x-near-public-key",
                token_request.public_key.parse().unwrap(),
            );
            headers.insert("x-near-signature", token_request.signature.parse().unwrap());

            // Use as_ref() here to avoid moving the message
            headers.insert(
                "x-near-message",
                token_request
                    .message
                    .as_ref()
                    .unwrap_or(&String::new())
                    .parse()
                    .unwrap(),
            );

            // Build a request with the headers
            let mut req = Request::builder()
                .method("POST")
                .uri("/auth/token")
                .body(Body::empty())
                .unwrap();

            *req.headers_mut() = headers;

            // Verify the request using the provider
            match provider.verify_request(&req) {
                Ok(verifier) => {
                    match verifier.verify().await {
                        Ok(auth_response) => {
                            if !auth_response.is_valid {
                                return (
                                    StatusCode::UNAUTHORIZED,
                                    Json(serde_json::json!({
                                        "error": "Authentication failed"
                                    })),
                                )
                                    .into_response();
                            }

                            // If authentication successful, generate new tokens
                            if let Some(key_id) = auth_response.key_id.as_ref() {
                                // Generate a client ID
                                let client_id = token_request.client_name.clone();

                                match state
                                    .0
                                    .token_generator
                                    .generate_token_pair(
                                        &client_id,
                                        key_id,
                                        &auth_response.permissions,
                                    )
                                    .await
                                {
                                    Ok((access_token, refresh_token)) => {
                                        let response = TokenResponse {
                                            access_token,
                                            refresh_token,
                                            token_type: "Bearer".to_string(),
                                            expires_in: state.0.config.jwt.access_token_expiry,
                                            client_id,
                                            error: None,
                                        };

                                        return (StatusCode::OK, Json(response)).into_response();
                                    }
                                    Err(err) => {
                                        error!("Failed to generate tokens: {}", err);
                                        return (
                                            StatusCode::INTERNAL_SERVER_ERROR,
                                            Json(serde_json::json!({
                                                "error": "Failed to generate tokens"
                                            })),
                                        )
                                            .into_response();
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            error!("Authentication failed: {}", err);
                            return (
                                StatusCode::UNAUTHORIZED,
                                Json(serde_json::json!({
                                    "error": format!("Authentication failed: {}", err)
                                })),
                            )
                                .into_response();
                        }
                    }
                }
                Err(err) => {
                    error!("Failed to verify request: {}", err);
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({
                            "error": format!("Failed to verify request: {}", err)
                        })),
                    )
                        .into_response();
                }
            }
        }
    }

    // Original implementation for other auth methods
    // Attempt to authenticate the request based on the token request
    match state
        .0
        .auth_service
        .authenticate_token_request(&token_request)
        .await
    {
        Ok(auth_response) => {
            if !auth_response.is_valid {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "error": "Authentication failed"
                    })),
                )
                    .into_response();
            }

            // If authentication successful, generate new tokens
            if let Some(key_id) = auth_response.key_id.as_ref() {
                // Generate a client ID
                let client_id = token_request.client_name.clone();

                match state
                    .0
                    .token_generator
                    .generate_token_pair(&client_id, key_id, &auth_response.permissions)
                    .await
                {
                    Ok((access_token, refresh_token)) => {
                        let response = TokenResponse {
                            access_token,
                            refresh_token,
                            token_type: "Bearer".to_string(),
                            expires_in: state.0.config.jwt.access_token_expiry,
                            client_id,
                            error: None,
                        };

                        return (StatusCode::OK, Json(response)).into_response();
                    }
                    Err(err) => {
                        error!("Failed to generate tokens: {}", err);
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({
                                "error": "Failed to generate tokens"
                            })),
                        )
                            .into_response();
                    }
                }
            }

            // If no key ID is available, token generation failed
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to generate tokens: no key ID available"
                })),
            )
                .into_response();
        }
        Err(err) => {
            error!("Authentication failed: {}", err);
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": format!("Authentication failed: {}", err)
                })),
            )
                .into_response();
        }
    }
}

/// Refresh token request
#[derive(Debug, Deserialize)]
pub struct RefreshTokenRequest {
    /// Refresh token
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
    Json(request): Json<RefreshTokenRequest>,
) -> impl IntoResponse {
    match state
        .0
        .token_generator
        .refresh_token_pair(&request.refresh_token)
        .await
    {
        Ok((access_token, refresh_token)) => {
            let response = TokenResponse {
                access_token,
                refresh_token,
                token_type: "Bearer".to_string(),
                expires_in: state.0.config.jwt.access_token_expiry,
                client_id: "client_123456".to_string(), // This should come from the token
                error: None,
            };

            (StatusCode::OK, Json(response))
        }
        Err(err) => {
            debug!("Failed to refresh token: {}", err);

            // Use the same response type but with error info
            let error_response = TokenResponse {
                access_token: String::new(),
                refresh_token: String::new(),
                token_type: String::new(),
                expires_in: 0,
                client_id: String::new(),
                error: Some("Invalid refresh token".to_string()),
            };

            (StatusCode::UNAUTHORIZED, Json(error_response))
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

            if let Some(key_id) = auth_response.key_id.as_ref() {
                response_headers.insert("X-Auth-User", key_id.parse().unwrap());
            }

            // Add permissions as a comma-separated list
            if !auth_response.permissions.is_empty() {
                response_headers.insert(
                    "X-Auth-Permissions",
                    auth_response.permissions.join(",").parse().unwrap(),
                );
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

/// Challenge handler
///
/// This endpoint generates a challenge for authentication.
///
/// # Arguments
///
/// * `state` - The application state
/// * `params` - The challenge request parameters
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn challenge_handler(
    state: Extension<Arc<AppState>>,
    Query(params): Query<ChallengeRequest>,
) -> impl IntoResponse {
    // Only process NEAR wallet challenges for now
    if params.provider != "near_wallet" && params.provider != "near" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Unsupported provider"
            })),
        )
            .into_response();
    }

    // Generate a new challenge
    let timestamp = Utc::now().timestamp() as u64;
    let message = format!(
        "Calimero Authentication Request {}:{}",
        timestamp,
        generate_random_challenge()
    );

    // Get the redirect URI
    let redirect_uri = params.redirect_uri.unwrap_or_else(|| "/".to_string());

    // Create the response
    let response = ChallengeResponse {
        message,
        timestamp,
        network: state.0.config.near.network.clone(),
        rpc_url: state.0.config.near.rpc_url.clone(),
        wallet_url: state.0.config.near.wallet_url.clone(),
        redirect_uri,
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// Revoke token request
#[derive(Debug, Deserialize)]
pub struct RevokeTokenRequest {
    /// Client ID to revoke
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
    Json(request): Json<RevokeTokenRequest>,
) -> impl IntoResponse {
    // Validate the client ID
    if request.client_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Client ID cannot be empty"
            })),
        );
    }

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

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "success": true,
                    "message": "Tokens revoked successfully"
                })),
            )
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
