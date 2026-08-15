//! Envelope signature for ephemeral presence.
//!
//! Closes the anti-impersonation gap on the presence envelope: a holder of the
//! group key cannot publish presence claiming another member as `author`. The
//! author signs a canonical payload binding `(context_id, author, seq, key_id,
//! sent_at_ms, nonce, sha256(ciphertext))`; the receive path verifies before
//! the awareness store is touched.
//!
//! **Every field that rides outside the AEAD is bound here.** `author`, `seq`,
//! `sent_at_ms` and `nonce` all travel in the clear, and the AEAD is sealed
//! with empty associated data, so without this signature they are rewritable in
//! flight by a party that cannot even decrypt. Binding them — together with
//! `key_id` and a hash of the ciphertext — is what makes the envelope
//! tamper-evident. The `nonce` in particular is not forgery-relevant (the
//! ciphertext hash is signed, so the sealed bytes cannot be swapped), but left
//! unbound it is a free tamper-and-drop lever: a relay could flip one byte, the
//! AEAD would fail, and the receive path would silently drop a still-validly-
//! signed update. Binding it turns that from an undetectable drop into a
//! signature failure, and costs 12 bytes in the signed buffer.
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
//! [`PRESENCE_MAX_SKEW_MS`] is set equal to
//! [`PRESENCE_TTL_MS`](crate::handlers::ephemeral::PRESENCE_TTL_MS) so the
//! window closes at exactly the moment a TTL sweep would make a recorded
//! envelope useful, leaving no interval in which a departed peer can be
//! resurrected. See that constant for the full trade.
//!
//! No I/O, no actix, no store access.

use borsh::BorshSerialize;
use calimero_crypto::Nonce;
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
    /// The AEAD nonce, exactly as it rides on the wire. Not forgery-relevant —
    /// `ciphertext_hash` already commits to the sealed bytes — but binding it
    /// removes the tamper-and-drop lever described in the module doc.
    pub nonce: Nonce,
    /// `sha256(ciphertext)` — commits to the sealed slice without growing the
    /// signed buffer with the slice itself.
    pub ciphertext_hash: [u8; 32],
}

/// The envelope fields a signature covers, as one value.
///
/// Sign and verify take this rather than a positional argument list: the two
/// must agree on every field, and at seven of them a swapped pair of same-typed
/// arguments (`seq` and `sent_at_ms` are both `u64`) would compile and silently
/// produce a signature nobody can verify.
#[derive(Clone, Copy, Debug)]
pub struct SignedEnvelope<'a> {
    pub context_id: ContextId,
    pub author: PublicKey,
    pub seq: u64,
    pub key_id: [u8; 32],
    pub sent_at_ms: u64,
    pub nonce: Nonce,
    /// The sealed slice as it rides on the wire; hashed into the payload.
    pub ciphertext: &'a [u8],
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
    envelope: SignedEnvelope<'_>,
) -> Result<Vec<u8>, borsh::io::Error> {
    let payload = EphemeralSignaturePayload {
        domain: *DOMAIN_SEPARATOR,
        context_id: envelope.context_id,
        author: envelope.author,
        seq: envelope.seq,
        key_id: envelope.key_id,
        sent_at_ms: envelope.sent_at_ms,
        nonce: envelope.nonce,
        ciphertext_hash: ciphertext_hash(envelope.ciphertext),
    };
    borsh::to_vec(&payload)
}

/// Verify a presence envelope signature against the canonical payload.
///
/// Returns `Ok(())` only on a valid signature.
///
/// **Caller contract:** `envelope.author` MUST be the author claimed on the
/// wire. This verifies that that key signed THESE bytes; it does not check the
/// invariant for you.
pub fn verify_ephemeral_signature(
    envelope: SignedEnvelope<'_>,
    signature: &[u8; 64],
) -> eyre::Result<()> {
    let payload = ephemeral_signature_payload(envelope)
        .map_err(|err| eyre::eyre!("failed to serialize ephemeral signature payload: {err}"))?;
    envelope
        .author
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
    const NONCE: Nonce = [0x5Au8; calimero_crypto::NONCE_LEN];

    fn fixture() -> (PrivateKey, Vec<u8>) {
        (PrivateKey::from([0x22u8; 32]), vec![9u8, 8, 7, 6])
    }

    /// The envelope every test signs, so each case can tamper with exactly one
    /// field and nothing else.
    fn envelope(author: PublicKey, ciphertext: &[u8]) -> SignedEnvelope<'_> {
        SignedEnvelope {
            context_id: ContextId::from([0x11u8; 32]),
            author,
            seq: 7,
            key_id: [0x33u8; 32],
            sent_at_ms: SENT_AT,
            nonce: NONCE,
            ciphertext,
        }
    }

    fn sign(sk: &PrivateKey, envelope: SignedEnvelope<'_>) -> [u8; 64] {
        let payload = ephemeral_signature_payload(envelope).expect("payload");
        sk.sign(&payload).expect("sign").to_bytes()
    }

    #[test]
    fn sign_verify_roundtrip() {
        let (sk, ct) = fixture();
        let env = envelope(sk.public_key(), &ct);
        verify_ephemeral_signature(env, &sign(&sk, env)).expect("verify");
    }

    #[test]
    fn tampered_author_fails() {
        let (sk, ct) = fixture();
        let env = envelope(sk.public_key(), &ct);
        let sig = sign(&sk, env);
        // Same signature, different claimed author — the impersonation case.
        let mut tampered = env;
        tampered.author = PrivateKey::from([0x44u8; 32]).public_key();
        assert!(
            verify_ephemeral_signature(tampered, &sig).is_err(),
            "a signature by one key must not verify under another author"
        );
    }

    #[test]
    fn tampered_seq_fails() {
        let (sk, ct) = fixture();
        let env = envelope(sk.public_key(), &ct);
        let sig = sign(&sk, env);
        let mut tampered = env;
        tampered.seq = 8;
        assert!(verify_ephemeral_signature(tampered, &sig).is_err());
    }

    #[test]
    fn tampered_key_id_fails() {
        let (sk, ct) = fixture();
        let env = envelope(sk.public_key(), &ct);
        let sig = sign(&sk, env);
        let mut tampered = env;
        tampered.key_id = [0xAAu8; 32];
        assert!(verify_ephemeral_signature(tampered, &sig).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let (sk, ct) = fixture();
        let env = envelope(sk.public_key(), &ct);
        let sig = sign(&sk, env);
        let mut tampered = env;
        tampered.ciphertext = &[1u8, 2, 3];
        assert!(verify_ephemeral_signature(tampered, &sig).is_err());
    }

    /// The AEAD nonce rides outside both the ciphertext and the AEAD's
    /// (empty) associated data. Unbound, a relay could flip a byte in it and
    /// the receive path would drop a legitimately-signed update at the decrypt
    /// step with nothing to distinguish that from ordinary loss. Bound, the
    /// tamper is a signature failure instead.
    #[test]
    fn tampered_nonce_fails() {
        let (sk, ct) = fixture();
        let env = envelope(sk.public_key(), &ct);
        let sig = sign(&sk, env);
        let mut tampered = env;
        tampered.nonce = [0x01u8; calimero_crypto::NONCE_LEN];
        assert!(
            verify_ephemeral_signature(tampered, &sig).is_err(),
            "flipping the nonce must break the signature, not silently fail the AEAD"
        );
    }

    /// `sent_at_ms` is INSIDE the signed payload, not merely alongside it: a
    /// replayer who restamps the wire field to look fresh invalidates the
    /// signature. Without this binding the freshness check would be worthless,
    /// since the field rides in the clear outside the AEAD.
    #[test]
    fn tampered_sent_at_ms_fails() {
        let (sk, ct) = fixture();
        let env = envelope(sk.public_key(), &ct);
        let sig = sign(&sk, env);
        let mut tampered = env;
        tampered.sent_at_ms = SENT_AT + 1;
        assert!(
            verify_ephemeral_signature(tampered, &sig).is_err(),
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
        let (sk, ct) = fixture();
        let env = envelope(sk.public_key(), &ct);
        let foreign = EphemeralSignaturePayload {
            domain: *b"calimero/delta/1\0\0\0\0",
            context_id: env.context_id,
            author: env.author,
            seq: env.seq,
            key_id: env.key_id,
            sent_at_ms: env.sent_at_ms,
            nonce: env.nonce,
            ciphertext_hash: super::ciphertext_hash(&ct),
        };
        let foreign_bytes = borsh::to_vec(&foreign).expect("borsh");
        let sig = sk.sign(&foreign_bytes).expect("sign").to_bytes();
        assert!(
            verify_ephemeral_signature(env, &sig).is_err(),
            "a signature over a foreign domain must not verify"
        );
    }
}
