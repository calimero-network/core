use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use tracing::error;

use crate::server::AppState;
use crate::storage::models::{prefixes, ClientKey};
use crate::storage::{deserialize, serialize};

/// Client creation request
#[derive(Debug, Deserialize)]
pub struct CreateClientRequest {
    /// Client name
    pub name: String,
    /// Permissions
    pub permissions: Vec<String>,
    /// Expiration time (in seconds from now)
    pub expires_in: Option<u64>,
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
    // List all client keys with the root_clients prefix
    let prefix = format!("{}{}", prefixes::ROOT_CLIENTS, key_id);

    match state.0.storage.list_keys(&prefix).await {
        Ok(keys) => {
            let mut client_keys = Vec::new();

            for key in keys {
                // Get the client ID
                let client_id = key.strip_prefix(&prefix).unwrap_or(&key);

                // Get the client key data
                let client_key = format!("{}{}", prefixes::CLIENT_KEY, client_id);

                if let Ok(Some(data)) = state.0.storage.get(&client_key).await {
                    if let Ok(client_key) = deserialize::<ClientKey>(&data) {
                        client_keys.push(serde_json::json!({
                            "client_id": client_id,
                            "root_key_id": client_key.root_key_id,
                            "name": client_key.name,
                            "permissions": client_key.permissions,
                            "created_at": client_key.created_at,
                            "expires_at": client_key.expires_at,
                            "revoked_at": client_key.revoked_at,
                        }));
                    }
                }
            }

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "clients": client_keys
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

/// Client creation handler
///
/// This endpoint creates a new client key for a root key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The root key ID
/// * `request` - The client creation request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn create_client_handler(
    state: Extension<Arc<AppState>>,
    Path(key_id): Path<String>,
    Json(request): Json<CreateClientRequest>,
) -> impl IntoResponse {
    // Check if the root key exists
    let root_key_key = format!("{}{}", prefixes::ROOT_KEY, key_id);

    match state.0.storage.get(&root_key_key).await {
        Ok(Some(_)) => {
            // Generate a client ID
            let client_id = format!(
                "client_{}",
                uuid::Uuid::new_v4().to_string().replace("-", "")
            );

            // Calculate expiration time
            let expires_at = request
                .expires_in
                .map(|secs| Utc::now().timestamp() as u64 + secs);

            // Create the client key
            let client_key = ClientKey::new(
                client_id.clone(),
                key_id.clone(),
                request.name,
                request.permissions,
                expires_at,
            );

            // Store the client key
            let client_key_key = format!("{}{}", prefixes::CLIENT_KEY, client_id);

            match serialize(&client_key) {
                Ok(data) => {
                    match state.0.storage.set(&client_key_key, &data).await {
                        Ok(_) => {
                            // Create the index from root key to client key
                            let index_key = format!("{}{}", prefixes::ROOT_CLIENTS, key_id);
                            let _ = state.0.storage.set(&index_key, &client_id.as_bytes()).await;

                            (
                                StatusCode::CREATED,
                                Json(serde_json::json!({
                                    "client_id": client_id,
                                    "root_key_id": key_id,
                                    "name": client_key.name,
                                    "permissions": client_key.permissions,
                                    "created_at": client_key.created_at,
                                    "expires_at": client_key.expires_at,
                                })),
                            )
                        }
                        Err(err) => {
                            error!("Failed to store client key: {}", err);
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({
                                    "error": "Failed to store client key"
                                })),
                            )
                        }
                    }
                }
                Err(err) => {
                    error!("Failed to serialize client key: {}", err);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Failed to serialize client key"
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
    let client_key_key = format!("{}{}", prefixes::CLIENT_KEY, client_id);

    match state.0.storage.get(&client_key_key).await {
        Ok(Some(data)) => {
            let mut client_key: ClientKey = match deserialize(&data) {
                Ok(key) => key,
                Err(err) => {
                    error!("Failed to deserialize client key: {}", err);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Failed to deserialize client key"
                        })),
                    );
                }
            };

            // Check if the client key belongs to the specified root key
            if client_key.root_key_id != key_id {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "Client key does not belong to the specified root key"
                    })),
                );
            }

            // Mark the client key as revoked
            client_key.revoked_at = Some(Utc::now().timestamp() as u64);

            // Store the updated client key
            match serialize(&client_key) {
                Ok(data) => match state.0.storage.set(&client_key_key, &data).await {
                    Ok(_) => (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "client_id": client_id,
                            "revoked_at": client_key.revoked_at,
                        })),
                    ),
                    Err(err) => {
                        error!("Failed to update client key: {}", err);
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({
                                "error": "Failed to update client key"
                            })),
                        )
                    }
                },
                Err(err) => {
                    error!("Failed to serialize client key: {}", err);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Failed to serialize client key"
                        })),
                    )
                }
            }
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
