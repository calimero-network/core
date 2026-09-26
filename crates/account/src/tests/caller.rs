//! [`CallerProof`] — the whole chain, checked against one request.
//!
//! Every negative case below removes exactly one property and leaves the rest
//! intact, because a chain that fails for two reasons proves nothing about
//! either. The positive case is built from the same fixture, so a test that
//! stops failing has stopped testing rather than started passing.

use calimero_primitives::identity::{DeviceId, PrivateKey, PublicKey};

use super::support::{genesis_for, key, sign_cert};
use crate::caller::{CallerProof, VerifiedCaller};
use crate::error::AccountError;
use crate::login::{Audience, LoginStatement};
use crate::request::RequestSig;
use crate::signed::AccountProof;

const METHOD: &str = "GET";
const PATH: &str = "/admin-api/namespaces";
const BODY: &[u8] = b"";
const NOW: u64 = 1_700_000_000;
const SKEW: u64 = 30;

fn root() -> PrivateKey {
    key(1)
}
fn device_sk() -> PrivateKey {
    key(2)
}
fn session_sk() -> PrivateKey {
    key(3)
}
fn node_key() -> PublicKey {
    key(4).public_key()
}

/// The long chain: root certifies a device, the device delegates a session, the
/// session signs the request.
fn long_chain() -> CallerProof {
    let genesis = genesis_for(&root());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0x22; 16]);
    let cert = sign_cert(&root(), account, device, &device_sk(), 0, 0);

    let session = LoginStatement::sign(
        &device_sk(),
        node_key(),
        Audience::Cli,
        [0x77; 32],
        session_sk().public_key(),
        NOW,
        NOW + 3600,
    )
    .expect("sign statement");

    CallerProof {
        account_proof: AccountProof {
            genesis,
            chain: vec![],
            statement: cert,
        },
        session: Some(session),
        request: RequestSig::sign(&session_sk(), METHOD, PATH, BODY, NOW, NOW + 300)
            .expect("sign request"),
    }
}

/// The short chain: no session, the device signs the request itself.
fn short_chain() -> CallerProof {
    let mut proof = long_chain();
    proof.session = None;
    proof.request =
        RequestSig::sign(&device_sk(), METHOD, PATH, BODY, NOW, NOW + 300).expect("sign request");
    proof
}

fn check(proof: &CallerProof) -> Result<VerifiedCaller, AccountError> {
    proof.verify(&node_key(), METHOD, PATH, BODY, NOW, SKEW)
}

#[test]
fn the_long_chain_verifies_and_names_its_account_and_device() {
    let proof = long_chain();
    let caller = check(&proof).expect("verifies");

    assert_eq!(caller.account, proof.account_proof.statement.account);
    assert_eq!(caller.device, proof.account_proof.statement.device);
    assert_eq!(caller.audience, Some(Audience::Cli));
}

/// The short chain is not a degraded long one: it is the whole contract for a
/// caller whose device key is already local.
#[test]
fn the_short_chain_verifies_and_reports_no_audience() {
    let caller = check(&short_chain()).expect("verifies");
    assert_eq!(
        caller.audience, None,
        "no session means no audience to report"
    );
}

/// A proof is for one request. This is the first check and the cheapest, and it
/// is what stops a captured signature being presented against anything else.
#[test]
fn a_proof_does_not_cover_another_request() {
    let proof = long_chain();
    assert!(matches!(
        proof.verify(&node_key(), "POST", PATH, BODY, NOW, SKEW),
        Err(AccountError::RequestMismatch { part: "method" }),
    ));
    assert!(matches!(
        proof.verify(&node_key(), METHOD, "/other", BODY, NOW, SKEW),
        Err(AccountError::RequestMismatch {
            part: "body" | "path"
        }),
    ));
}

/// The node binding is what makes a hostile relay unable to forward a statement
/// a user signed for it.
#[test]
fn a_session_for_another_node_is_refused() {
    let other_node = key(9).public_key();
    assert!(matches!(
        long_chain().verify(&other_node, METHOD, PATH, BODY, NOW, SKEW),
        Err(AccountError::LoginNodeMismatch),
    ));
}

/// A request signed by a key the session never authorized.
#[test]
fn a_request_signed_by_the_wrong_key_is_refused() {
    let mut proof = long_chain();
    proof.request =
        RequestSig::sign(&key(8), METHOD, PATH, BODY, NOW, NOW + 300).expect("sign request");

    assert!(matches!(
        check(&proof),
        Err(AccountError::RequestSignatureInvalid),
    ));
}

/// A session signed by one of the account's OTHER devices.
///
/// It verifies perfectly as a statement, and the certificate presented beside
/// it verifies perfectly as a certificate. Only the link between them is
/// missing, which is exactly the shape that looks like success.
#[test]
fn a_session_from_a_device_the_certificate_does_not_cover_is_refused() {
    let mut proof = long_chain();
    let other_device_sk = key(6);
    proof.session = Some(
        LoginStatement::sign(
            &other_device_sk,
            node_key(),
            Audience::Cli,
            [0x77; 32],
            session_sk().public_key(),
            NOW,
            NOW + 3600,
        )
        .expect("sign statement"),
    );

    assert!(matches!(
        check(&proof),
        Err(AccountError::SessionDeviceMismatch),
    ));
}

/// A certificate for a device of a DIFFERENT account, presented with a genesis
/// that does not hash to it.
#[test]
fn a_certificate_that_does_not_anchor_to_its_genesis_is_refused() {
    let mut proof = long_chain();
    let other_root = key(5);
    proof.account_proof.genesis = genesis_for(&other_root);

    assert!(
        check(&proof).is_err(),
        "a genesis that does not hash to the certificate's account must not verify",
    );
}

/// Expiry and future-dating are different diagnoses.
///
/// One is a caller that waited too long; the other is a clock that disagrees.
/// Reporting the second as the first is how a skew problem gets treated as a
/// session-length problem and "fixed" by lengthening sessions.
#[test]
fn a_closed_window_says_which_way_it_closed() {
    let proof = long_chain();

    assert!(matches!(
        proof.verify(&node_key(), METHOD, PATH, BODY, NOW + 301 + SKEW, SKEW),
        Err(AccountError::ProofExpired { part: "request" }),
    ));
    assert!(matches!(
        proof.verify(&node_key(), METHOD, PATH, BODY, NOW - 1 - SKEW, SKEW),
        Err(AccountError::ProofNotYetValid { part: "request" }),
    ));
}

/// Skew tolerance is applied, or every client with a drifting clock is locked
/// out for a reason it cannot see.
#[test]
fn a_clock_inside_the_tolerance_still_verifies() {
    let proof = long_chain();
    proof
        .verify(&node_key(), METHOD, PATH, BODY, NOW - SKEW, SKEW)
        .expect("a client running slightly fast still verifies");
    proof
        .verify(&node_key(), METHOD, PATH, BODY, NOW + 300 + SKEW, SKEW)
        .expect("a client running slightly slow still verifies");
}

/// The session's own window is checked too, not only the request's.
#[test]
fn an_expired_session_is_refused_even_with_a_fresh_request() {
    let mut proof = long_chain();
    proof.session = Some(
        LoginStatement::sign(
            &device_sk(),
            node_key(),
            Audience::Cli,
            [0x77; 32],
            session_sk().public_key(),
            NOW - 7200,
            NOW - 3600,
        )
        .expect("sign statement"),
    );
    // Re-sign the request so the ONLY thing wrong is the session's window.
    proof.request =
        RequestSig::sign(&session_sk(), METHOD, PATH, BODY, NOW, NOW + 300).expect("sign request");

    assert!(matches!(
        check(&proof),
        Err(AccountError::ProofExpired { part: "session" }),
    ));
}
