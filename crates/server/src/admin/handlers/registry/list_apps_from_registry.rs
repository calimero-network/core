use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use std::sync::Arc;

use crate::admin::service::ApiResponse;
use crate::registry::client::RegistryClientFactory;
use crate::AdminState;
use calimero_server_primitives::registry::{ListAppsRequest, ListAppsResponse};

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(request): Json<ListAppsRequest>,
) -> impl IntoResponse {
    // Get registry configuration
    let registry_manager = state.registry_manager.lock().await;
    let registry_config = match registry_manager.get_registry_config(&request.registry_name) {
        Some(config) => config,
        None => {
            return ApiResponse {
                payload: ListAppsResponse { apps: vec![] },
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

    // Fetch apps from registry
    let filters = request.filters.unwrap_or_default();
    match client.get_apps(filters).await {
        Ok(apps) => ApiResponse {
            payload: ListAppsResponse { apps },
        }
        .into_response(),
        Err(err) => {
            tracing::error!("Failed to fetch apps from registry: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to fetch apps: {}", err),
            )
                .into_response()
        }
    }
}
