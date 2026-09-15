//! The account root's public key, tagged with its algorithm.
//!
//! # Why it is shaped this way
//!
//! **A separate type from [`PublicKey`], not a widening of it.** `PublicKey` is a
//! bare `[u8; 32]` that is simultaneously a verification key, an X25519 agreement
//! input, a fixed-width RocksDB key component (`typenum::U32`, four column
//! families) and a CRDT replica id. A root key is none of those: it is never a
//! store key, never an ECDH input, never an index. Only the root can afford to
//! carry an algorithm tag, and only the root needs one — every secure element
//! that can hold a key holds P-256, and none holds Ed25519.
//!
//! **Ed25519 is variant 0, permanently.** Borsh writes the discriminant first, so
//! the tag is the first byte on the wire and the append-only rule applies: a
//! third algorithm is variant 2, and 0 never moves.
//!
//! **[`crate::AccountGenesis`] deliberately does NOT use this type.** The genesis
//! is the preimage of [`crate::AccountId`], so tagging it would change every
//! account id in existence. Instead an account is always *born* Ed25519 from a
//! recovery phrase and *rotates onto* hardware, which is what
//! [`crate::RootKeyHandoff`] is for. That constraint is what makes this change
//! cost one wire field rather than a namespace fork.
//!
//! Mixed chains need no special case: each handoff is verified with the
//! **outgoing** key, so an Ed25519 epoch 0 authorises a P-256 epoch 1 exactly as
//! it would authorise an Ed25519 one.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

/// A root signing key and the algorithm it is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
#[non_exhaustive]
pub enum RootPublicKey {
    /// Ed25519. The only algorithm a genesis key may be, and therefore the only
    /// one a recovery phrase can restore.
    Ed25519([u8; 32]),
    /// NIST P-256, SEC1 **compressed** — `0x02`/`0x03` ‖ X.
    ///
    /// Compressed rather than the 65-byte X9.63 form the platform APIs hand back,
    /// because the tag plus 33 bytes is the smaller thing to put on a wire that
    /// carries this inside a credential chain. Callers compress; see the
    /// platform notes in the design.
    P256([u8; 33]),
}

impl RootPublicKey {
    /// Verify `signature` over `payload` under this key.
    ///
    /// `payload` is always a 32-byte domain-separated digest — every statement a
    /// root signs is built by `domain_hash` — which is the shape both algorithms
    /// and every secure element accept without reshaping.
    ///
    /// # Errors
    /// [`RootKeyError`] if the key is malformed, the signature is malformed, or
    /// the signature does not verify.
    pub fn verify(&self, payload: &[u8; 32], signature: &[u8; 64]) -> Result<(), RootKeyError> {
        match self {
            Self::Ed25519(key) => PublicKey::from(*key)
                .verify_raw_signature(payload, signature)
                .map_err(|_ignored| RootKeyError::SignatureInvalid),
            Self::P256(key) => {
                use p256::ecdsa::signature::hazmat::PrehashVerifier as _;

                let verifying = p256::ecdsa::VerifyingKey::from_sec1_bytes(key)
                    .map_err(|_ignored| RootKeyError::MalformedKey)?;
                // Raw `r ‖ s`, never DER: the wire field is a fixed `[u8; 64]`,
                // and the platform APIs that emit DER convert before they get here.
                let sig = p256::ecdsa::Signature::from_slice(signature)
                    .map_err(|_ignored| RootKeyError::SignatureInvalid)?;
                // `verify_prehash`, not `verify`: the payload IS the digest. The
                // message-verifying call would hash it a second time.
                verifying
                    .verify_prehash(payload, &sig)
                    .map_err(|_ignored| RootKeyError::SignatureInvalid)
            }
        }
    }

    /// A fixed-width, algorithm-distinguishing encoding: `tag ‖ key ‖ zero-pad`.
    ///
    /// Fixed-width so it can be a `Copy` map key and a stable hash input, which
    /// borsh's variable-length form cannot be. The tag is first, so two keys of
    /// different algorithms can never collide however their bytes line up, and
    /// lexicographic order puts Ed25519 before P-256 — deterministic on every
    /// replica, which is what the handoff tie-break needs.
    #[must_use]
    pub const fn to_wire(&self) -> [u8; 34] {
        let mut out = [0u8; 34];
        match self {
            Self::Ed25519(key) => {
                out[0] = 0;
                let mut i = 0;
                while i < 32 {
                    out[i + 1] = key[i];
                    i += 1;
                }
            }
            Self::P256(key) => {
                out[0] = 1;
                let mut i = 0;
                while i < 33 {
                    out[i + 1] = key[i];
                    i += 1;
                }
            }
        }
        out
    }

    /// Which algorithm this key is, for logs and errors.
    #[must_use]
    pub const fn algorithm(&self) -> &'static str {
        match self {
            Self::Ed25519(_) => "ed25519",
            Self::P256(_) => "p256",
        }
    }
}

/// An Ed25519 root key, which is what a genesis always holds.
impl From<PublicKey> for RootPublicKey {
    fn from(key: PublicKey) -> Self {
        Self::Ed25519(*AsRef::<[u8; 32]>::as_ref(&key))
    }
}

/// Why a root key could not verify something.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum RootKeyError {
    /// The key bytes do not decode as a point on the named curve.
    #[error("the root key is malformed for its algorithm")]
    MalformedKey,
    /// The signature does not verify under this key.
    #[error("the signature does not verify under the root key")]
    SignatureInvalid,
}

#[cfg(test)]
mod tests {
    use super::RootPublicKey;

    /// Ed25519 must stay variant 0 or every existing handoff changes meaning.
    #[test]
    fn ed25519_is_variant_zero() {
        let encoded = borsh::to_vec(&RootPublicKey::Ed25519([7; 32])).expect("encodes");
        assert_eq!(
            encoded[0], 0,
            "Ed25519 must be borsh variant 0, permanently"
        );
        assert_eq!(encoded.len(), 33, "tag + 32 key bytes");
    }

    #[test]
    fn p256_is_variant_one_and_compressed() {
        let encoded = borsh::to_vec(&RootPublicKey::P256([2; 33])).expect("encodes");
        assert_eq!(encoded[0], 1);
        assert_eq!(encoded.len(), 34, "tag + 33 compressed key bytes");
    }
}
