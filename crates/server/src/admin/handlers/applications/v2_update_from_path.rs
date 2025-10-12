use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{
    UpdateApplicationFromSourceRequest, UpdateApplicationResponse,
};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Path(application_id): Path<ApplicationId>,
    Json(req): Json<UpdateApplicationFromSourceRequest>,
) -> impl IntoResponse {
    let result = if let Some(path) = req.path {
        state
            .node_client
            .update_application_from_path(&application_id, path, req.metadata)
            .await
    } else if let Some(url) = req.url {
        state
            .node_client
            .update_application_from_url(&application_id, url, req.metadata, req.hash.as_ref())
            .await
    } else {
        Err(eyre::eyre!("no source provided"))
    };

    match result {
        Ok(()) => ApiResponse {
            payload: UpdateApplicationResponse::new(application_id),
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}
