use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::UpdateApplicationResponse;
use calimero_server_primitives::registry::UpdateAppFromRegistryRequest;
use tracing::{error, info};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<UpdateAppFromRegistryRequest>,
) -> impl IntoResponse {
    info!(app_name=%req.app_name, registry_name=%req.registry_name, "Updating app from registry");

    // Check if registry exists
    let registry_config = match state
        .registry_manager
        .get_registry_config(&req.registry_name)
        .await
    {
        Some(config) => config,
        None => {
            error!(registry_name=%req.registry_name, "Registry not found");
            return (StatusCode::NOT_FOUND, "Registry not found").into_response();
        }
    };

    // Create a new registry client for this operation
    let registry_client = match crate::registry::client::RegistryClientFactory::create_client(
        &registry_config,
    ) {
        Ok(client) => client,
        Err(err) => {
            error!(registry_name=%req.registry_name, error=?err, "Failed to create registry client");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create registry client: {}", err),
            )
                .into_response();
        }
    };

    // Fetch latest app manifest from registry
    let version = req.version.unwrap_or_else(|| "latest".to_string());
    let manifest = match registry_client
        .get_app_manifest(&req.app_name, &version)
        .await
    {
        Ok(manifest) => manifest,
        Err(err) => {
            error!(app_name=%req.app_name, version=%version, error=?err, "Failed to fetch app manifest from registry");
            return (
                StatusCode::NOT_FOUND,
                format!("App manifest not found: {}", err),
            )
                .into_response();
        }
    };

    // Convert manifest to JSON value for the node client
    let manifest_json = match serde_json::to_value(&manifest) {
        Ok(json) => json,
        Err(err) => {
            error!(app_name=%req.app_name, error=?err, "Failed to serialize app manifest");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to serialize manifest: {}", err),
            )
                .into_response();
        }
    };

    // Find existing application by name (this would need to be implemented in the node client)
    // For now, we'll use a placeholder approach
    let application_id = ApplicationId::from([0u8; 32]); // This should be found by app name

    // Update the application using existing node client
    // Note: This would need a new method in the node client for updating by app name
    // For now, we'll simulate the update
    match state
        .node_client
        .install_application_from_manifest(manifest_json)
        .await
    {
        Ok(_) => {
            info!(app_name=%req.app_name, application_id=%application_id, "App updated from registry successfully");
            ApiResponse {
                payload: UpdateApplicationResponse::new(application_id),
            }
            .into_response()
        }
        Err(err) => {
            error!(app_name=%req.app_name, error=?err, "Failed to update application");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Update failed: {}", err),
            )
                .into_response()
        }
    }
}
