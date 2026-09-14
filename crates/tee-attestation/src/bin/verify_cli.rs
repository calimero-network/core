//! Verify a TDX quote supplied by an untrusted caller, and report the verdict as JSON.
//!
//! Exists because a service that is not written in Rust cannot reach
//! [`calimero_tee_attestation::verify_attestation`], which is a library
//! function with no binary in front of it. mdma's manager is Python, and its
//! node-registration path (calimero-network/mdma#225) has to verify a quote a
//! node hands it — over the fleet API, from a caller it has no other reason to
//! trust.
//!
//! **The distinction this binary exists to preserve.** mdma today reads
//! measurements by slicing raw quote bytes at fixed offsets
//! (`shared/mdma_common/tee_quote_measurements.py`) and comparing them to an
//! allowlist. That is safe where it is used now — the dispatcher talks directly
//! to a VM it has just provisioned, so trust comes from the connection — but it
//! is not attestation. Applied to a quote an arbitrary caller submits, it is
//! satisfied by fabricating a blob with allowlisted bytes at the right offsets.
//! So the measurements reported here are read out of a quote whose signature
//! and certificate chain have been verified, and the caller is handed those
//! rather than being left to re-parse the bytes it was given.
//!
//! **`valid` is crypto/structural only.** It mirrors `VerificationResult::is_valid`:
//! signature + chain, nonce, and the mandatory app-hash binding. It deliberately
//! says nothing about whether the platform's TCB is acceptable or whether the
//! measurements are ones you allow — both are reported, and both are the
//! caller's to enforce. A `Revoked` platform still produces `valid: true`.
//!
//! **Exit codes distinguish "no" from "don't know",** which matters more here
//! than usual: collateral is fetched from Intel PCS over the network, so a
//! verdict and an outage must not look alike to the caller, or an outage
//! becomes an open door.
//!
//!   0 — a verdict was reached. Read `valid`; it may be `false`.
//!   1 — the input was unusable (bad JSON, bad hex, wrong lengths).
//!   2 — no verdict could be reached (quote unparseable, collateral fetch
//!       failed). NOT a rejection: fail closed, do not treat as invalid-and-move-on.
//!
//! Reads one JSON object on stdin so there is no argument parsing to get wrong,
//! and writes one JSON object on stdout:
//!
//! ```text
//! {"quote_b64": "...", "nonce_hex": "<64 hex>", "app_hash_hex": "<64 hex>"}
//!   -> {"valid": true, "quote_verified": true, "nonce_verified": true,
//!       "application_hash_verified": true, "tcb_status": "UpToDate",
//!       "advisory_ids": [], "tcb_evaluation_data_number": 17,
//!       "measurements": {"mrtd": "...", "rtmr0": "...", ...}}
//! ```
//!
//! `quote_hex` is accepted in place of `quote_b64`. The collateral endpoint is
//! whatever `CALIMERO_TEE_COLLATERAL_URL` names, as for any other caller of
//! this crate.

use std::io::Read;
use std::process::ExitCode;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use calimero_tee_attestation::verify_attestation;
use serde_json::{json, Value};

const EXIT_BAD_INPUT: u8 = 1;
const EXIT_NO_VERDICT: u8 = 2;

fn fail(code: u8, message: &str) -> ExitCode {
    // On stdout as JSON, not just stderr: the caller is a program, and an error
    // it can parse is one it can log and act on rather than guess at.
    println!("{}", json!({"error": message}));
    eprintln!("calimero-tee-verify: {message}");
    ExitCode::from(code)
}

fn hex32(value: Option<&str>, field: &str) -> Result<[u8; 32], String> {
    let raw = value.ok_or_else(|| format!("{field} is required"))?;
    let bytes = hex::decode(raw.trim()).map_err(|err| format!("{field} is not hex: {err}"))?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| format!("{field} must be 32 bytes (64 hex characters)"))
}

fn quote_bytes(request: &Value) -> Result<Vec<u8>, String> {
    if let Some(b64) = request.get("quote_b64").and_then(Value::as_str) {
        return BASE64
            .decode(b64.trim())
            .map_err(|err| format!("quote_b64 is not valid base64: {err}"));
    }
    if let Some(raw) = request.get("quote_hex").and_then(Value::as_str) {
        return hex::decode(raw.trim()).map_err(|err| format!("quote_hex is not hex: {err}"));
    }
    Err("one of quote_b64 or quote_hex is required".to_owned())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut input = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut input) {
        return fail(EXIT_BAD_INPUT, &format!("could not read stdin: {err}"));
    }

    let request: Value = match serde_json::from_str(&input) {
        Ok(value) => value,
        Err(err) => return fail(EXIT_BAD_INPUT, &format!("stdin is not JSON: {err}")),
    };

    let quote = match quote_bytes(&request) {
        Ok(bytes) => bytes,
        Err(err) => return fail(EXIT_BAD_INPUT, &err),
    };
    let nonce = match hex32(
        request.get("nonce_hex").and_then(Value::as_str),
        "nonce_hex",
    ) {
        Ok(value) => value,
        Err(err) => return fail(EXIT_BAD_INPUT, &err),
    };
    let app_hash = match hex32(
        request.get("app_hash_hex").and_then(Value::as_str),
        "app_hash_hex",
    ) {
        Ok(value) => value,
        Err(err) => return fail(EXIT_BAD_INPUT, &err),
    };

    // No mock path here on purpose. This binary exists to judge quotes from
    // callers the verifier does not trust, and a mock quote is valid by
    // construction — accepting one would mean a caller could hand over
    // something that verifies without any hardware behind it.
    let result = match verify_attestation(&quote, &nonce, &app_hash).await {
        Ok(result) => result,
        Err(err) => return fail(EXIT_NO_VERDICT, &format!("could not verify: {err}")),
    };

    let body = &result.quote.body;
    println!(
        "{}",
        json!({
            "valid": result.is_valid(),
            "quote_verified": result.quote_verified,
            "nonce_verified": result.nonce_verified,
            "application_hash_verified": result.application_hash_verified,
            "tcb_status": result.tcb_status,
            "advisory_ids": result.advisory_ids,
            "tcb_evaluation_data_number": result.tcb_evaluation_data_number,
            // From the VERIFIED quote. The caller compares these against its own
            // allowlist; this crate deliberately compares them against nothing.
            "measurements": {
                "mrtd": body.mrtd,
                "rtmr0": body.rtmr0,
                "rtmr1": body.rtmr1,
                "rtmr2": body.rtmr2,
                "rtmr3": body.rtmr3,
            },
        })
    );
    ExitCode::SUCCESS
}
