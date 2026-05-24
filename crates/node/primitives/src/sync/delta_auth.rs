//! Delta-envelope signature primitive.
//!
//! Closes the anti-impersonation gap on the delta-envelope level: a
//! current group-key holder can no longer write a delta claiming
//! another member as `author_id`. The author signs a canonical
//! payload that binds `(context_id, delta_id, author_id,
//! governance_position)`; every receive path verifies before
//! applying.
//!
//! The signature primitive is intentionally separate from per-action
//! signatures (which live in `StorageType::{User, Shared}::signature_data`
//! and verify in `Interface::apply_action`). Per-action signatures
//! attribute INDIVIDUAL writes within a delta; the envelope
//! signature binds the WHOLE delta to its author. Both are needed for
//! full coverage — per-action sigs don't catch envelope forgery
//! (a current member relabeling a foreign delta as their own), and
//! the envelope signature doesn't catch per-action forgery within a
//! Public-only delta.
//!
//! ## Payload shape
//!
//! ```ignore
//! DeltaSignaturePayload {
//!     context_id,        // pins to the context (cross-context replay)
//!     delta_id,          // hash(parents || actions); commits to the content
//!     author_id,         // claimed author
//!     governance_position, // cited cut for the membership check
//! }
//! ```
//!
//! Borsh-serialized. Signed with the author's ed25519 identity key.
//! `delta_id` is the existing content hash, so committing to it covers
//! the action bytes via the hash chain.

use borsh::BorshSerialize;
use calimero_context_config::types::GovernancePosition;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;

/// Domain separator prefixed to every delta-envelope signature payload.
///
/// Without this, an ed25519 signature produced for a `DeltaSignaturePayload`
/// could in principle be replayed as a signature for a different protocol
/// message that happened to borsh-serialize to identical bytes (cross-
/// protocol replay). The separator is included as a typed field on
/// `DeltaSignaturePayload` so its borsh-serialization is part of the
/// signed bytes; receivers reconstruct the payload with the same
/// constant, so any signature produced for a different domain fails
/// verification.
///
/// The literal string is part of the protocol — never change it without
/// a wire-format version bump.
pub const DOMAIN_SEPARATOR: &[u8; 16] = b"calimero/delta/1";

/// Canonical payload for the delta-envelope signature. Borsh-serialized
/// and signed by `author_id`'s ed25519 key. Only used for serialization —
/// receivers re-construct it from their own data and compare signature
/// bytes, so `BorshDeserialize` isn't needed (and wouldn't work with the
/// `&GovernancePosition` borrow anyway).
///
/// The `domain` field is always [`DOMAIN_SEPARATOR`] — it's serialized
/// into the signed bytes so signatures from other protocols using the
/// same key can't be replayed here.
#[derive(BorshSerialize)]
pub struct DeltaSignaturePayload<'a> {
    pub domain: [u8; 16],
    pub context_id: ContextId,
    pub delta_id: [u8; 32],
    pub author_id: PublicKey,
    pub governance_position: Option<&'a GovernancePosition>,
}

/// Borsh-serialize the canonical payload. Used at sign time (execute
/// path) and verify time (every delta receive path).
///
/// Returns `borsh::io::Error` only if the borsh writer fails on the
/// in-memory buffer — practically infallible for these field types,
/// but the result type matches `borsh::to_vec`'s shape.
pub fn delta_signature_payload(
    context_id: ContextId,
    delta_id: [u8; 32],
    author_id: PublicKey,
    governance_position: Option<&GovernancePosition>,
) -> Result<Vec<u8>, borsh::io::Error> {
    let payload = DeltaSignaturePayload {
        domain: *DOMAIN_SEPARATOR,
        context_id,
        delta_id,
        author_id,
        governance_position,
    };
    borsh::to_vec(&payload)
}

/// Verify a per-delta envelope signature against the canonical payload.
///
/// Reconstructs the payload the author signed at send time
/// (`delta_signature_payload`) and verifies the ed25519 signature with
/// the claimed author's public key. Receivers call this on every apply
/// path (gossip receive, DAG-catchup receive, snapshot-buffer replay)
/// before the delta touches storage.
///
/// Returns `Ok(())` only on a valid signature. Any borsh-serialize
/// failure on the payload, or signature mismatch, returns `Err`.
///
/// **Caller contract:** the `author_id` passed here MUST be the same
/// author bound into the payload — verification doesn't check that
/// invariant for you, it just verifies that `author_id`'s key signed
/// THIS payload bytes. If you pass a different author for the
/// verification key vs. the payload, you're checking the wrong thing.
pub fn verify_delta_signature(
    context_id: ContextId,
    delta_id: [u8; 32],
    author_id: PublicKey,
    governance_position: Option<&GovernancePosition>,
    signature: &[u8; 64],
) -> eyre::Result<()> {
    let payload = delta_signature_payload(context_id, delta_id, author_id, governance_position)
        .map_err(|err| eyre::eyre!("failed to serialize delta signature payload: {err}"))?;
    author_id
        .verify_raw_signature(&payload, signature)
        .map_err(|err| eyre::eyre!("delta envelope signature verification failed: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_primitives::identity::PrivateKey;

    fn fixture() -> (ContextId, [u8; 32], PrivateKey, PublicKey) {
        let private_key = PrivateKey::from([3u8; 32]);
        let author_id = private_key.public_key();
        let context_id = ContextId::from([7u8; 32]);
        let delta_id = [9u8; 32];
        (context_id, delta_id, private_key, author_id)
    }

    #[test]
    fn sign_then_verify_roundtrip_no_position() {
        let (context_id, delta_id, sk, pk) = fixture();
        let payload = delta_signature_payload(context_id, delta_id, pk, None).unwrap();
        let sig = sk.sign(&payload).unwrap().to_bytes();
        assert!(verify_delta_signature(context_id, delta_id, pk, None, &sig).is_ok());
    }

    #[test]
    fn verify_rejects_tampered_context_id() {
        let (context_id, delta_id, sk, pk) = fixture();
        let payload = delta_signature_payload(context_id, delta_id, pk, None).unwrap();
        let sig = sk.sign(&payload).unwrap().to_bytes();
        // Author signed for `context_id`; verifier reconstructs payload
        // with a *different* context_id, so the bytes diverge and the
        // signature should not verify. This is the anti-cross-context-
        // replay property the payload buys us.
        let other_context = ContextId::from([1u8; 32]);
        assert!(verify_delta_signature(other_context, delta_id, pk, None, &sig).is_err());
    }

    #[test]
    fn verify_rejects_tampered_author() {
        let (context_id, delta_id, sk, pk) = fixture();
        let payload = delta_signature_payload(context_id, delta_id, pk, None).unwrap();
        let sig = sk.sign(&payload).unwrap().to_bytes();
        // Same signature bytes, but a *different* author is claimed on
        // the wire. `verify_raw_signature` uses the claimed author's key,
        // which never signed this payload — must fail. This is the
        // anti-impersonation property that gossip's
        // `membership_status_at` check alone doesn't catch.
        let other_pk = PrivateKey::from([4u8; 32]).public_key();
        assert!(verify_delta_signature(context_id, delta_id, other_pk, None, &sig).is_err());
    }

    #[test]
    fn verify_rejects_tampered_signature_bytes() {
        let (context_id, delta_id, sk, pk) = fixture();
        let payload = delta_signature_payload(context_id, delta_id, pk, None).unwrap();
        let mut sig = sk.sign(&payload).unwrap().to_bytes();
        sig[0] ^= 0xff;
        assert!(verify_delta_signature(context_id, delta_id, pk, None, &sig).is_err());
    }

    #[test]
    fn sign_then_verify_roundtrip_with_position() {
        let (context_id, delta_id, sk, pk) = fixture();
        let pos = calimero_context_config::types::GovernancePosition {
            group_id: calimero_context_config::types::ContextGroupId::from([5u8; 32]),
            governance_dag_heads: vec![[6u8; 32], [7u8; 32]],
            group_state_hash: [8u8; 32],
        };
        let payload = delta_signature_payload(context_id, delta_id, pk, Some(&pos)).unwrap();
        let sig = sk.sign(&payload).unwrap().to_bytes();
        assert!(verify_delta_signature(context_id, delta_id, pk, Some(&pos), &sig).is_ok());
    }

    #[test]
    fn verify_rejects_tampered_governance_position() {
        let (context_id, delta_id, sk, pk) = fixture();
        let pos_signed = calimero_context_config::types::GovernancePosition {
            group_id: calimero_context_config::types::ContextGroupId::from([5u8; 32]),
            governance_dag_heads: vec![[6u8; 32]],
            group_state_hash: [8u8; 32],
        };
        let payload = delta_signature_payload(context_id, delta_id, pk, Some(&pos_signed)).unwrap();
        let sig = sk.sign(&payload).unwrap().to_bytes();

        // Verifier reconstructs payload with a different position
        // (different `group_state_hash`); signature must not verify.
        // This is the per-cut binding property — a signature for one
        // governance cut can't be reused for a different cut even if
        // every other field matches.
        let pos_other = calimero_context_config::types::GovernancePosition {
            group_id: pos_signed.group_id,
            governance_dag_heads: pos_signed.governance_dag_heads.clone(),
            group_state_hash: [99u8; 32],
        };
        assert!(verify_delta_signature(context_id, delta_id, pk, Some(&pos_other), &sig).is_err());
    }
}
