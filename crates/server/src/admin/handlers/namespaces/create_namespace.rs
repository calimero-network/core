use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::CreateGroupRequest;
use calimero_server_primitives::admin::{
    CreateNamespaceApiRequest, CreateNamespaceApiResponse, CreateNamespaceApiResponseData,
};
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<CreateNamespaceApiRequest>,
) -> impl IntoResponse {
    info!(application_id=%req.application_id, "Creating namespace");

    let result = state
        .ctx_client
        .create_group(CreateGroupRequest {
            parent_group_id: None,
            group_id: None,
            app_key: None,
            application_id: req.application_id,
            upgrade_policy: req.upgrade_policy,
            alias: req.alias,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(response) => ApiResponse {
            payload: CreateNamespaceApiResponse {
                data: CreateNamespaceApiResponseData {
                    namespace_id: hex::encode(response.group_id.to_bytes()),
                },
            },
        }
        .into_response(),
        Err(err) => {
            error!(error=?err, "Failed to create namespace");
            err.into_response()
        }
    }
}
