//! Signed, self-authenticating blob-provider DHT records.
//!
//! A blob-provider record announces "peer P holds blob B in context C" on the
//! Kademlia DHT. Peers that resolve the record then dial P to fetch the blob.
//!
//! The record value used to be a bare `peer_id_bytes ‖ size` concatenation with
//! no authentication, so **any** peer could publish (or replicate) a record
//! naming an **arbitrary** peer id as the provider. A malicious peer could plant
//! records pointing queriers at peers of its choosing — a misdirection / eclipse
//! vector on blob discovery. (Blob *content* can't be forged this way, since
//! blobs are content-addressed and re-hashed on receipt; the risk is who you're
//! pointed at, and the wasted/observable requests that follow.)
//!
//! [`BlobProviderRecord`] binds the announcement to the announcing peer: the
//! value carries the provider's network public key and an Ed25519 signature
//! over `DOMAIN ‖ record_key ‖ peer_id ‖ size`, and verification requires both
//! that the signature is valid **and** that the embedded public key hashes to
//! the claimed peer id. A record can therefore only name the peer that signed
//! it — an attacker can announce *itself*, but cannot impersonate a victim or
//! name a peer whose key it does not hold. Verification is self-contained: the
//! public key travels in the record, so no external key distribution is needed.

use borsh::{BorshDeserialize, BorshSerialize};
use libp2p::identity::{Keypair, PublicKey};
use libp2p::PeerId;

/// Domain separator for the signed message. Bump the version suffix on any
/// change to the signed layout so signatures never cross protocol versions.
const DOMAIN: &[u8] = b"calimero:dht:blob-provider:v1";

/// The authenticated value stored under a blob-provider DHT record key.
///
/// Borsh-encoded into `Record::value`. This is a wire-format change from the
/// previous bare `peer_id ‖ size` layout — see the module docs.
///
/// Fields are private: the only ways to obtain one are [`Self::signed_value`]
/// (which produces a correctly signed record) and borsh-deserializing a value
/// through [`Self::verify`] (which authenticates it). This prevents callers
/// elsewhere in the crate from hand-constructing an unauthenticated record.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct BlobProviderRecord {
    /// Provider `PeerId::to_bytes()`.
    peer_id: Vec<u8>,
    /// Blob size in bytes.
    size: u64,
    /// Provider's network public key, protobuf-encoded. Must hash to `peer_id`.
    public_key: Vec<u8>,
    /// Ed25519 signature by the provider's network key over [`Self::signed_message`].
    signature: Vec<u8>,
}

impl BlobProviderRecord {
    /// Canonical bytes the signature covers:
    /// `DOMAIN ‖ len(record_key) ‖ record_key ‖ len(peer_id) ‖ peer_id ‖ size`.
    ///
    /// `record_key` is the full DHT key (`context_id ‖ blob_id`), so a record
    /// signed for one (context, blob) can't be lifted onto another key. The
    /// variable-length fields are length-prefixed (`u32` LE) so no crafted
    /// `record_key` / `peer_id` pair can shift the field boundary and collide
    /// with a different `(record_key, peer_id)` under the same signed message.
    fn signed_message(record_key: &[u8], peer_id: &[u8], size: u64) -> Vec<u8> {
        let mut message =
            Vec::with_capacity(DOMAIN.len() + 4 + record_key.len() + 4 + peer_id.len() + 8);
        message.extend_from_slice(DOMAIN);
        message.extend_from_slice(&(record_key.len() as u32).to_le_bytes());
        message.extend_from_slice(record_key);
        message.extend_from_slice(&(peer_id.len() as u32).to_le_bytes());
        message.extend_from_slice(peer_id);
        message.extend_from_slice(&size.to_le_bytes());
        message
    }

    /// Build a signed record value for `record_key`, provided by the local node
    /// (identified by `keypair`), advertising a blob of `size` bytes.
    pub fn signed_value(record_key: &[u8], keypair: &Keypair, size: u64) -> eyre::Result<Vec<u8>> {
        let peer_id = keypair.public().to_peer_id().to_bytes();
        let message = Self::signed_message(record_key, &peer_id, size);
        let signature = keypair.sign(&message)?;
        let record = Self {
            peer_id,
            size,
            public_key: keypair.public().encode_protobuf(),
            signature,
        };
        Ok(borsh::to_vec(&record)?)
    }

    /// Parse and authenticate a record value against its DHT key.
    ///
    /// Returns the provider `PeerId` only when the record is well-formed, the
    /// embedded public key hashes to the claimed peer id, and the signature
    /// verifies. Any failure yields `None` — the record must be dropped, not
    /// dispatched or stored.
    #[must_use]
    pub fn verify(record_key: &[u8], value: &[u8]) -> Option<PeerId> {
        let record = Self::try_from_slice(value).ok()?;

        let claimed_peer = PeerId::from_bytes(&record.peer_id).ok()?;
        let public_key = PublicKey::try_decode_protobuf(&record.public_key).ok()?;

        // Bind the record to its signer: the embedded key must hash to the
        // claimed peer id, so a record can only ever name the peer that holds
        // this key. Without this an attacker could pair a victim's peer id with
        // its own key + signature.
        if public_key.to_peer_id() != claimed_peer {
            return None;
        }

        let message = Self::signed_message(record_key, &record.peer_id, record.size);
        if !public_key.verify(&message, &record.signature) {
            return None;
        }

        Some(claimed_peer)
    }
}

#[cfg(test)]
mod tests {
    use libp2p::identity::Keypair;

    use super::*;

    fn key() -> Vec<u8> {
        // context_id (32) || blob_id (32)
        [[7u8; 32], [9u8; 32]].concat()
    }

    #[test]
    fn signed_value_verifies_and_returns_signer_peer() {
        let kp = Keypair::generate_ed25519();
        let value = BlobProviderRecord::signed_value(&key(), &kp, 4096).expect("sign");
        assert_eq!(
            BlobProviderRecord::verify(&key(), &value),
            Some(kp.public().to_peer_id())
        );
    }

    #[test]
    fn rejects_value_on_a_different_key() {
        let kp = Keypair::generate_ed25519();
        let value = BlobProviderRecord::signed_value(&key(), &kp, 4096).expect("sign");
        let other_key = [[1u8; 32], [2u8; 32]].concat();
        assert_eq!(BlobProviderRecord::verify(&other_key, &value), None);
    }

    #[test]
    fn rejects_tampered_size() {
        let kp = Keypair::generate_ed25519();
        let value = BlobProviderRecord::signed_value(&key(), &kp, 4096).expect("sign");
        let mut record = BlobProviderRecord::try_from_slice(&value).unwrap();
        record.size = 1; // signature was over size=4096
        let tampered = borsh::to_vec(&record).unwrap();
        assert_eq!(BlobProviderRecord::verify(&key(), &tampered), None);
    }

    #[test]
    fn rejects_peer_id_not_matching_embedded_key() {
        // Attacker pairs a victim's peer id with its own key + a signature it
        // can legitimately produce for that key. The peer-id/key binding check
        // must reject it.
        let attacker = Keypair::generate_ed25519();
        let victim = Keypair::generate_ed25519();

        let peer_id = victim.public().to_peer_id().to_bytes();
        let message = BlobProviderRecord::signed_message(&key(), &peer_id, 4096);
        let forged = BlobProviderRecord {
            peer_id,
            size: 4096,
            public_key: attacker.public().encode_protobuf(),
            signature: attacker.sign(&message).unwrap(),
        };
        let value = borsh::to_vec(&forged).unwrap();
        assert_eq!(BlobProviderRecord::verify(&key(), &value), None);
    }

    #[test]
    fn rejects_tampered_signature() {
        let kp = Keypair::generate_ed25519();
        let value = BlobProviderRecord::signed_value(&key(), &kp, 4096).expect("sign");
        let mut record = BlobProviderRecord::try_from_slice(&value).unwrap();
        record.signature[0] ^= 0xff;
        let tampered = borsh::to_vec(&record).unwrap();
        assert_eq!(BlobProviderRecord::verify(&key(), &tampered), None);
    }

    #[test]
    fn rejects_garbage_value() {
        assert_eq!(BlobProviderRecord::verify(&key(), b"not a record"), None);
    }
}
