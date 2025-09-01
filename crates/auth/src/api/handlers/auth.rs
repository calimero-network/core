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
) -> impl IntoResponse {
    let enabled_providers = state.0.auth_service.providers();

    if !enabled_providers.is_empty() {
        info!("Loading authentication UI");

        if let Some(file) = AuthUiStaticFiles::get("index.html") {
            let html_content = String::from_utf8_lossy(&file.data);

            return (
                StatusCode::OK,
                [("Content-Type", "text/html")],
                html_content.into_owned().into_bytes(),
            );
        }

        error!("Failed to load authentication UI - index.html not found");
    }

    warn!("No authentication providers available");
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

    // Extract node URL from client_name for node-specific token generation
    let node_url = Some(token_request.client_name.clone());

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

    // Authenticate directly using the token request with node context
    let auth_response = match state
        .0
        .auth_service
        .authenticate_token_request(&token_request, node_url.as_deref())
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

    // Generate tokens using the validated permissions from auth_response and node_id
    match state
        .0
        .token_generator
        .generate_token_pair(key_id.clone(), auth_response.permissions, node_url)
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
    headers: HeaderMap,
    ValidatedJson(refresh_request): ValidatedJson<RefreshTokenRequest>,
) -> impl IntoResponse {
    // First check if access token is still valid
    match state
        .0
        .token_generator
        .verify_token(&refresh_request.access_token)
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

    // Verify the refresh token and extract claims
    let refresh_claims = match state
        .0
        .token_generator
        .verify_token(&refresh_request.refresh_token)
        .await
    {
        Ok(claims) => claims,
        Err(err) => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                format!("Invalid refresh token: {}", err),
                None,
            );
        }
    };

    // Check node URL if token has node information
    if let Some(token_node_url) = &refresh_claims.node_url {
        // Get referrer URL from headers
        if let Some(referrer) = headers.get("referer") {
            if let Ok(referrer_str) = referrer.to_str() {
                // Compare if referrer starts with the token's node URL
                if !referrer_str.starts_with(token_node_url) {
                    let mut error_headers = HeaderMap::new();
                    error_headers.insert("X-Auth-Error", "invalid_node".parse().unwrap());
                    return error_response(
                        StatusCode::FORBIDDEN,
                        "Token is not valid for this node",
                        Some(error_headers),
                    );
                }
            }
        }
    }

    // Use the refresh token to generate new tokens
    // Note: refresh_token_pair automatically preserves node_url from the refresh token
    match state
        .0
        .token_generator
        .refresh_token_pair(&refresh_request.refresh_token)
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
    let token =
        extract_token_from_headers(&headers).or_else(|| extract_token_from_forwarded_uri(&headers));

    let token = match token {
        Some(token) => token.to_string(),
        None => {
            let mut error_headers = HeaderMap::new();
            error_headers.insert("X-Auth-Error", "missing_token".parse().unwrap());
            return error_response(
                StatusCode::UNAUTHORIZED,
                "No token provided",
                Some(error_headers),
            );
        }
    };

    // Validate the token
    match state.0.token_generator.verify_token(&token).await {
        Ok(claims) => {
            // Check node URL if token has node information
            if let Some(token_node_url) = &claims.node_url {
                // Get referrer URL from headers
                if let Some(referrer) = headers.get("referer") {
                    if let Ok(referrer_str) = referrer.to_str() {
                        // Compare if referrer starts with the token's node URL
                        if !referrer_str.starts_with(token_node_url) {
                            let mut error_headers = HeaderMap::new();
                            error_headers.insert("X-Auth-Error", "invalid_node".parse().unwrap());
                            return error_response(
                                StatusCode::FORBIDDEN,
                                "Token is not valid for this node",
                                Some(error_headers),
                            );
                        }
                    }
                }
            }

            // Verify the key exists and is valid
            let _key = match state.0.key_manager.get_key(&claims.sub).await {
                Ok(Some(key)) if key.is_valid() => key,
                Ok(Some(_)) => {
                    let mut error_headers = HeaderMap::new();
                    error_headers.insert("X-Auth-Error", "token_revoked".parse().unwrap());
                    return error_response(
                        StatusCode::FORBIDDEN,
                        "Key has been revoked",
                        Some(error_headers),
                    );
                }
                Ok(None) => {
                    let mut error_headers = HeaderMap::new();
                    error_headers.insert("X-Auth-Error", "invalid_token".parse().unwrap());
                    return error_response(
                        StatusCode::UNAUTHORIZED,
                        "Key not found",
                        Some(error_headers),
                    );
                }
                Err(_) => {
                    let mut error_headers = HeaderMap::new();
                    error_headers.insert("X-Auth-Error", "invalid_token".parse().unwrap());
                    return error_response(
                        StatusCode::UNAUTHORIZED,
                        "Failed to verify key",
                        Some(error_headers),
                    );
                }
            };

            // Create response headers
            let mut response_headers = HeaderMap::new();

            // Add user ID header
            response_headers.insert("X-Auth-User", claims.sub.parse().unwrap());

            // Add permissions as a comma-separated list
            if !claims.permissions.is_empty() {
                response_headers.insert(
                    "X-Auth-Permissions",
                    claims.permissions.join(",").parse().unwrap(),
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

/// Extracts the token from the Authorization header.
fn extract_token_from_headers(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("Authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.trim())
}

/// Extracts the token from the X-Forwarded-Uri header.
fn extract_token_from_forwarded_uri<'a>(headers: &'a HeaderMap) -> Option<&'a str> {
    headers
        .get("X-Forwarded-Uri")
        .and_then(|value| value.to_str().ok())
        .and_then(|uri_str| {
            uri_str.split('?').nth(1).and_then(|query| {
                query
                    .split('&')
                    .find(|param| param.starts_with("token="))
                    .map(|param| &param[6..])
            })
        })
}

/// OAuth callback handler for meroctl authentication flow
///
/// This endpoint serves a simple authentication form that allows users
/// to authenticate and then redirects back to the meroctl callback server.
///
/// # Arguments
///
/// * `state` - The application state
/// * `Query(params)` - Query parameters including callback-url
///
/// # Returns
///
/// * `impl IntoResponse` - HTML form for authentication
pub async fn callback_handler(
    _state: Extension<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    // Extract the callback URL from query parameters
    let default_callback = "http://127.0.0.1:9080/callback".to_string();
    let callback_url = params.get("callback-url").unwrap_or(&default_callback);

    // Create a simple authentication form
    let html = format!(
        r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Calimero Authentication</title>
    <style>
        * {{
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }}
        
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
            color: white;
        }}
        
        .container {{
            text-align: center;
            background: rgba(255, 255, 255, 0.1);
            backdrop-filter: blur(10px);
            border-radius: 20px;
            padding: 3rem 2rem;
            border: 1px solid rgba(255, 255, 255, 0.2);
            box-shadow: 0 8px 32px rgba(0, 0, 0, 0.1);
            max-width: 500px;
            width: 90%;
        }}
        
        h1 {{
            font-size: 2rem;
            margin-bottom: 2rem;
            font-weight: 600;
        }}
        
        .form-group {{
            margin-bottom: 1.5rem;
            text-align: left;
        }}
        
        label {{
            display: block;
            margin-bottom: 0.5rem;
            font-weight: 500;
        }}
        
        input, select {{
            width: 100%;
            padding: 0.75rem;
            border: none;
            border-radius: 8px;
            background: rgba(255, 255, 255, 0.9);
            color: #333;
            font-size: 1rem;
        }}
        
        button {{
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
            border: none;
            padding: 1rem 2rem;
            border-radius: 8px;
            font-size: 1.1rem;
            font-weight: 600;
            cursor: pointer;
            transition: transform 0.2s;
            margin-top: 1rem;
        }}
        
        button:hover {{
            transform: translateY(-2px);
        }}
        
        .info {{
            background: rgba(255, 255, 255, 0.1);
            padding: 1rem;
            border-radius: 8px;
            margin-bottom: 1.5rem;
            font-size: 0.9rem;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>🔐 Calimero Authentication</h1>
        
        <div class="info">
            <strong>Note:</strong> This is a simplified authentication flow for meroctl CLI.
            For production use, implement proper NEAR wallet authentication.
        </div>
        
        <form id="authForm">
            <div class="form-group">
                <label for="authMethod">Authentication Method:</label>
                <select id="authMethod" name="authMethod" required>
                    <option value="near_wallet">NEAR Wallet</option>
                    <option value="user_password">Username/Password</option>
                </select>
            </div>
            
            <div class="form-group">
                <label for="publicKey">Public Key:</label>
                <input type="text" id="publicKey" name="publicKey" placeholder="ed25519:..." required>
            </div>
            
            <div class="form-group">
                <label for="clientName">Client Name:</label>
                <input type="text" id="clientName" name="clientName" placeholder="meroctl-cli" required>
            </div>
            
            <button type="submit">Authenticate</button>
        </form>
    </div>
    
    <script>
        document.getElementById('authForm').addEventListener('submit', async function(e) {{
            e.preventDefault();
            
            const formData = new FormData(e.target);
            const data = {{
                auth_method: formData.get('authMethod'),
                public_key: formData.get('publicKey'),
                client_name: formData.get('clientName'),
                permissions: ['admin'],
                timestamp: Math.floor(Date.now() / 1000),
                provider_data: {{}}
            }};
            
            try {{
                // For now, generate a simple token (in production, this would validate credentials)
                const accessToken = 'temp_access_token_' + Date.now();
                const refreshToken = 'temp_refresh_token_' + Date.now();
                
                // Redirect back to meroctl with tokens
                const callbackUrl = '{callback_url}';
                const redirectUrl = new URL(callbackUrl);
                redirectUrl.searchParams.set('access_token', accessToken);
                redirectUrl.searchParams.set('refresh_token', refreshToken);
                
                window.location.href = redirectUrl.toString();
            }} catch (error) {{
                console.error('Authentication failed:', error);
                alert('Authentication failed. Please try again.');
            }}
        }});
    </script>
</body>
</html>
        "#,
        callback_url = callback_url
    );

    (
        StatusCode::OK,
        [("Content-Type", "text/html")],
        html.into_bytes(),
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
