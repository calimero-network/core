use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::error;

use crate::server::AppState;
use crate::storage::models::{Key, KeyType};

/// Key creation request
#[derive(Debug, Deserialize)]
pub struct CreateKeyRequest {
    /// Public key
    pub public_key: String,
    /// Authentication method
    pub auth_method: String,
    /// Wallet address (if applicable)
    pub wallet_address: Option<String>,
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
            let root_keys = keys.into_iter().map(|(key_id, key)| {
                serde_json::json!({
                    "key_id": key_id,
                    "public_key": key.public_key,
                    "auth_method": key.auth_method,
                    "created_at": key.metadata.created_at,
                    "revoked_at": key.metadata.revoked_at,
                })
            }).collect::<Vec<_>>();

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "keys": root_keys
                })),
            )
        }
        Err(err) => {
            error!("Failed to list keys: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to list keys"
                })),
            )
        }
    }
}

/// Key creation handler
///
/// This endpoint creates a new root key.
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
    Json(request): Json<CreateKeyRequest>,
) -> impl IntoResponse {
    // Create a hash of the public key to use as the key ID
    let mut hasher = Sha256::new();
    hasher.update(request.public_key.as_bytes());
    let hash = hasher.finalize();
    let key_id = hex::encode(hash);

    // Create the root key
    let root_key = Key::new_root_key(request.public_key, request.auth_method);

    // Store the root key
    match state.0.key_manager.set_key(&key_id, &root_key).await {
        Ok(_) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "key_id": key_id,
                "public_key": root_key.public_key,
                "auth_method": root_key.auth_method,
                "created_at": root_key.metadata.created_at,
            })),
        ),
        Err(err) => {
            error!("Failed to store root key: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to store root key"
                })),
            )
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
    match state.0.key_manager.get_key(&key_id).await {
        Ok(Some(mut key)) if key.is_root_key() => {
            // Mark the key as revoked
            key.revoke();

            // Store the updated key
            match state.0.key_manager.set_key(&key_id, &key).await {
                Ok(_) => (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "key_id": key_id,
                        "revoked_at": key.metadata.revoked_at,
                    })),
                ),
                Err(err) => {
                    error!("Failed to update root key: {}", err);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Failed to update root key"
                        })),
                    )
                }
            }
        }
        Ok(Some(_)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Not a root key"
            })),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Root key not found"
            })),
        ),
        Err(err) => {
            error!("Failed to get root key: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to get root key"
                })),
            )
        }
    }
}
