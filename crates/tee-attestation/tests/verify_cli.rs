//! The `calimero-tee-verify` exit-code contract.
//!
//! The verdict itself needs real TDX hardware and Intel PCS collateral, so what
//! is testable here is the part a caller's security actually rests on: that
//! "this quote is not valid" and "I could not tell" are distinguishable.
//!
//! mdma fails closed on the second (calimero-network/mdma#225). If a collateral
//! fetch failure or an unparseable blob came back as `valid: false` with exit 0,
//! an Intel PCS outage would be indistinguishable from a rejection — and a
//! caller that treats "not valid" as "reject and carry on" would keep serving
//! while verifying nothing.
#![cfg(feature = "cli")]

use std::io::Write;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_calimero-tee-verify");

fn run(stdin: &str) -> (i32, String) {
    let mut child = Command::new(BIN)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn calimero-tee-verify");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().expect("exit code"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn hex32(byte: &str) -> String {
    byte.repeat(32)
}

#[test]
fn unusable_input_exits_one() {
    let short_nonce = format!(
        r#"{{"quote_hex":"00","nonce_hex":"abcd","app_hash_hex":"{}"}}"#,
        hex32("bb")
    );
    for body in [
        "not json",
        r#"{"nonce_hex":"x","app_hash_hex":"y"}"#,
        short_nonce.as_str(),
    ] {
        let (code, stdout) = run(body);
        assert_eq!(code, 1, "input {body:?} should be rejected as unusable");
        assert!(stdout.contains("error"), "an error is reported as JSON");
    }
}

#[test]
fn a_quote_that_cannot_be_judged_exits_two_rather_than_reporting_invalid() {
    // The distinction the caller's fail-closed behaviour depends on: this is
    // "no verdict", NOT "verdict: invalid".
    let (code, stdout) = run(&format!(
        r#"{{"quote_hex":"deadbeef","nonce_hex":"{}","app_hash_hex":"{}"}}"#,
        hex32("aa"),
        hex32("bb")
    ));
    assert_eq!(code, 2, "an unjudgeable quote must not exit 0 or 1");
    assert!(
        !stdout.contains("\"valid\""),
        "no verdict may be reported when none was reached, got: {stdout}"
    );
}

#[test]
fn a_mock_quote_is_not_accepted() {
    // This binary judges quotes from callers it does not trust, and a mock
    // quote is valid by construction. Accepting one would let a caller present
    // something that verifies with no hardware behind it.
    let mock = hex::encode(b"MOCK_TDX_QUOTE_V1_not_a_real_quote_at_all_padding");
    let (code, _) = run(&format!(
        r#"{{"quote_hex":"{mock}","nonce_hex":"{}","app_hash_hex":"{}"}}"#,
        hex32("aa"),
        hex32("bb")
    ));
    assert_ne!(code, 0, "a mock quote must never produce a verdict here");
}
