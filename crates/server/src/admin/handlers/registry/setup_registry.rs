use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::registry::{
    RegistryConfig, SetupRegistryRequest, SetupRegistryResponse,
};
use tracing::{error, info};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<SetupRegistryRequest>,
) -> impl IntoResponse {
    info!(name=%req.name, "Setting up registry");

    let config = RegistryConfig {
        name: req.name.clone(),
        registry_type: req.registry_type,
        config: req.config,
    };

    let mut manager = state.registry_manager.lock().await;
    let setup_result = manager.setup_registry(config).await;
    drop(manager); // Explicitly drop the lock

    match setup_result {
        Ok(_) => {
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
