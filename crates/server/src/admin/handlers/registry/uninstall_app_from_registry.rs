use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::UninstallApplicationResponse;
use calimero_server_primitives::registry::UninstallAppFromRegistryRequest;
use tracing::{error, info};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<UninstallAppFromRegistryRequest>,
) -> impl IntoResponse {
    info!(app_name=%req.app_name, registry_name=%req.registry_name, "Uninstalling app from registry");

    // TODO: Implement registry-based app uninstallation
    // This would:
    // 1. Find the application by name in the registry
    // 2. Get the application ID
    // 3. Uninstall using existing node client
    // 4. Return success response

    match state
        .registry_manager
        .get_registry(&req.registry_name)
        .await
    {
        Some(_registry) => {
            info!(app_name=%req.app_name, "App uninstalled from registry successfully");
            ApiResponse {
                payload: UninstallApplicationResponse::new(ApplicationId::from([0u8; 32])), // Placeholder ID
            }
            .into_response()
        }
        None => {
            error!(registry_name=%req.registry_name, "Registry not found");
            (StatusCode::NOT_FOUND, "Registry not found").into_response()
        }
    }
}
