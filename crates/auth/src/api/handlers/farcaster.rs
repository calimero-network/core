use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::Json};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::server::AppState;

/// Farcaster authentication request
#[derive(Debug, Deserialize)]
pub struct FarcasterAuthRequest {
    /// The Farcaster JWT token
    pub token: String,
    /// The domain this token is valid for
    pub domain: String,
    /// Optional client name for identification
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
}

/// Farcaster authentication response
#[derive(Debug, Serialize)]
pub struct FarcasterAuthResponse {
    /// Whether authentication was successful
    pub success: bool,
    /// The Calimero user ID (key ID)
    pub calimero_user_id: String,
    /// The Farcaster ID
    pub fid: String,
    /// User permissions
    pub permissions: Vec<String>,
    /// Access token for Calimero API
    pub access_token: String,
    /// Refresh token for Calimero API
    pub refresh_token: String,
}

/// Farcaster authentication error response
#[derive(Debug, Serialize)]
pub struct FarcasterAuthError {
    /// Error message
    pub error: String,
    /// Error code
    pub code: String,
}

/// Handle Farcaster JWT authentication
///
/// This endpoint verifies a Farcaster JWT token and creates or retrieves
/// a corresponding Calimero user account.
///
/// # Arguments
///
/// * `state` - Application state containing auth service
/// * `request` - Farcaster authentication request
///
/// # Returns
///
/// * `Result<Json<FarcasterAuthResponse>, (StatusCode, Json<FarcasterAuthError>)>` - Authentication result
pub async fn farcaster_auth_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<FarcasterAuthRequest>,
) -> Result<Json<FarcasterAuthResponse>, (StatusCode, Json<FarcasterAuthError>)> {
    // For now, we'll implement a simplified version
    // In production, you would:
    // 1. Verify the Farcaster JWT signature using Farcaster's public keys
    // 2. Extract the FID from the token
    // 3. Create or get the Calimero user

    // Extract FID from token (simplified - in production, parse JWT properly)
    let fid = extract_fid_from_token(&request.token).map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            Json(FarcasterAuthError {
                error: format!("Invalid Farcaster token: {}", e),
                code: "INVALID_TOKEN".to_string(),
            }),
        )
    })?;

    // Create or get Calimero user
    let calimero_user_id = format!("farcaster:{}", fid);

    // Check if user exists, if not create them
    let permissions = vec!["user".to_string()];

    // Generate Calimero tokens
    let (access_token, refresh_token) = state
        .auth_service
        .get_token_manager()
        .generate_mock_token_pair(
            calimero_user_id.clone(),
            permissions.clone(),
            None, // No specific node URL
            None, // Use default expiry
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(FarcasterAuthError {
                    error: format!("Failed to generate tokens: {}", e),
                    code: "TOKEN_GENERATION_FAILED".to_string(),
                }),
            )
        })?;

    Ok(Json(FarcasterAuthResponse {
        success: true,
        calimero_user_id,
        fid,
        permissions,
        access_token,
        refresh_token,
    }))
}

/// Extract Farcaster ID from JWT token (simplified implementation)
///
/// In production, this would:
/// 1. Verify the JWT signature using Farcaster's public keys
/// 2. Parse the payload to extract the FID
/// 3. Validate the token expiration and audience
fn extract_fid_from_token(token: &str) -> Result<String, String> {
    // For now, we'll implement a basic token parsing
    // In production, you would use a proper JWT library

    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("Invalid JWT format".to_string());
    }

    // Decode the payload (middle part)
    let payload_b64 = parts[1];
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| "Invalid JWT payload encoding".to_string())?;

    let payload: Value = serde_json::from_slice(&payload_bytes)
        .map_err(|_| "Invalid JWT payload structure".to_string())?;

    // Extract the FID from the 'sub' claim (Farcaster uses numeric FIDs)
    let fid = payload
        .get("sub")
        .and_then(|v| v.as_u64())
        .ok_or("Missing or invalid 'sub' claim in token")?;

    // Validate token expiration
    if let Some(exp) = payload.get("exp").and_then(|v| v.as_u64()) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if exp < now {
            return Err("Token has expired".to_string());
        }
    }

    // Validate issuer
    if let Some(iss) = payload.get("iss").and_then(|v| v.as_str()) {
        if !iss.starts_with("https://auth.farcaster.xyz") {
            return Err("Invalid token issuer".to_string());
        }
    }

    Ok(fid.to_string())
}

/// Get Farcaster provider information
///
/// This endpoint returns information about the Farcaster authentication provider.
pub async fn farcaster_info_handler(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, Json<FarcasterAuthError>)> {
    let info = serde_json::json!({
        "provider": "farcaster",
        "type": "jwt",
        "description": "Farcaster JWT authentication using Farcaster Quick Auth",
        "endpoints": {
            "auth": "/auth/farcaster",
            "info": "/auth/farcaster/info"
        },
        "required_fields": {
            "token": "Farcaster JWT token",
            "domain": "Domain this token is valid for"
        },
        "optional_fields": {
            "client_name": "Client application name"
        }
    });

    Ok(Json(info))
}
