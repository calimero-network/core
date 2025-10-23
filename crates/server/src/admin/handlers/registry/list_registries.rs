use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use std::sync::Arc;

use crate::admin::service::ApiResponse;
use crate::AdminState;
use calimero_server_primitives::registry::{ListRegistriesResponse, RegistryInfo};

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    let registry_manager = state.registry_manager.lock().unwrap();
    let registry_names = registry_manager.list_registries().await;

    let registries: Vec<RegistryInfo> = registry_names
        .into_iter()
        .filter_map(|name| {
            registry_manager
                .get_registry_config(&name)
                .map(|config| RegistryInfo {
                    name: config.name.clone(),
                    registry_type: config.registry_type.clone(),
                    config: config.config.clone(),
                    status: "active".to_string(),
                })
        })
        .collect();

    ApiResponse {
        payload: ListRegistriesResponse { registries },
    }
    .into_response()
}
