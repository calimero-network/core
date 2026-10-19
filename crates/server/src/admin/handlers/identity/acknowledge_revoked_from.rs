use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_governance_store::NodeDeviceRepository;
use calimero_server_primitives::admin::{AcknowledgeRevokedFromApiResponse, Empty};
use reqwest::StatusCode;
use tracing::error;

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

/// `DELETE /identity/revoked-from`: forget the withdrawal `GET /identity`
/// reports as `revokedFrom`. Idempotent, so acknowledging twice is not an error.
pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    let store = state.ctx_client.datastore();

    if let Err(err) = NodeDeviceRepository::new(store).acknowledge_revoked_from() {
        error!(error = ?err, "Failed to clear what withdrew this node's device");
        return ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to clear what withdrew this node's device".to_owned(),
        }
        .into_response();
    }

    ApiResponse {
        payload: AcknowledgeRevokedFromApiResponse { data: Empty },
    }
    .into_response()
}
