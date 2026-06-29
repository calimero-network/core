//! TDX quote verification.

use calimero_server_primitives::admin::Quote;
use dcap_qvl::collateral::get_collateral_from_pcs;
use dcap_qvl::verify::verify;
use tdx_quote::Quote as TdxQuote;
use tracing::{error, info, warn};

use subtle::ConstantTimeEq;

use crate::error::AttestationError;
use crate::generate::{is_mock_quote, MOCK_QUOTE_HEADER};

/// Result of verifying a TEE attestation.
///
/// # Verification contract (read before trusting this type)
///
/// This struct reports the outcome of the *cryptographic* attestation checks
/// only. [`VerificationResult::crypto_valid`] is `true` when the quote
/// signature is valid, the nonce matches, and (if an expected app hash was
/// supplied) the app-hash binding matches.
///
/// It deliberately does **not** consult the TEE *policy* fields:
/// - [`tcb_status`](VerificationResult::tcb_status) — the DCAP TCB status
///   (e.g. `UpToDate` / `OutOfDate` / `SWHardeningNeeded`).
/// - [`advisory_ids`](VerificationResult::advisory_ids) — DCAP advisory IDs.
/// - the measurement registers carried in
///   [`quote`](VerificationResult::quote) (mrtd / rtmr0-3).
///
/// Callers MUST enforce TCB-status and measurement allowlists from policy
/// **separately**. In this repo that enforcement lives in governance — see
/// `admit_tee_node` / `validate_tee_attestation_allowlists`. This crate
/// intentionally does not duplicate that policy logic to avoid drift.
#[derive(Debug, Clone)]
pub struct VerificationResult {
    /// Whether the quote's cryptographic signature is valid.
    pub quote_verified: bool,
    /// Whether the nonce in the report data matches the expected value.
    pub nonce_verified: bool,
    /// Whether the application hash matches (if an expected hash was provided).
    pub application_hash_verified: Option<bool>,
    /// TCB status reported by DCAP verification (for policy decisions).
    pub tcb_status: Option<String>,
    /// Advisory IDs reported by DCAP verification.
    pub advisory_ids: Vec<String>,
    /// The parsed quote structure.
    pub quote: Quote,
}

impl VerificationResult {
    /// Returns `true` when the *cryptographic* attestation checks passed.
    ///
    /// This is `quote_verified && nonce_verified && app-hash-binding`, where
    /// the app-hash binding contributes `true` when no expected hash was
    /// supplied (see below). It checks **signature + nonce + (optional)
    /// app-hash only**.
    ///
    /// # This is NOT a full security gate
    ///
    /// This method does **not** check
    /// [`tcb_status`](VerificationResult::tcb_status),
    /// [`advisory_ids`](VerificationResult::advisory_ids), or the measurement
    /// registers (mrtd / rtmr0-3) carried in
    /// [`quote`](VerificationResult::quote). A `crypto_valid() == true` result
    /// can still come from a node with an out-of-date TCB or an unexpected
    /// measurement. Callers MUST enforce TCB-status and measurement allowlists
    /// from policy separately — in this repo that is done by governance
    /// (`admit_tee_node` / `validate_tee_attestation_allowlists`).
    ///
    /// # App-hash binding semantics
    ///
    /// When [`application_hash_verified`](VerificationResult::application_hash_verified)
    /// is `None`, **no expected hash was supplied**, so the app-hash binding is
    /// NOT checked and is treated as satisfied (`unwrap_or(true)`). Callers that
    /// require the quote to be bound to a specific application MUST pass
    /// `Some(expected)` to [`verify_attestation`] / [`verify_mock_attestation`];
    /// otherwise a quote from any application will pass this gate.
    pub fn crypto_valid(&self) -> bool {
        self.quote_verified && self.nonce_verified && self.application_hash_verified.unwrap_or(true)
    }

    /// Retained for backwards compatibility — delegates to
    /// [`crypto_valid`](VerificationResult::crypto_valid).
    ///
    /// The name `is_valid` misleads callers into reading it as a full security
    /// gate, which it is not (it covers crypto + nonce + optional app-hash
    /// only, never TCB status or measurements). Prefer
    /// [`crypto_valid`](VerificationResult::crypto_valid) in new code; this
    /// alias is kept so downstream consumers do not break.
    pub fn is_valid(&self) -> bool {
        self.crypto_valid()
    }
}

/// Verify a TDX attestation quote.
///
/// # Arguments
/// * `quote_bytes` - Raw quote bytes to verify.
/// * `nonce` - Expected 32-byte nonce that should be in report_data[0..32].
/// * `expected_app_hash` - Optional expected 32-byte app hash that should be in report_data[32..64].
///
/// # Returns
/// A `VerificationResult` with the verification status for each check.
///
/// # Errors
/// Returns an error if the quote cannot be parsed or if collateral fetch fails.
pub async fn verify_attestation(
    quote_bytes: &[u8],
    nonce: &[u8; 32],
    expected_app_hash: Option<&[u8; 32]>,
) -> Result<VerificationResult, AttestationError> {
    // Parse TDX quote
    let tdx_quote = TdxQuote::from_bytes(quote_bytes).map_err(|err| {
        error!(error=?err, "Failed to parse TDX quote");
        AttestationError::QuoteParsingFailed(format!("{err:?}"))
    })?;

    info!("Quote parsed successfully");

    // Extract report data from quote
    let report_data = tdx_quote.report_input_data();
    let report_data_hex = hex::encode(report_data);
    info!(report_data=%report_data_hex, "Extracted report data from quote");

    // Fetch collateral from Intel PCS
    let collateral = get_collateral_from_pcs(quote_bytes).await.map_err(|err| {
        error!(error=?err, "Failed to fetch collateral from Intel PCS");
        AttestationError::CollateralFetchFailed(format!("{err:?}"))
    })?;

    info!("Collateral fetched from Intel PCS");

    // Verify quote signature and certificate chain
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| {
            error!(error=?err, "Failed to get current time");
            AttestationError::SystemTimeError(err.to_string())
        })?
        .as_secs();

    let (quote_verified, tcb_status, advisory_ids) = match verify(quote_bytes, &collateral, now) {
        Ok(verified_report) => {
            info!("Quote cryptographic verification: PASSED");
            info!(
                tcb_status = %verified_report.status,
                advisory_ids = ?verified_report.advisory_ids,
                "Quote TCB status extracted from DCAP verification"
            );
            (
                true,
                Some(verified_report.status),
                verified_report.advisory_ids,
            )
        }
        Err(err) => {
            error!(error=?err, "Quote cryptographic verification: FAILED");
            (false, None, Vec::new())
        }
    };

    // Verify nonce matches report_data[0..32] (constant-time compare).
    let nonce_verified: bool = report_data[..32].ct_eq(nonce).into();
    if nonce_verified {
        info!("Nonce verification: PASSED");
    } else {
        error!(
            expected=%hex::encode(nonce),
            actual=%hex::encode(&report_data[..32]),
            "Nonce verification: FAILED"
        );
    }

    // Verify application hash if provided (constant-time compare).
    let application_hash_verified = expected_app_hash.map(|expected_hash| {
        let actual_hash = &report_data[32..64];
        let verified: bool = actual_hash.ct_eq(expected_hash).into();
        if verified {
            info!("Application hash verification: PASSED");
        } else {
            error!(
                expected=%hex::encode(expected_hash),
                actual=%hex::encode(actual_hash),
                "Application hash verification: FAILED"
            );
        }
        verified
    });

    // Convert tdx_quote to our serializable Quote type
    let quote = Quote::try_from(tdx_quote).map_err(|err| {
        error!(error=%err, "Failed to convert TDX quote to serializable format");
        AttestationError::QuoteConversionFailed(err.to_string())
    })?;

    let result = VerificationResult {
        quote_verified,
        nonce_verified,
        application_hash_verified,
        tcb_status,
        advisory_ids,
        quote,
    };

    if result.crypto_valid() {
        info!("Overall verification: PASSED");
    } else {
        error!("Overall verification: FAILED");
    }

    Ok(result)
}

/// Verify a mock TEE attestation for development/testing.
///
/// This function verifies mock attestations generated on non-Linux platforms.
/// It extracts the report data from the mock quote format and verifies the nonce
/// and optional application hash match.
///
/// # Arguments
/// * `quote_bytes` - Raw mock quote bytes.
/// * `nonce` - Expected 32-byte nonce.
/// * `expected_app_hash` - Optional expected 32-byte app hash.
///
/// # Returns
/// A `VerificationResult` where `quote_verified` is always `true` for mock quotes
/// (since there's no cryptographic signature to verify).
///
/// # Security Warning
/// This function bypasses all cryptographic verification. It should ONLY be used
/// for development and testing purposes.
pub fn verify_mock_attestation(
    quote_bytes: &[u8],
    nonce: &[u8; 32],
    expected_app_hash: Option<&[u8; 32]>,
) -> Result<VerificationResult, AttestationError> {
    use crate::generate::create_mock_quote;

    warn!("Verifying MOCK attestation - NOT FOR PRODUCTION USE");

    // Verify this is actually a mock quote
    if !is_mock_quote(quote_bytes) {
        return Err(AttestationError::QuoteParsingFailed(
            "Not a valid mock quote - missing MOCK_TDX_QUOTE_V1 header".to_owned(),
        ));
    }

    // Extract report data from mock quote
    // Format: MOCK_TDX_QUOTE_V1 (17 bytes) || report_data (64 bytes) || padding
    let header_len = MOCK_QUOTE_HEADER.len();
    if quote_bytes.len() < header_len + 64 {
        return Err(AttestationError::QuoteParsingFailed(
            "Mock quote too short to contain report data".to_owned(),
        ));
    }

    let report_data = &quote_bytes[header_len..header_len + 64];
    let report_data_hex = hex::encode(report_data);
    info!(report_data=%report_data_hex, "Extracted report data from mock quote");

    // Verify nonce matches report_data[0..32] (constant-time compare).
    let nonce_verified: bool = report_data[..32].ct_eq(nonce).into();
    if nonce_verified {
        info!("Nonce verification: PASSED");
    } else {
        error!(
            expected=%hex::encode(nonce),
            actual=%hex::encode(&report_data[..32]),
            "Nonce verification: FAILED"
        );
    }

    // Verify application hash if provided (constant-time compare).
    let application_hash_verified = expected_app_hash.map(|expected_hash| {
        let actual_hash = &report_data[32..64];
        let verified: bool = actual_hash.ct_eq(expected_hash).into();
        if verified {
            info!("Application hash verification: PASSED");
        } else {
            error!(
                expected=%hex::encode(expected_hash),
                actual=%hex::encode(actual_hash),
                "Application hash verification: FAILED"
            );
        }
        verified
    });

    // Create a mock quote structure with the report data
    let mut mock_report_data = [0u8; 64];
    mock_report_data.copy_from_slice(report_data);
    let quote = create_mock_quote(&mock_report_data);

    let result = VerificationResult {
        quote_verified: true, // Mock quotes always pass signature verification
        nonce_verified,
        application_hash_verified,
        tcb_status: Some("Mock".to_owned()),
        advisory_ids: Vec::new(),
        quote,
    };

    if result.crypto_valid() {
        info!("Overall mock verification: PASSED");
    } else {
        warn!("Overall mock verification: FAILED (nonce or app_hash mismatch)");
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::create_mock_quote;

    fn result_with(
        quote_verified: bool,
        nonce_verified: bool,
        application_hash_verified: Option<bool>,
        tcb_status: Option<String>,
    ) -> VerificationResult {
        VerificationResult {
            quote_verified,
            nonce_verified,
            application_hash_verified,
            tcb_status,
            advisory_ids: Vec::new(),
            quote: create_mock_quote(&[0u8; 64]),
        }
    }

    #[test]
    fn crypto_valid_ignores_tcb_status() {
        // The crypto gate is intentionally independent of TCB status: an
        // out-of-date TCB must NOT flip crypto_valid(). Policy (TCB/measurement
        // allowlists) is enforced by callers, not by this method.
        let result = result_with(true, true, Some(true), Some("OutOfDate".to_owned()));
        assert!(
            result.crypto_valid(),
            "crypto_valid() must be true regardless of tcb_status"
        );
    }

    #[test]
    fn is_valid_alias_agrees_with_crypto_valid() {
        // The retained `is_valid` alias must return exactly what `crypto_valid`
        // returns for every input combination.
        let cases = [
            result_with(true, true, None, None),
            result_with(true, true, Some(true), Some("OutOfDate".to_owned())),
            result_with(true, true, Some(false), Some("UpToDate".to_owned())),
            result_with(false, true, Some(true), None),
            result_with(true, false, None, Some("Mock".to_owned())),
            result_with(false, false, Some(false), None),
        ];
        for case in &cases {
            assert_eq!(
                case.is_valid(),
                case.crypto_valid(),
                "is_valid() must agree with crypto_valid()"
            );
        }
    }

    #[test]
    fn none_app_hash_is_treated_as_satisfied() {
        // `application_hash_verified == None` means no expected hash was
        // supplied, so the binding is not checked (treated as satisfied).
        let result = result_with(true, true, None, None);
        assert!(result.crypto_valid());
    }
}
