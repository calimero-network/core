//! Wire-level discriminated message envelopes for the namespace and group
//! gossipsub topics, plus signed-ack / readiness primitives.
//!
//! Phase 2 of the three-phase governance contract introduces the
//! [`NamespaceTopicMsg`] and [`GroupTopicMsg`] enums to replace the bare
//! `borsh(SignedNamespaceOp)` / `borsh(SignedGroupOp)` payloads previously
//! published on `ns/<id>` and `group/<id>` topics. Only the [`Op`](NamespaceTopicMsg::Op)
//! variant is emitted at this stage; the [`Ack`](NamespaceTopicMsg::Ack),
//! [`ReadinessBeacon`](NamespaceTopicMsg::ReadinessBeacon) and
//! [`ReadinessProbe`](NamespaceTopicMsg::ReadinessProbe) variants reserve
//! discriminants for later phases (#5, #7, #8) so this phase is mergeable
//! independently and the wire format does not have to roll forward again
//! when those phases land.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use super::{GovernanceError, SignedGroupOp, SignedNamespaceOp};

/// Domain separation prefix for [`SignedAck`] signatures.
pub const ACK_SIGN_DOMAIN: &[u8] = b"calimero.ack.v1";

/// Domain separation prefix for [`SignedReadinessBeacon`] signatures.
pub const READINESS_BEACON_SIGN_DOMAIN: &[u8] = b"calimero.beacon.v1";

/// Topic-scoped op hash: `blake3(topic_id || borsh(SignedNamespaceOp))`.
///
/// The hash binds an op to the topic on which it was published so an ack
/// for one namespace cannot be replayed against an identical op on another
/// namespace. This is the canonical hash signed by ack senders and verified
/// by the originator's `AckRouter`.
pub fn hash_scoped_namespace(
    topic_id: &[u8],
    op: &SignedNamespaceOp,
) -> Result<[u8; 32], GovernanceError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(topic_id);
    let body = borsh::to_vec(op).map_err(|e| GovernanceError::BorshSerialize(e.to_string()))?;
    hasher.update(&body);
    Ok(*hasher.finalize().as_bytes())
}

/// Topic-scoped op hash: `blake3(topic_id || borsh(SignedGroupOp))`.
///
/// See [`hash_scoped_namespace`] for the cross-topic-replay rationale.
pub fn hash_scoped_group(topic_id: &[u8], op: &SignedGroupOp) -> Result<[u8; 32], GovernanceError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(topic_id);
    let body = borsh::to_vec(op).map_err(|e| GovernanceError::BorshSerialize(e.to_string()))?;
    hasher.update(&body);
    Ok(*hasher.finalize().as_bytes())
}

/// Receiver-signed acknowledgment of a successful op apply.
///
/// `op_hash` is the topic-scoped hash returned by [`hash_scoped_namespace`]
/// or [`hash_scoped_group`]. `signature` is an Ed25519 signature over
/// `signable_bytes(op_hash)` produced with the namespace identity key,
/// allowing the originator's `AckRouter` to attribute the ack to a
/// specific peer without trusting the gossip-layer source PeerId.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct SignedAck {
    pub op_hash: [u8; 32],
    pub signer_pubkey: PublicKey,
    pub signature: [u8; 64],
}

impl SignedAck {
    /// Canonical bytes that the ack signature covers:
    /// [`ACK_SIGN_DOMAIN`] || `op_hash`. The domain prefix prevents an
    /// attacker from substituting an ack signature for a structurally
    /// identical message on a different protocol surface.
    #[must_use]
    pub fn signable_bytes(op_hash: &[u8; 32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(ACK_SIGN_DOMAIN.len() + op_hash.len());
        out.extend_from_slice(ACK_SIGN_DOMAIN);
        out.extend_from_slice(op_hash);
        out
    }

    /// Verify the Ed25519 signature over [`Self::signable_bytes`].
    ///
    /// Consumed by Phase 3's `AckRouter::verify_ack` once that lands.
    pub fn verify_signature(&self) -> Result<(), GovernanceError> {
        let msg = Self::signable_bytes(&self.op_hash);
        self.signer_pubkey
            .verify_raw_signature(&msg, &self.signature)?;
        Ok(())
    }
}

/// Body of a readiness beacon — every field except the signature.
/// Borsh-serialized inside [`SignedReadinessBeacon::signable_bytes`] so
/// the Ed25519 signature covers all six fields and field-substitution
/// replays (e.g. flipping `strong` or rewinding `applied_through`) are
/// detected at verification time.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct SignableReadinessBeacon {
    pub namespace_id: [u8; 32],
    pub peer_pubkey: PublicKey,
    pub dag_head: [u8; 32],
    pub applied_through: u64,
    pub ts_millis: u64,
    pub strong: bool,
}

/// Periodic readiness signal a peer publishes on the namespace topic to
/// advertise its current DAG tip + applied-through level.
///
/// `strong = true` indicates the publisher has fully validated the tip
/// (peer-validated readiness); `strong = false` is the boot-grace
/// fallback emitted by a single locally-ready peer to break the
/// cold-fleet deadlock without requiring a quorum.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct SignedReadinessBeacon {
    pub namespace_id: [u8; 32],
    pub peer_pubkey: PublicKey,
    pub dag_head: [u8; 32],
    pub applied_through: u64,
    pub ts_millis: u64,
    pub strong: bool,
    pub signature: [u8; 64],
}

impl SignedReadinessBeacon {
    /// Strip the signature to obtain the signable body.
    #[must_use]
    pub fn to_signable(&self) -> SignableReadinessBeacon {
        SignableReadinessBeacon {
            namespace_id: self.namespace_id,
            peer_pubkey: self.peer_pubkey,
            dag_head: self.dag_head,
            applied_through: self.applied_through,
            ts_millis: self.ts_millis,
            strong: self.strong,
        }
    }

    /// Canonical bytes that the beacon signature covers:
    /// [`READINESS_BEACON_SIGN_DOMAIN`] || `borsh(SignableReadinessBeacon)`.
    pub fn signable_bytes(&self) -> Result<Vec<u8>, GovernanceError> {
        let body = borsh::to_vec(&self.to_signable())
            .map_err(|e| GovernanceError::BorshSerialize(e.to_string()))?;
        let mut out = Vec::with_capacity(READINESS_BEACON_SIGN_DOMAIN.len() + body.len());
        out.extend_from_slice(READINESS_BEACON_SIGN_DOMAIN);
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Verify the Ed25519 signature over [`Self::signable_bytes`].
    ///
    /// Consumed by Phase 7's `ReadinessManager` once that lands.
    pub fn verify_signature(&self) -> Result<(), GovernanceError> {
        let msg = self.signable_bytes()?;
        self.peer_pubkey
            .verify_raw_signature(&msg, &self.signature)?;
        Ok(())
    }
}

/// Solicits an out-of-cycle [`SignedReadinessBeacon`] from any peer
/// subscribed to the namespace topic. Used by joiners to short-circuit
/// the periodic beacon interval when waiting for `await_namespace_ready`.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct ReadinessProbe {
    pub namespace_id: [u8; 32],
    pub nonce: [u8; 16],
}

/// Discriminated envelope for messages on the `ns/<id>` topic.
///
/// Adding a variant requires a coordinated cluster upgrade (pre-1.0,
/// no rolling-upgrade compatibility path).
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum NamespaceTopicMsg {
    Op(SignedNamespaceOp),
    Ack(SignedAck),
    ReadinessBeacon(SignedReadinessBeacon),
    ReadinessProbe(ReadinessProbe),
}

/// Discriminated envelope for messages on the `group/<id>` topic.
///
/// Currently group ops travel inside `NamespaceOp::Group` on the namespace
/// topic; this enum reserves the wire format so a future migration to
/// per-group topics does not require another schema bump.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum GroupTopicMsg {
    Op(SignedGroupOp),
    Ack(SignedAck),
    ReadinessBeacon(SignedReadinessBeacon),
    ReadinessProbe(ReadinessProbe),
}

#[cfg(test)]
mod tests {
    use calimero_primitives::identity::PrivateKey;

    use super::*;

    #[test]
    fn signed_ack_roundtrip() {
        let ack = SignedAck {
            op_hash: [7u8; 32],
            signer_pubkey: PrivateKey::random(&mut rand::thread_rng()).public_key(),
            signature: [9u8; 64],
        };
        let bytes = borsh::to_vec(&ack).expect("ser");
        let parsed: SignedAck = borsh::from_slice(&bytes).expect("de");
        assert_eq!(parsed.op_hash, ack.op_hash);
        assert_eq!(parsed.signature, ack.signature);
    }

    #[test]
    fn namespace_topic_msg_discriminates_kinds() {
        let probe = NamespaceTopicMsg::ReadinessProbe(ReadinessProbe {
            namespace_id: [1u8; 32],
            nonce: [2u8; 16],
        });
        let bytes = borsh::to_vec(&probe).expect("ser");
        let parsed: NamespaceTopicMsg = borsh::from_slice(&bytes).expect("de");
        match parsed {
            NamespaceTopicMsg::ReadinessProbe(p) => {
                assert_eq!(p.namespace_id, [1u8; 32]);
                assert_eq!(p.nonce, [2u8; 16]);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn group_topic_msg_discriminates_kinds() {
        let beacon = GroupTopicMsg::ReadinessBeacon(SignedReadinessBeacon {
            namespace_id: [3u8; 32],
            peer_pubkey: PrivateKey::random(&mut rand::thread_rng()).public_key(),
            dag_head: [4u8; 32],
            applied_through: 17,
            ts_millis: 42,
            strong: true,
            signature: [5u8; 64],
        });
        let bytes = borsh::to_vec(&beacon).expect("ser");
        let parsed: GroupTopicMsg = borsh::from_slice(&bytes).expect("de");
        match parsed {
            GroupTopicMsg::ReadinessBeacon(b) => {
                assert_eq!(b.namespace_id, [3u8; 32]);
                assert_eq!(b.applied_through, 17);
                assert!(b.strong);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn hash_scoped_namespace_is_topic_bound() {
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let op = SignedNamespaceOp::sign(
            &sk,
            [0u8; 32],
            Vec::new(),
            [0u8; 32],
            0,
            super::super::NamespaceOp::Root(super::super::RootOp::AdminChanged {
                new_admin: sk.public_key(),
            }),
        )
        .expect("sign");
        let h_a = hash_scoped_namespace(b"ns/aaaa", &op).expect("hash a");
        let h_b = hash_scoped_namespace(b"ns/bbbb", &op).expect("hash b");
        assert_ne!(h_a, h_b, "topic-scoped hash must differ across topics");
    }

    #[test]
    fn signed_ack_verify_round_trip() {
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let op_hash = [42u8; 32];
        let msg = SignedAck::signable_bytes(&op_hash);
        let signature = sk.sign(&msg).expect("sign").to_bytes();
        let ack = SignedAck {
            op_hash,
            signer_pubkey: sk.public_key(),
            signature,
        };
        ack.verify_signature().expect("valid ack must verify");
    }

    #[test]
    fn signed_ack_rejects_tampered_op_hash() {
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let op_hash = [42u8; 32];
        let msg = SignedAck::signable_bytes(&op_hash);
        let signature = sk.sign(&msg).expect("sign").to_bytes();
        let ack = SignedAck {
            op_hash: [0u8; 32], // tampered after signing
            signer_pubkey: sk.public_key(),
            signature,
        };
        assert!(
            ack.verify_signature().is_err(),
            "verify must reject mutated op_hash"
        );
    }

    #[test]
    fn signed_ack_rejects_wrong_domain() {
        // An attacker cannot lift a SignedAck signature from another protocol
        // surface that signs a 32-byte hash without the calimero.ack.v1 prefix.
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let op_hash = [42u8; 32];
        let signature = sk.sign(&op_hash).expect("sign").to_bytes(); // signed without domain prefix
        let ack = SignedAck {
            op_hash,
            signer_pubkey: sk.public_key(),
            signature,
        };
        assert!(
            ack.verify_signature().is_err(),
            "verify must reject signature without ACK domain prefix"
        );
    }

    #[test]
    fn signed_readiness_beacon_verify_round_trip() {
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let mut beacon = SignedReadinessBeacon {
            namespace_id: [7u8; 32],
            peer_pubkey: sk.public_key(),
            dag_head: [9u8; 32],
            applied_through: 42,
            ts_millis: 1_700_000_000_000,
            strong: true,
            signature: [0u8; 64],
        };
        beacon.signature = sk
            .sign(&beacon.signable_bytes().expect("signable"))
            .expect("sign")
            .to_bytes();
        beacon.verify_signature().expect("valid beacon must verify");
    }

    #[test]
    fn signed_readiness_beacon_rejects_strong_flip() {
        // Field-substitution attack: flipping `strong` from false to true
        // must break the signature.
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let mut beacon = SignedReadinessBeacon {
            namespace_id: [7u8; 32],
            peer_pubkey: sk.public_key(),
            dag_head: [9u8; 32],
            applied_through: 42,
            ts_millis: 1_700_000_000_000,
            strong: false,
            signature: [0u8; 64],
        };
        beacon.signature = sk
            .sign(&beacon.signable_bytes().expect("signable"))
            .expect("sign")
            .to_bytes();
        beacon.strong = true; // tampered after signing
        assert!(
            beacon.verify_signature().is_err(),
            "verify must reject mutated `strong` flag"
        );
    }

    #[test]
    fn signed_readiness_beacon_rejects_applied_through_rewind() {
        // Replay/rewind attack: substituting an older applied_through
        // must break the signature.
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let mut beacon = SignedReadinessBeacon {
            namespace_id: [7u8; 32],
            peer_pubkey: sk.public_key(),
            dag_head: [9u8; 32],
            applied_through: 100,
            ts_millis: 1_700_000_000_000,
            strong: true,
            signature: [0u8; 64],
        };
        beacon.signature = sk
            .sign(&beacon.signable_bytes().expect("signable"))
            .expect("sign")
            .to_bytes();
        beacon.applied_through = 50; // rewound after signing
        assert!(
            beacon.verify_signature().is_err(),
            "verify must reject rewound applied_through"
        );
    }
}
