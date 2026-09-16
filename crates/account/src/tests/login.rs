//! What a [`LoginStatement`] establishes, and the four substitutions it refuses.
//!
//! The signature is the easy half. The tests that matter here are the ones where
//! the signature is perfectly valid and the statement is still wrong: a session
//! key swapped for the attacker's, a statement replayed at another node, or one
//! collected from a browser and presented by a native client. Each of those is a
//! working credential in the wrong place, which is the shape of the bug a login
//! exchange exists to prevent.

use calimero_primitives::identity::PublicKey;

use super::support::key;
use crate::login::{Audience, LoginStatement};

/// Fixed inputs, so a failure names the change rather than the fixture.
fn statement() -> LoginStatement {
    LoginStatement::sign(
        &key(3),             // the device signing key
        key(9).public_key(), // the node
        Audience::WebOrigin("https://app.example".into()),
        [0x5a; 32],          // the challenge
        key(4).public_key(), // the session key
        1_800_000_000,
        1_800_003_600,
    )
    .expect("sign")
}

#[test]
fn a_statement_verifies_against_the_device_key_it_names() {
    let s = statement();
    assert_eq!(
        s.device_key,
        key(3).public_key(),
        "the device key must be derived from the secret, never taken on trust"
    );
    assert!(s.verify_signature().is_ok());
}

/// The substitution the `session_key` field exists to stop.
///
/// An attacker who intercepts a statement and swaps in their own session key
/// would otherwise hold a session the victim's device authorized. The field is
/// inside the signed payload precisely so this fails.
#[test]
fn swapping_the_session_key_breaks_the_signature() {
    let mut s = statement();
    s.session_key = key(0x77).public_key();
    assert!(s.verify_signature().is_err());
}

/// The same, for the key the signature is checked against.
///
/// Without this, an attacker could re-point a statement at their own device key
/// and the "signature" would be checked against the wrong key entirely.
#[test]
fn swapping_the_device_key_breaks_the_signature() {
    let mut s = statement();
    s.device_key = key(0x77).public_key();
    assert!(s.verify_signature().is_err());
}

#[test]
fn swapping_the_challenge_breaks_the_signature() {
    let mut s = statement();
    s.challenge = [0x5b; 32];
    assert!(s.verify_signature().is_err());
}

/// A valid statement presented to the wrong node is refused, and says so.
///
/// This is the one that must not collapse into a signature error: the signature
/// is genuine, and the client's mistake is that it pinned the wrong node.
#[test]
fn a_statement_for_another_node_is_refused_as_a_node_mismatch() {
    let s = statement();
    let other: PublicKey = key(0x21).public_key();
    let err = s
        .addressed_to(&other, &Audience::WebOrigin("https://app.example".into()))
        .expect_err("a statement for another node must be refused");
    assert!(
        err.to_string().contains("different node"),
        "expected a node mismatch, got: {err}"
    );
    // The signature itself is untouched — that is the point.
    assert!(s.verify_signature().is_ok());
}

/// A statement collected from one client surface must not be usable from another.
#[test]
fn a_statement_for_another_audience_is_refused() {
    let s = statement();
    let err = s
        .addressed_to(&key(9).public_key(), &Audience::Cli)
        .expect_err("a statement for another audience must be refused");
    assert!(
        err.to_string().contains("different audience"),
        "expected an audience mismatch, got: {err}"
    );
}

#[test]
fn the_right_node_and_audience_are_accepted() {
    let s = statement();
    assert!(s
        .addressed_to(
            &key(9).public_key(),
            &Audience::WebOrigin("https://app.example".into())
        )
        .is_ok());
}

/// Two audiences carrying the same string must not produce the same preimage.
///
/// Without the variant tag in the signing bytes, a statement minted for the web
/// origin `"x"` would verify as one minted for the code-signing id `"x"` — the
/// audience would bind the string while binding nothing about the surface.
#[test]
fn audiences_with_equal_payloads_do_not_share_a_preimage() {
    let common = "x".to_owned();
    let web = LoginStatement::signing_payload(
        &key(9).public_key(),
        &Audience::WebOrigin(common.clone()),
        &[0u8; 32],
        &key(4).public_key(),
        &key(3).public_key(),
        0,
        0,
    );
    let native = LoginStatement::signing_payload(
        &key(9).public_key(),
        &Audience::CodeSigningId(common),
        &[0u8; 32],
        &key(4).public_key(),
        &key(3).public_key(),
        0,
        0,
    );
    assert_ne!(web, native);
}

/// A login statement and a warrant are signed by the same KIND of key, so their
/// domains must not collide — otherwise a statement minted to log in could be
/// presented as a warrant authorizing a write.
///
/// `signing_domains_are_pairwise_distinct` covers the constants; this covers the
/// payloads they produce, which is what actually reaches a verifier.
#[test]
fn a_login_payload_is_not_a_warrant_payload() {
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::AccountId;

    use crate::warrant::Warrant;

    let login = LoginStatement::signing_payload(
        &key(9).public_key(),
        &Audience::Cli,
        &[0u8; 32],
        &key(4).public_key(),
        &key(3).public_key(),
        0,
        0,
    );
    let warrant = Warrant {
        context: ContextId::from([0u8; 32]),
        author_account: AccountId::from([0u8; 32]),
        author_device_key: key(3).public_key(),
        executor: AccountId::from([0u8; 32]),
        app_version: ApplicationId::from([0u8; 32]),
        method: String::new(),
        intent_hash: [0u8; 32],
        account_heads: vec![],
        governance_floor: vec![],
        nonce: 0,
        not_after: 0,
        signature: [0u8; 64],
    }
    .signing_payload();
    assert_ne!(login, warrant);
}
