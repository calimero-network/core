use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use validator::Validate;

use crate::auth::validation::ValidatedJson;
use crate::server::AppState;
use crate::storage::deserialize;
use crate::storage::models::{prefixes, Permission};

/// Permission creation request
#[derive(Debug, Deserialize, Validate)]
pub struct CreatePermissionRequest {
    /// The name of the permission
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    /// The description of the permission
    #[validate(length(min = 1, max = 500))]
    pub description: String,

    /// The resource type
    #[validate(length(min = 1, max = 50))]
    pub resource_type: String,

    /// Optional specific resource IDs
    pub resource_ids: Option<Vec<String>>,

    /// Optional specific method
    #[validate(length(min = 1, max = 50))]
    pub method: Option<String>,

    /// Optional specific user ID
    #[validate(length(min = 1, max = 100))]
    pub user_id: Option<String>,
}

/// Key permissions update request
#[derive(Debug, Deserialize, Validate)]
pub struct UpdateKeyPermissionsRequest {
    /// Permissions to add
    pub add: Option<Vec<String>>,

    /// Permissions to remove
    pub remove: Option<Vec<String>>,
}

/// Permission response model
#[derive(Debug, Serialize)]
pub struct PermissionResponse {
    pub permission_id: String,
    pub name: String,
    pub description: String,
    pub resource_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

/// Create permission handler
///
/// This endpoint creates a new permission.
pub async fn create_permission_handler(
    state: Extension<Arc<AppState>>,
    ValidatedJson(request): ValidatedJson<CreatePermissionRequest>,
) -> impl IntoResponse {
    // Generate a unique permission ID
    let permission_id = format!(
        "{}{}",
        request.resource_type,
        if let Some(method) = &request.method {
            format!(":{}", method)
        } else {
            String::new()
        }
    );

    let permission = Permission::new_scoped(
        permission_id.clone(),
        request.name,
        request.description,
        request.resource_type,
        request.resource_ids,
        request.method,
        request.user_id,
    );

    // Store the permission
    match state
        .0
        .storage
        .set(
            &format!("{}{}", prefixes::PERMISSION, permission_id),
            &serde_json::to_vec(&permission).unwrap(),
        )
        .await
    {
        Ok(_) => {
            info!("Created permission: {}", permission_id);
            (
                StatusCode::CREATED,
                Json(
                    serde_json::to_value(PermissionResponse {
                        permission_id: permission.permission_id,
                        name: permission.name,
                        description: permission.description,
                        resource_type: permission.resource_type,
                        resource_ids: permission.resource_ids,
                        method: permission.method,
                        user_id: permission.user_id,
                    })
                    .unwrap(),
                ),
            )
        }
        Err(err) => {
            error!("Failed to create permission: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to create permission"
                })),
            )
        }
    }
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
                        permissions.push(PermissionResponse {
                            permission_id: permission.permission_id,
                            name: permission.name,
                            description: permission.description,
                            resource_type: permission.resource_type,
                            resource_ids: permission.resource_ids,
                            method: permission.method,
                            user_id: permission.user_id,
                        });
                    }
                }
            }

            (
                StatusCode::OK,
                Json(serde_json::json!({ "permissions": permissions })),
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
    state: Extension<Arc<AppState>>,
    Path(key_id): Path<String>,
) -> impl IntoResponse {
    // Get the key's permissions
    match state
        .0
        .storage
        .get(&format!("{}{}", prefixes::KEY_PERMISSIONS, key_id))
        .await
    {
        Ok(Some(data)) => {
            if let Ok(permissions) = deserialize::<Vec<String>>(&data) {
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "permissions": permissions })),
                )
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": "Failed to deserialize permissions"
                    })),
                )
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Key not found"
            })),
        ),
        Err(err) => {
            error!("Failed to get key permissions: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to get key permissions"
                })),
            )
        }
    }
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
    state: Extension<Arc<AppState>>,
    Path(key_id): Path<String>,
    ValidatedJson(request): ValidatedJson<UpdateKeyPermissionsRequest>,
) -> impl IntoResponse {
    // Get current permissions
    let current_permissions = match state
        .0
        .storage
        .get(&format!("{}{}", prefixes::KEY_PERMISSIONS, key_id))
        .await
    {
        Ok(Some(data)) => deserialize::<Vec<String>>(&data).unwrap_or_default(),
        _ => Vec::new(),
    };

    // Update permissions
    let mut updated_permissions = current_permissions.clone();

    // Add new permissions
    if let Some(add) = request.add {
        for perm in add {
            if !updated_permissions.contains(&perm) {
                updated_permissions.push(perm);
            }
        }
    }

    // Remove permissions
    if let Some(remove) = request.remove {
        updated_permissions.retain(|p| !remove.contains(p));
    }

    // Store updated permissions
    match state
        .0
        .storage
        .set(
            &format!("{}{}", prefixes::KEY_PERMISSIONS, key_id),
            &serde_json::to_vec(&updated_permissions).unwrap(),
        )
        .await
    {
        Ok(_) => {
            info!("Updated permissions for key: {}", key_id);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "permissions": updated_permissions
                })),
            )
        }
        Err(err) => {
            error!("Failed to update key permissions: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to update key permissions"
                })),
            )
        }
    }
}
