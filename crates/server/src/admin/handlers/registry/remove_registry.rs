use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::registry::{RemoveRegistryRequest, RemoveRegistryResponse};
use tracing::{error, info};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<RemoveRegistryRequest>,
) -> impl IntoResponse {
    info!(name=%req.name, "Removing registry");

    // TODO: Implement registry removal logic
    // This would:
    // 1. Check if registry is in use
    // 2. Remove registry configuration
    // 3. Clean up any associated resources

    match state.registry_manager.remove_registry(&req.name).await {
        Ok(()) => {
            info!(name=%req.name, "Registry removed successfully");
            ApiResponse {
                payload: RemoveRegistryResponse::new(req.name),
            }
            .into_response()
        }
        Err(err) => {
            error!(name=%req.name, error=?err, "Failed to remove registry");
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}
