use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::registry::ListRegistriesResponse;
use tracing::{error, info};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    info!("Listing registries");

    // Get all configured registries
    let registry_names = state.registry_manager.list_registries().await;
    info!(count=%registry_names.len(), "Registries listed successfully");

    // Convert registry names to RegistryInfo objects
    let mut registry_infos = Vec::new();
    for name in registry_names {
        if let Some(config) = state.registry_manager.get_registry_config(&name).await {
            let registry_info = calimero_server_primitives::registry::RegistryInfo {
                name: config.name.clone(),
                registry_type: config.registry_type,
                status: "configured".to_string(),
                config: config.config.clone(),
            };
            registry_infos.push(registry_info);
        }
    }

    ApiResponse {
        payload: ListRegistriesResponse::new(registry_infos),
    }
    .into_response()
}
