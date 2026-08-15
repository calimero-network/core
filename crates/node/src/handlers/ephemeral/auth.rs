//! Envelope signature for ephemeral presence.
//!
//! Closes the anti-impersonation gap on the presence envelope: a holder of the
//! group key cannot publish presence claiming another member as `author`. The
//! author signs a canonical payload binding `(context_id, author, seq, key_id,
//! sent_at_ms, sha256(ciphertext))`; the receive path verifies before the
//! awareness store is touched.
//!
//! `author` and `seq` travel outside the AEAD ciphertext, and the AEAD is
//! sealed with empty associated data, so without this signature they are
//! rewritable in flight by a party that cannot even decrypt. Binding them here
//! — together with `key_id` and a hash of the ciphertext — is what makes the
//! envelope tamper-evident.
//!
//! **Modeled on** `node/primitives/src/sync/delta_auth.rs`, which solves the
//! identical problem for state deltas. This is a copied *shape*, not a
//! coupling: presence is not a state change, carries no `delta_id` and no
//! `governance_position`, and never reaches the DAG. The domain separators
//! differ precisely so a signature from one protocol can never verify in the
//! other.
//!
//! # Security — freshness is bound into the signature
//!
//! The signed payload carries the sender's `sent_at_ms` wall clock, and the
//! receive path refuses an envelope whose stamp is further than
//! [`PRESENCE_MAX_SKEW_MS`] from the receiver's own clock in either direction.
//!
//! This closes a replay hole that authorship alone could not: any gossipsub
//! mesh peer subscribed to a context's presence topic — which requires no
//! group key, only mesh membership — can record one valid envelope from
//! `author` at some `seq`. Without a freshness binding, once `author` goes
//! idle and the entry TTL-sweeps on receivers
//! ([`crate::handlers::ephemeral::PRESENCE_TTL_MS`], 7s), the recorder could
//! re-inject the exact same bytes forever: the signature still verifies, the
//! key can still be current, and with no local entry left `seq` looks fresh
//! again — leaving a departed peer rendered present indefinitely.
//!
//! `sent_at_ms` must live **inside** the signed payload. Carried alongside it,
//! the recorder would simply restamp it, since the AEAD is sealed with empty
//! associated data and the field rides in the clear.
//!
//! No I/O, no actix, no store access.

use borsh::BorshSerialize;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use sha2::{Digest, Sha256};

use crate::handlers::ephemeral::PRESENCE_MAX_SKEW_MS;

/// Domain separator prefixed to every ephemeral envelope signature payload.
///
/// Serialized as a typed field on [`EphemeralSignaturePayload`] so it is part
/// of the signed bytes; a signature produced for another protocol that happened
/// to borsh-serialize to the same shape cannot be replayed here.
///
/// The literal is part of the protocol — never change it without a wire-format
/// version bump.
pub const DOMAIN_SEPARATOR: &[u8; 20] = b"calimero/ephemeral/1";

/// Canonical payload for the presence envelope signature. Borsh-serialized and
/// signed by `author`'s ed25519 key. Serialization only — receivers rebuild it
/// from their own data and compare signature bytes.
#[derive(BorshSerialize)]
pub struct EphemeralSignaturePayload {
    pub domain: [u8; 20],
    pub context_id: ContextId,
    pub author: PublicKey,
    pub seq: u64,
    pub key_id: [u8; 32],
    /// The sender's wall clock (ms since the UNIX epoch) at publish time.
    /// Signed, so a mesh peer replaying a recorded envelope cannot restamp it;
    /// checked against the receiver's clock by [`is_fresh`].
    pub sent_at_ms: u64,
    /// `sha256(ciphertext)` — commits to the sealed slice without growing the
    /// signed buffer with the slice itself.
    pub ciphertext_hash: [u8; 32],
}

/// Is `sent_at_ms` close enough to `now_ms` to accept?
///
/// The window is symmetric: a far-future stamp is as suspicious as a stale one
/// (and, left unchecked, a stamp far enough ahead would stay "fresh" for as
/// long as the attacker cared to keep replaying it).
///
/// See [`PRESENCE_MAX_SKEW_MS`] for why the window is sized the way it is.
pub fn is_fresh(now_ms: u64, sent_at_ms: u64) -> bool {
    now_ms.abs_diff(sent_at_ms) <= PRESENCE_MAX_SKEW_MS
}

/// `sha256` over the sealed slice, as bound into the signature payload.
pub(crate) fn ciphertext_hash(ciphertext: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ciphertext);
    hasher.finalize().into()
}

/// Borsh-serialize the canonical payload. Used at sign time (publish path) and
/// verify time (receive path), so the two cannot drift.
pub fn ephemeral_signature_payload(
    context_id: ContextId,
    author: PublicKey,
    seq: u64,
    key_id: [u8; 32],
    sent_at_ms: u64,
    ciphertext: &[u8],
) -> Result<Vec<u8>, borsh::io::Error> {
    let payload = EphemeralSignaturePayload {
        domain: *DOMAIN_SEPARATOR,
        context_id,
        author,
        seq,
        key_id,
        sent_at_ms,
        ciphertext_hash: ciphertext_hash(ciphertext),
    };
    borsh::to_vec(&payload)
}

/// Verify a presence envelope signature against the canonical payload.
///
/// Returns `Ok(())` only on a valid signature.
///
/// **Caller contract:** the `author` passed here MUST be the same author bound
/// into the payload. This verifies that `author`'s key signed THESE bytes; it
/// does not check that invariant for you.
pub fn verify_ephemeral_signature(
    context_id: ContextId,
    author: PublicKey,
    seq: u64,
    key_id: [u8; 32],
    sent_at_ms: u64,
    ciphertext: &[u8],
    signature: &[u8; 64],
) -> eyre::Result<()> {
    let payload =
        ephemeral_signature_payload(context_id, author, seq, key_id, sent_at_ms, ciphertext)
            .map_err(|err| eyre::eyre!("failed to serialize ephemeral signature payload: {err}"))?;
    author
        .verify_raw_signature(&payload, signature)
        .map_err(|err| eyre::eyre!("ephemeral envelope signature verification failed: {err}"))
}

#[cfg(test)]
mod tests {
    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::PrivateKey;

    use super::*;

    /// Arbitrary but fixed publish stamp; freshness is checked by `is_fresh`,
    /// not by `verify_ephemeral_signature`, so these signature tests can use
    /// any value as long as sign and verify agree on it.
    const SENT_AT: u64 = 1_700_000_000_000;

    fn fixture() -> (ContextId, PrivateKey, [u8; 32], Vec<u8>) {
        let context_id = ContextId::from([0x11u8; 32]);
        let sk = PrivateKey::from([0x22u8; 32]);
        let key_id = [0x33u8; 32];
        let ciphertext = vec![9u8, 8, 7, 6];
        (context_id, sk, key_id, ciphertext)
    }

    #[test]
    fn sign_verify_roundtrip() {
        let (context_id, sk, key_id, ct) = fixture();
        let author = sk.public_key();
        let payload = ephemeral_signature_payload(context_id, author, 7, key_id, SENT_AT, &ct)
            .expect("payload");
        let sig = sk.sign(&payload).expect("sign").to_bytes();
        verify_ephemeral_signature(context_id, author, 7, key_id, SENT_AT, &ct, &sig)
            .expect("verify");
    }

    #[test]
    fn tampered_author_fails() {
        let (context_id, sk, key_id, ct) = fixture();
        let author = sk.public_key();
        let payload = ephemeral_signature_payload(context_id, author, 7, key_id, SENT_AT, &ct)
            .expect("payload");
        let sig = sk.sign(&payload).expect("sign").to_bytes();
        // Same signature, different claimed author — the impersonation case.
        let other = PrivateKey::from([0x44u8; 32]).public_key();
        assert!(
            verify_ephemeral_signature(context_id, other, 7, key_id, SENT_AT, &ct, &sig).is_err(),
            "a signature by one key must not verify under another author"
        );
    }

    #[test]
    fn tampered_seq_fails() {
        let (context_id, sk, key_id, ct) = fixture();
        let author = sk.public_key();
        let payload = ephemeral_signature_payload(context_id, author, 7, key_id, SENT_AT, &ct)
            .expect("payload");
        let sig = sk.sign(&payload).expect("sign").to_bytes();
        assert!(
            verify_ephemeral_signature(context_id, author, 8, key_id, SENT_AT, &ct, &sig).is_err()
        );
    }

    #[test]
    fn tampered_key_id_fails() {
        let (context_id, sk, key_id, ct) = fixture();
        let author = sk.public_key();
        let payload = ephemeral_signature_payload(context_id, author, 7, key_id, SENT_AT, &ct)
            .expect("payload");
        let sig = sk.sign(&payload).expect("sign").to_bytes();
        assert!(verify_ephemeral_signature(
            context_id,
            author,
            7,
            [0xAAu8; 32],
            SENT_AT,
            &ct,
            &sig
        )
        .is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let (context_id, sk, key_id, ct) = fixture();
        let author = sk.public_key();
        let payload = ephemeral_signature_payload(context_id, author, 7, key_id, SENT_AT, &ct)
            .expect("payload");
        let sig = sk.sign(&payload).expect("sign").to_bytes();
        assert!(verify_ephemeral_signature(
            context_id,
            author,
            7,
            key_id,
            SENT_AT,
            &[1u8, 2, 3],
            &sig
        )
        .is_err());
    }

    /// `sent_at_ms` is INSIDE the signed payload, not merely alongside it: a
    /// replayer who restamps the wire field to look fresh invalidates the
    /// signature. Without this binding the freshness check would be worthless,
    /// since the field rides in the clear outside the AEAD.
    #[test]
    fn tampered_sent_at_ms_fails() {
        let (context_id, sk, key_id, ct) = fixture();
        let author = sk.public_key();
        let payload = ephemeral_signature_payload(context_id, author, 7, key_id, SENT_AT, &ct)
            .expect("payload");
        let sig = sk.sign(&payload).expect("sign").to_bytes();
        assert!(
            verify_ephemeral_signature(context_id, author, 7, key_id, SENT_AT + 1, &ct, &sig)
                .is_err(),
            "restamping sent_at_ms must break the signature"
        );
    }

    #[test]
    fn freshness_window_is_symmetric_and_bounded() {
        let now = 1_000_000_000_000u64;
        assert!(is_fresh(now, now), "an unskewed stamp is fresh");
        assert!(
            is_fresh(now, now - PRESENCE_MAX_SKEW_MS),
            "a stamp exactly at the stale edge is still accepted"
        );
        assert!(
            is_fresh(now, now + PRESENCE_MAX_SKEW_MS),
            "a stamp exactly at the future edge is still accepted"
        );
        assert!(
            !is_fresh(now, now - PRESENCE_MAX_SKEW_MS - 1),
            "a stamp past the stale edge is refused"
        );
        assert!(
            !is_fresh(now, now + PRESENCE_MAX_SKEW_MS + 1),
            "a far-future stamp is refused too — the window is symmetric"
        );
    }

    #[test]
    fn domain_separator_is_exactly_the_protocol_literal() {
        // Guards against an accidental edit: the literal is wire protocol.
        assert_eq!(DOMAIN_SEPARATOR, b"calimero/ephemeral/1");
        assert_eq!(DOMAIN_SEPARATOR.len(), 20);
    }

    #[test]
    fn payload_binds_domain_separator() {
        // A payload built with a different domain must not verify — this is the
        // cross-protocol replay guard. Rebuild the payload by hand with the
        // delta domain and confirm the signature over it fails here.
        let (context_id, sk, key_id, ct) = fixture();
        let author = sk.public_key();
        let mut foreign = EphemeralSignaturePayload {
            domain: *DOMAIN_SEPARATOR,
            context_id,
            author,
            seq: 7,
            key_id,
            sent_at_ms: SENT_AT,
            ciphertext_hash: super::ciphertext_hash(&ct),
        };
        foreign.domain = *b"calimero/delta/1\0\0\0\0";
        let foreign_bytes = borsh::to_vec(&foreign).expect("borsh");
        let sig = sk.sign(&foreign_bytes).expect("sign").to_bytes();
        assert!(
            verify_ephemeral_signature(context_id, author, 7, key_id, SENT_AT, &ct, &sig).is_err(),
            "a signature over a foreign domain must not verify"
        );
    }
}
