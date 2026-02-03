use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{TeeAttestRequest, TeeAttestResponse};
use calimero_tee_attestation::{build_report_data, generate_attestation, AttestationError};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<TeeAttestRequest>,
) -> impl IntoResponse {
    info!(nonce=%req.nonce, application_id=?req.application_id, "Generating TEE attestation");

    // Nonce format is pre-validated by ValidatedJson, so we can decode safely
    let nonce = hex::decode(&req.nonce).expect("pre-validated hex string");
    let nonce_array: [u8; 32] = nonce.try_into().expect("pre-validated length");

    // 2. Get application bytecode hash (if requested)
    let app_hash = if let Some(application_id) = req.application_id {
        match state.node_client.get_application(&application_id) {
            Ok(Some(application)) => {
                // Use the bytecode BlobId (which is already a hash) directly
                // BlobId derefs to &[u8; 32]
                Some(*application.blob.bytecode)
            }
            Ok(None) => {
                error!(application_id=%application_id, "Application not found");
                return ApiError {
                    status_code: StatusCode::NOT_FOUND,
                    message: format!("Application '{}' not found", application_id),
                }
                .into_response();
            }
            Err(err) => {
                error!(application_id=%application_id, error=?err, "Failed to get application");
                return ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: format!("Failed to get application: {}", err),
                }
                .into_response();
            }
        }
    } else {
        None
    };

    // 3. Build report_data using the tee-attestation crate
    let report_data = build_report_data(&nonce_array, app_hash.as_ref());

    // 4. Generate attestation using the tee-attestation crate
    match generate_attestation(report_data) {
        Ok(result) => {
            // Reject mock attestations - they indicate unsupported platform
            if result.is_mock {
                error!("Mock attestation generated - platform does not support TDX");
                return ApiError {
                    status_code: StatusCode::NOT_IMPLEMENTED,
                    message:
                        "TDX attestation generation is only supported on Linux with TDX hardware"
                            .to_owned(),
                }
                .into_response();
            }

            info!("TEE attestation generated successfully");
            ApiResponse {
                payload: TeeAttestResponse::new(result.quote_b64, result.quote),
            }
            .into_response()
        }
        Err(err) => {
            let (status_code, message) = match &err {
                AttestationError::NotSupported => (
                    StatusCode::NOT_IMPLEMENTED,
                    "TDX attestation generation is only supported on Linux with TDX hardware"
                        .to_owned(),
                ),
                _ => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
            };
            error!(error=%err, "Failed to generate attestation");
            ApiError {
                status_code,
                message,
            }
            .into_response()
        }
    }
}
