//! Decode-side hostility, for both types on the wire.
//!
//! A malformed buffer must error, never yield a partial or misread value: an
//! op arrives from a peer, so every one of these inputs is one an attacker gets
//! to choose.

use calimero_account::AccountId;
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;

use crate::{Op, OpPayload};

#[test]
fn op_payload_borsh_roundtrips() {
    let payload = OpPayload::SetWriters {
        object: Id::new([5u8; 32]),
        writers: [(AccountId::from([9u8; 32]), OpMask::FULL)]
            .into_iter()
            .collect(),
    };
    let bytes = borsh::to_vec(&payload).unwrap();
    let decoded: OpPayload = borsh::from_slice(&bytes).unwrap();
    assert_eq!(payload, decoded);
}

#[test]
fn op_payload_rejects_out_of_range_discriminant() {
    // 17 variants are pinned (tags 0..=16). Any higher tag must fail to
    // decode rather than silently map onto a variant — the guard that
    // keeps a corrupt/forward-version byte from being mistaken for a valid
    // op.
    for tag in [17u8, 18, 20, 42, 200, 255] {
        let bytes = [tag];
        assert!(
            borsh::from_slice::<OpPayload>(&bytes).is_err(),
            "discriminant {tag} must not decode to any OpPayload variant"
        );
    }
}

#[test]
fn op_payload_rejects_truncated_bytes() {
    // Encode a real payload, then assert every strict prefix fails to
    // decode. A truncated buffer must error, never yield a partial value.
    let payload = OpPayload::SetWriters {
        object: Id::new([5u8; 32]),
        writers: [(AccountId::from([9u8; 32]), OpMask::FULL)]
            .into_iter()
            .collect(),
    };
    let bytes = borsh::to_vec(&payload).unwrap();
    for len in 0..bytes.len() {
        assert!(
            borsh::from_slice::<OpPayload>(&bytes[..len]).is_err(),
            "truncated OpPayload ({len}/{} bytes) must not decode",
            bytes.len()
        );
    }
    // The full buffer still decodes (guards against an off-by-one that
    // would make the loop vacuous).
    assert!(borsh::from_slice::<OpPayload>(&bytes).is_ok());
}

#[test]
fn op_payload_rejects_trailing_garbage() {
    // borsh requires the whole buffer to be consumed; extra bytes after a
    // complete payload must be rejected, not ignored.
    let payload = OpPayload::Delete {
        entity: Id::new([2u8; 32]),
    };
    let mut bytes = borsh::to_vec(&payload).unwrap();
    bytes.push(0xFF);
    assert!(
        borsh::from_slice::<OpPayload>(&bytes).is_err(),
        "trailing bytes after a complete payload must be rejected"
    );
}

#[test]
fn op_rejects_malformed_bytes() {
    // Full `Op` decode over degenerate buffers must error cleanly rather
    // than panic on an unexpected EOF or a bogus length prefix.
    assert!(borsh::from_slice::<Op>(&[]).is_err());
    assert!(borsh::from_slice::<Op>(&[0u8; 8]).is_err());
    assert!(borsh::from_slice::<Op>(&[0xFFu8; 16]).is_err());
}
