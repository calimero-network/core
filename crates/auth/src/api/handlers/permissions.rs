use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use tracing::error;

use crate::server::AppState;
use crate::storage::deserialize;
use crate::storage::models::{prefixes, Permission};

/// Key permissions update request
#[derive(Debug, Deserialize)]
pub struct UpdateKeyPermissionsRequest {
    /// Permissions to add
    pub add: Option<Vec<String>>,
    /// Permissions to remove
    pub remove: Option<Vec<String>>,
}

/// Permission list handler
///
/// This endpoint lists all available permissions.
///
/// # Arguments
///
/// * `state` - The application state
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn list_permissions_handler(state: Extension<Arc<AppState>>) -> impl IntoResponse {
    // List all permissions with the permission prefix
    match state.0.storage.list_keys(prefixes::PERMISSION).await {
        Ok(keys) => {
            let mut permissions = Vec::new();

            for key in keys {
                // Get the permission data
                if let Ok(Some(data)) = state.0.storage.get(&key).await {
                    if let Ok(permission) = deserialize::<Permission>(&data) {
                        permissions.push(serde_json::json!({
                            "permission_id": permission.permission_id,
                            "name": permission.name,
                            "description": permission.description,
                            "resource_type": permission.resource_type,
                        }));
                    }
                }
            }

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "permissions": permissions
                })),
            )
        }
        Err(err) => {
            error!("Failed to list permissions: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to list permissions"
                })),
            )
        }
    }
}

/// Key permissions handler
///
/// This endpoint gets the permissions for a root key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The root key ID
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn get_key_permissions_handler(
    _state: Extension<Arc<AppState>>,
    Path(_key_id): Path<String>,
) -> impl IntoResponse {
    // In a real implementation, you would look up the permissions for the key
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "permissions": ["admin"]
        })),
    )
}

/// Key permissions update handler
///
/// This endpoint updates the permissions for a root key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The root key ID
/// * `request` - The permissions update request
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn update_key_permissions_handler(
    _state: Extension<Arc<AppState>>,
    Path(_key_id): Path<String>,
    Json(_request): Json<UpdateKeyPermissionsRequest>,
) -> impl IntoResponse {
    // In a real implementation, you would update the permissions for the key
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "permissions": ["admin"]
        })),
    )
}
