use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::SetTeeAuthoringPolicyRequest;
use calimero_server_primitives::admin::{
    SetTeeAuthoringPolicyApiRequest, SetTeeAuthoringPolicyApiResponse,
};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<SetTeeAuthoringPolicyApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Setting TEE authoring policy");

    let result = state
        .ctx_client
        .set_tee_authoring_policy(SetTeeAuthoringPolicyRequest {
            group_id,
            allowed_mrtd: req.allowed_mrtd,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => ApiResponse {
            payload: SetTeeAuthoringPolicyApiResponse {},
        }
        .into_response(),
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to set TEE authoring policy");
            err.into_response()
        }
    }
}
