use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_config::types::AppKey;
use calimero_context_primitives::group::CreateGroupRequest;
use calimero_server_primitives::admin::{
    CreateGroupApiRequest, CreateGroupApiResponse, CreateGroupApiResponseData,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<CreateGroupApiRequest>,
) -> impl IntoResponse {
    let app_key_bytes: [u8; 32] = match hex::decode(&req.app_key)
        .map_err(|_| ())
        .and_then(|v| v.try_into().map_err(|_| ()))
    {
        Ok(bytes) => bytes,
        Err(()) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid app_key: expected hex-encoded 32 bytes".into(),
            }
            .into_response();
        }
    };

    info!(application_id=%req.application_id, "Creating group");

    let result = state
        .ctx_client
        .create_group(CreateGroupRequest {
            group_id: None,
            app_key: AppKey::from(app_key_bytes),
            application_id: req.application_id,
            upgrade_policy: req.upgrade_policy,
            admin_identity: req.admin_identity,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(response) => {
            let group_id_hex = hex::encode(response.group_id.to_bytes());
            info!(group_id=%group_id_hex, "Group created successfully");
            ApiResponse {
                payload: CreateGroupApiResponse {
                    data: CreateGroupApiResponseData {
                        group_id: group_id_hex,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to create group");
            err.into_response()
        }
    }
}
