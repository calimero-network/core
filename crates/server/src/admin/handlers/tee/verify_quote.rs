use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use base64::{engine::general_purpose::STANDARD as base64_engine, Engine};
use calimero_server_primitives::admin::{
    TeeVerifyQuoteRequest, TeeVerifyQuoteResponse, TeeVerifyQuoteResponseData,
};
use calimero_tee_attestation::{verify_attestation, AttestationError};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(_state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<TeeVerifyQuoteRequest>,
) -> impl IntoResponse {
    info!(
        nonce=%req.nonce,
        has_expected_hash=%req.expected_application_hash.is_some(),
        "Verifying TDX quote"
    );

    match verify_quote(req).await {
        Ok(response) => ApiResponse { payload: response }.into_response(),
        Err(err) => err.into_response(),
    }
}

async fn verify_quote(req: TeeVerifyQuoteRequest) -> Result<TeeVerifyQuoteResponse, ApiError> {
    // Nonce format is pre-validated by ValidatedJson
    let nonce = hex::decode(&req.nonce).expect("pre-validated hex string");
    let nonce_array: [u8; 32] = nonce.try_into().expect("pre-validated length");

    // Expected application hash format is pre-validated by ValidatedJson
    let expected_app_hash = req.expected_application_hash.as_ref().map(|hash_hex| {
        let h = hex::decode(hash_hex).expect("pre-validated hex string");
        let hash_array: [u8; 32] = h.try_into().expect("pre-validated length");
        hash_array
    });

    // Decode base64 quote
    let quote_bytes = base64_engine.decode(&req.quote_b64).map_err(|err| {
        error!(error=?err, "Failed to decode base64 quote");
        ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("Invalid base64 quote: {}", err),
        }
    })?;

    info!(quote_size=%quote_bytes.len(), "Quote decoded successfully");

    // 4. Verify using tee-attestation crate
    let result = verify_attestation(&quote_bytes, &nonce_array, expected_app_hash.as_ref())
        .await
        .map_err(|err| {
            let (status_code, message) = match &err {
                AttestationError::QuoteParsingFailed(_) => {
                    (StatusCode::BAD_REQUEST, err.to_string())
                }
                AttestationError::CollateralFetchFailed(_) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
                }
                _ => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
            };
            error!(error=%err, "Attestation verification failed");
            ApiError {
                status_code,
                message,
            }
        })?;

    let is_valid = result.is_valid();

    let response_data = TeeVerifyQuoteResponseData {
        quote_verified: result.quote_verified,
        nonce_verified: result.nonce_verified,
        application_hash_verified: result.application_hash_verified,
        quote: result.quote,
    };

    if is_valid {
        info!("✓ Overall verification: PASSED");
    } else {
        error!("✗ Overall verification: FAILED");
    }

    Ok(TeeVerifyQuoteResponse::new(response_data))
}
