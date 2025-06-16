use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use validator::Validate;

use crate::api::handlers::auth::{error_response, success_response};
use crate::auth::validation::ValidatedJson;
use crate::server::AppState;
use crate::storage::StorageError;

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
    pub permissions: Vec<String>,
}

/// Key permissions handler
///
/// This endpoint gets the permissions for a key.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The key ID
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn get_key_permissions_handler(
    state: Extension<Arc<AppState>>,
    Path(key_id): Path<String>,
) -> impl IntoResponse {
    // Get the key's permissions using KeyManager
    match state.0.key_manager.get_key(&key_id).await {
        Ok(Some(key)) => {
            if key.is_valid() {
                success_response(
                    PermissionResponse {
                        permissions: key.permissions,
                    },
                    None,
                )
            } else {
                error_response(StatusCode::NOT_FOUND, "Key is revoked", None)
            }
        }
        Ok(None) => error_response(StatusCode::NOT_FOUND, "Key not found", None),
        Err(err) => {
            error!("Failed to get key permissions: {}", err);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to get key permissions",
                None,
            )
        }
    }
}

/// Key permissions update handler
///
/// This endpoint updates the permissions for a key.
/// A key must always have at least one permission.
///
/// # Arguments
///
/// * `state` - The application state
/// * `key_id` - The key ID
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
    // Get current key
    let key_result = state.0.key_manager.get_key(&key_id).await;

    match key_result {
        Ok(Some(mut key)) => {
            if !key.is_valid() {
                return error_response(StatusCode::BAD_REQUEST, "Key is revoked", None);
            }

            // Create a copy of current permissions to validate the final state
            let mut final_permissions = key.permissions.clone();

            // Remove permissions first if specified
            if let Some(remove) = &request.remove {
                final_permissions.retain(|p| !remove.contains(p));
            }

            // Add new permissions if specified
            if let Some(add) = &request.add {
                for perm in add {
                    if !final_permissions.contains(perm) {
                        final_permissions.push(perm.clone());
                    }
                }
            }

            // Validate that we'll have at least one permission after the update
            if final_permissions.is_empty() {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "Key must have at least one permission",
                    None,
                );
            }

            // Now apply the changes to the actual key
            let mut updated = false;

            // Remove permissions
            if let Some(remove) = request.remove {
                let before_len = key.permissions.len();
                key.permissions.retain(|p| !remove.contains(p));
                if key.permissions.len() != before_len {
                    updated = true;
                }
            }

            // Add new permissions
            if let Some(add) = request.add {
                for perm in add {
                    match key.add_permission(&perm) {
                        Ok(_) => updated = true,
                        Err(e) => {
                            return error_response(
                                StatusCode::BAD_REQUEST,
                                format!("Invalid permission format: {}", e),
                                None,
                            );
                        }
                    }
                }
            }

            // Only save if there were changes
            if updated {
                match state.0.key_manager.set_key(&key_id, &key).await {
                    Ok(_) => {
                        info!("Updated permissions for key: {}", key_id);
                        success_response(
                            PermissionResponse {
                                permissions: key.permissions,
                            },
                            None,
                        )
                    }
                    Err(StorageError::ValidationError(msg)) => {
                        error!("Permission validation failed: {}", msg);
                        error_response(StatusCode::BAD_REQUEST, msg, None)
                    }
                    Err(err) => {
                        error!("Failed to update key permissions: {}", err);
                        error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "Failed to update key permissions",
                            None,
                        )
                    }
                }
            } else {
                // No changes were made
                success_response(
                    PermissionResponse {
                        permissions: key.permissions,
                    },
                    None,
                )
            }
        }
        Ok(None) => error_response(StatusCode::NOT_FOUND, "Key not found", None),
        Err(err) => {
            error!("Failed to get key: {}", err);
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "Failed to get key", None)
        }
    }
}
