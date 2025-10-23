use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{UpdateApplicationFromUrlRequest, UpdateApplicationResponse};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    Path(application_id): Path<ApplicationId>,
    Json(req): Json<UpdateApplicationFromUrlRequest>,
) -> impl IntoResponse {
    match state
        .node_client
        .update_application_from_url(&application_id, req.url, req.metadata, req.hash.as_ref())
        .await
    {
        Ok(()) => ApiResponse {
            payload: UpdateApplicationResponse::new(application_id),
        }
        .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}


