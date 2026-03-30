use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_primitives::group::SetTeeAdmissionPolicyRequest;
use calimero_server_primitives::admin::SetTeeAdmissionPolicyApiRequest;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse, Empty};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<SetTeeAdmissionPolicyApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, max_replicas=req.max_replicas, "Setting TEE admission policy");

    let result = state
        .ctx_client
        .set_tee_admission_policy(SetTeeAdmissionPolicyRequest {
            group_id,
            allowed_mrtd: req.allowed_mrtd,
            allowed_rtmr0: req.allowed_rtmr0,
            allowed_rtmr1: req.allowed_rtmr1,
            allowed_rtmr2: req.allowed_rtmr2,
            allowed_rtmr3: req.allowed_rtmr3,
            allowed_tcb_statuses: req.allowed_tcb_statuses,
            accept_mock: req.accept_mock,
            max_replicas: req.max_replicas,
            requester: auth_key.map(|Extension(k)| k.0).or(req.requester),
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, "TEE admission policy updated");
            ApiResponse { payload: Empty }.into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to set TEE admission policy");
            err.into_response()
        }
    }
}
