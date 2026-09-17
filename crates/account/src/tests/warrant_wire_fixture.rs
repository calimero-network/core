//! The byte contract a non-Rust warrant signer must reproduce.
//!
//! A warrant is signed by whoever holds the author's device key, and that holder
//! is increasingly not a Rust process — a browser tab, a Node service, anything
//! whose account holds no node at all. Such a signer has to reproduce three
//! things exactly: the intent hash, the signing preimage, and the borsh wire
//! encoding.
//!
//! **Why this test lives in core rather than beside the JS implementation.** The
//! constants it pins (`WARRANT_SIGN_DOMAIN`, `WARRANT_INTENT_DOMAIN`) are
//! `pub(crate)`, and the field order is implicit in a `derive`. Nothing here
//! forces anyone to notice that another language depends on either. A conformance
//! test in the JS repository would be run by the repository that *cannot* break
//! the contract; this one is run by the repository that can.
//!
//! What a drift looks like without it: the other signer keeps producing
//! well-formed warrants whose signatures verify nowhere, and the failure arrives
//! at a relay as a 403 — an authorization refusal, not an encoding error, and
//! nowhere near the change that caused it.
//!
//! The vectors below are also the fixture the JS test reads. Regenerate them only
//! when the wire format is *deliberately* changing, and treat that as the
//! breaking change it is.

use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::AccountId;

use super::support::key;
use crate::account::borsh_bytes;
use crate::warrant::{Warrant, WarrantTerms};

const METHOD: &str = "set";
const ARGS: &[u8] = br#"{"key":"k","value":"v"}"#;

/// Fixed inputs, so the vectors below are a function of the format alone.
///
/// One entry in each cited-head list, and two different bytes: an empty pair
/// would not exercise the length prefixes, and equal lists would not catch the
/// two being swapped.
fn terms() -> WarrantTerms {
    WarrantTerms {
        context: ContextId::from([0x11; 32]),
        author_account: AccountId::from([0x22; 32]),
        executor: AccountId::from([0x33; 32]),
        app_version: ApplicationId::from([0x44; 32]),
        method: METHOD.to_owned(),
        intent_hash: Warrant::intent_hash(METHOD, ARGS),
        account_heads: vec![[0x55; 32]],
        governance_floor: vec![[0x66; 32]],
        nonce: 42,
        not_after: 1_700_000_000,
    }
}

/// The signed warrant every vector below is taken from.
fn fixture() -> Warrant {
    Warrant::sign(&key(7), terms()).expect("sign")
}

/// `intent_hash` is `H(method ‖ args)` under its own domain.
///
/// Its own domain, distinct from the signing one: the same bytes hashed under a
/// single domain would let a value computed for one purpose be presented for the
/// other.
#[test]
fn the_intent_hash_is_stable() {
    assert_eq!(
        hex::encode(Warrant::intent_hash(METHOD, ARGS)),
        "dc066cc8524c74dc21714174009df536376e3151f5b92f0a676defde599dbae5",
        "the intent hash changed; a JS signer computing the old one produces \
         warrants refused as not covering their intent",
    );
}

/// The 32 bytes an author's device key actually signs.
#[test]
fn the_signing_preimage_is_stable() {
    assert_eq!(
        hex::encode(fixture().signing_payload()),
        "cd03cfaad8fb5f5aa143c76f35f39363b6608b820ab318a53ed01be901c7c6c7",
        "the signing preimage changed; every warrant signed elsewhere now \
         verifies nowhere",
    );
}

/// The full wire encoding.
///
/// Worth pinning the length on its own. v1 was pure fixed-width concatenation —
/// no length prefixes, no tags — which is what made a non-borsh implementation
/// viable in the first place. v2 gives that up deliberately: `method` travels in
/// the clear as a `String` and each cited-head list is a `Vec`, so three
/// `u32` little-endian counts now sit inside the encoding. A signer in another
/// language has to write those counts, and the length below is the cheapest
/// signal that a field changed shape again.
#[test]
fn the_wire_encoding_is_stable() {
    let bytes = borsh_bytes(&fixture());

    assert_eq!(
        bytes.len(),
        351,
        "32*6 ids and hashes + (4 + 3) method + 2*(4 + 32) cited heads + 8 + 8 \
         + 64. A different length means a field changed shape, and a signer in \
         another language is now wrong",
    );
    // Split by field so the layout is legible: a reviewer can check the
    // boundaries without counting hex, and a diff names which field moved.
    assert_eq!(
        hex::encode(&bytes),
        concat!(
            // context
            "1111111111111111111111111111111111111111111111111111111111111111",
            // author_account
            "2222222222222222222222222222222222222222222222222222222222222222",
            // author_device_key (derived from key(7))
            "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c",
            // executor
            "3333333333333333333333333333333333333333333333333333333333333333",
            // app_version
            "4444444444444444444444444444444444444444444444444444444444444444",
            // method: u32 LE length 3, then "set" in ASCII
            "03000000",
            "736574",
            // intent_hash
            "dc066cc8524c74dc21714174009df536376e3151f5b92f0a676defde599dbae5",
            // account_heads: u32 LE count 1, then one 32-byte head
            "01000000",
            "5555555555555555555555555555555555555555555555555555555555555555",
            // governance_floor: u32 LE count 1, then one 32-byte head
            "01000000",
            "6666666666666666666666666666666666666666666666666666666666666666",
            // nonce 42, u64 LITTLE-endian
            "2a00000000000000",
            // not_after 1_700_000_000, u64 little-endian
            "00f1536500000000",
            // signature
            "4007d4164a6a15f4b6b251b45e9afad623c274451127afc1453e35667d4ec6fe",
            "7aa8daa223c5320823c61612058c8053dcff9b361ced58bed5f05f4114099a06",
        ),
    );
}
