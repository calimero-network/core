use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::GetTeeAdmissionPolicyApiResponse;
use tracing::info;

use super::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Getting TEE admission policy");

    match calimero_context::group_store::read_tee_admission_policy(&state.store, &group_id) {
        Ok(Some(policy)) => ApiResponse {
            payload: GetTeeAdmissionPolicyApiResponse {
                enabled: true,
                allowed_mrtd: policy.allowed_mrtd,
                allowed_rtmr0: policy.allowed_rtmr0,
                allowed_rtmr1: policy.allowed_rtmr1,
                allowed_rtmr2: policy.allowed_rtmr2,
                allowed_rtmr3: policy.allowed_rtmr3,
                allowed_tcb_statuses: policy.allowed_tcb_statuses,
                accept_mock: policy.accept_mock,
            },
        }
        .into_response(),
        Ok(None) => ApiResponse {
            payload: GetTeeAdmissionPolicyApiResponse {
                enabled: false,
                allowed_mrtd: vec![],
                allowed_rtmr0: vec![],
                allowed_rtmr1: vec![],
                allowed_rtmr2: vec![],
                allowed_rtmr3: vec![],
                allowed_tcb_statuses: vec![],
                accept_mock: false,
            },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}
