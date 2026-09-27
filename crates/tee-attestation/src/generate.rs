//! TDX quote generation (Linux) and mock attestation for non-Linux platforms.

#[cfg(any(target_os = "linux", feature = "mock-attestation"))]
use base64::{engine::general_purpose::STANDARD as base64_engine, Engine};
use calimero_server_primitives::admin::Quote;
#[cfg(feature = "mock-attestation")]
use calimero_server_primitives::admin::{
    CertificationData, QeReportCertificationDataInfo, QuoteBody, QuoteHeader,
};
#[cfg(target_os = "linux")]
use configfs_tsm::create_tdx_quote;
#[cfg(target_os = "linux")]
use tdx_quote::Quote as TdxQuote;
#[cfg(target_os = "linux")]
use tracing::error;
#[cfg(all(not(target_os = "linux"), feature = "mock-attestation"))]
use tracing::warn;

use crate::error::AttestationError;

/// Magic header for mock quotes - used to identify mock attestations.
#[cfg(feature = "mock-attestation")]
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
        AttestationError::QuoteGenerationFailed(format!("{err:?}"))
    })?;

    // Parse the generated quote
    let tdx_quote = TdxQuote::from_bytes(&quote_bytes).map_err(|err| {
        error!(error=?err, "Failed to parse generated TDX quote");
        AttestationError::QuoteParsingFailed(format!("{err:?}"))
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
#[cfg(all(not(target_os = "linux"), feature = "mock-attestation"))]
pub fn generate_attestation(report_data: [u8; 64]) -> Result<AttestationResult, AttestationError> {
    warn!("Generating MOCK attestation on non-Linux platform - NOT FOR PRODUCTION USE");
    Ok(generate_mock_attestation(report_data))
}

/// Non-Linux platforms without the `mock-attestation` feature have no way to
/// produce a quote: real TDX generation is Linux-only and the mock fallback is
/// compiled out. Fail explicitly rather than silently returning an unverifiable
/// quote.
#[cfg(all(not(target_os = "linux"), not(feature = "mock-attestation")))]
pub fn generate_attestation(_report_data: [u8; 64]) -> Result<AttestationResult, AttestationError> {
    Err(AttestationError::QuoteGenerationFailed(
        "mock attestation not compiled in; build with --features mock-attestation on non-TDX platforms"
            .to_owned(),
    ))
}

/// Build a mock attestation result on ANY platform. Dev/test only — the quote
/// is cryptographically invalid and must only be accepted by an `accept_mock`
/// policy. Used by `merod --mock-tee`.
#[cfg(feature = "mock-attestation")]
pub fn generate_mock_attestation(report_data: [u8; 64]) -> AttestationResult {
    let quote = create_mock_quote(&report_data);
    let mut quote_bytes = Vec::with_capacity(256);
    quote_bytes.extend_from_slice(MOCK_QUOTE_HEADER);
    quote_bytes.extend_from_slice(&report_data);
    quote_bytes.resize(256, 0);
    let quote_b64 = base64_engine.encode(&quote_bytes);
    AttestationResult {
        quote_bytes,
        quote_b64,
        quote,
        is_mock: true,
    }
}

/// Check if the given quote bytes represent a mock attestation.
#[cfg(feature = "mock-attestation")]
pub fn is_mock_quote(quote_bytes: &[u8]) -> bool {
    quote_bytes.len() >= MOCK_QUOTE_HEADER.len()
        && &quote_bytes[..MOCK_QUOTE_HEADER.len()] == MOCK_QUOTE_HEADER
}

/// Create a mock Quote structure with the given report data.
#[cfg(feature = "mock-attestation")]
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

/// Domain separator for [`attest_key_binding`].
pub const ATTEST_KEY_BINDING_DOMAIN: &[u8] = b"calimero.tee-attest.key-binding.v1";

/// The value the attest endpoint puts in report data bytes `32..64` when a
/// client asks it to bind the node's key: SHA-256 over the domain, the app's
/// bytecode hash (32 zero bytes when none was requested), and the key.
///
/// Binding the key is what lets a client tie the quote to the node it talks to.
/// Without it the quote proves some genuine TEE ran this image, and a relay
/// could forward the attest call to that TEE while answering everything else
/// itself. With it, anything later signed by `public_key` traces back to the
/// attested machine. Client and node must compute this identically, so both
/// use this function.
#[must_use]
pub fn attest_key_binding(app_hash: Option<&[u8; 32]>, public_key: &[u8; 32]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(ATTEST_KEY_BINDING_DOMAIN);
    hasher.update(app_hash.unwrap_or(&[0u8; 32]));
    hasher.update(public_key);
    hasher.finalize().into()
}

/// Domain separator for [`attest_transport_binding`].
pub const ATTEST_TRANSPORT_BINDING_DOMAIN: &[u8] = b"calimero.tee-attest.transport-key.v1";

/// The value the attest endpoint puts in report data bytes `32..64` when a
/// client asks it to bind the node's X25519 transport key: SHA-256 over the
/// domain, the 32 bytes that would have been there otherwise (`inner`: the
/// key binding, else the app hash, else zeros), and the transport key.
///
/// The transport key is what a client encrypts its requests to (the server's
/// sealed transport). TLS in front of a node ends wherever the operator points
/// it, so a proxy there reads everything; a request sealed to a key that only
/// the attested TD holds is unreadable to it. Binding the key into the quote is
/// what makes "only the attested TD holds it" something a client can check.
/// Client and node must compute this identically, so both use this function.
#[must_use]
pub fn attest_transport_binding(inner: &[u8; 32], transport_key: &[u8; 32]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(ATTEST_TRANSPORT_BINDING_DOMAIN);
    hasher.update(inner);
    hasher.update(transport_key);
    hasher.finalize().into()
}

/// Domain separator for [`attest_tls_binding`].
pub const ATTEST_TLS_BINDING_DOMAIN: &[u8] = b"calimero.tee-attest.tls-key.v1";

/// The value the attest endpoint puts in report data bytes `32..64` when a
/// client asks it to bind the key of the TLS certificate the TD serves:
/// SHA-256 over the domain, the 32 bytes that would have been there otherwise
/// (`inner`: the key binding, else the app hash, else zeros), and the SHA-256 of
/// the certificate's `SubjectPublicKeyInfo` ([`tls_spki_sha256`]).
///
/// This is what makes TLS terminate in the attested TD as far as a client can
/// tell. A certificate from a public CA says who controls the domain; it says
/// nothing about where the key lives, and an operator or a proxy can obtain one
/// too. A quote that commits to the key says the TD holds it, so a client that
/// checks the binding and then accepts only that key knows the far end of its
/// TLS connection is the attested TD. The key is bound rather than the
/// certificate so the binding survives a renewal, which keeps the key.
/// Client and node must compute this identically, so both use this function.
#[must_use]
pub fn attest_tls_binding(inner: &[u8; 32], spki_sha256: &[u8; 32]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(ATTEST_TLS_BINDING_DOMAIN);
    hasher.update(inner);
    hasher.update(spki_sha256);
    hasher.finalize().into()
}

/// Report data bytes `32..64` for what a client asked the attest endpoint to
/// bind, nested in the order the endpoint applies them: `inner` (the key
/// binding, else the app hash), wrapped by the TLS-key binding, wrapped by the
/// transport-key binding. `None` when nothing is bound, which the endpoint
/// leaves as 32 zero bytes.
///
/// Client and node must nest the bindings identically, so both use this.
#[must_use]
pub fn attest_report_data_suffix(
    inner: Option<[u8; 32]>,
    tls_spki_sha256: Option<&[u8; 32]>,
    transport_key: Option<&[u8; 32]>,
) -> Option<[u8; 32]> {
    let inner = match tls_spki_sha256 {
        Some(spki) => Some(attest_tls_binding(&inner.unwrap_or([0; 32]), spki)),
        None => inner,
    };
    match transport_key {
        Some(key) => Some(attest_transport_binding(&inner.unwrap_or([0; 32]), key)),
        None => inner,
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

#[cfg(test)]
mod tests {
    use super::{
        attest_key_binding, attest_report_data_suffix, attest_tls_binding,
        attest_transport_binding, build_report_data,
    };

    #[test]
    fn the_key_binding_commits_to_the_key_and_the_app() {
        let key = [0x11; 32];
        let app = [0x22; 32];
        let bound = attest_key_binding(Some(&app), &key);

        assert_ne!(
            bound,
            attest_key_binding(Some(&app), &[0x12; 32]),
            "another key"
        );
        assert_ne!(
            bound,
            attest_key_binding(Some(&[0x23; 32]), &key),
            "another app"
        );
        assert_ne!(bound, attest_key_binding(None, &key), "no app at all");
        assert_ne!(
            bound, app,
            "never the bare app hash an unbound quote carries"
        );

        let report_data = build_report_data(&[0x33; 32], Some(&bound));
        assert_eq!(&report_data[32..], &bound);
    }

    #[test]
    fn the_transport_binding_commits_to_the_key_and_what_it_wraps() {
        let transport = [0x44; 32];
        let inner = attest_key_binding(Some(&[0x22; 32]), &[0x11; 32]);
        let bound = attest_transport_binding(&inner, &transport);

        assert_ne!(
            bound,
            attest_transport_binding(&inner, &[0x45; 32]),
            "another key"
        );
        assert_ne!(
            bound,
            attest_transport_binding(&[0u8; 32], &transport),
            "the key binding it wraps"
        );
        assert_ne!(bound, inner, "never the unwrapped binding");
    }

    /// Fixed vector, repeated verbatim in mero-js: the two sides are separate
    /// implementations of one binding, and this keeps them the same one.
    #[test]
    fn the_transport_binding_matches_the_published_vector() {
        assert_eq!(
            hex::encode(attest_transport_binding(&[0x11; 32], &[0x22; 32])),
            TRANSPORT_BINDING_VECTOR
        );
    }

    #[test]
    fn the_tls_binding_commits_to_the_key_and_what_it_wraps() {
        let spki = [0x55; 32];
        let inner = attest_key_binding(None, &[0x11; 32]);
        let bound = attest_tls_binding(&inner, &spki);

        assert_ne!(
            bound,
            attest_tls_binding(&inner, &[0x56; 32]),
            "another key"
        );
        assert_ne!(
            bound,
            attest_tls_binding(&[0u8; 32], &spki),
            "the binding it wraps"
        );
        assert_ne!(
            bound,
            attest_transport_binding(&inner, &spki),
            "never a transport binding of the same bytes"
        );
    }

    #[test]
    fn the_suffix_nests_the_tls_binding_inside_the_transport_binding() {
        let app = [0x22; 32];
        let spki = [0x55; 32];
        let transport = [0x44; 32];
        assert_eq!(attest_report_data_suffix(None, None, None), None);
        assert_eq!(attest_report_data_suffix(Some(app), None, None), Some(app));
        assert_eq!(
            attest_report_data_suffix(Some(app), Some(&spki), None),
            Some(attest_tls_binding(&app, &spki))
        );
        assert_eq!(
            attest_report_data_suffix(None, None, Some(&transport)),
            Some(attest_transport_binding(&[0; 32], &transport))
        );
        assert_eq!(
            attest_report_data_suffix(Some(app), Some(&spki), Some(&transport)),
            Some(attest_transport_binding(
                &attest_tls_binding(&app, &spki),
                &transport
            ))
        );
    }

    /// Fixed vector for clients that are not Rust, like the transport one.
    #[test]
    fn the_tls_binding_matches_the_published_vector() {
        assert_eq!(
            hex::encode(attest_tls_binding(&[0x11; 32], &[0x22; 32])),
            TLS_BINDING_VECTOR
        );
    }

    const TRANSPORT_BINDING_VECTOR: &str =
        "30274595433e8afc5d4035e30a2b599d93caf0d470867527f13dc6af92fa12a8";

    const TLS_BINDING_VECTOR: &str =
        "38250e671ba2f113657c78387ce29f56e3a0416e983c3fe9efcc0d48c0b199ee";
}
