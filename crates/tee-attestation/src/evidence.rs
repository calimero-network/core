//! Offline verification of attestation **evidence**: a quote together with the
//! collateral it was appraised against and the time of that appraisal.
//!
//! [`verify_attestation`](crate::verify_attestation) fetches collateral from the
//! network and appraises the quote *now*. That suits a node deciding whether to
//! admit a TEE. It does not suit a check that every node must repeat and agree
//! on, such as the governance apply of a TEE's evidence. Collateral expires,
//! Intel revises TCB levels, and a node replaying the op log months later
//! would reach a different verdict, or none at all without network access.
//!
//! [`verify_evidence`] is a pure function of its inputs, so every node reaches
//! the same verdict. The collateral is signed by Intel, so whoever assembled the
//! evidence cannot forge it. What they do choose is `attested_at`, the moment
//! the collateral is judged at. It must fall inside that collateral's validity
//! window, and it fixes the TCB status as of that moment, not as of today.

use dcap_qvl::QuoteCollateralV3;
use tdx_quote::Quote as TdxQuote;

use calimero_server_primitives::admin::Quote;

use crate::error::AttestationError;
#[cfg(feature = "mock-attestation")]
use crate::generate::{create_mock_quote, is_mock_quote, MOCK_QUOTE_HEADER};

/// What an offline check of attestation evidence established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceVerdict {
    /// A mock quote, accepted only by a build with the `mock-attestation`
    /// feature. The caller decides whether its policy admits one.
    pub is_mock: bool,
    /// DCAP's TCB status for the platform as of `attested_at`, or `"Mock"`.
    pub tcb_status: String,
    /// The measurement registers, hex-encoded exactly as admission records them.
    pub mrtd: String,
    pub rtmr0: String,
    pub rtmr1: String,
    pub rtmr2: String,
    pub rtmr3: String,
}

/// Verify attestation evidence without the network.
///
/// Succeeds only when the quote is a genuine TDX quote whose signature chain
/// verifies against `collateral` at `attested_at` (seconds since the epoch), and
/// whose report data binds `bound_key_hash` in bytes `32..64`. That binding is
/// what ties the quote to one specific signing key.
///
/// Under the `mock-attestation` feature, a mock quote is accepted with
/// `collateral = None` and reported with `is_mock = true`. Without the feature,
/// a mock quote fails to parse like any other malformed quote.
///
/// # Errors
/// - `QuoteParsingFailed` if the quote does not parse;
/// - `CollateralFetchFailed` if a real quote comes without collateral;
/// - `QuoteVerificationFailed` if the signature chain does not verify;
/// - `ApplicationHashMismatch` if the quote binds a different key.
pub fn verify_evidence(
    quote_bytes: &[u8],
    collateral: Option<&QuoteCollateralV3>,
    attested_at: u64,
    bound_key_hash: &[u8; 32],
) -> Result<EvidenceVerdict, AttestationError> {
    #[cfg(feature = "mock-attestation")]
    if is_mock_quote(quote_bytes) {
        return verify_mock_evidence(quote_bytes, bound_key_hash);
    }

    let tdx_quote = TdxQuote::from_bytes(quote_bytes)
        .map_err(|err| AttestationError::QuoteParsingFailed(format!("{err:?}")))?;
    let collateral = collateral.ok_or_else(|| {
        AttestationError::CollateralFetchFailed(
            "evidence for a real quote must carry the collateral it was appraised against"
                .to_owned(),
        )
    })?;
    let report = dcap_qvl::verify::verify(quote_bytes, collateral, attested_at)
        .map_err(|err| AttestationError::QuoteVerificationFailed(format!("{err:?}")))?;

    check_binding(&tdx_quote.report_input_data()[32..64], bound_key_hash)?;

    let quote = Quote::try_from(tdx_quote)
        .map_err(|err| AttestationError::QuoteConversionFailed(err.to_string()))?;
    Ok(verdict(quote, report.status, false))
}

/// Fetch the collateral for `quote_bytes` from this node's configured source,
/// so it can travel with the quote as evidence.
///
/// # Errors
/// `CollateralFetchFailed` if the source is unusable or the fetch fails.
pub async fn fetch_collateral(quote_bytes: &[u8]) -> Result<QuoteCollateralV3, AttestationError> {
    let source = crate::verify::collateral_source();
    let client = dcap_qvl::collateral::CollateralClient::with_default_http(source.clone())
        .map_err(|err| {
            AttestationError::CollateralFetchFailed(format!(
                "collateral source {source:?}: {err:?}"
            ))
        })?;
    client.fetch(quote_bytes).await.map_err(|err| {
        AttestationError::CollateralFetchFailed(format!("collateral source {source:?}: {err:?}"))
    })
}

fn check_binding(actual: &[u8], expected: &[u8; 32]) -> Result<(), AttestationError> {
    if actual == expected {
        Ok(())
    } else {
        Err(AttestationError::ApplicationHashMismatch {
            expected: hex::encode(expected),
            actual: hex::encode(actual),
        })
    }
}

fn verdict(quote: Quote, tcb_status: String, is_mock: bool) -> EvidenceVerdict {
    EvidenceVerdict {
        is_mock,
        tcb_status,
        mrtd: quote.body.mrtd,
        rtmr0: quote.body.rtmr0,
        rtmr1: quote.body.rtmr1,
        rtmr2: quote.body.rtmr2,
        rtmr3: quote.body.rtmr3,
    }
}

#[cfg(feature = "mock-attestation")]
fn verify_mock_evidence(
    quote_bytes: &[u8],
    bound_key_hash: &[u8; 32],
) -> Result<EvidenceVerdict, AttestationError> {
    let header_len = MOCK_QUOTE_HEADER.len();
    let report_data: [u8; 64] = quote_bytes
        .get(header_len..header_len + 64)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| {
            AttestationError::QuoteParsingFailed("mock quote too short for report data".to_owned())
        })?;
    check_binding(&report_data[32..64], bound_key_hash)?;
    Ok(verdict(
        create_mock_quote(&report_data),
        "Mock".to_owned(),
        true,
    ))
}

#[cfg(test)]
mod tests {
    use dcap_qvl::QuoteCollateralV3;

    use super::verify_evidence;
    use crate::error::AttestationError;

    // A real TDX quote and the Intel-signed collateral for it, from dcap-qvl's
    // own test samples (MIT, Phala Network).
    const QUOTE: &[u8] = include_bytes!("../tests/fixtures/tdx_quote");
    const COLLATERAL: &[u8] = include_bytes!("../tests/fixtures/tdx_quote_collateral.json");
    // A moment inside that collateral's validity window.
    const ATTESTED_AT: u64 = 1_751_000_000; // 2025-06-27

    fn collateral() -> QuoteCollateralV3 {
        serde_json::from_slice(COLLATERAL).unwrap()
    }

    fn bound_hash() -> [u8; 32] {
        let quote = tdx_quote::Quote::from_bytes(QUOTE).unwrap();
        quote.report_input_data()[32..64].try_into().unwrap()
    }

    #[test]
    fn genuine_evidence_verifies_and_reports_its_measurements() {
        let verdict = verify_evidence(QUOTE, Some(&collateral()), ATTESTED_AT, &bound_hash())
            .expect("the sample quote verifies against its own collateral");
        assert!(!verdict.is_mock);
        assert_eq!(verdict.tcb_status, "UpToDate");
        assert_eq!(verdict.mrtd.len(), 96, "MRTD is 48 bytes, hex-encoded");
    }

    #[test]
    fn evidence_bound_to_another_key_is_refused() {
        let result = verify_evidence(QUOTE, Some(&collateral()), ATTESTED_AT, &[0x42; 32]);
        assert!(matches!(
            result,
            Err(AttestationError::ApplicationHashMismatch { .. })
        ));
    }

    #[test]
    fn a_tampered_quote_is_refused() {
        let mut tampered = QUOTE.to_vec();
        // A byte inside the signed TD report body.
        tampered[200] ^= 0x01;
        let result = verify_evidence(&tampered, Some(&collateral()), ATTESTED_AT, &bound_hash());
        assert!(
            result.is_err(),
            "an edited quote must not verify, got {result:?}"
        );
    }

    #[test]
    fn a_real_quote_without_collateral_is_refused() {
        let result = verify_evidence(QUOTE, None, ATTESTED_AT, &bound_hash());
        assert!(matches!(
            result,
            Err(AttestationError::CollateralFetchFailed(_))
        ));
    }

    #[test]
    fn evidence_judged_outside_the_collateral_window_is_refused() {
        let result = verify_evidence(QUOTE, Some(&collateral()), 1, &bound_hash());
        assert!(matches!(
            result,
            Err(AttestationError::QuoteVerificationFailed(_))
        ));
    }

    #[cfg(feature = "mock-attestation")]
    #[test]
    fn mock_evidence_verifies_only_for_the_key_it_binds() {
        let key_hash = [0x5A; 32];
        let report_data = crate::build_report_data(&[0x01; 32], Some(&key_hash));
        let quote = crate::generate_mock_attestation(report_data).quote_bytes;

        let verdict = verify_evidence(&quote, None, ATTESTED_AT, &key_hash).unwrap();
        assert!(verdict.is_mock);
        assert_eq!(verdict.tcb_status, "Mock");
        assert!(matches!(
            verify_evidence(&quote, None, ATTESTED_AT, &[0x5B; 32]),
            Err(AttestationError::ApplicationHashMismatch { .. })
        ));
    }
}
