use axum::{
    extract::{Extension, Query},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    Json,
};
use std::collections::HashMap;
use std::sync::Arc;

use crate::auth::service::AuthService;
use crate::auth::token::service::TokenService;
use crate::auth::token::{TokenRequest, TokenResponse};
use crate::config::Config;
use crate::error::AuthError;

/// Serve the login page
///
/// # Arguments
///
/// * `Extension(config)` - The server configuration
///
/// # Returns
///
/// * `impl IntoResponse` - The login page HTML
pub async fn login_page(
    Extension(config): Extension<Config>,
) -> impl IntoResponse {
    // Read the login template from the templates directory
    let template_path = config.server.template_dir.join("login.html");
    
    match std::fs::read_to_string(template_path) {
        Ok(content) => Html(content).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to load login page template"
        ).into_response(),
    }
}

/// Handle authentication callback
///
/// # Arguments
///
/// * `Query(params)` - The callback query parameters
/// * `Extension(auth_service)` - The authentication service
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn callback_handler(
    Query(params): Query<HashMap<String, String>>,
    Extension(auth_service): Extension<Arc<AuthService>>,
) -> impl IntoResponse {
    // Process the callback parameters based on the authentication method
    // For now, just return a simple response
    "Authentication callback processed"
}

/// Handle token requests
///
/// # Arguments
///
/// * `Json(request)` - The token request
/// * `Extension(auth_service)` - The authentication service
/// * `Extension(token_service)` - The token service
///
/// # Returns
///
/// * `Result<Json<TokenResponse>, StatusCode>` - The token response or error
pub async fn token_handler(
    Json(request): Json<TokenRequest>,
    Extension(auth_service): Extension<Arc<AuthService>>,
    Extension(token_service): Extension<Arc<TokenService>>,
) -> Result<Json<TokenResponse>, StatusCode> {
    // First, authenticate the request using the auth service
    let auth_result = auth_service.authenticate_token_request(&request).await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    
    if !auth_result.is_valid {
        return Err(StatusCode::UNAUTHORIZED);
    }
    
    // If authenticated, create a token response
    let key_id = auth_result.key_id.unwrap_or_else(|| "anonymous".to_string());
    
    // Generate tokens using the token service
    let token_response = token_service
        .create_token_response(&request.client_name, &key_id, &auth_result.permissions)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    Ok(Json(token_response))
} 