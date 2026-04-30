use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::SetSubgroupVisibilityRequest;
use calimero_context_config::VisibilityMode;
use calimero_server_primitives::admin::{
    SetSubgroupVisibilityApiRequest, SetSubgroupVisibilityApiResponse,
};
use reqwest::StatusCode;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<SetSubgroupVisibilityApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let mode = match req.subgroup_visibility.as_str() {
        "open" => VisibilityMode::Open,
        "restricted" => VisibilityMode::Restricted,
        _ => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "subgroup_visibility must be 'open' or 'restricted'".into(),
            }
            .into_response()
        }
    };

    info!(group_id=%group_id_str, ?mode, "Setting subgroup visibility");

    let result = state
        .ctx_client
        .set_subgroup_visibility(SetSubgroupVisibilityRequest {
            group_id,
            subgroup_visibility: mode,
            requester: auth_key.map(|Extension(k)| k.0).or(req.requester),
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, "Subgroup visibility updated");
            ApiResponse {
                payload: SetSubgroupVisibilityApiResponse {},
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to set subgroup visibility");
            err.into_response()
        }
    }
}
