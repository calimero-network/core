use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use std::sync::Arc;

use crate::admin::service::ApiResponse;
use crate::AdminState;
use calimero_server_primitives::registry::RemoveRegistryResponse;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mut registry_manager = state.registry_manager.lock().unwrap();
    match registry_manager.remove_registry(&name).await {
        Ok(_) => ApiResponse {
            payload: RemoveRegistryResponse::new(name.clone()),
        }
        .into_response(),
        Err(err) => {
            tracing::error!("Failed to remove registry: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to remove registry: {}", err),
            )
                .into_response()
        }
    }
}
