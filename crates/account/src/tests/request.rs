//! [`RequestSig`] — signing one request rather than exchanging a proof for a
//! session.

use calimero_primitives::identity::PublicKey;

use super::support::key;
use crate::error::AccountError;
use crate::request::RequestSig;

const METHOD: &str = "GET";
const PATH: &str = "/admin-api/namespaces";
const BODY: &[u8] = b"";

fn signed() -> RequestSig {
    RequestSig::sign(&key(3), METHOD, PATH, BODY, 1_700_000_000, 1_700_000_300).expect("sign")
}

fn signer() -> PublicKey {
    key(3).public_key()
}

#[test]
fn a_signature_verifies_for_the_key_that_made_it_and_covers_its_request() {
    let sig = signed();
    sig.verify_signature(&signer()).expect("verifies");
    sig.covers(METHOD, PATH, BODY).expect("covers");
}

/// The expected key comes from the link above, so presenting a proof to a
/// verifier holding a different one must fail — this is what stops a captured
/// proof being replayed under somebody else's session.
#[test]
fn a_signature_does_not_verify_for_another_key() {
    assert!(matches!(
        signed().verify_signature(&key(4).public_key()),
        Err(AccountError::RequestSignatureInvalid),
    ));
}

/// Each part is bound independently, and the error says which one.
///
/// Three separate cases rather than one: a method mismatch is a proof reused
/// across verbs, a path mismatch across routes, and a body mismatch is the one
/// that matters most — a signature captured from one write presented with
/// different arguments. A single "does not match" would collapse three
/// different diagnoses into one.
#[test]
fn a_signature_covers_exactly_one_request() {
    let sig = signed();

    assert!(matches!(
        sig.covers("POST", PATH, BODY),
        Err(AccountError::RequestMismatch { part: "method" }),
    ));
    assert!(matches!(
        sig.covers(METHOD, "/admin-api/contexts", BODY),
        Err(AccountError::RequestMismatch { part: "path" }),
    ));
    assert!(matches!(
        sig.covers(METHOD, PATH, b"{}"),
        Err(AccountError::RequestMismatch { part: "body" }),
    ));
}

/// `HEAD` and `GET` are the same permission and NOT the same request.
///
/// The permission layer folds them deliberately, because a HEAD read of a
/// mapped GET route needs the GET permission. A signature layer that folded
/// them too would let a proof minted for one be presented as the other, which
/// is a different property from the one the permission layer is asserting.
#[test]
fn head_is_not_get_at_this_layer() {
    assert!(matches!(
        signed().covers("HEAD", PATH, BODY),
        Err(AccountError::RequestMismatch { part: "method" }),
    ));
}

/// A body must be committed to even when it is empty.
///
/// Otherwise an empty-bodied proof would cover a request that grew a body, and
/// the growth is exactly the interesting case.
#[test]
fn an_empty_body_is_still_committed_to() {
    let empty = RequestSig::body_hash(b"");
    assert_ne!(empty, [0_u8; 32], "an empty body must hash, not default");
    assert_ne!(empty, RequestSig::body_hash(b"{}"));
}

/// Every field enters the preimage, so changing any one of them changes what
/// was signed. Without this, a field could be added to the struct and left out
/// of `signing_payload` — a signature that covers less than it appears to.
#[test]
fn every_field_changes_the_preimage() {
    let base = RequestSig::signing_payload(METHOD, PATH, &RequestSig::body_hash(BODY), 10, 20);

    let variants = [
        RequestSig::signing_payload("POST", PATH, &RequestSig::body_hash(BODY), 10, 20),
        RequestSig::signing_payload(METHOD, "/other", &RequestSig::body_hash(BODY), 10, 20),
        RequestSig::signing_payload(METHOD, PATH, &RequestSig::body_hash(b"x"), 10, 20),
        RequestSig::signing_payload(METHOD, PATH, &RequestSig::body_hash(BODY), 11, 20),
        RequestSig::signing_payload(METHOD, PATH, &RequestSig::body_hash(BODY), 10, 21),
    ];

    for (index, variant) in variants.iter().enumerate() {
        assert_ne!(base, *variant, "field {index} does not reach the preimage");
    }
}

/// The parts are length-prefixed, so concatenation is not ambiguous.
///
/// `domain_hash` prefixes each part with its length for this reason; the test
/// pins the consequence rather than the mechanism, because the mechanism is one
/// helper call away from being replaced by a plain concatenation that looks
/// equivalent and is not.
#[test]
fn a_boundary_between_parts_cannot_be_moved() {
    let a = RequestSig::signing_payload("GE", "T/path", &RequestSig::body_hash(BODY), 10, 20);
    let b = RequestSig::signing_payload("GET", "/path", &RequestSig::body_hash(BODY), 10, 20);
    assert_ne!(a, b, "method and path run together in the preimage");
}
