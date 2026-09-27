use std::path::Path;
use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{TeeAttestRequest, TeeAttestResponse};
#[cfg(feature = "mock-attestation")]
use calimero_tee_attestation::generate_mock_attestation;
use calimero_tee_attestation::{
    attest_key_binding, attest_report_data_suffix, build_report_data, generate_attestation,
    tls_spki_sha256_from_pem, AttestationError,
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
    // With `bindTlsKey` the second half also commits to the key of the TLS
    // certificate this TD serves, wrapping the bindings above so they still
    // hold under it. Read on every call, so a renewed certificate is what is
    // bound; renewals keep the key, so a client's pin survives them.
    let tls_spki_sha256 = if req.bind_tls_key {
        match tls_key(state.attested_tls_certificate.as_deref()).await {
            Ok(digest) => Some(digest),
            Err(refusal) => return refusal.into_response(),
        }
    } else {
        None
    };
    // With `bindTransportKey` it commits to the key sealed requests are
    // encrypted to, wrapping all of the above in turn.
    let transport_public_key = req.bind_transport_key.then_some(state.transport_public_key);
    let second_half = attest_report_data_suffix(
        inner,
        tls_spki_sha256.as_ref(),
        transport_public_key.as_ref(),
    );
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
            tls_spki_sha256.map(hex::encode),
        ),
    }
    .into_response()
}

/// SHA-256 of the key of the TLS certificate this TD serves, or why there is
/// none to bind.
async fn tls_key(certificate: Option<&Path>) -> Result<[u8; 32], ApiError> {
    let Some(path) = certificate else {
        return Err(ApiError {
            status_code: StatusCode::CONFLICT,
            message: "This node names no attested TLS certificate ([server.attested_tls] \
                      certificate), so there is no TLS key to bind into the attestation"
                .to_owned(),
        });
    };
    let pem = tokio::fs::read(path).await.map_err(|err| {
        error!(path=%path.display(), error=%err, "Failed to read the attested TLS certificate");
        ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to read the attested TLS certificate".to_owned(),
        }
    })?;
    tls_spki_sha256_from_pem(&pem).map_err(|err| {
        error!(path=%path.display(), error=%err, "The attested TLS certificate does not parse");
        ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "The attested TLS certificate does not parse".to_owned(),
        }
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use reqwest::StatusCode;

    use super::tls_key;

    /// A self-signed P-256 certificate and its SPKI digest, as `openssl`
    /// computes it (the fixture `calimero-tee-attestation` tests against).
    const CERTIFICATE: &str = include_str!("../../../../tests/fixtures/tls-cert.pem");
    const SPKI_SHA256: &str = include_str!("../../../../tests/fixtures/tls-cert.spki-sha256");

    fn scratch(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("attest-tls-{}-{name}", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[tokio::test]
    async fn the_served_certificate_key_is_what_is_bound() {
        let path = scratch("cert.pem", CERTIFICATE);
        let digest = tls_key(Some(&path)).await.unwrap();
        assert_eq!(hex::encode(digest), SPKI_SHA256.trim());
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn a_node_naming_no_certificate_refuses_to_bind_one() {
        let refusal = tls_key(None).await.unwrap_err();
        assert_eq!(refusal.status_code, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn a_missing_or_broken_certificate_is_an_error_not_a_binding() {
        let missing = std::env::temp_dir().join("attest-tls-no-such-certificate.pem");
        assert_eq!(
            tls_key(Some(&missing)).await.unwrap_err().status_code,
            StatusCode::INTERNAL_SERVER_ERROR
        );

        let path = scratch(
            "broken.pem",
            "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
        );
        assert_eq!(
            tls_key(Some(&path)).await.unwrap_err().status_code,
            StatusCode::INTERNAL_SERVER_ERROR
        );
        std::fs::remove_file(path).unwrap();
    }
}
