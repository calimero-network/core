//! The byte contract a non-Rust warrant signer must reproduce.
//!
//! A warrant is signed by whoever holds the author's device key, and that holder
//! is increasingly not a Rust process — a browser tab, a Node service, anything
//! whose account holds no node at all. Such a signer has to reproduce three
//! things exactly: the intent hash, the signing preimage, and the 240-byte wire
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

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::AccountId;

use super::support::key;
use crate::account::borsh_bytes;
use crate::warrant::Warrant;

/// Fixed inputs, so the vectors below are a function of the format alone.
fn vectors() -> (ContextId, AccountId, AccountId, &'static str, &'static [u8]) {
    (
        ContextId::from([0x11; 32]),
        AccountId::from([0x22; 32]),
        AccountId::from([0x33; 32]),
        "set",
        br#"{"key":"k","value":"v"}"#,
    )
}

/// `intent_hash` is `H(method ‖ args)` under its own domain.
///
/// Its own domain, distinct from the signing one: the same bytes hashed under a
/// single domain would let a value computed for one purpose be presented for the
/// other.
#[test]
fn the_intent_hash_is_stable() {
    let (_, _, _, method, args) = vectors();

    assert_eq!(
        hex::encode(Warrant::intent_hash(method, args)),
        "dc066cc8524c74dc21714174009df536376e3151f5b92f0a676defde599dbae5",
        "the intent hash changed; a JS signer computing the old one produces \
         warrants refused as not covering their intent",
    );
}

/// The 32 bytes an author's device key actually signs.
#[test]
fn the_signing_preimage_is_stable() {
    let (context, author, executor, method, args) = vectors();
    let device_pk = key(7).public_key();

    let payload = Warrant::signing_payload(
        context,
        author,
        &device_pk,
        executor,
        &Warrant::intent_hash(method, args),
        42,
        1_700_000_000,
    );

    assert_eq!(
        hex::encode(payload),
        "a5d5f7aa368f8d79edd0667f60c41239a7dedaf8765e4e72ef3031bbacd15f35",
        "the signing preimage changed; every warrant signed elsewhere now \
         verifies nowhere",
    );
}

/// The full wire encoding — 240 bytes, and every field fixed-width.
///
/// Worth pinning the length on its own: the encoding is pure concatenation, with
/// no length prefixes and no tags, which is exactly why a non-borsh
/// implementation is viable. A field becoming variable-width would break that
/// assumption silently, and the length is the cheapest signal that it happened.
#[test]
fn the_wire_encoding_is_240_fixed_bytes() {
    let (context, author, executor, method, args) = vectors();
    let warrant = Warrant::sign(
        &key(7),
        context,
        author,
        executor,
        Warrant::intent_hash(method, args),
        42,
        1_700_000_000,
    )
    .expect("sign");

    let bytes = borsh_bytes(&warrant);

    assert_eq!(
        bytes.len(),
        240,
        "32*5 ids and hashes + 8 + 8 + 64. A different length means a field is \
         no longer fixed-width, and a concatenating signer is now wrong",
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
            // intent_hash
            "dc066cc8524c74dc21714174009df536376e3151f5b92f0a676defde599dbae5",
            // nonce 42, u64 LITTLE-endian
            "2a00000000000000",
            // not_after 1_700_000_000, u64 little-endian
            "00f1536500000000",
            // signature
            "a3ddd5294a755f875a275d61f79105115a2d4fb115339ee2483fc17c0ae96c6d",
            "e21e02f26ea103cfe87bb2365ca858c27fb688a99cf1835ed473de226eb24709",
        ),
    );
}
