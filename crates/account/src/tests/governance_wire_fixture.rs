//! The byte contract a non-Rust governance-warrant signer must reproduce.
//! Same purpose as `warrant_wire_fixture`; the JS implementation is tested
//! against these vectors.

use calimero_primitives::identity::{AccountId, PublicKey};

use super::support::key;
use crate::account::borsh_bytes;
use crate::governance::{GovernanceOpKind, GovernanceTerms, GovernanceWarrant};

/// A stand-in for an op's borsh bytes: the commitment treats them as opaque.
const OP: &[u8] = b"\x01\x02\x03";

fn fixture() -> GovernanceWarrant {
    GovernanceWarrant::sign(
        &key(7),
        GovernanceTerms {
            scope: [0x11; 32],
            kind: GovernanceOpKind::Root,
            author_account: AccountId::from([0x22; 32]),
            executor: AccountId::from([0x33; 32]),
            executor_key: PublicKey::from([0x77; 32]),
            op_hash: GovernanceWarrant::op_hash(GovernanceOpKind::Root, OP),
            account_heads: vec![[0x55; 32]],
            governance_floor: vec![[0x66; 32]],
            nonce: 42,
            not_after: 1_700_000_000,
        },
    )
    .expect("sign")
}

#[test]
fn the_op_hash_is_stable() {
    assert_eq!(
        hex::encode(GovernanceWarrant::op_hash(GovernanceOpKind::Root, OP)),
        "d6bc121f9fcf7b85bea94d356d620c14dd8e1ae5fa2317cbcfc2c486cf04dfb3"
    );
}

#[test]
fn the_signing_preimage_is_stable() {
    assert_eq!(
        hex::encode(fixture().signing_payload()),
        "b71a40cfbbe50e40d3423e8729a9ca8359e69f2762396922e6b314a971c5f720"
    );
}

#[test]
fn the_wire_encoding_is_stable() {
    let bytes = borsh_bytes(&fixture());
    assert_eq!(
        bytes.len(),
        345,
        "32 scope + 1 kind + 32*4 + 32 op_hash + 2*(4+32) + 8 + 8 + 64"
    );
    assert_eq!(
        hex::encode(&bytes),
        concat!(
            // scope
            "1111111111111111111111111111111111111111111111111111111111111111",
            // kind: Root
            "01",
            // author_account
            "2222222222222222222222222222222222222222222222222222222222222222",
            // author_device_key (derived from key(7))
            "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c",
            // executor
            "3333333333333333333333333333333333333333333333333333333333333333",
            // executor_key
            "7777777777777777777777777777777777777777777777777777777777777777",
            // op_hash
            "d6bc121f9fcf7b85bea94d356d620c14dd8e1ae5fa2317cbcfc2c486cf04dfb3",
            // account_heads: count 1
            "01000000",
            "5555555555555555555555555555555555555555555555555555555555555555",
            // governance_floor: count 1
            "01000000",
            "6666666666666666666666666666666666666666666666666666666666666666",
            // nonce, not_after
            "2a00000000000000",
            "00f1536500000000",
            // signature
            "03ce6c011f565924099b2c776032a1cfe540d1c4c09fad0b470723f8ee76138d",
            "1d685a20948edf4dee2901d3f9187e3363a16e3b141b0bc94553f5948becae07",
        ),
    );
}
