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

use super::{SignedGroupOp, SignedNamespaceOp};

/// Topic-scoped op hash: `blake3(topic_id || borsh(SignedNamespaceOp))`.
///
/// The hash binds an op to the topic on which it was published so an ack
/// for one namespace cannot be replayed against an identical op on another
/// namespace. This is the canonical hash signed by ack senders and verified
/// by the originator's `AckRouter`.
pub fn hash_scoped_namespace(topic_id: &[u8], op: &SignedNamespaceOp) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(topic_id);
    hasher.update(&borsh::to_vec(op).expect("borsh::to_vec on SignedNamespaceOp"));
    *hasher.finalize().as_bytes()
}

/// Topic-scoped op hash: `blake3(topic_id || borsh(SignedGroupOp))`.
///
/// See [`hash_scoped_namespace`] for the cross-topic-replay rationale.
pub fn hash_scoped_group(topic_id: &[u8], op: &SignedGroupOp) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(topic_id);
    hasher.update(&borsh::to_vec(op).expect("borsh::to_vec on SignedGroupOp"));
    *hasher.finalize().as_bytes()
}

/// Receiver-signed acknowledgment of a successful op apply.
///
/// `op_hash` is the topic-scoped hash returned by [`hash_scoped_namespace`]
/// or [`hash_scoped_group`]. `signature` is an Ed25519 signature over
/// `op_hash` produced with the namespace identity key, allowing the
/// originator's `AckRouter` to attribute the ack to a specific peer
/// without trusting the gossip-layer source PeerId.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct SignedAck {
    pub op_hash: [u8; 32],
    pub signer_pubkey: PublicKey,
    pub signature: [u8; 64],
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
        let h_a = hash_scoped_namespace(b"ns/aaaa", &op);
        let h_b = hash_scoped_namespace(b"ns/bbbb", &op);
        assert_ne!(h_a, h_b, "topic-scoped hash must differ across topics");
    }
}
