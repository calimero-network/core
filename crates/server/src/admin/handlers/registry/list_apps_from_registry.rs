use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_server_primitives::registry::{
    ListAppsFromRegistryRequest, ListAppsFromRegistryResponse,
};
use tracing::{error, info};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<ListAppsFromRegistryRequest>,
) -> impl IntoResponse {
    info!(registry_name=%req.registry_name, "Listing apps from registry");

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

    // Convert filters to the expected format
    let filters = req.filters.map(|f| crate::registry::client::AppFilters {
        developer: f.developer,
        name: f.name,
    });

    // Fetch apps from registry with filters
    let apps = match registry_client.get_apps(filters).await {
        Ok(apps) => apps,
        Err(err) => {
            error!(registry_name=%req.registry_name, error=?err, "Failed to fetch apps from registry");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to fetch apps: {}", err),
            )
                .into_response();
        }
    };

    info!(registry_name=%req.registry_name, count=%apps.len(), "Apps listed from registry successfully");
    ApiResponse {
        payload: ListAppsFromRegistryResponse::new(apps),
    }
    .into_response()
}
