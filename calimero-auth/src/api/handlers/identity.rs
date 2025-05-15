use axum::{
    extract::Extension,
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Serialize;
use std::sync::Arc;

use crate::auth::token::service::TokenService;

/// Identity response
#[derive(Serialize)]
pub struct IdentityResponse {
    /// The key ID (user ID)
    pub key_id: String,
    /// The permissions granted to the user
    pub permissions: Vec<String>,
}

/// Get the identity information from a token
///
/// # Arguments
///
/// * `headers` - The request headers
/// * `Extension(token_service)` - The token service
///
/// # Returns
///
/// * `Result<Json<IdentityResponse>, StatusCode>` - The identity or error
pub async fn get_identity(
    headers: HeaderMap,
    Extension(token_service): Extension<Arc<TokenService>>,
) -> Result<Json<IdentityResponse>, StatusCode> {
    // Validate the token in the headers
    let auth_response = token_service
        .validate_token_from_headers(&headers)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    
    if !auth_response.is_valid || auth_response.key_id.is_none() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    
    // Return the identity information
    Ok(Json(IdentityResponse {
        key_id: auth_response.key_id.unwrap(),
        permissions: auth_response.permissions,
    }))
} 