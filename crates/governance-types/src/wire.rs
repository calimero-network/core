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

/// Domain separation prefix for [`SignedMigrationHeartbeat`] signatures.
pub const MIGRATION_HEARTBEAT_SIGN_DOMAIN: &[u8] = b"calimero.migheartbeat.v1";

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

/// Body of a migration heartbeat — every field except the signature.
/// Borsh-serialized inside [`SignedMigrationHeartbeat::signable_bytes`] so
/// the Ed25519 signature covers all seven fields and field-substitution
/// replays (e.g. zeroing `residue_identity` to fake a completed migration)
/// are detected at verification time.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct SignableMigrationHeartbeat {
    pub namespace_id: [u8; 32],
    pub peer_pubkey: PublicKey,
    /// Schema/binary version the publishing node has loaded.
    pub schema_version: u32,
    /// Unconverted Convergent ("auto") contexts still pending (from the 6a marker).
    pub residue_auto: u64,
    /// Unconverted identity-gated entries still pending (from the 6c.6 local scan).
    pub residue_identity: u64,
    /// Governance HLC the publisher has synced/applied through.
    pub synced_up_to_hlc: u64,
    pub ts_millis: u64,
}

/// Ephemeral, per-node migration-status gossip a peer publishes on the
/// namespace topic to advertise its loaded `schema_version` and remaining
/// unconverted residue. Purely observability telemetry — it is signed TTL
/// gossip, NOT replicated governance state, and must never be treated as a
/// migration gate.
///
/// `residue_auto == 0 && residue_identity == 0 && schema_version >= target`
/// across every pinned cohort member is what a rollup reads as "all migrated";
/// the signature prevents a peer from forging another peer's completion.
// MAINTENANCE: this struct has a hand-written `BorshDeserialize` (below) that
// reads these fields positionally. If you ADD / REMOVE / REORDER any field
// here, update that impl in lockstep (new fields go AFTER migration_failed as
// another trailing read, or behind a version discriminant). The round-trip +
// mixed-fleet tests (serialize via this derive, deserialize via the custom impl)
// fail loudly on a desync — keep them in step.
#[derive(Debug, Clone, BorshSerialize)]
pub struct SignedMigrationHeartbeat {
    pub namespace_id: [u8; 32],
    pub peer_pubkey: PublicKey,
    pub schema_version: u32,
    pub residue_auto: u64,
    pub residue_identity: u64,
    pub synced_up_to_hlc: u64,
    pub ts_millis: u64,
    pub signature: [u8; 64],
    /// Self-reported pending-authored count (sum across the publisher's
    /// namespace contexts; 6f). DELIBERATELY OUTSIDE the signature and the
    /// signable body: it is best-effort advisory telemetry (decision #3), not a
    /// migration gate — the signed residue_* fields cover completion. Appended
    /// as the trailing field so a heartbeat from an older node (which omits it)
    /// still deserializes (EOF ⇒ 0) and verifies against the unchanged 7-field
    /// signed body — full mixed-fleet compatibility, no version discriminator.
    pub authored_remaining: u64,
    /// Self-reported migration-failure reason as a discriminant: `0` = none,
    /// `1` = the developer's migration-check aborted, `2` = the migrate apply
    /// errored. Like `authored_remaining`, DELIBERATELY OUTSIDE the signature —
    /// advisory telemetry, not a gate (a forged value can only make the
    /// publisher's OWN status look failed; completion is covered by the signed
    /// residue fields). The trailing byte after `authored_remaining`, so a node
    /// that omits it still deserializes (EOF ⇒ 0) and verifies unchanged.
    pub migration_failed: u8,
}

/// Read an optional trailing fixed-width LE value: `None` when no trailing field
/// is present (clean EOF before any byte — an old heartbeat), `Some` when the
/// full width is present, `Err` on a genuine partial read. A leading 1-byte
/// sentinel probe distinguishes "absent" from "present" without depending on
/// `Read::read` returning `Ok(0)` exactly at the field boundary; once any byte
/// is read, `read_exact` fills the rest and a short read is a hard error. No
/// error-message-text matching (not stable across borsh versions).
fn read_trailing<R: borsh::io::Read, const N: usize>(
    reader: &mut R,
) -> borsh::io::Result<Option<[u8; N]>> {
    let mut buf = [0u8; N];
    // Probe one byte. Loop over Interrupted; a true EOF here (0 bytes) means
    // the trailing field is absent (old format).
    let read_first = loop {
        match reader.read(&mut buf[..1]) {
            Ok(n) => break n,
            Err(e) if e.kind() == borsh::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    };
    if read_first == 0 {
        return Ok(None);
    }
    // A byte was present ⇒ the field must be complete; a short read is an error.
    reader.read_exact(&mut buf[1..])?;
    Ok(Some(buf))
}

// Custom deserialize: `authored_remaining` is an unsigned trailing field added
// after the original layout. Read the original fields, then tolerate a clean
// EOF (old heartbeat ⇒ 0) by byte count rather than matching borsh's error text.
//
// FIELD ORDER CONTRACT: borsh is positional (no field names), so this reader
// MUST deserialize the prefix fields in the exact order the derived
// `BorshSerialize` above writes them — namespace_id, peer_pubkey, schema_version,
// residue_auto, residue_identity, synced_up_to_hlc, ts_millis, signature — then
// the trailing authored_remaining, then migration_failed. Reordering/adding a
// field above without updating this reader silently misreads. The round-trip +
// mixed-fleet tests (which serialize via the derive and deserialize via this
// impl) guard against such a desync; keep them in step. A future field should
// go AFTER migration_failed (another trailing read) or behind a discriminant.
impl BorshDeserialize for SignedMigrationHeartbeat {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        let namespace_id = <[u8; 32]>::deserialize_reader(reader)?;
        let peer_pubkey = PublicKey::deserialize_reader(reader)?;
        let schema_version = u32::deserialize_reader(reader)?;
        let residue_auto = u64::deserialize_reader(reader)?;
        let residue_identity = u64::deserialize_reader(reader)?;
        let synced_up_to_hlc = u64::deserialize_reader(reader)?;
        let ts_millis = u64::deserialize_reader(reader)?;
        let signature = <[u8; 64]>::deserialize_reader(reader)?;
        // borsh integers are little-endian; absent trailing bytes ⇒ old hb ⇒ 0.
        let authored_remaining = read_trailing::<_, 8>(reader)?.map_or(0, u64::from_le_bytes);
        // migration_failed is the NEXT trailing byte (after authored_remaining).
        // Absent on any node that predates it ⇒ 0 (no failure on record).
        let migration_failed = read_trailing::<_, 1>(reader)?.map_or(0, |[b]| b);
        Ok(Self {
            namespace_id,
            peer_pubkey,
            schema_version,
            residue_auto,
            residue_identity,
            synced_up_to_hlc,
            ts_millis,
            signature,
            authored_remaining,
            migration_failed,
        })
    }
}

impl SignedMigrationHeartbeat {
    /// Strip the signature to obtain the signable body.
    #[must_use]
    pub fn to_signable(&self) -> SignableMigrationHeartbeat {
        SignableMigrationHeartbeat {
            namespace_id: self.namespace_id,
            peer_pubkey: self.peer_pubkey,
            schema_version: self.schema_version,
            residue_auto: self.residue_auto,
            residue_identity: self.residue_identity,
            synced_up_to_hlc: self.synced_up_to_hlc,
            ts_millis: self.ts_millis,
        }
    }

    /// Canonical bytes that the heartbeat signature covers:
    /// [`MIGRATION_HEARTBEAT_SIGN_DOMAIN`] || `borsh(SignableMigrationHeartbeat)`.
    pub fn signable_bytes(&self) -> Result<Vec<u8>, GovernanceError> {
        let body = borsh::to_vec(&self.to_signable())
            .map_err(|e| GovernanceError::BorshSerialize(e.to_string()))?;
        let mut out = Vec::with_capacity(MIGRATION_HEARTBEAT_SIGN_DOMAIN.len() + body.len());
        out.extend_from_slice(MIGRATION_HEARTBEAT_SIGN_DOMAIN);
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Verify the Ed25519 signature over [`Self::signable_bytes`].
    ///
    /// Consumed by 6c.8's `MigrationStatusCache` ingest once that lands.
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
#[allow(
    clippy::large_enum_variant,
    reason = "gossip/stored message: constructed, borsh-serialized, then dropped; boxing the common large variant adds a per-message heap allocation without real benefit"
)]
pub enum NamespaceTopicMsg {
    Op(SignedNamespaceOp),
    Ack(SignedAck),
    ReadinessBeacon(SignedReadinessBeacon),
    ReadinessProbe(ReadinessProbe),
    MigrationHeartbeat(SignedMigrationHeartbeat),
}

/// Discriminated envelope for messages on the `group/<id>` topic.
///
/// Currently group ops travel inside `NamespaceOp::Group` on the namespace
/// topic; this enum reserves the wire format so a future migration to
/// per-group topics does not require another schema bump.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
#[allow(
    clippy::large_enum_variant,
    reason = "gossip/stored message: constructed, borsh-serialized, then dropped; boxing the common large variant adds a per-message heap allocation without real benefit"
)]
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

    #[test]
    fn signed_migration_heartbeat_verify_round_trip() {
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let mut hb = SignedMigrationHeartbeat {
            namespace_id: [7u8; 32],
            peer_pubkey: sk.public_key(),
            schema_version: 2,
            residue_auto: 5,
            residue_identity: 3,
            synced_up_to_hlc: 99,
            ts_millis: 1_700_000_000_000,
            signature: [0u8; 64],
            authored_remaining: 0,
            migration_failed: 0,
        };
        hb.signature = sk
            .sign(&hb.signable_bytes().expect("signable"))
            .expect("sign")
            .to_bytes();
        hb.verify_signature().expect("valid heartbeat must verify");
    }

    #[test]
    fn signed_migration_heartbeat_rejects_residue_identity_flip() {
        // Field-substitution attack: rewriting `residue_identity` to 0 to
        // fake a completed migration must break the signature.
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let mut hb = SignedMigrationHeartbeat {
            namespace_id: [7u8; 32],
            peer_pubkey: sk.public_key(),
            schema_version: 2,
            residue_auto: 0,
            residue_identity: 4,
            synced_up_to_hlc: 99,
            ts_millis: 1_700_000_000_000,
            signature: [0u8; 64],
            authored_remaining: 0,
            migration_failed: 0,
        };
        hb.signature = sk
            .sign(&hb.signable_bytes().expect("signable"))
            .expect("sign")
            .to_bytes();
        hb.residue_identity = 0; // tampered after signing
        assert!(
            hb.verify_signature().is_err(),
            "verify must reject mutated `residue_identity`"
        );
    }

    // 6f: authored_remaining is an UNSIGNED trailing field. A new heartbeat
    // round-trips it; an old-format heartbeat (no trailing bytes) deserializes
    // to 0 and still verifies (signature never covered it); tampering it does
    // not break verification. This is the mixed-fleet back-compat guarantee.
    #[test]
    fn migration_heartbeat_authored_remaining_unsigned_and_eof_tolerant() {
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let mut hb = SignedMigrationHeartbeat {
            namespace_id: [9u8; 32],
            peer_pubkey: sk.public_key(),
            schema_version: 2,
            residue_auto: 0,
            residue_identity: 0,
            synced_up_to_hlc: 50,
            ts_millis: 1_700_000_000_000,
            signature: [0u8; 64],
            authored_remaining: 5,
            migration_failed: 0,
        };
        hb.signature = sk
            .sign(&hb.signable_bytes().expect("signable"))
            .expect("sign")
            .to_bytes();
        assert!(hb.verify_signature().is_ok(), "freshly signed verifies");

        // New heartbeat round-trips the field.
        let bytes = borsh::to_vec(&hb).expect("ser");
        let back = SignedMigrationHeartbeat::try_from_slice(&bytes).expect("de");
        assert_eq!(back.authored_remaining, 5);
        assert!(back.verify_signature().is_ok());

        // Old-format heartbeat: drop BOTH trailing fields (authored_remaining
        // u64 + migration_failed u8). Both default to 0 and the signature (over
        // the unchanged 7-field body) still verifies.
        let legacy = &bytes[..bytes.len() - core::mem::size_of::<u64>() - 1];
        let old = SignedMigrationHeartbeat::try_from_slice(legacy).expect("de legacy");
        assert_eq!(old.authored_remaining, 0, "absent trailing field ⇒ 0");
        assert_eq!(old.migration_failed, 0, "absent trailing field ⇒ 0");
        assert!(
            old.verify_signature().is_ok(),
            "signature unaffected by the unsigned trailing fields"
        );

        // Tampering the advisory field does not break verification.
        let mut tampered = hb.clone();
        tampered.authored_remaining = 999;
        assert!(
            tampered.verify_signature().is_ok(),
            "authored_remaining is unsigned advisory telemetry"
        );
    }

    // migration_failed is the trailing byte AFTER authored_remaining: a fresh
    // heartbeat round-trips it; a heartbeat that carries authored_remaining but
    // omits migration_failed (a node from before this field) reads it as 0; it
    // is unsigned advisory telemetry, so tampering it never breaks verification.
    #[test]
    fn migration_heartbeat_migration_failed_unsigned_and_eof_tolerant() {
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let mut hb = SignedMigrationHeartbeat {
            namespace_id: [7u8; 32],
            peer_pubkey: sk.public_key(),
            schema_version: 2,
            residue_auto: 0,
            residue_identity: 0,
            synced_up_to_hlc: 50,
            ts_millis: 1_700_000_000_000,
            signature: [0u8; 64],
            authored_remaining: 5,
            migration_failed: 1, // check-aborted
        };
        hb.signature = sk
            .sign(&hb.signable_bytes().expect("signable"))
            .expect("sign")
            .to_bytes();

        // Fresh heartbeat round-trips both trailing fields.
        let bytes = borsh::to_vec(&hb).expect("ser");
        let back = SignedMigrationHeartbeat::try_from_slice(&bytes).expect("de");
        assert_eq!(back.authored_remaining, 5);
        assert_eq!(back.migration_failed, 1);
        assert!(back.verify_signature().is_ok());

        // A node that carries authored_remaining but omits migration_failed:
        // drop only the final byte. authored_remaining is preserved, the absent
        // migration_failed reads as 0, and the signature still verifies.
        let no_failed = &bytes[..bytes.len() - 1];
        let prior = SignedMigrationHeartbeat::try_from_slice(no_failed).expect("de prior");
        assert_eq!(prior.authored_remaining, 5, "preceding field intact");
        assert_eq!(prior.migration_failed, 0, "absent trailing byte ⇒ 0");
        assert!(prior.verify_signature().is_ok());

        // Tampering the advisory field does not break verification.
        let mut tampered = hb.clone();
        tampered.migration_failed = 2;
        assert!(
            tampered.verify_signature().is_ok(),
            "migration_failed is unsigned advisory telemetry"
        );
    }

    #[test]
    fn signed_migration_heartbeat_rejects_wrong_domain() {
        // An attacker cannot lift a heartbeat signature from another protocol
        // surface that signs borsh(body) without the migration-heartbeat
        // domain prefix.
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let hb_unsigned = SignableMigrationHeartbeat {
            namespace_id: [7u8; 32],
            peer_pubkey: sk.public_key(),
            schema_version: 2,
            residue_auto: 0,
            residue_identity: 0,
            synced_up_to_hlc: 99,
            ts_millis: 1_700_000_000_000,
        };
        let body = borsh::to_vec(&hb_unsigned).expect("ser");
        let signature = sk.sign(&body).expect("sign").to_bytes(); // no domain prefix
        let hb = SignedMigrationHeartbeat {
            namespace_id: hb_unsigned.namespace_id,
            peer_pubkey: hb_unsigned.peer_pubkey,
            schema_version: hb_unsigned.schema_version,
            residue_auto: hb_unsigned.residue_auto,
            residue_identity: hb_unsigned.residue_identity,
            synced_up_to_hlc: hb_unsigned.synced_up_to_hlc,
            ts_millis: hb_unsigned.ts_millis,
            signature,
            authored_remaining: 0,
            migration_failed: 0,
        };
        assert!(
            hb.verify_signature().is_err(),
            "verify must reject signature without migration-heartbeat domain prefix"
        );
    }

    #[test]
    fn namespace_topic_msg_migration_heartbeat_roundtrip() {
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let envelope = NamespaceTopicMsg::MigrationHeartbeat(SignedMigrationHeartbeat {
            namespace_id: [3u8; 32],
            peer_pubkey: sk.public_key(),
            schema_version: 2,
            residue_auto: 7,
            residue_identity: 1,
            synced_up_to_hlc: 1234,
            ts_millis: 42,
            signature: [5u8; 64],
            authored_remaining: 0,
            migration_failed: 0,
        });
        let bytes = borsh::to_vec(&envelope).expect("ser");
        let parsed: NamespaceTopicMsg = borsh::from_slice(&bytes).expect("de");
        match parsed {
            NamespaceTopicMsg::MigrationHeartbeat(hb) => {
                assert_eq!(hb.namespace_id, [3u8; 32]);
                assert_eq!(hb.schema_version, 2);
                assert_eq!(hb.residue_auto, 7);
                assert_eq!(hb.residue_identity, 1);
                assert_eq!(hb.synced_up_to_hlc, 1234);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
