use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use std::sync::Arc;

use crate::admin::service::ApiResponse;
use crate::AdminState;
use calimero_server_primitives::registry::{
    RegistryConfig, SetupRegistryRequest, SetupRegistryResponse,
};

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(request): Json<SetupRegistryRequest>,
) -> impl IntoResponse {
    let config = RegistryConfig {
        name: request.registry_name.clone(),
        registry_type: request.registry_type,
        config: request.config,
    };

    let mut registry_manager = state.registry_manager.lock().unwrap();
    match registry_manager.setup_registry(config).await {
        Ok(_) => ApiResponse {
            payload: SetupRegistryResponse {
                success: true,
                message: format!("Registry '{}' setup successfully", request.registry_name),
            },
        }
        .into_response(),
        Err(err) => {
            tracing::error!("Failed to setup registry: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to setup registry: {}", err),
            )
                .into_response()
        }
    }
}
