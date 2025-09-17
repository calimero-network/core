use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::error;
use validator::Validate;

use crate::api::handlers::auth::{error_response, success_response};
use crate::auth::validation::{sanitize_identifier, sanitize_string};
use crate::server::AppState;
use crate::storage::models::KeyType;

/// Create key request
#[derive(Debug, Deserialize, Validate)]
pub struct CreateKeyRequest {
    /// Public key
    #[validate(length(min = 1, message = "Public key is required"))]
    pub public_key: String,

    /// Authentication method
    #[validate(length(min = 1, message = "Authentication method is required"))]
    pub auth_method: String,

    /// Provider-specific data
    pub provider_data: Value,

    /// Target node URL for which to create the root key
    pub target_node_url: Option<String>,
}

/// Key list handler
///
/// This endpoint lists all root keys.
///
/// # Arguments
///
/// * `state` - The application state
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn list_keys_handler(state: Extension<Arc<AppState>>) -> impl IntoResponse {
    match state.0.key_manager.list_keys(KeyType::Root).await {
        Ok(keys) => {
            let root_keys = keys
                .into_iter()
                .map(|(key_id, key)| {
                    serde_json::json!({
                        "key_id": key_id,
                        "public_key": key.public_key,
                        "auth_method": key.auth_method,
                        "created_at": key.metadata.created_at,
                        "revoked_at": key.metadata.revoked_at,
                        "permissions": key.permissions,
                    })
                })
                .collect::<Vec<_>>();

            success_response(root_keys, None)
        }
        Err(err) => {
            error!("Failed to list keys: {}", err);
            error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string(), None)
        }
    }
}

/// Key creation handler
///
/// This endpoint creates a new root key using the appropriate auth provider.
///
/// # Arguments
///
/// * `state` - The application state
/// * `request` - The key creation request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn create_key_handler(
    state: Extension<Arc<AppState>>,
    Json(mut request): Json<CreateKeyRequest>,
) -> impl IntoResponse {
    // Sanitize inputs to prevent injection attacks
    request.auth_method = sanitize_identifier(&request.auth_method);
    request.public_key = sanitize_string(&request.public_key);

    // Validate sanitized inputs are not empty
    if request.auth_method.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Authentication method must contain valid characters",
            None,
        );
    }

    if request.public_key.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Public key cannot be empty after sanitization",
            None,
        );
    }

    // Extract node URL from request for node-specific key creation
    let node_url = request.target_node_url.clone();

    let provider = match state.0.auth_service.get_provider(&request.auth_method) {
        Some(provider) => provider,
        None => {
            error!("Failed to get provider: {}", request.auth_method);
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Provider not found".to_string(),
                None,
            );
        }
    };

    match provider
        .create_root_key(
            &request.public_key,
            &request.auth_method,
            request.provider_data,
            node_url.as_deref(),
        )
        .await
    {
        Ok(was_updated) => success_response(
            serde_json::json!({
                "status": true,
                "message": if was_updated { "Key was updated" } else { "Key was created" }
            }),
            None,
        ),
        Err(e) => {
            error!("Failed to create root key: {}", e);
            error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
        }
    }
}

/// Key deletion handler
///
/// This endpoint revokes a root key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The key ID to delete
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn delete_key_handler(
    state: Extension<Arc<AppState>>,
    Path(key_id): Path<String>,
) -> impl IntoResponse {
    // First check if there's at least one active root key
    match state.0.key_manager.list_keys(KeyType::Root).await {
        Ok(keys) => {
            let active_keys = keys
                .iter()
                .filter(|(_, key)| key.is_root_key() && key.is_valid())
                .count();

            // If this is the last active key, prevent deletion
            if active_keys <= 1 {
                error!("Cannot delete the last active root key");
                return success_response(
                    serde_json::json!({
                        "status": false,
                        "message": "Cannot delete the last active root key"
                    }),
                    None,
                );
            }
        }
        Err(err) => {
            error!("Failed to list root keys: {}", err);
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to check root keys".to_string(),
                None,
            );
        }
    }

    match state.0.key_manager.get_key(&key_id).await {
        Ok(Some(mut key)) if key.is_root_key() => {
            // Mark the key as revoked
            key.revoke();

            // Store the updated key
            match state.0.key_manager.set_key(&key_id, &key).await {
                Ok(_) => success_response(
                    serde_json::json!({
                        "key_id": key_id,
                        "revoked_at": key.metadata.revoked_at,
                    }),
                    None,
                ),
                Err(err) => {
                    error!("Failed to update root key: {}", err);
                    error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Failed to update root key".to_string(),
                        None,
                    )
                }
            }
        }
        Ok(Some(_)) => error_response(StatusCode::BAD_REQUEST, "Not a root key".to_string(), None),
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            "Root key not found".to_string(),
            None,
        ),
        Err(err) => {
            error!("Failed to get root key: {}", err);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to get root key".to_string(),
                None,
            )
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}
