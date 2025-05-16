use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::error;

use crate::server::AppState;
use crate::storage::models::{prefixes, RootKey};
use crate::storage::{deserialize, serialize};

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
    // List all keys with the root key prefix
    match state.0.storage.list_keys(prefixes::ROOT_KEY).await {
        Ok(keys) => {
            let mut root_keys = Vec::new();

            for key in keys {
                // Skip the prefix
                let key_id = key.strip_prefix(prefixes::ROOT_KEY).unwrap_or(&key);

                // Get the key data
                if let Ok(Some(data)) = state.0.storage.get(&key).await {
                    if let Ok(root_key) = deserialize::<RootKey>(&data) {
                        root_keys.push(serde_json::json!({
                            "key_id": key_id,
                            "public_key": root_key.public_key,
                            "auth_method": root_key.auth_method,
                            "created_at": root_key.created_at,
                            "revoked_at": root_key.revoked_at,
                            "last_used_at": root_key.last_used_at,
                        }));
                    }
                }
            }

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
    let root_key = RootKey {
        public_key: request.public_key,
        auth_method: request.auth_method,
        created_at: Utc::now().timestamp() as u64,
        revoked_at: None,
        last_used_at: None,
        permissions: Vec::new(), // Empty permissions by default
        metadata: None,
    };

    // Store the root key
    let key = format!("{}{}", prefixes::ROOT_KEY, key_id);
    match serialize(&root_key) {
        Ok(data) => match state.0.storage.set(&key, &data).await {
            Ok(_) => (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "key_id": key_id,
                    "public_key": root_key.public_key,
                    "auth_method": root_key.auth_method,
                    "created_at": root_key.created_at,
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
        },
        Err(err) => {
            error!("Failed to serialize root key: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to serialize root key"
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
    // Get the key
    let key = format!("{}{}", prefixes::ROOT_KEY, key_id);

    match state.0.storage.get(&key).await {
        Ok(Some(data)) => {
            let mut root_key: RootKey = match deserialize(&data) {
                Ok(key) => key,
                Err(err) => {
                    error!("Failed to deserialize root key: {}", err);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Failed to deserialize root key"
                        })),
                    );
                }
            };

            // Mark the key as revoked
            root_key.revoked_at = Some(Utc::now().timestamp() as u64);

            // Store the updated key
            match serialize(&root_key) {
                Ok(data) => match state.0.storage.set(&key, &data).await {
                    Ok(_) => (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "key_id": key_id,
                            "revoked_at": root_key.revoked_at,
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
                },
                Err(err) => {
                    error!("Failed to serialize root key: {}", err);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Failed to serialize root key"
                        })),
                    )
                }
            }
        }
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
