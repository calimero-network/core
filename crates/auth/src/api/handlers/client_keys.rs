use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::error;
use validator::Validate;

use super::auth::{internal_error_response, success_response, unauthorized_response};
use crate::api::handlers::auth::TokenResponse;
use crate::auth::validation::ValidatedJson;
use crate::server::AppState;
use crate::storage::models::Key;

/// Generate client key request
#[derive(Debug, Deserialize, Validate)]
pub struct GenerateClientKeyRequest {
    /// Context ID selected by user
    #[validate(length(min = 1, message = "Context ID is required"))]
    pub context_id: String,

    /// Context identity selected by user
    #[validate(length(min = 1, message = "Context identity is required"))]
    pub context_identity: String,

    /// Additional permissions requested
    pub permissions: Option<Vec<String>>,
}

/// Client list handler
///
/// This endpoint lists all client keys for a root key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The root key ID
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn list_clients_handler(
    state: Extension<Arc<AppState>>,
    Path(key_id): Path<String>,
) -> impl IntoResponse {
    match state.0.key_manager.list_client_keys_for_root(&key_id).await {
        Ok(client_keys) => {
            let clients = client_keys
                .into_iter()
                .map(|key| {
                    serde_json::json!({
                        "client_id": key.root_key_id.clone().unwrap_or_default(),
                        "root_key_id": key.root_key_id.unwrap_or_default(),
                        "name": key.name.unwrap_or_default(),
                        "permissions": key.permissions,
                        "created_at": key.metadata.created_at,
                        "revoked_at": key.metadata.revoked_at,
                    })
                })
                .collect::<Vec<_>>();

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "clients": clients
                })),
            )
        }
        Err(err) => {
            error!("Failed to list client keys: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to list client keys"
                })),
            )
        }
    }
}

/// Generate client key handler
///
/// This endpoint generates a client key and its JWT tokens after context selection.
/// It requires a valid Root JWT token in the Authorization header.
///
/// # Arguments
///
/// * `state` - The application state
/// * `headers` - Request headers containing Root JWT token
/// * `request` - The client key generation request
///
/// # Returns
///
/// * `impl IntoResponse` - The response with client key tokens
pub async fn generate_client_key_handler(
    state: Extension<Arc<AppState>>,
    headers: HeaderMap,
    ValidatedJson(request): ValidatedJson<GenerateClientKeyRequest>,
) -> impl IntoResponse {
    // Verify the Root JWT token from headers
    let auth_response = match state
        .0
        .token_generator
        .verify_token_from_headers(&headers)
        .await
    {
        Ok(response) => response,
        Err(err) => {
            error!("Failed to verify token: {}", err);
            return unauthorized_response("Invalid token");
        }
    };

    // Check if the token is valid and has admin permissions
    if !auth_response.is_valid {
        return unauthorized_response("Invalid token");
    }
    if !auth_response.permissions.contains(&"admin".to_string()) {
        return unauthorized_response("Token does not have admin permissions");
    }

    // The key_id in auth_response is the root key ID
    let root_key_id = auth_response.key_id;

    // Verify the root key exists and is not revoked
    match state.0.key_manager.get_key(&root_key_id).await {
        Ok(Some(key)) if !key.is_valid() => return unauthorized_response("Root key is revoked"),
        Ok(None) => return unauthorized_response("Root key not found"),
        Err(err) => {
            error!("Failed to get root key: {}", err);
            return internal_error_response("Failed to get root key");
        }
        Ok(Some(_)) => (), // Key exists and is valid
    };

    // Get current timestamp for unique client ID
    let timestamp = Utc::now().timestamp();

    // Create a client ID using SHA256 hash
    let mut hasher = Sha256::new();
    hasher.update(
        format!(
            "client:{}:{}:{}",
            request.context_id, request.context_identity, timestamp
        )
        .as_bytes(),
    );
    let hash = hasher.finalize();
    let client_id = hex::encode(hash);

    // Create context-specific permission
    let mut permissions = vec![format!(
        "context[{},{}]",
        request.context_id, request.context_identity
    )];

    // Add any additional permissions requested
    if let Some(additional_perms) = request.permissions {
        permissions.extend(additional_perms);
    }

    // Create a descriptive name for the client key
    let name = format!(
        "Context Client - {} ({})",
        request.context_id, request.context_identity
    );

    // Create the client key with context permission and any additional permissions
    let client_key = Key::new_client_key(root_key_id.clone(), name, permissions);

    // Store the client key
    if let Err(err) = state.0.key_manager.set_key(&client_id, &client_key).await {
        error!("Failed to store client key: {}", err);
        return internal_error_response("Failed to store client key");
    }

    // Generate JWT tokens for the client key
    match state
        .0
        .token_generator
        .generate_token_pair(client_id.clone(), client_key.permissions)
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
            error!("Failed to generate client tokens: {}", err);
            internal_error_response("Failed to generate client tokens")
        }
    }
}

/// Client deletion handler
///
/// This endpoint revokes a client key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The root key ID
/// * `client_id` - The client ID to delete
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn delete_client_handler(
    state: Extension<Arc<AppState>>,
    Path((key_id, client_id)): Path<(String, String)>,
) -> impl IntoResponse {
    // Get the client key
    match state.0.key_manager.get_key(&client_id).await {
        Ok(Some(client_key)) => {
            // Verify this client belongs to the specified root key
            if client_key.root_key_id.as_deref() != Some(&key_id) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "Client key does not belong to specified root key"
                    })),
                );
            }

            // Delete/revoke the client key
            if let Err(err) = state.0.key_manager.delete_key(&client_id).await {
                error!("Failed to delete client key: {}", err);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": "Failed to delete client key"
                    })),
                );
            }

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "message": "Client key deleted successfully"
                })),
            )
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Client key not found"
            })),
        ),
        Err(err) => {
            error!("Failed to get client key: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to get client key"
                })),
            )
        }
    }
}
