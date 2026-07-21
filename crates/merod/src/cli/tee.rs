use calimero_config::ConfigFile;
use calimero_tee_attestation::{
    build_report_data, generate_attestation, verify_attestation, AttestationError,
    VerificationResult,
};
#[cfg(feature = "mock-attestation")]
use calimero_tee_attestation::{generate_mock_attestation, verify_mock_attestation};
use clap::{Parser, Subcommand};
use eyre::{bail, eyre, Result as EyreResult, WrapErr};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
#[cfg(feature = "mock-attestation")]
use tracing::warn;

use crate::cli::RootArgs;

#[derive(Debug, Parser)]
pub struct TeeCommand {
    #[command(subcommand)]
    action: TeeSubcommands,
}

#[derive(Debug, Subcommand)]
enum TeeSubcommands {
    /// Self-test the node's TDX attestation generate + verify round-trip
    Probe(TeeProbeCommand),
}

#[derive(Debug, Parser)]
pub struct TeeProbeCommand {
    /// DEV/TEST ONLY. Produce and verify a MOCK TEE quote (no real TDX).
    /// Insecure — never use in production. Refuses to run alongside a real KMS.
    /// CLI-only flag (no env inheritance); only present under the default-off
    /// `mock-attestation` build feature.
    #[cfg(feature = "mock-attestation")]
    #[clap(long, default_value_t = false)]
    mock_tee: bool,
    /// Emit machine-readable probe result as JSON
    #[arg(long, default_value_t = false)]
    json: bool,
}

/// Outcome of a single probe check.
///
/// Every check carries a `passed` boolean; a follow-up mero-tee CI probe script
/// parses this shape, so the field names and nesting are a stable contract.
#[derive(Debug, Serialize)]
struct TeeProbeResult {
    /// `"success"` when every check met its expectation, otherwise `"failure"`.
    outcome: &'static str,
    /// True when the quote under test was a (cryptographically invalid) mock
    /// quote. CI must reject a probe result with `is_mock: true`.
    is_mock: bool,
    checks: TeeProbeChecks,
}

#[derive(Debug, Serialize)]
struct TeeProbeChecks {
    /// Correct nonce + correct expected app hash must verify as valid.
    positive: PositiveCheck,
    /// A different nonce than embedded must NOT verify (nonce check fails).
    wrong_nonce: WrongNonceCheck,
    /// A mutated quote must be rejected (verify errors or is not valid).
    tampered_quote: TamperedQuoteCheck,
}

#[derive(Debug, Serialize)]
struct PositiveCheck {
    passed: bool,
    quote_verified: bool,
    nonce_verified: bool,
    application_hash_verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tcb_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct WrongNonceCheck {
    passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce_verified: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct TamperedQuoteCheck {
    passed: bool,
    /// True when verification rejected the tampered quote (errored or invalid).
    rejected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl TeeCommand {
    pub async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        match self.action {
            TeeSubcommands::Probe(command) => command.run(root_args).await,
        }
    }
}

impl TeeProbeCommand {
    async fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        let json = self.json;
        let path = root_args.home.join(&root_args.node_name);
        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let config = ConfigFile::load(&path)
            .await
            .wrap_err("Failed to load node configuration")?;

        // Mock TEE is dev/test only and must never coexist with real attestation.
        // Same deny-guard as `merod run --mock-tee`: refuse a mock probe on a node
        // configured for real KMS attestation, so a mock "success" can never be
        // mistaken for real-hardware assurance. Only compiled with the
        // `mock-attestation` feature (the flag exists only there).
        #[cfg(feature = "mock-attestation")]
        if self.mock_tee
            && config
                .tee
                .as_ref()
                .is_some_and(calimero_config::TeeConfig::has_real_attestation)
        {
            bail!(
                "--mock-tee refused: a real KMS/attestation is configured. \
                 Mock TEE is dev/test only and cannot coexist with real attestation."
            );
        }

        // Bind the attestation to this node's identity: report_data is
        // nonce[0..32] || SHA256(peer_id)[32..64], mirroring how the real
        // attestation/KMS paths bind a quote to the node.
        let peer_id = config.identity.keypair.public().to_peer_id().to_base58();
        let app_hash: [u8; 32] = Sha256::digest(peer_id.as_bytes()).into();

        let mut nonce = [0u8; 32];
        OsRng.fill_bytes(&mut nonce);

        // Generate a fresh quote via the SAME code path as `/tee/attest`:
        // real TDX hardware unless `--mock-tee` explicitly opts into a mock quote.
        let report_data = build_report_data(&nonce, Some(&app_hash));
        #[cfg(feature = "mock-attestation")]
        let attestation = if self.mock_tee {
            warn!("Generating MOCK TEE attestation for probe — INSECURE, DEV/TEST ONLY");
            generate_mock_attestation(report_data)
        } else {
            generate_attestation(report_data)
                .wrap_err("Failed to generate TDX attestation for TEE probe")?
        };
        #[cfg(not(feature = "mock-attestation"))]
        let attestation = generate_attestation(report_data)
            .wrap_err("Failed to generate TDX attestation for TEE probe")?;

        // A mock result outside `--mock-tee` means the platform has no real TDX
        // hardware. Do not silently pass — this mirrors `/tee/attest` returning
        // NOT_IMPLEMENTED on an unsupported platform. Without the
        // `mock-attestation` feature any mock result is always rejected.
        #[cfg(feature = "mock-attestation")]
        let reject_mock = attestation.is_mock && !self.mock_tee;
        #[cfg(not(feature = "mock-attestation"))]
        let reject_mock = attestation.is_mock;
        if reject_mock {
            bail!(
                "TEE is not available on this node: attestation produced a mock quote, \
                 which means no real TDX hardware is present. Refusing to report success. \
                 (Use --mock-tee for a dev/test self-check on non-TDX platforms.)"
            );
        }

        let result = run_probe_checks(
            &attestation.quote_bytes,
            nonce,
            app_hash,
            attestation.is_mock,
        )
        .await;

        print_result(json, &result)?;

        if result.outcome == "success" {
            return Ok(());
        }

        bail!("TEE probe failed: one or more attestation self-checks did not meet expectation");
    }
}

/// Verify a quote via the mock or real path depending on how it was generated.
async fn verify_quote(
    quote_bytes: &[u8],
    nonce: &[u8; 32],
    app_hash: &[u8; 32],
    is_mock: bool,
) -> Result<VerificationResult, AttestationError> {
    #[cfg(feature = "mock-attestation")]
    if is_mock {
        return verify_mock_attestation(quote_bytes, nonce, app_hash);
    }
    // Without the `mock-attestation` feature there is no mock verify path.
    #[cfg(not(feature = "mock-attestation"))]
    let _ = is_mock;
    verify_attestation(quote_bytes, nonce, app_hash).await
}

/// Classify a verify error for the tampered-quote check.
///
/// Returns `true` only when the error is a cryptographic/parse rejection of the
/// quote itself — meaning the tamper was genuinely detected. This covers a
/// corrupted real quote (`QuoteParsingFailed` / `QuoteConversionFailed` /
/// `QuoteVerificationFailed`), the mock verifier's "missing MOCK_TDX_QUOTE_V1
/// header" rejection (surfaced as `QuoteParsingFailed`), and binding mismatches.
///
/// Returns `false` for infrastructure/transient failures — most importantly
/// `CollateralFetchFailed` (an Intel PCS network/DCAP flake) — which say nothing
/// about tamper detection and must therefore fail the probe as inconclusive
/// rather than be miscounted as a successful rejection.
///
/// The match is exhaustive on purpose: a new `AttestationError` variant will fail
/// to compile here until it is explicitly classified as rejection or infra.
fn is_quote_rejection_error(err: &AttestationError) -> bool {
    match err {
        AttestationError::QuoteParsingFailed(_)
        | AttestationError::QuoteConversionFailed(_)
        | AttestationError::QuoteVerificationFailed(_)
        | AttestationError::NonceMismatch { .. }
        | AttestationError::ApplicationHashMismatch { .. } => true,
        AttestationError::CollateralFetchFailed(_)
        | AttestationError::NotSupported
        | AttestationError::SystemTimeError(_)
        | AttestationError::QuoteGenerationFailed(_)
        | AttestationError::InvalidNonce(_)
        | AttestationError::InvalidApplicationHash(_)
        | AttestationError::InfoRetrievalFailed(_) => false,
    }
}

/// Corrupt one byte of the quote so verification must reject it.
fn tamper_quote(quote_bytes: &[u8], is_mock: bool) -> Vec<u8> {
    let mut tampered = quote_bytes.to_vec();
    // Pick a byte whose corruption is guaranteed to be detected:
    // - real quote: a byte in the middle lands in the signed TD report body or
    //   the ECDSA signature, so DCAP cryptographic verification fails.
    // - mock quote: the middle is zero padding, so corrupt the magic header
    //   instead, which makes verify_mock_attestation reject it as non-mock.
    let idx = if is_mock { 0 } else { tampered.len() / 2 };
    if let Some(byte) = tampered.get_mut(idx) {
        *byte ^= 0xFF;
    }
    tampered
}

/// Run the three attestation self-checks against a freshly generated quote.
///
/// Pure orchestration over `calimero_tee_attestation`: no config or process
/// exit, so it is unit-testable via the mock quote path without real TDX.
async fn run_probe_checks(
    quote_bytes: &[u8],
    nonce: [u8; 32],
    app_hash: [u8; 32],
    is_mock: bool,
) -> TeeProbeResult {
    // positive: correct nonce + correct app hash must verify as valid.
    let positive = match verify_quote(quote_bytes, &nonce, &app_hash, is_mock).await {
        Ok(res) => PositiveCheck {
            passed: res.is_valid(),
            quote_verified: res.quote_verified,
            nonce_verified: res.nonce_verified,
            application_hash_verified: res.application_hash_verified,
            tcb_status: res.tcb_status,
            error: None,
        },
        Err(err) => PositiveCheck {
            passed: false,
            quote_verified: false,
            nonce_verified: false,
            application_hash_verified: false,
            tcb_status: None,
            error: Some(err.to_string()),
        },
    };

    // wrong_nonce: verifying against a different nonce must fail the nonce check.
    let mut altered_nonce = nonce;
    altered_nonce[0] ^= 0xFF;
    let wrong_nonce = match verify_quote(quote_bytes, &altered_nonce, &app_hash, is_mock).await {
        Ok(res) => WrongNonceCheck {
            passed: !res.nonce_verified,
            nonce_verified: Some(res.nonce_verified),
            error: None,
        },
        Err(err) => WrongNonceCheck {
            passed: false,
            nonce_verified: None,
            error: Some(err.to_string()),
        },
    };

    // tampered_quote: a mutated quote must be rejected. A rejection is either
    // `Ok(result)` that is not valid, or an `Err` whose variant is a
    // cryptographic/parse rejection of the quote itself. An infrastructure or
    // transient error (e.g. Intel PCS collateral fetch) is INCONCLUSIVE — it
    // cannot confirm tamper detection, so it must fail the probe rather than be
    // counted as a pass (otherwise a network flake masks a real regression).
    let tampered_bytes = tamper_quote(quote_bytes, is_mock);
    let tampered_quote = match verify_quote(&tampered_bytes, &nonce, &app_hash, is_mock).await {
        Ok(res) => {
            let rejected = !res.is_valid();
            TamperedQuoteCheck {
                passed: rejected,
                rejected,
                error: None,
            }
        }
        Err(err) => {
            let rejected = is_quote_rejection_error(&err);
            TamperedQuoteCheck {
                passed: rejected,
                rejected,
                error: Some(err.to_string()),
            }
        }
    };

    let outcome = if positive.passed && wrong_nonce.passed && tampered_quote.passed {
        "success"
    } else {
        "failure"
    };

    TeeProbeResult {
        outcome,
        is_mock,
        checks: TeeProbeChecks {
            positive,
            wrong_nonce,
            tampered_quote,
        },
    }
}

fn print_result(json: bool, result: &TeeProbeResult) -> EyreResult<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(result)
                .map_err(|err| eyre!("Failed to serialize TEE probe result: {err}"))?
        );
        return Ok(());
    }

    let checks = &result.checks;
    if result.outcome == "success" {
        println!(
            "TEE probe succeeded (is_mock={}): positive=pass wrong_nonce=pass tampered_quote=pass",
            result.is_mock
        );
    } else {
        eprintln!(
            "TEE probe FAILED (is_mock={}): positive={} wrong_nonce={} tampered_quote={}",
            result.is_mock,
            pass_str(checks.positive.passed),
            pass_str(checks.wrong_nonce.passed),
            pass_str(checks.tampered_quote.passed),
        );
        if let Some(err) = checks.positive.error.as_deref() {
            eprintln!("  positive error: {err}");
        }
        if let Some(err) = checks.wrong_nonce.error.as_deref() {
            eprintln!("  wrong_nonce error: {err}");
        }
        if let Some(err) = checks.tampered_quote.error.as_deref() {
            eprintln!("  tampered_quote error: {err}");
        }
    }

    Ok(())
}

fn pass_str(passed: bool) -> &'static str {
    if passed {
        "pass"
    } else {
        "fail"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "mock-attestation")]
    #[tokio::test]
    async fn probe_checks_pass_for_wellformed_mock_quote() {
        let nonce = [7u8; 32];
        let app_hash = [9u8; 32];
        let report_data = build_report_data(&nonce, Some(&app_hash));
        let attestation = generate_mock_attestation(report_data);

        let result = run_probe_checks(&attestation.quote_bytes, nonce, app_hash, true).await;

        assert_eq!(result.outcome, "success");
        assert!(result.is_mock);
        assert!(result.checks.positive.passed);
        assert!(result.checks.positive.nonce_verified);
        assert!(result.checks.positive.application_hash_verified);
        // wrong nonce must fail the nonce check.
        assert!(result.checks.wrong_nonce.passed);
        assert_eq!(result.checks.wrong_nonce.nonce_verified, Some(false));
        // tampering must be rejected.
        assert!(result.checks.tampered_quote.passed);
        assert!(result.checks.tampered_quote.rejected);
    }

    #[cfg(feature = "mock-attestation")]
    #[tokio::test]
    async fn probe_positive_fails_on_app_hash_mismatch() {
        let nonce = [1u8; 32];
        let embedded_app_hash = [2u8; 32];
        let report_data = build_report_data(&nonce, Some(&embedded_app_hash));
        let attestation = generate_mock_attestation(report_data);

        // Verify against a DIFFERENT expected app hash than was embedded.
        let expected_app_hash = [3u8; 32];
        let result =
            run_probe_checks(&attestation.quote_bytes, nonce, expected_app_hash, true).await;

        assert_eq!(result.outcome, "failure");
        assert!(!result.checks.positive.passed);
        assert!(!result.checks.positive.application_hash_verified);
    }

    #[test]
    fn infra_errors_are_not_counted_as_tamper_rejections() {
        // A collateral fetch flake (or any other non-rejection variant) must not
        // be classified as "tamper detected" — it is inconclusive.
        assert!(!is_quote_rejection_error(
            &AttestationError::CollateralFetchFailed("Intel PCS unreachable".to_owned())
        ));
        assert!(!is_quote_rejection_error(&AttestationError::NotSupported));
        assert!(!is_quote_rejection_error(
            &AttestationError::SystemTimeError("clock skew".to_owned())
        ));
        assert!(!is_quote_rejection_error(
            &AttestationError::QuoteGenerationFailed("tsm busy".to_owned())
        ));
    }

    #[test]
    fn parse_and_crypto_errors_are_tamper_rejections() {
        // A corrupted mock quote surfaces as QuoteParsingFailed.
        assert!(is_quote_rejection_error(
            &AttestationError::QuoteParsingFailed(
                "Not a valid mock quote - missing MOCK_TDX_QUOTE_V1 header".to_owned()
            )
        ));
        assert!(is_quote_rejection_error(
            &AttestationError::QuoteVerificationFailed("bad signature".to_owned())
        ));
        assert!(is_quote_rejection_error(
            &AttestationError::QuoteConversionFailed("malformed body".to_owned())
        ));
        assert!(is_quote_rejection_error(&AttestationError::NonceMismatch {
            expected: "aa".to_owned(),
            actual: "bb".to_owned(),
        }));
    }

    #[test]
    fn tamper_quote_mutates_a_byte() {
        let original = vec![0xAAu8; 256];
        let mock = tamper_quote(&original, true);
        assert_ne!(mock[0], original[0]);
        let real = tamper_quote(&original, false);
        assert_ne!(real[original.len() / 2], original[original.len() / 2]);
    }
}
