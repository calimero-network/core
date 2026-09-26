use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::{SetTeeAdmissionPolicyRequest, SignedReleaseTrust};
use calimero_server_primitives::admin::{
    SetTeeAdmissionPolicyApiRequest, SetTeeAdmissionPolicyApiResponse,
};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<SetTeeAdmissionPolicyApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Setting TEE admission policy");

    // Checked here so a malformed version is a 400 on this request; the
    // context manager's copy of the check would surface as a 500.
    let signed_release = match req.signed_release.map(|s| {
        let min_release_version = s
            .min_release_version
            .map(|v| {
                calimero_tee_release::normalize_release_version(
                    &v,
                    calimero_tee_release::NODE_RELEASE_TAG_PREFIX,
                )
            })
            .transpose()?;
        Ok::<_, eyre::Report>(SignedReleaseTrust {
            allowed_profiles: s.allowed_profiles,
            min_release_version,
        })
    }) {
        None => None,
        Some(Ok(trust)) => Some(trust),
        Some(Err(err)) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: format!("signedRelease.minReleaseVersion: {err}"),
            }
            .into_response()
        }
    };

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
            signed_release,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, "TEE admission policy updated");
            ApiResponse {
                payload: SetTeeAdmissionPolicyApiResponse {},
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to set TEE admission policy");
            err.into_response()
        }
    }
}
