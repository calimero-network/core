//! TDX quote verification.

use calimero_server_primitives::admin::Quote;
use dcap_qvl::collateral::get_collateral_from_pcs;
use dcap_qvl::verify::verify;
use tdx_quote::Quote as TdxQuote;
#[cfg(feature = "mock-attestation")]
use tracing::warn;
use tracing::{error, info};

use crate::error::AttestationError;
#[cfg(feature = "mock-attestation")]
use crate::generate::{is_mock_quote, MOCK_QUOTE_HEADER};

/// Result of verifying a TEE attestation.
///
/// This is a *report* of the crypto/structural checks the verifier performed
/// plus the raw material a caller needs to make a policy decision. It is NOT an
/// authorization verdict on its own. In particular, `tcb_status`, `advisory_ids`
/// and the measurement registers in `quote.body` (`mrtd` / `rtmr0..3` /
/// `mrsigner`) are surfaced here precisely *because* the caller — not this crate
/// — is expected to enforce a policy over them. See [`Self::is_valid`].
#[derive(Debug, Clone)]
pub struct VerificationResult {
    /// Whether the quote's cryptographic signature is valid.
    pub quote_verified: bool,
    /// Whether the nonce in the report data matches the expected value.
    pub nonce_verified: bool,
    /// Whether the application hash matches the expected (mandatory) value.
    pub application_hash_verified: bool,
    /// TCB status reported by DCAP verification (for policy decisions).
    ///
    /// NOT consulted by [`Self::is_valid`]. A crypto-valid quote from an
    /// `OutOfDate`, `SWHardeningNeeded`, or `Revoked` platform still reports
    /// `quote_verified == true`; the caller must gate on this field.
    pub tcb_status: Option<String>,
    /// Advisory IDs reported by DCAP verification.
    ///
    /// NOT consulted by [`Self::is_valid`]; provided for caller-side policy.
    pub advisory_ids: Vec<String>,
    /// The parsed quote structure.
    ///
    /// Its `body` carries the measurement registers (`mrtd` / `rtmr0..3` /
    /// `mrsigner`). These are parsed but NOT compared against anything by this
    /// crate — matching them to an allowlist is the caller's responsibility.
    pub quote: Quote,
}

impl VerificationResult {
    /// Crypto/structural validity ONLY — this is **not** an authorization
    /// decision.
    ///
    /// Returns `true` iff all three built-in checks passed:
    /// - `quote_verified` — DCAP signature + certificate chain verified against
    ///   Intel PCS collateral (for a mock quote this is unconditionally `true`);
    /// - `nonce_verified` — `report_data[0..32]` matched the challenge nonce
    ///   (anti-replay);
    /// - `application_hash_verified` — `report_data[32..64]` matched the
    ///   mandatory app/identity binding.
    ///
    /// It deliberately does **not** consider, and a caller therefore MUST
    /// enforce on top of it:
    /// - **`tcb_status` / `advisory_ids`** — a cryptographically valid quote
    ///   from an `OutOfDate`, `SWHardeningNeeded`, or even `Revoked` platform
    ///   still returns `quote_verified == true`, so `is_valid()` returns `true`.
    ///   Gate on `tcb_status` against an allowlist, failing closed on an empty
    ///   allowlist and on `Revoked`.
    /// - **Measurement registers `mrtd` / `rtmr0..3` / `mrsigner`** (in
    ///   `self.quote.body`) — parsed but never compared here. Check them against
    ///   a measurement allowlist to establish *which* workload was attested; a
    ///   valid signature only proves *some* genuine TDX platform produced the
    ///   quote, not that it is the approved one.
    ///
    /// # Never admit on `is_valid()` alone
    /// Admitting a peer, releasing a key, or granting any capability on
    /// `is_valid()` without the TCB + measurement gate trusts *any* well-formed
    /// TDX platform rather than a specific approved one.
    ///
    /// New callers should use [`Self::policy_valid`] instead: it is the
    /// safe-by-construction gate that folds these crypto checks together with a
    /// fail-closed TCB allowlist, the mock decision, the measurement
    /// allowlists, and the app-hash binding, and returns a typed
    /// [`crate::PolicyRejection`].
    ///
    /// The existing enforcement layers below keep their own equivalent
    /// enforcement by design (mode-specific errors `policy_valid` cannot
    /// express), and `is_valid()` stays crypto-only for them:
    /// - `crates/context/src/handlers/admit_tee_node.rs` — per-group
    ///   `TeeAdmissionPolicy` (MRTD/RTMR allowlists + `tcb_status_allowed`);
    /// - `crates/merod/src/kms/mod.rs` — `enforce_attestation_policy`
    ///   (measurement + TCB allowlists for KMS self-attestation);
    /// - the mero-tee KMS `get_key.rs` `enforce_attestation_policy` on the
    ///   key-release side (external consumer of this crate).
    pub fn is_valid(&self) -> bool {
        self.quote_verified && self.nonce_verified && self.application_hash_verified
    }
}

/// Verify a TDX attestation quote.
///
/// This performs **crypto/structural verification only**: it checks the DCAP
/// signature/collateral, matches the nonce, and matches the mandatory app-hash
/// binding. It does **not** make an authorization decision. The returned
/// [`VerificationResult`] carries `tcb_status`, `advisory_ids`, and the parsed
/// measurement registers (`quote.body.mrtd` / `rtmr0..3`) so the caller can
/// apply its own policy — see [`VerificationResult::is_valid`] for the full
/// contract and the list of enforcement call sites.
///
/// # Caller contract
/// A successful (`is_valid() == true`) result means only that *some* genuine TDX
/// platform produced a fresh quote bound to `expected_app_hash`. Callers MUST
/// additionally enforce:
/// - a `tcb_status` allowlist (fail closed on empty allowlist and on `Revoked`);
/// - a measurement allowlist over `mrtd` / `rtmr0..3` to pin *which* workload;
/// - the mock-vs-real acceptance policy (`is_mock_quote` routes here vs.
///   `verify_mock_attestation`; this function never runs for mock quotes).
///
/// # Arguments
/// * `quote_bytes` - Raw quote bytes to verify.
/// * `nonce` - Expected 32-byte nonce that should be in report_data[0..32].
/// * `expected_app_hash` - Mandatory expected 32-byte app hash that must be in report_data[32..64].
///   Binding the attestation to an application/identity is required; there is no
///   "skip" path that would otherwise leave the quote unbound yet considered valid.
///   (An earlier `Option`-based signature that defaulted the check open via
///   `unwrap_or(true)` has been removed — the binding is now unconditional.)
///
/// # Returns
/// A `VerificationResult` with the verification status for each check.
///
/// # Errors
/// Returns an error if the quote cannot be parsed or if collateral fetch fails.
pub async fn verify_attestation(
    quote_bytes: &[u8],
    nonce: &[u8; 32],
    expected_app_hash: &[u8; 32],
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

    // Verify application hash matches report_data[32..64] (mandatory binding)
    let actual_hash = &report_data[32..64];
    let application_hash_verified = actual_hash == expected_app_hash;
    if application_hash_verified {
        info!("Application hash verification: PASSED");
    } else {
        error!(
            expected=%hex::encode(expected_app_hash),
            actual=%hex::encode(actual_hash),
            "Application hash verification: FAILED"
        );
    }

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
/// * `expected_app_hash` - Mandatory expected 32-byte app hash.
///
/// # Returns
/// A `VerificationResult` where `quote_verified` is always `true` for mock quotes
/// (since there's no cryptographic signature to verify).
///
/// # Security Warning
/// This function bypasses all cryptographic verification. It should ONLY be used
/// for development and testing purposes.
#[cfg(feature = "mock-attestation")]
pub fn verify_mock_attestation(
    quote_bytes: &[u8],
    nonce: &[u8; 32],
    expected_app_hash: &[u8; 32],
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

    // Verify application hash matches report_data[32..64] (mandatory binding)
    let actual_hash = &report_data[32..64];
    let application_hash_verified = actual_hash == expected_app_hash;
    if application_hash_verified {
        info!("Application hash verification: PASSED");
    } else {
        error!(
            expected=%hex::encode(expected_app_hash),
            actual=%hex::encode(actual_hash),
            "Application hash verification: FAILED"
        );
    }

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

    if result.is_valid() {
        info!("Overall mock verification: PASSED");
    } else {
        warn!("Overall mock verification: FAILED (nonce or app_hash mismatch)");
    }

    Ok(result)
}
