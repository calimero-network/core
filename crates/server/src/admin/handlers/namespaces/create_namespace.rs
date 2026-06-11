use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::CreateGroupRequest;
use calimero_context_config::types::AppKey;
use calimero_server_primitives::admin::{
    CreateNamespaceApiRequest, CreateNamespaceApiResponse, CreateNamespaceApiResponseData,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<CreateNamespaceApiRequest>,
) -> impl IntoResponse {
    // Optional version pin: hex blob id of an installed version. Existence
    // and package-match are verified by the create-group handler.
    let app_key = match &req.app_key {
        Some(hex_str) => {
            let bytes: [u8; 32] = match hex::decode(hex_str)
                .map_err(|_| ())
                .and_then(|v| v.try_into().map_err(|_| ()))
            {
                Ok(b) => b,
                Err(()) => {
                    return ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Invalid appKey: expected hex-encoded 32 bytes".into(),
                    }
                    .into_response();
                }
            };
            Some(AppKey::from(bytes))
        }
        None => None,
    };

    info!(application_id=%req.application_id, has_app_key=app_key.is_some(), "Creating namespace");

    let result = state
        .ctx_client
        .create_group(CreateGroupRequest {
            parent_group_id: None,
            group_id: None,
            app_key,
            application_id: req.application_id,
            upgrade_policy: req.upgrade_policy,
            name: req.name,
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
