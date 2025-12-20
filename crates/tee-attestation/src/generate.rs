//! TDX quote generation (Linux) and mock attestation for non-Linux platforms.

use base64::{engine::general_purpose::STANDARD as base64_engine, Engine};
use calimero_server_primitives::admin::{
    CertificationData, QeReportCertificationDataInfo, Quote, QuoteBody, QuoteHeader,
};
#[cfg(target_os = "linux")]
use configfs_tsm::create_tdx_quote;
#[cfg(target_os = "linux")]
use tdx_quote::Quote as TdxQuote;
#[cfg(target_os = "linux")]
use tracing::error;
use tracing::warn;

use crate::error::AttestationError;

/// Magic header for mock quotes - used to identify mock attestations.
pub const MOCK_QUOTE_HEADER: &[u8] = b"MOCK_TDX_QUOTE_V1";

/// Result of generating a TEE attestation.
#[derive(Debug, Clone)]
pub struct AttestationResult {
    /// Raw quote bytes.
    pub quote_bytes: Vec<u8>,
    /// Base64-encoded quote string.
    pub quote_b64: String,
    /// Parsed and serializable quote structure.
    pub quote: Quote,
    /// Whether this is a mock attestation (for development/testing).
    pub is_mock: bool,
}

/// Generate a TDX attestation with the given report data.
///
/// The report data is typically constructed as: `nonce[32] || app_hash[32]`
///
/// # Arguments
/// * `report_data` - 64 bytes of data to include in the attestation.
///
/// # Returns
/// An `AttestationResult` containing the quote bytes, base64 encoding, and parsed quote.
///
/// # Errors
/// Returns an error if quote generation fails.
///
/// # Platform Behavior
/// - On Linux with TDX: Generates a real TDX attestation quote.
/// - On non-Linux platforms: Returns a mock attestation for development/testing.
#[cfg(target_os = "linux")]
pub fn generate_attestation(report_data: [u8; 64]) -> Result<AttestationResult, AttestationError> {
    // Generate TDX quote using configfs-tsm
    let quote_bytes = create_tdx_quote(report_data).map_err(|err| {
        error!(error=?err, "Failed to generate TDX quote");
        AttestationError::QuoteGenerationFailed(format!("{:?}", err))
    })?;

    // Parse the generated quote
    let tdx_quote = TdxQuote::from_bytes(&quote_bytes).map_err(|err| {
        error!(error=?err, "Failed to parse generated TDX quote");
        AttestationError::QuoteParsingFailed(format!("{:?}", err))
    })?;

    // Convert to serializable format
    let quote = Quote::try_from(tdx_quote).map_err(|err| {
        error!(error=%err, "Failed to convert TDX quote to serializable format");
        AttestationError::QuoteConversionFailed(err.to_string())
    })?;

    let quote_b64 = base64_engine.encode(&quote_bytes);

    Ok(AttestationResult {
        quote_bytes,
        quote_b64,
        quote,
        is_mock: false,
    })
}

/// Generate a mock TEE attestation on non-Linux platforms.
///
/// This function creates a syntactically valid but cryptographically unverifiable
/// attestation for development and testing purposes.
///
/// # Security Warning
/// Mock attestations bypass all TEE security guarantees. The quote signature is
/// invalid and will fail cryptographic verification. This is only suitable for
/// testing attestation protocol flow on non-TEE platforms.
#[cfg(not(target_os = "linux"))]
pub fn generate_attestation(report_data: [u8; 64]) -> Result<AttestationResult, AttestationError> {
    warn!("Generating MOCK attestation on non-Linux platform - NOT FOR PRODUCTION USE");

    // Create mock quote structure with placeholder values
    let quote = create_mock_quote(&report_data);

    // Create mock "quote bytes" with marker + report_data
    let mut quote_bytes = Vec::with_capacity(128);

    // Mock quote header marker (identifies this as mock)
    quote_bytes.extend_from_slice(MOCK_QUOTE_HEADER);

    // Include the report data so it can be extracted during mock verification
    quote_bytes.extend_from_slice(&report_data);

    // Pad to reasonable size (real quotes are ~4-6KB)
    quote_bytes.resize(256, 0);

    let quote_b64 = base64_engine.encode(&quote_bytes);

    Ok(AttestationResult {
        quote_bytes,
        quote_b64,
        quote,
        is_mock: true,
    })
}

/// Check if the given quote bytes represent a mock attestation.
pub fn is_mock_quote(quote_bytes: &[u8]) -> bool {
    quote_bytes.len() >= MOCK_QUOTE_HEADER.len()
        && &quote_bytes[..MOCK_QUOTE_HEADER.len()] == MOCK_QUOTE_HEADER
}

/// Create a mock Quote structure with the given report data.
pub fn create_mock_quote(report_data: &[u8; 64]) -> Quote {
    // Standard mock values - 48-byte measurements as hex (96 chars)
    let mock_measurement_48 =
        "000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
    // 16-byte values as hex (32 chars)
    let mock_16_bytes = "00000000000000000000000000000000";
    // 8-byte values as hex (16 chars)
    let mock_8_bytes = "0000000000000000";

    Quote {
        header: QuoteHeader {
            version: 4,
            attestation_key_type: 2, // ECDSA-256-with-P-256
            tee_type: 0x81,          // TDX
            qe_vendor_id: "939a7233f79c4ca9940a0db3957f0607".to_owned(), // Intel QE vendor ID
            user_data: "00000000000000000000000000000000".to_owned(), // 16 bytes of zeros
        },
        body: QuoteBody {
            tdx_version: "1.0".to_owned(),
            tee_tcb_svn: mock_16_bytes.to_owned(),
            mrseam: mock_measurement_48.to_owned(),
            mrsignerseam: mock_measurement_48.to_owned(),
            seamattributes: mock_8_bytes.to_owned(),
            tdattributes: mock_8_bytes.to_owned(),
            xfam: mock_8_bytes.to_owned(),
            mrtd: mock_measurement_48.to_owned(),
            mrconfigid: mock_measurement_48.to_owned(),
            mrowner: mock_measurement_48.to_owned(),
            mrownerconfig: mock_measurement_48.to_owned(),
            rtmr0: mock_measurement_48.to_owned(),
            rtmr1: mock_measurement_48.to_owned(),
            rtmr2: mock_measurement_48.to_owned(),
            rtmr3: mock_measurement_48.to_owned(),
            reportdata: hex::encode(report_data), // 64 bytes = 128 hex chars
            tee_tcb_svn_2: None,
            mrservicetd: None,
        },
        // Mock signature (64 bytes for ECDSA-256)
        signature: "0".repeat(128),
        // Mock attestation key (65 bytes for uncompressed P-256 public key)
        attestation_key: "04".to_owned() + &"0".repeat(128),
        // Mock certification data
        certification_data: CertificationData::QeReportCertificationData(
            QeReportCertificationDataInfo {
                qe_report: "0".repeat(768),             // 384 bytes
                signature: "0".repeat(128),             // 64 bytes
                qe_authentication_data: "0".repeat(64), // 32 bytes
                certification_data_type: "PckCertChain".to_owned(),
                certification_data: "0".repeat(200), // Placeholder
            },
        ),
    }
}

/// Build report data from nonce and optional application hash.
///
/// # Arguments
/// * `nonce` - 32-byte nonce value.
/// * `app_hash` - Optional 32-byte application bytecode hash.
///
/// # Returns
/// A 64-byte array suitable for use as TDX report data.
pub fn build_report_data(nonce: &[u8; 32], app_hash: Option<&[u8; 32]>) -> [u8; 64] {
    let mut report_data = [0u8; 64];
    report_data[..32].copy_from_slice(nonce);
    if let Some(hash) = app_hash {
        report_data[32..].copy_from_slice(hash);
    }
    report_data
}
