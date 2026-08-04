use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use tracing::error;
use uuid::Uuid;
use validator::Validate;

use super::auth::{error_response, success_response};
use crate::api::handlers::auth::TokenResponse;
use crate::auth::validation::{escape_html, sanitize_identifier, ValidatedJson};
use crate::server::AppState;
use crate::storage::models::{Key, KeyType};

/// Client key generation request
#[derive(Debug, Deserialize, Validate)]
pub struct GenerateClientKeyRequest {
    /// Context ID selected by user
    pub context_id: Option<String>,

    /// Context identity selected by user
    pub context_identity: Option<String>,

    /// Additional permissions requested
    pub permissions: Option<Vec<String>>,

    /// Target node URL for which to generate the client key
    pub target_node_url: Option<String>,
}

/// Client list handler
///
/// This endpoint lists all client keys.
///
/// # Arguments
///
/// * `state` - The application state
///
/// # Returns
///
/// * `impl IntoResponse` - The response
pub async fn list_clients_handler(state: Extension<Arc<AppState>>) -> impl IntoResponse {
    match state.0.key_manager.list_keys(KeyType::Client).await {
        Ok(client_keys) => {
            let clients = client_keys
                .into_iter()
                .map(|(key_id, key)| {
                    serde_json::json!({
                        "client_id": key_id,
                        "root_key_id": key.root_key_id.clone().unwrap_or_default(),
                        "name": key.name.clone().unwrap_or_default(),
                        "permissions": key.permissions,
                        "created_at": key.metadata.created_at,
                        "revoked_at": key.metadata.revoked_at,
                        "is_valid": key.is_valid()
                    })
                })
                .collect::<Vec<_>>();

            success_response(clients, None)
        }
        Err(err) => {
            error!("Failed to list client keys: {}", err);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to list client keys",
                None,
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
    let auth_response = match state
        .0
        .token_generator
        .verify_token_from_headers(&headers)
        .await
    {
        Ok(response) => response,
        Err(err) => {
            error!("Failed to verify token: {}", err);
            return error_response(StatusCode::UNAUTHORIZED, "Invalid token", None);
        }
    };

    if !auth_response.is_valid {
        return error_response(StatusCode::UNAUTHORIZED, "Invalid token", None);
    }
    if !auth_response.permissions.contains(&"admin".to_string()) {
        return error_response(
            StatusCode::FORBIDDEN,
            "Token does not have admin permissions",
            None,
        );
    }

    let root_key_id = auth_response.key_id;

    // Extract node URL from request for node-specific token generation
    let node_url = request.target_node_url.clone();

    // Sanitize identifiers to prevent injection attacks
    let context_id = match request.context_id {
        Some(id) => sanitize_identifier(&id),
        None => "".to_string(),
    };
    let context_identity = match request.context_identity {
        Some(id) => sanitize_identifier(&id),
        None => "".to_string(),
    };

    // Get and validate root key
    let root_key = match state.0.key_manager.get_key(&root_key_id).await {
        Ok(Some(key)) if !key.is_valid() => {
            return error_response(StatusCode::UNAUTHORIZED, "Root key is revoked", None);
        }
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, "Root key not found", None);
        }
        Err(err) => {
            error!("Failed to get root key: {}", err);
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to get root key",
                None,
            );
        }
        Ok(Some(key)) => key,
    };

    let client_id = Uuid::new_v4().to_string();

    // Build permissions list starting with required context permission
    // Only add context permission if context_id and context_identity are not empty
    let mut all_permissions = Vec::new();

    if !context_id.is_empty() && !context_identity.is_empty() {
        let default_permission = format!("context[{context_id},{context_identity}]");
        all_permissions.push(default_permission);
    }

    // Add and validate additional permissions
    if let Some(additional_perms) = request.permissions {
        for perm in additional_perms {
            // Validate each permission against root key
            if !root_key.has_permission(&perm) {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!("Root key does not have permission: {}", escape_html(&perm)),
                    None,
                );
            }
            if !all_permissions.contains(&perm) {
                all_permissions.push(perm);
            }
        }
    }

    let name = format!("Context Client - {context_id} ({context_identity})");

    let client_key =
        Key::new_client_key(root_key_id.clone(), name, all_permissions, node_url.clone());

    if let Err(err) = state.0.key_manager.set_key(&client_id, &client_key).await {
        error!("Failed to store client key: {}", err);
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to store client key",
            None,
        );
    }

    match state
        .0
        .token_generator
        .generate_token_pair(client_id.clone(), client_key.permissions, node_url)
        .await
    {
        Ok((access_token, refresh_token)) => {
            let response = TokenResponse::new(access_token, refresh_token);
            success_response(response, None)
        }
        Err(err) => {
            error!("Failed to generate client tokens: {}", err);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to generate client tokens",
                None,
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
    match state.0.key_manager.get_key(&client_id).await {
        Ok(Some(mut client_key)) => {
            if client_key.root_key_id.as_deref() != Some(&key_id) {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "Client key does not belong to specified root key",
                    None,
                );
            }

            // Revoke the key instead of deleting it
            client_key.revoke();

            // Store the updated key
            if let Err(err) = state.0.key_manager.set_key(&client_id, &client_key).await {
                error!("Failed to revoke client key: {}", err);
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to revoke client key",
                    None,
                );
            }

            success_response(
                serde_json::json!({
                    "message": "Client key revoked successfully",
                    "revoked_at": client_key.metadata.revoked_at
                }),
                None,
            )
        }
        Ok(None) => error_response(StatusCode::NOT_FOUND, "Client key not found", None),
        Err(err) => {
            error!("Failed to get client key: {}", err);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to get client key",
                None,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{header, HeaderValue};

    use super::*;
    use crate::auth::rate_limit::LoginRateLimiter;
    use crate::auth::token::TokenManager;
    use crate::embedded::default_config;
    use crate::secrets::SecretManager;
    use crate::storage::{KeyManager, MemoryStorage, Storage};
    use crate::utils::AuthMetrics;
    use crate::AuthService;

    async fn admin_state() -> (Arc<AppState>, HeaderMap) {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let secret_manager = Arc::new(SecretManager::new(Arc::clone(&storage)));
        secret_manager.initialize().await.unwrap();

        let config = default_config();
        let token_manager =
            TokenManager::new(config.jwt.clone(), Arc::clone(&storage), secret_manager);
        let key_manager = KeyManager::new(Arc::clone(&storage));

        let root = Key::new_root_key_with_permissions(
            "test-public-key".to_string(),
            "user_password".to_string(),
            vec!["admin".to_string()],
            None,
        );
        let _ = key_manager.set_key("root-1", &root).await.unwrap();

        let (access, _refresh) = token_manager
            .generate_token_pair("root-1".to_string(), vec!["admin".to_string()], None)
            .await
            .unwrap();

        let mut headers = HeaderMap::new();
        let _ = headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {access}")).unwrap(),
        );

        let state = Arc::new(AppState {
            auth_service: AuthService::new(vec![], token_manager.clone()),
            storage,
            key_manager,
            token_generator: token_manager,
            config,
            metrics: AuthMetrics::new(),
            login_rate_limiter: Arc::new(LoginRateLimiter::default()),
        });

        (state, headers)
    }

    #[tokio::test]
    async fn back_to_back_mints_do_not_overwrite_each_other() {
        let (state, headers) = admin_state().await;

        for _ in 0..2 {
            let response = generate_client_key_handler(
                Extension(Arc::clone(&state)),
                headers.clone(),
                ValidatedJson(GenerateClientKeyRequest {
                    context_id: None,
                    context_identity: None,
                    permissions: Some(vec!["admin".to_string()]),
                    target_node_url: None,
                }),
            )
            .await
            .into_response();

            assert_eq!(response.status(), StatusCode::OK);
        }

        let clients = state.key_manager.list_keys(KeyType::Client).await.unwrap();
        assert_eq!(
            clients.len(),
            2,
            "two admin-scoped mints in the same second must yield two distinct keys, got {clients:?}"
        );

        for (client_id, _) in &clients {
            assert!(
                state
                    .key_manager
                    .get_key(client_id)
                    .await
                    .unwrap()
                    .is_some(),
                "minted key {client_id} must be retrievable"
            );
        }
    }
}
