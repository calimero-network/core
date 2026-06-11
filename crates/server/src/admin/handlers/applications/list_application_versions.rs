use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{ApplicationVersionEntry, ListApplicationVersionsResponse};
use tracing::{error, info};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Path(application_id): Path<ApplicationId>,
) -> impl IntoResponse {
    info!(application_id=%application_id, "Listing application versions");

    match state
        .node_client
        .list_application_versions(&application_id)
        .await
    {
        Ok(versions) => ApiResponse {
            payload: ListApplicationVersionsResponse {
                data: versions
                    .into_iter()
                    .map(|v| ApplicationVersionEntry {
                        version: v.version,
                        blob_id: v.blob_id.to_string(),
                        size: v.size,
                        package: v.package,
                    })
                    .collect(),
            },
        }
        .into_response(),
        Err(err) => {
            error!(application_id=%application_id, error=?err, "Failed to list application versions");
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}
