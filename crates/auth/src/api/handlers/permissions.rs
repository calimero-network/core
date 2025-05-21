use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use validator::Validate;

use crate::server::AppState;
use crate::storage::deserialize;
use crate::storage::models::{prefixes, Permission};
use crate::auth::validation::ValidatedJson;

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
    match state.0.storage.set(
        &format!("{}{}", prefixes::PERMISSION, permission_id),
        &serde_json::to_vec(&permission).unwrap(),
    ).await {
        Ok(_) => {
            info!("Created permission: {}", permission_id);
            (
                StatusCode::CREATED,
                Json(serde_json::to_value(PermissionResponse {
                    permission_id: permission.permission_id,
                    name: permission.name,
                    description: permission.description,
                    resource_type: permission.resource_type,
                    resource_ids: permission.resource_ids,
                    method: permission.method,
                    user_id: permission.user_id,
                }).unwrap()),
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

            (StatusCode::OK, Json(serde_json::json!({ "permissions": permissions })))
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
    match state.0.storage.get(&format!("{}{}", prefixes::KEY_PERMISSIONS, key_id)).await {
        Ok(Some(data)) => {
            if let Ok(permissions) = deserialize::<Vec<String>>(&data) {
                (StatusCode::OK, Json(serde_json::json!({ "permissions": permissions })))
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
    let current_permissions = match state.0.storage.get(&format!("{}{}", prefixes::KEY_PERMISSIONS, key_id)).await {
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
    match state.0.storage.set(
        &format!("{}{}", prefixes::KEY_PERMISSIONS, key_id),
        &serde_json::to_vec(&updated_permissions).unwrap(),
    ).await {
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

/// Validate resource IDs
fn validate_resource_ids(resource_ids: &Option<Vec<String>>) -> Result<(), validator::ValidationError> {
    if let Some(ids) = resource_ids {
        if ids.is_empty() {
            let mut err = validator::ValidationError::new("empty_resource_ids");
            err.message = Some("Resource IDs list cannot be empty".into());
            return Err(err);
        }
        for id in ids {
            if id.is_empty() {
                let mut err = validator::ValidationError::new("empty_resource_id");
                err.message = Some("Resource ID cannot be empty".into());
                return Err(err);
            }
            if id.len() > 100 {
                let mut err = validator::ValidationError::new("resource_id_too_long");
                err.message = Some("Resource ID cannot be longer than 100 characters".into());
                return Err(err);
            }
        }
    }
    Ok(())
}

/// Validate permissions
fn validate_permissions(permissions: &Option<Vec<String>>) -> Result<(), validator::ValidationError> {
    if let Some(perms) = permissions {
        if perms.is_empty() {
            let mut err = validator::ValidationError::new("empty_permissions");
            err.message = Some("Permissions list cannot be empty".into());
            return Err(err);
        }
        for perm in perms {
            if perm.is_empty() {
                let mut err = validator::ValidationError::new("empty_permission");
                err.message = Some("Permission cannot be empty".into());
                return Err(err);
            }
            if perm.len() > 200 {
                let mut err = validator::ValidationError::new("permission_too_long");
                err.message = Some("Permission cannot be longer than 200 characters".into());
                return Err(err);
            }
            // Validate permission format (e.g., "resource:action[id]")
            if !is_valid_permission_format(perm) {
                let mut err = validator::ValidationError::new("invalid_permission_format");
                err.message = Some("Invalid permission format".into());
                return Err(err);
            }
        }
    }
    Ok(())
}

/// Check if a permission string has valid format
fn is_valid_permission_format(permission: &str) -> bool {
    // Basic format validation
    let parts: Vec<&str> = permission.split(&[':', '[', ']', '<', '>']).collect();
    
    // Must have at least a resource type
    if parts.is_empty() {
        return false;
    }
    
    // Resource type must not be empty
    if parts[0].is_empty() {
        return false;
    }
    
    // If there's an action, it must not be empty
    if permission.contains(':') && parts.get(1).map_or(true, |p| p.is_empty()) {
        return false;
    }
    
    // Check balanced brackets
    let open_square = permission.chars().filter(|&c| c == '[').count();
    let close_square = permission.chars().filter(|&c| c == ']').count();
    let open_angle = permission.chars().filter(|&c| c == '<').count();
    let close_angle = permission.chars().filter(|&c| c == '>').count();
    
    open_square == close_square && open_angle == close_angle
}
