use std::sync::{Arc, LazyLock};

use axum::response::IntoResponse;
use axum::Extension;
use base64::{engine::general_purpose::STANDARD as base64_engine, Engine};
use calimero_server_primitives::admin::{
    TeeVerifyQuoteRequest, TeeVerifyQuoteResponse, TeeVerifyQuoteResponseData,
};
use calimero_tee_attestation::{verify_attestation, AttestationError};
use reqwest::StatusCode;
use tracing::{error, info, warn};

use super::verify_quote_throttle::{Decision, VerifyQuoteThrottle};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

/// Process-global rate + concurrency limiter for the public, unauthenticated
/// `/verify-quote` endpoint (TEE-01 / audit #325). The endpoint must stay
/// public (the mdma manager proxies to it with no node admin token), so this
/// is the boundary that keeps an attacker from driving the heavy
/// `verify_attestation` (outbound Intel-PCS fetch + DCAP verify) path
/// unbounded.
static VERIFY_QUOTE_THROTTLE: LazyLock<VerifyQuoteThrottle> =
    LazyLock::new(VerifyQuoteThrottle::default);

/// Generic message returned for any internal (500) verification failure. The
/// real error is logged server-side; the response body never leaks internals
/// (PCS endpoint shape, cert-chain/TCB parser detail, etc.).
const GENERIC_INTERNAL_ERROR: &str = "Attestation verification failed";

pub async fn handler(
    Extension(_state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<TeeVerifyQuoteRequest>,
) -> impl IntoResponse {
    info!(
        nonce=%req.nonce,
        "Verifying TDX quote"
    );

    // Throttle BEFORE any decode/verify work so a flood is rejected cheaply.
    // The permit is held for the lifetime of the verify so the global inflight
    // cap stays accurate.
    let _permit = match VERIFY_QUOTE_THROTTLE.check() {
        Decision::Proceed(permit) => permit,
        Decision::RateLimited | Decision::AtCapacity => {
            warn!("Rejecting /verify-quote: attestation verification throttle exceeded");
            return ApiError {
                status_code: StatusCode::TOO_MANY_REQUESTS,
                message: "Too many attestation verification requests; please retry later"
                    .to_owned(),
            }
            .into_response();
        }
    };

    match verify_quote(req).await {
        Ok(response) => ApiResponse { payload: response }.into_response(),
        Err(err) => err.into_response(),
    }
}

async fn verify_quote(req: TeeVerifyQuoteRequest) -> Result<TeeVerifyQuoteResponse, ApiError> {
    // Defense-in-depth: ValidatedJson already validates format, but we keep defensive
    // error handling here in case validation is bypassed or has bugs. This prevents
    // panics and provides clear error messages.
    let nonce = hex::decode(&req.nonce).map_err(|_| {
        error!("Invalid nonce format");
        ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Invalid nonce format (must be hex string)".to_owned(),
        }
    })?;

    let nonce_array: [u8; 32] = nonce.try_into().map_err(|_| {
        error!(nonce_len=%req.nonce.len() / 2, "Invalid nonce length");
        ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Nonce must be exactly 32 bytes (64 hex characters)".to_owned(),
        }
    })?;

    // Decode and validate the mandatory expected application hash. The attestation
    // is only meaningful when bound to an expected app hash, so this is required.
    let expected_app_hash: [u8; 32] = {
        let h = hex::decode(&req.expected_application_hash).map_err(|_| {
            error!("Invalid application hash format");
            ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid application hash format (must be hex string)".to_owned(),
            }
        })?;

        h.try_into().map_err(|_| {
            error!(hash_len=%req.expected_application_hash.len() / 2, "Invalid application hash length");
            ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Application hash must be exactly 32 bytes (64 hex characters)".to_owned(),
            }
        })?
    };

    // Decode base64 quote
    let quote_bytes = base64_engine.decode(&req.quote_b64).map_err(|err| {
        error!(error=?err, "Failed to decode base64 quote");
        ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Invalid base64 quote".to_owned(),
        }
    })?;

    info!(quote_size=%quote_bytes.len(), "Quote decoded successfully");

    // 4. Verify using tee-attestation crate
    let result = verify_attestation(&quote_bytes, &nonce_array, &expected_app_hash)
        .await
        .map_err(|err| {
            // Always log the real error server-side; never put it in the
            // response body (TEE-01 / audit #325). See `scrub_verify_error`.
            error!(error=%err, "Attestation verification failed");
            scrub_verify_error(&err)
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

/// Map an [`AttestationError`] to a client-safe [`ApiError`].
///
/// A malformed quote is a client error (400) with a fixed generic message —
/// no parser internals. Everything else (including the outbound Intel-PCS
/// collateral fetch and DCAP crypto-verify failures) is a scrubbed 500: the
/// response body NEVER carries `err.to_string()` / `{err:?}`, so an
/// unauthenticated caller can't fingerprint the PCS endpoint, cert chain, or
/// TCB internals. The real error is logged server-side by the caller.
fn scrub_verify_error(err: &AttestationError) -> ApiError {
    match err {
        AttestationError::QuoteParsingFailed(_) => ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "Invalid TDX quote".to_owned(),
        },
        _ => ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: GENERIC_INTERNAL_ERROR.to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A malformed-quote error is a 400 with a fixed message that does NOT
    /// echo the underlying parser detail.
    #[test]
    fn quote_parsing_error_is_generic_400() {
        let detail = "frobnicated header at offset 0x41414141";
        let api = scrub_verify_error(&AttestationError::QuoteParsingFailed(detail.to_owned()));
        assert_eq!(api.status_code, StatusCode::BAD_REQUEST);
        assert_eq!(api.message, "Invalid TDX quote");
        assert!(
            !api.message.contains(detail),
            "400 body must not leak parser detail"
        );
    }

    /// The PCS collateral-fetch failure is the headline leak vector: it must
    /// be a scrubbed 500 that never echoes the PCS URL / connection detail.
    #[test]
    fn collateral_fetch_error_is_scrubbed_500() {
        let detail = "https://api.trustedservices.intel.com/... connection refused";
        let api = scrub_verify_error(&AttestationError::CollateralFetchFailed(detail.to_owned()));
        assert_eq!(api.status_code, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(api.message, GENERIC_INTERNAL_ERROR);
        assert!(
            !api.message.contains("intel") && !api.message.contains(detail),
            "500 body must not leak the PCS endpoint / fetch detail"
        );
    }

    /// Any other internal error (e.g. signature verify) is also a scrubbed 500.
    #[test]
    fn other_internal_errors_are_scrubbed_500() {
        for err in [
            AttestationError::QuoteVerificationFailed("cert chain X leaf Y".to_owned()),
            AttestationError::QuoteConversionFailed("secret".to_owned()),
            AttestationError::SystemTimeError("clock".to_owned()),
            AttestationError::NotSupported,
        ] {
            let api = scrub_verify_error(&err);
            assert_eq!(api.status_code, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(api.message, GENERIC_INTERNAL_ERROR);
        }
    }
}
