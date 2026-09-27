use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{TeeAttestResponse, TeeRegistrationAttestRequest};
use calimero_tee_attestation::{attest_registration_binding, build_report_data};
use reqwest::StatusCode;
use tracing::{error, info};

use super::attest::generate;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

/// A quote over `nonce || attest_registration_binding()`.
///
/// The public `/attest` quotes over any nonce, so a quote from it cannot show
/// that a registration came from the node itself. This route is mounted on the
/// protected router, and its second half is a domain `/attest` never produces,
/// so a verifier that requires that binding knows the quote was asked for by
/// whoever holds this node's admin access -- on a fleet node, the node itself.
pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<TeeRegistrationAttestRequest>,
) -> impl IntoResponse {
    let nonce: [u8; 32] = match hex::decode(&req.nonce)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
    {
        Some(nonce) => nonce,
        None => {
            error!("Invalid registration nonce");
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Nonce must be exactly 32 bytes (64 hex characters)".to_owned(),
            }
            .into_response();
        }
    };

    let report_data = build_report_data(&nonce, Some(&attest_registration_binding()));
    let result = match generate(&state, report_data) {
        Ok(result) => result,
        Err(err) => return err.into_response(),
    };

    info!("TEE registration attestation generated");
    ApiResponse {
        payload: TeeAttestResponse::new(result.quote_b64, result.quote, None, None, None),
    }
    .into_response()
}
