use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_governance_store::TeeAdmissionPolicyRead;
use calimero_server_primitives::admin::GetTeeAdmissionPolicyApiResponse;
use tracing::info;

use super::parse_group_id;
use axum::http::StatusCode;

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
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

    match calimero_governance_store::read_tee_admission_policy(&state.store, &group_id) {
        Ok(TeeAdmissionPolicyRead::Set(policy)) => ApiResponse {
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
        Ok(TeeAdmissionPolicyRead::NotSet) => ApiResponse {
            payload: GetTeeAdmissionPolicyApiResponse::disabled(),
        }
        .into_response(),
        // NOT `disabled()`. Reporting an unreadable log as "TEE admission is
        // off" is the same lie the caller would have been told before, in the
        // one place an operator goes to check whether a policy exists.
        Ok(TeeAdmissionPolicyRead::Unreadable { undecodable }) => {
            let detail = undecodable
                .iter()
                .map(|e| format!("seq {}: {}", e.sequence, e.error))
                .collect::<Vec<_>>()
                .join("; ");
            // Built directly rather than through `parse_api_error`, which
            // scrubs an unrecognised error down to "Internal server error".
            // The detail IS the answer here: an operator hitting this endpoint
            // is asking whether a policy exists, and "the log will not decode,
            // at these sequence numbers" is what they need to act on.
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!(
                    "the TEE admission policy could not be read: {} op-log entr{} do not \
                     decode ({detail}). This is not the same as no policy being set.",
                    undecodable.len(),
                    if undecodable.len() == 1 { "y" } else { "ies" },
                ),
            }
            .into_response()
        }
        Err(err) => parse_api_error(err).into_response(),
    }
}
