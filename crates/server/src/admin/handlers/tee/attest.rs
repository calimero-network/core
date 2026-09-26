use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{TeeAttestRequest, TeeAttestResponse};
#[cfg(feature = "mock-attestation")]
use calimero_tee_attestation::generate_mock_attestation;
use calimero_tee_attestation::{
    attest_key_binding, attest_transport_binding, build_report_data, generate_attestation,
    AttestationError,
};
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

    // Defense-in-depth: ValidatedJson already validates format, but we keep defensive
    // error handling here in case validation is bypassed or has bugs. This prevents
    // panics and provides clear error messages.
    let nonce = match hex::decode(&req.nonce) {
        Ok(n) => n,
        Err(_) => {
            error!("Invalid nonce format");
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid nonce format (must be hex string)".to_owned(),
            }
            .into_response();
        }
    };

    let nonce_array: [u8; 32] = match nonce.try_into() {
        Ok(arr) => arr,
        Err(_) => {
            error!(nonce_len=%req.nonce.len() / 2, "Invalid nonce length");
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Nonce must be exactly 32 bytes (64 hex characters)".to_owned(),
            }
            .into_response();
        }
    };

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
                    message: format!("Application '{application_id}' not found"),
                }
                .into_response();
            }
            Err(err) => {
                error!(application_id=%application_id, error=?err, "Failed to get application");
                return ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: format!("Failed to get application: {err}"),
                }
                .into_response();
            }
        }
    } else {
        None
    };

    // 3. Build report_data using the tee-attestation crate. With `bindNodeKey`
    // the second half also commits to this node's signing key, so a client can
    // tie the quote to the node it is talking to rather than to some TEE a relay
    // forwarded the call to. The node signs with one identity for every
    // namespace, so that is the key bound.
    let bound_public_key = if req.bind_node_key {
        match calimero_governance_store::NamespaceRepository::new(&state.store).node_identity() {
            Ok(Some(identity)) => Some(identity.public_key),
            Ok(None) => {
                return ApiError {
                    status_code: StatusCode::CONFLICT,
                    message: "This node holds no signing identity yet, so there is no key to \
                              bind into the attestation"
                        .to_owned(),
                }
                .into_response();
            }
            Err(err) => {
                error!(error=?err, "Failed to read the node identity");
                return ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "Failed to read the node identity".to_owned(),
                }
                .into_response();
            }
        }
    } else {
        None
    };
    let binding = bound_public_key
        .as_ref()
        .map(|key| attest_key_binding(app_hash.as_ref(), key));
    let inner = binding.or(app_hash);
    // With `bindTransportKey` the second half commits to the key sealed requests
    // are encrypted to, wrapping whatever would have been there, so the node-key
    // and app bindings still hold under it.
    let transport_public_key = req.bind_transport_key.then_some(state.transport_public_key);
    let second_half = match transport_public_key {
        Some(key) => Some(attest_transport_binding(&inner.unwrap_or([0; 32]), &key)),
        None => inner,
    };
    let report_data = build_report_data(&nonce_array, second_half.as_ref());

    // 4. Generate attestation using the tee-attestation crate.
    //
    // Under --mock-tee, deliberately produce a mock quote (any OS, no TDX
    // hardware) and accept it below. The real path is unchanged: it generates a
    // hardware attestation and still rejects any mock result.
    #[cfg(feature = "mock-attestation")]
    let result = if state.mock_tee {
        generate_mock_attestation(report_data)
    } else {
        match generate_attestation(report_data) {
            Ok(result) => result,
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
                return ApiError {
                    status_code,
                    message,
                }
                .into_response();
            }
        }
    };
    #[cfg(not(feature = "mock-attestation"))]
    let result = match generate_attestation(report_data) {
        Ok(result) => result,
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
            return ApiError {
                status_code,
                message,
            }
            .into_response();
        }
    };

    // Reject mock attestations only when NOT in mock-tee mode - they otherwise
    // indicate an unsupported platform. Without the `mock-attestation` feature
    // there is no mock-tee mode, so any mock result is always rejected.
    #[cfg(feature = "mock-attestation")]
    let reject_mock = result.is_mock && !state.mock_tee;
    #[cfg(not(feature = "mock-attestation"))]
    let reject_mock = result.is_mock;
    if reject_mock {
        error!("Mock attestation generated - platform does not support TDX");
        return ApiError {
            status_code: StatusCode::NOT_IMPLEMENTED,
            message: "TDX attestation generation is only supported on Linux with TDX hardware"
                .to_owned(),
        }
        .into_response();
    }

    info!("TEE attestation generated successfully");
    ApiResponse {
        payload: TeeAttestResponse::new(
            result.quote_b64,
            result.quote,
            bound_public_key,
            transport_public_key.map(hex::encode),
        ),
    }
    .into_response()
}
