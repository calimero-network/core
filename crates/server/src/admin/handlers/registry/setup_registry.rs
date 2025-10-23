use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::registry::{SetupRegistryRequest, SetupRegistryResponse};
use tracing::{error, info};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<SetupRegistryRequest>,
) -> impl IntoResponse {
    info!(name=%req.name, "Setting up registry");

    // TODO: Implement registry setup logic
    // This would:
    // 1. Validate the registry configuration
    // 2. Test connection to the registry
    // 3. Store the configuration
    // 4. Initialize the registry client

    // TODO: Convert SetupRegistryRequest to RegistryConfig
    // For now, create a placeholder config
    let config = calimero_server_primitives::registry::RegistryConfig {
        name: req.name.clone(),
        registry_type: req.registry_type,
        config: req.config,
    };

    match state.registry_manager.setup_registry(config).await {
        Ok(()) => {
            info!(name=%req.name, "Registry setup successfully");
            ApiResponse {
                payload: SetupRegistryResponse::new(req.name),
            }
            .into_response()
        }
        Err(err) => {
            error!(name=%req.name, error=?err, "Failed to setup registry");
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}
