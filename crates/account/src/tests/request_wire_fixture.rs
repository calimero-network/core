//! The byte contract a non-Rust request signer must reproduce.
//!
//! A request signature is minted by whoever holds the caller's session or device
//! key, and that holder is a browser tab or a Node service far more often than a
//! Rust process. Such a signer has to reproduce three things exactly: the body
//! commitment, the signing preimage, and the borsh wire encoding.
//!
//! This lives in core rather than beside the JS implementation for the same
//! reason [`super::warrant_wire_fixture`] does: the constants it pins are
//! `pub(crate)` and the field order is implicit in a `derive`, so nothing here
//! forces anyone to notice that another language depends on either. A
//! conformance test in the JS repository would be run by the repository that
//! *cannot* break the contract; this one is run by the repository that can.
//!
//! What a drift looks like without it: the other signer keeps producing
//! well-formed proofs whose signatures verify nowhere, and the failure arrives
//! as a 401 — an authentication refusal, nowhere near the change that caused it,
//! and indistinguishable from a caller that simply is not who it says.
//!
//! Regenerate these vectors only when the wire format is *deliberately*
//! changing, and treat that as the breaking change it is.

use super::support::key;
use crate::account::borsh_bytes;
use crate::request::RequestSig;

const METHOD: &str = "GET";
const PATH: &str = "/admin-api/namespaces";
const BODY: &[u8] = br#"{"k":"v"}"#;
const ISSUED_AT: u64 = 1_700_000_000;
const EXPIRES_AT: u64 = 1_700_000_300;

/// A non-empty body on purpose: an empty one would not exercise the commitment,
/// and a `GET` with a body is still the encoding under test rather than a
/// realistic request.
fn fixture() -> RequestSig {
    RequestSig::sign(&key(3), METHOD, PATH, BODY, ISSUED_AT, EXPIRES_AT).expect("sign")
}

/// The body commitment, under its own domain.
#[test]
fn the_body_hash_is_stable() {
    assert_eq!(
        hex::encode(RequestSig::body_hash(BODY)),
        "3b26137eb7b296bdf7d84b9193dd52d1947b2056731f38020f4a3b9b88d95121",
        "the body commitment changed; a signer elsewhere now commits to \
         different bytes and every proof it mints is refused as covering a \
         different request",
    );
}

/// The 32 bytes a session or device key actually signs.
#[test]
fn the_signing_preimage_is_stable() {
    assert_eq!(
        hex::encode(RequestSig::signing_payload(
            METHOD,
            PATH,
            &RequestSig::body_hash(BODY),
            ISSUED_AT,
            EXPIRES_AT,
        )),
        "7ea8e1179b3f364faefbd4b2a18491f90b3b43bf18af1efad740493f934df8f8",
        "the signing preimage changed; every request signed elsewhere now \
         verifies nowhere",
    );
}

/// The full wire encoding, split by field so a diff names which one moved.
#[test]
fn the_wire_encoding_is_stable() {
    let bytes = borsh_bytes(&fixture());

    assert_eq!(
        bytes.len(),
        144,
        "(4 + 3) method + (4 + 21) path + 32 body hash + 8 + 8 + 64 signature. \
         A different length means a field changed shape, and a signer in \
         another language is now wrong",
    );
    assert_eq!(
        hex::encode(&bytes),
        concat!(
            // method: u32 LE length 3, then "GET"
            "03000000",
            "474554",
            // path: u32 LE length 21, then the ASCII path
            "15000000",
            "2f61646d696e2d6170692f6e616d65737061636573",
            // body_hash
            "3b26137eb7b296bdf7d84b9193dd52d1947b2056731f38020f4a3b9b88d95121",
            // issued_at, u64 LE
            "00f1536500000000",
            // expires_at, u64 LE
            "2cf2536500000000",
            // signature
            "295cdac7a269f7f773735dcc35511d4174d042aa4c20454b6c14d5c5661a4f2e6401e8d1dc457040a388359982f2b915fdd8ca56a84d26f2b40867f092013706",
        ),
    );
}
