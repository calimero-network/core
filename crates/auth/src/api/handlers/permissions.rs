use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};
use validator::Validate;

use crate::api::handlers::auth::{error_response, success_response};
use crate::auth::middleware::CallerPermissions;
use crate::auth::permissions::{Permission, PermissionValidator};
use crate::auth::validation::{sanitize_string, ValidatedJson};
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

/// Reason a requested permission grant was denied.
#[derive(Debug, PartialEq, Eq)]
enum GrantDenial {
    /// The permission string could not be parsed into a known permission.
    InvalidFormat(String),
    /// The caller does not hold a permission that satisfies the requested one.
    Escalation(String),
}

/// Ensure the caller is only granting permissions they themselves hold.
///
/// For every permission in `add`, the caller must hold a permission that
/// satisfies it (`admin` satisfies everything; scoped permissions follow the
/// usual hierarchy). Permissions that fail to parse are rejected so we never
/// grant something we cannot reason about.
fn validate_grantable_permissions(
    caller_permissions: &[String],
    add: &[String],
) -> Result<(), GrantDenial> {
    let validator = PermissionValidator::new();

    for perm in add {
        let parsed = perm
            .parse::<Permission>()
            .map_err(GrantDenial::InvalidFormat)?;

        if !validator.validate_permissions(caller_permissions, std::slice::from_ref(&parsed)) {
            return Err(GrantDenial::Escalation(perm.clone()));
        }
    }

    Ok(())
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
    Extension(caller_permissions): Extension<CallerPermissions>,
    Path(key_id): Path<String>,
    ValidatedJson(mut request): ValidatedJson<UpdateKeyPermissionsRequest>,
) -> impl IntoResponse {
    // Sanitize permission strings to prevent injection attacks
    if let Some(ref mut add) = request.add {
        *add = add.iter().map(|p| sanitize_string(p)).collect();
        add.retain(|p| !p.is_empty()); // Remove empty permissions after sanitization
    }

    if let Some(ref mut remove) = request.remove {
        *remove = remove.iter().map(|p| sanitize_string(p)).collect();
        remove.retain(|p| !p.is_empty()); // Remove empty permissions after sanitization
    }

    // Prevent privilege escalation: a caller may only grant permissions that
    // they themselves hold. Without this check, any key with
    // `keys:update_permissions` could add arbitrary permissions (including
    // `admin`) to a key and escalate its own privileges.
    //
    // Only additions are restricted; removing permissions is always allowed.
    if let Some(add) = &request.add {
        match validate_grantable_permissions(&caller_permissions.0, add) {
            Ok(()) => {}
            Err(GrantDenial::InvalidFormat(e)) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!("Invalid permission format: {e}"),
                    None,
                );
            }
            Err(GrantDenial::Escalation(perm)) => {
                warn!(
                    "Privilege escalation blocked: caller attempted to grant '{}' to key '{}' without holding it",
                    perm, key_id
                );
                return error_response(
                    StatusCode::FORBIDDEN,
                    "Cannot grant permissions that exceed your own",
                    None,
                );
            }
        }
    }

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
                                format!("Invalid permission format: {e}"),
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

#[cfg(test)]
mod tests {
    use super::{validate_grantable_permissions, GrantDenial};

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn non_admin_cannot_grant_admin() {
        // A key that can only update permissions must not be able to grant
        // `admin` (the core privilege-escalation vector).
        let caller = strings(&["keys:permissions:update"]);
        let result = validate_grantable_permissions(&caller, &strings(&["admin"]));
        assert_eq!(result, Err(GrantDenial::Escalation("admin".to_string())));
    }

    #[test]
    fn caller_cannot_grant_permission_it_does_not_hold() {
        // Holding the update-permissions capability does not let the caller
        // hand out unrelated permissions such as creating keys.
        let caller = strings(&["keys:permissions:update", "keys:list"]);
        let result = validate_grantable_permissions(&caller, &strings(&["keys:create"]));
        assert_eq!(
            result,
            Err(GrantDenial::Escalation("keys:create".to_string()))
        );
    }

    #[test]
    fn admin_can_grant_anything() {
        let caller = strings(&["admin"]);
        assert_eq!(
            validate_grantable_permissions(
                &caller,
                &strings(&["admin", "keys:create", "context:execute"])
            ),
            Ok(())
        );
    }

    #[test]
    fn caller_can_grant_permission_it_holds() {
        let caller = strings(&["keys:permissions:update", "keys:create", "keys:list"]);
        assert_eq!(
            validate_grantable_permissions(&caller, &strings(&["keys:create", "keys:list"])),
            Ok(())
        );
    }

    #[test]
    fn caller_can_grant_narrower_scope_than_it_holds() {
        // A caller holding a global-scoped permission may grant the same
        // permission scoped to a specific resource (a subset of its own).
        let caller = strings(&["context:list"]);
        assert_eq!(
            validate_grantable_permissions(&caller, &strings(&["context:list[ctx-1]"])),
            Ok(())
        );
    }

    #[test]
    fn caller_cannot_widen_scope_beyond_what_it_holds() {
        // The reverse is not allowed: a caller scoped to one resource cannot
        // grant a global-scoped permission.
        let caller = strings(&["context:list[ctx-1]"]);
        let result = validate_grantable_permissions(&caller, &strings(&["context:list"]));
        assert_eq!(
            result,
            Err(GrantDenial::Escalation("context:list".to_string()))
        );
    }

    #[test]
    fn unparseable_permission_is_rejected() {
        let caller = strings(&["admin"]);
        let result = validate_grantable_permissions(&caller, &strings(&["not-a-real-permission"]));
        assert!(matches!(result, Err(GrantDenial::InvalidFormat(_))));
    }

    #[test]
    fn empty_add_list_is_allowed() {
        let caller = strings(&["keys:permissions:update"]);
        assert_eq!(validate_grantable_permissions(&caller, &[]), Ok(()));
    }
}
