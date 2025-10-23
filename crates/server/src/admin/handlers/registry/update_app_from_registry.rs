use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use std::sync::Arc;

use crate::admin::service::ApiResponse;
use crate::registry::client::RegistryClientFactory;
use crate::AdminState;
use calimero_server_primitives::registry::{UpdateAppRequest, UpdateAppResponse};

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(request): Json<UpdateAppRequest>,
) -> impl IntoResponse {
    // Get registry configuration
    let registry_manager = state.registry_manager.lock().unwrap();
    let registry_config = match registry_manager.get_registry_config(&request.registry_name) {
        Some(config) => config,
        None => {
            return ApiResponse {
                payload: UpdateAppResponse {
                    success: false,
                    message: format!("Registry '{}' not found", request.registry_name),
                    app_id: None,
                },
            }
            .into_response();
        }
    };

    // Create registry client
    let client = match RegistryClientFactory::create_client(registry_config) {
        Ok(client) => client,
        Err(err) => {
            tracing::error!("Failed to create registry client: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to create registry client",
            )
                .into_response();
        }
    };

    // Fetch latest app manifest
    let version = request.version.as_deref().unwrap_or("latest");
    let manifest = match client.get_app_manifest(&request.app_name, version).await {
        Ok(manifest) => manifest,
        Err(err) => {
            tracing::error!("Failed to fetch app manifest: {}", err);
            return ApiResponse {
                payload: UpdateAppResponse {
                    success: false,
                    message: format!("App manifest not found: {}", err),
                    app_id: None,
                },
            }
            .into_response();
        }
    };

    // Convert manifest to JSON value
    let manifest_json = match serde_json::to_value(manifest) {
        Ok(json) => json,
        Err(err) => {
            tracing::error!("Failed to convert manifest to JSON: {}", err);
            return ApiResponse {
                payload: UpdateAppResponse {
                    success: false,
                    message: format!("Failed to convert manifest: {}", err),
                    app_id: None,
                },
            }
            .into_response();
        }
    };

    // Update application (using install as placeholder for update)
    match state
        .node_client
        .install_application_from_manifest(manifest_json)
        .await
    {
        Ok(app_id) => ApiResponse {
            payload: UpdateAppResponse {
                success: true,
                message: format!("App '{}' updated successfully", request.app_name),
                app_id: Some(app_id.to_string()),
            },
        }
        .into_response(),
        Err(err) => {
            tracing::error!("Failed to update application: {}", err);
            ApiResponse {
                payload: UpdateAppResponse {
                    success: false,
                    message: format!("Update failed: {}", err),
                    app_id: None,
                },
            }
            .into_response()
        }
    }
}
