use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use base64::{engine::general_purpose::STANDARD as base64_engine, Engine};
use calimero_server_primitives::admin::{
    Quote, TeeVerifyQuoteRequest, TeeVerifyQuoteResponse, TeeVerifyQuoteResponseData,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

use dcap_qvl::collateral::get_collateral_from_pcs;
use dcap_qvl::verify::verify;
use tdx_quote::Quote as TdxQuote;

pub async fn handler(
    Extension(_state): Extension<Arc<AdminState>>,
    Json(req): Json<TeeVerifyQuoteRequest>,
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
    // 1. Validate and decode nonce
    let nonce = hex::decode(&req.nonce).map_err(|_| {
        error!("Invalid nonce format");
        ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Invalid nonce format (must be 64 hex characters)".to_owned(),
        }
    })?;

    if nonce.len() != 32 {
        error!(nonce_len=%nonce.len(), "Invalid nonce length");
        return Err(ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Nonce must be exactly 32 bytes (64 hex characters)".to_owned(),
        });
    }

    // 2. Validate expected application hash if provided
    let expected_app_hash = if let Some(hash_hex) = &req.expected_application_hash {
        let h = hex::decode(hash_hex).map_err(|_| {
            error!("Invalid application hash format");
            ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid application hash format (must be 64 hex characters)".to_owned(),
            }
        })?;

        if h.len() != 32 {
            error!(hash_len=%h.len(), "Invalid application hash length");
            return Err(ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Application hash must be exactly 32 bytes (64 hex characters)".to_owned(),
            });
        }
        Some(h)
    } else {
        None
    };

    // 3. Decode base64 quote
    let quote_bytes = base64_engine.decode(&req.quote_b64).map_err(|err| {
        error!(error=?err, "Failed to decode base64 quote");
        ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("Invalid base64 quote: {}", err),
        }
    })?;

    info!(quote_size=%quote_bytes.len(), "Quote decoded successfully");

    // 4. Parse TDX quote
    let tdx_quote = TdxQuote::from_bytes(&quote_bytes).map_err(|err| {
        error!(error=?err, "Failed to parse TDX quote");
        ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("Failed to parse TDX quote: {:?}", err),
        }
    })?;

    info!("Quote parsed successfully");

    // 5. Extract report data from quote
    let report_data = tdx_quote.report_input_data();
    let report_data_hex = hex::encode(report_data);

    info!(report_data=%report_data_hex, "Extracted report data from quote");

    // 6. Fetch collateral from Intel PCS
    let collateral = get_collateral_from_pcs(&quote_bytes).await.map_err(|err| {
        error!(error=?err, "Failed to fetch collateral from Intel PCS");
        ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to fetch collateral from Intel PCS: {:?}", err),
        }
    })?;

    info!("Collateral fetched from Intel PCS");

    // 7. Verify quote signature and certificate chain
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| {
            error!(error=?err, "Failed to get current time");
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: format!("Failed to get current time: {}", err),
            }
        })?
        .as_secs();

    let quote_verified = match verify(&quote_bytes, &collateral, now) {
        Ok(_) => {
            info!("Quote cryptographic verification: PASSED");
            true
        }
        Err(err) => {
            error!(error=?err, "Quote cryptographic verification: FAILED");
            // We don't return early here - we continue to check nonce and app hash
            // and return all verification results
            false
        }
    };

    // 8. Verify nonce matches report_data[0..32]
    let nonce_verified = &report_data[..32] == nonce.as_slice();
    if nonce_verified {
        info!("Nonce verification: PASSED");
    } else {
        error!(
            expected=%hex::encode(&nonce),
            actual=%hex::encode(&report_data[..32]),
            "Nonce verification: FAILED"
        );
    }

    // 9. Verify application hash if provided
    let application_hash_verified = if let Some(expected_hash) = expected_app_hash {
        let actual_hash = &report_data[32..64];
        let verified = actual_hash == expected_hash.as_slice();
        if verified {
            info!("Application hash verification: PASSED");
        } else {
            error!(
                expected=%hex::encode(&expected_hash),
                actual=%hex::encode(actual_hash),
                "Application hash verification: FAILED"
            );
        }
        Some(verified)
    } else {
        None
    };

    // 10. Convert tdx_quote to our serializable Quote type
    let quote = Quote::try_from(tdx_quote).map_err(|err| {
        error!(error=%err, "Failed to convert TDX quote to serializable format");
        ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to convert TDX quote: {}", err),
        }
    })?;

    let response_data = TeeVerifyQuoteResponseData {
        quote_verified,
        nonce_verified,
        application_hash_verified,
        quote,
    };

    let overall_success =
        quote_verified && nonce_verified && application_hash_verified.unwrap_or(true);

    if overall_success {
        info!("✓ Overall verification: PASSED");
    } else {
        error!("✗ Overall verification: FAILED");
    }

    Ok(TeeVerifyQuoteResponse::new(response_data))
}
