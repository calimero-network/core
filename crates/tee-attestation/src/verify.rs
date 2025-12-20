//! TDX quote verification.

use calimero_server_primitives::admin::Quote;
use dcap_qvl::collateral::get_collateral_from_pcs;
use dcap_qvl::verify::verify;
use tdx_quote::Quote as TdxQuote;
use tracing::{error, info, warn};

use crate::error::AttestationError;
use crate::generate::{is_mock_quote, MOCK_QUOTE_HEADER};

/// Result of verifying a TEE attestation.
#[derive(Debug, Clone)]
pub struct VerificationResult {
    /// Whether the quote's cryptographic signature is valid.
    pub quote_verified: bool,
    /// Whether the nonce in the report data matches the expected value.
    pub nonce_verified: bool,
    /// Whether the application hash matches (if an expected hash was provided).
    pub application_hash_verified: Option<bool>,
    /// The parsed quote structure.
    pub quote: Quote,
}

impl VerificationResult {
    /// Check if all verification checks passed.
    pub fn is_valid(&self) -> bool {
        self.quote_verified && self.nonce_verified && self.application_hash_verified.unwrap_or(true)
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
        AttestationError::QuoteParsingFailed(format!("{:?}", err))
    })?;

    info!("Quote parsed successfully");

    // Extract report data from quote
    let report_data = tdx_quote.report_input_data();
    let report_data_hex = hex::encode(report_data);
    info!(report_data=%report_data_hex, "Extracted report data from quote");

    // Fetch collateral from Intel PCS
    let collateral = get_collateral_from_pcs(quote_bytes).await.map_err(|err| {
        error!(error=?err, "Failed to fetch collateral from Intel PCS");
        AttestationError::CollateralFetchFailed(format!("{:?}", err))
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

    let quote_verified = match verify(quote_bytes, &collateral, now) {
        Ok(_verified_report) => {
            info!("Quote cryptographic verification: PASSED");
            true
        }
        Err(err) => {
            error!(error=?err, "Quote cryptographic verification: FAILED");
            false
        }
    };

    // Verify nonce matches report_data[0..32]
    let nonce_verified = &report_data[..32] == nonce;
    if nonce_verified {
        info!("Nonce verification: PASSED");
    } else {
        error!(
            expected=%hex::encode(nonce),
            actual=%hex::encode(&report_data[..32]),
            "Nonce verification: FAILED"
        );
    }

    // Verify application hash if provided
    let application_hash_verified = expected_app_hash.map(|expected_hash| {
        let actual_hash = &report_data[32..64];
        let verified = actual_hash == expected_hash;
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
        quote,
    };

    if result.is_valid() {
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

    // Verify nonce matches report_data[0..32]
    let nonce_verified = &report_data[..32] == nonce;
    if nonce_verified {
        info!("Nonce verification: PASSED");
    } else {
        error!(
            expected=%hex::encode(nonce),
            actual=%hex::encode(&report_data[..32]),
            "Nonce verification: FAILED"
        );
    }

    // Verify application hash if provided
    let application_hash_verified = expected_app_hash.map(|expected_hash| {
        let actual_hash = &report_data[32..64];
        let verified = actual_hash == expected_hash;
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
        quote,
    };

    if result.is_valid() {
        info!("Overall mock verification: PASSED");
    } else {
        warn!("Overall mock verification: FAILED (nonce or app_hash mismatch)");
    }

    Ok(result)
}
