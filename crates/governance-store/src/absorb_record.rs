//! Borsh-serializable mirror of [`BufferedDelta`] for durable absorb storage.
//!
//! [`BufferedDelta`] is deliberately NOT Borsh-derivable — it carries a
//! `libp2p::PeerId` (no clean Borsh derive). This hand-written mirror holds
//! every field in a Borsh-friendly shape (`source_peer` as the raw
//! `PeerId::to_bytes()` vector). `from_buffered` / `into_buffered` convert
//! losslessly; the `PeerId` parse back can fail, so `into_buffered` returns a
//! `Result`. Keep the serialization concern isolated here rather than deriving
//! Borsh on `BufferedDelta` itself.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_context_config::types::GovernancePosition;
use calimero_node_primitives::delta_buffer::BufferedDelta;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::logical_clock::HybridTimestamp;
use eyre::{Result as EyreResult, WrapErr};

/// Durable, Borsh-serializable mirror of a [`BufferedDelta`].
///
/// Every field mirrors `BufferedDelta` 1:1 except `source_peer`, which is
/// stored as the raw `PeerId::to_bytes()` byte vector (the `PeerId` type has no
/// clean Borsh derive). `GovernancePosition`, `HybridTimestamp`, `Hash`, and
/// `PublicKey` all carry their own Borsh impls, so they are mirrored directly.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct AbsorbRecord {
    /// Delta ID.
    pub id: [u8; 32],
    /// Parent IDs.
    pub parents: Vec<[u8; 32]>,
    /// HLC timestamp — full `(time, id)` tuple, preserved verbatim.
    pub hlc: HybridTimestamp,
    /// Serialized (encrypted) payload — the original signed bytes, never
    /// translated (translating would break `payload_for_signing`).
    pub payload: Vec<u8>,
    /// Nonce for decryption (12 bytes for XChaCha20-Poly1305).
    pub nonce: [u8; 12],
    /// Author public key.
    pub author_id: PublicKey,
    /// Expected root hash after applying this delta.
    pub root_hash: Hash,
    /// Optional serialized events (for handler execution after replay).
    pub events: Option<Vec<u8>>,
    /// Source peer ID as `PeerId::to_bytes()` (no clean Borsh derive on
    /// `PeerId`, so we round-trip the byte form).
    pub source_peer: Vec<u8>,
    /// Group key identifier for decryption.
    pub key_id: [u8; 32],
    /// Cross-DAG reference — preserved so the apply-time authorization check
    /// fires correctly on replay.
    pub governance_position: Option<GovernancePosition>,
    /// Per-delta envelope signature.
    pub delta_signature: Option<[u8; 64]>,
    /// Governance-pending drain re-buffer counter.
    pub governance_drain_attempts: u8,
    /// App-schema key the sender stamped onto the state-delta wire.
    pub producing_app_key: Option<[u8; 32]>,
    /// Set when this record holds a buffered sync-repair leaf rather than a
    /// straggler delta. The sync-repair paths bypass the gossip state-delta
    /// fence, so a receiver on an older reader buffers a future-schema leaf here
    /// instead of LWW-storing unreadable bytes. `None` for the delta-absorb
    /// path; the drain branches on this tag (a delta replays verbatim, a leaf
    /// re-applies through `apply_leaf_with_crdt_merge`).
    pub leaf: Option<AbsorbedLeaf>,
    /// Set when this record holds a buffered snapshot entity rather than a delta
    /// or a leaf. The snapshot apply path persists each verified entity via a
    /// raw `handle.put` (no `TreeLeafData`), so a receiver on an older reader
    /// buffers the raw `entry` + `index` blobs here; the drain re-verifies +
    /// persists them once the reader advances. Mutually exclusive with `leaf`.
    pub entity: Option<AbsorbedEntity>,
}

/// A buffered sync-repair leaf.
///
/// Holds the original `TreeLeafData` borsh bytes (re-applied verbatim once the
/// reader advances — never translated) plus the `schema_app_key` it was
/// authored under, so the drain only re-applies it when the loaded reader has
/// caught up to that schema. The `AbsorbBuffer` column is new, so no legacy
/// records exist on disk; the field is kept trailing for forward hygiene.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct AbsorbedLeaf {
    /// Borsh-serialized `TreeLeafData` — replayed verbatim on drain.
    pub leaf_bytes: Vec<u8>,
    /// App-schema (loaded-reader) key the leaf was authored under.
    pub schema_app_key: [u8; 32],
}

/// A buffered future-schema snapshot entity.
///
/// The snapshot wire ships an entity as its raw persisted blobs — the `entry`
/// (data) and the borsh-encoded `EntityIndex` (metadata) — verified together
/// and written via `handle.put`. When the receiver's loaded reader can't read
/// the sender's `schema_app_key`, those blobs are held here verbatim and
/// re-verified + persisted on drain once the reader advances.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct AbsorbedEntity {
    /// The entity's 32-byte id.
    pub id: [u8; 32],
    /// Raw `Key::Entry(id)` blob — the entity's data, persisted verbatim.
    pub entry: Vec<u8>,
    /// Raw `Key::Index(id)` blob — the borsh-encoded `EntityIndex` metadata.
    pub index: Vec<u8>,
    /// App-schema (loaded-reader) key the entity was authored under.
    pub schema_app_key: [u8; 32],
}

impl AbsorbRecord {
    /// Build a durable mirror from a live [`BufferedDelta`]. Lossless.
    #[must_use]
    pub fn from_buffered(bd: &BufferedDelta) -> Self {
        Self {
            id: bd.id,
            parents: bd.parents.clone(),
            hlc: bd.hlc,
            payload: bd.payload.clone(),
            nonce: bd.nonce,
            author_id: bd.author_id,
            root_hash: bd.root_hash,
            events: bd.events.clone(),
            source_peer: bd.source_peer.to_bytes(),
            key_id: bd.key_id,
            governance_position: bd.governance_position.clone(),
            delta_signature: bd.delta_signature,
            governance_drain_attempts: bd.governance_drain_attempts,
            producing_app_key: bd.producing_app_key,
            leaf: None,
            entity: None,
        }
    }

    /// Build a leaf-shaped absorb record. The buffered leaf is re-applied
    /// verbatim through `apply_leaf_with_crdt_merge` once the loaded reader
    /// advances to `schema_app_key`; it is NOT a replayable delta, so the
    /// delta-only fields are defaulted and the drain branches on
    /// `self.leaf.is_some()`. `id` is the leaf's entity key (the buffer key's
    /// `delta_id`), giving idempotent overwrite on re-delivery.
    #[must_use]
    pub fn from_leaf(leaf_key: [u8; 32], leaf_bytes: Vec<u8>, schema_app_key: [u8; 32]) -> Self {
        Self {
            id: leaf_key,
            parents: Vec::new(),
            hlc: HybridTimestamp::zero(),
            payload: Vec::new(),
            nonce: [0; 12],
            author_id: PublicKey::from([0; 32]),
            root_hash: Hash::from([0; 32]),
            events: None,
            source_peer: Vec::new(),
            key_id: [0; 32],
            governance_position: None,
            delta_signature: None,
            governance_drain_attempts: 0,
            producing_app_key: Some(schema_app_key),
            leaf: Some(AbsorbedLeaf {
                leaf_bytes,
                schema_app_key,
            }),
            entity: None,
        }
    }

    /// Build a snapshot-entity-shaped absorb record. The buffered entity is
    /// re-verified + persisted via `handle.put` once the loaded reader advances
    /// to `schema_app_key`; it is neither a delta nor a `TreeLeafData`, so those
    /// fields are defaulted and the drain branches on `self.entity.is_some()`.
    /// `id` is the entity's key (the buffer key's `delta_id`), giving idempotent
    /// overwrite on re-delivery.
    #[must_use]
    pub fn from_snapshot_entity(
        id: [u8; 32],
        entry: Vec<u8>,
        index: Vec<u8>,
        schema_app_key: [u8; 32],
    ) -> Self {
        Self {
            id,
            parents: Vec::new(),
            hlc: HybridTimestamp::zero(),
            payload: Vec::new(),
            nonce: [0; 12],
            author_id: PublicKey::from([0; 32]),
            root_hash: Hash::from([0; 32]),
            events: None,
            source_peer: Vec::new(),
            key_id: [0; 32],
            governance_position: None,
            delta_signature: None,
            governance_drain_attempts: 0,
            producing_app_key: Some(schema_app_key),
            leaf: None,
            entity: Some(AbsorbedEntity {
                id,
                entry,
                index,
                schema_app_key,
            }),
        }
    }

    /// Reconstruct a [`BufferedDelta`] from this mirror. The `PeerId` parse can
    /// fail (corrupt on-disk bytes), so this returns a `Result`.
    ///
    /// Only valid for delta-shaped records (`leaf.is_none() && entity.is_none()`);
    /// leaf-/entity-shaped records have no replayable delta and must be drained
    /// via the leaf/entity path. Calling this on one returns `Err` rather than
    /// fabricating a garbage `BufferedDelta` from the empty/defaulted delta
    /// fields.
    pub fn into_buffered(self) -> EyreResult<BufferedDelta> {
        if self.leaf.is_some() || self.entity.is_some() {
            eyre::bail!(
                "AbsorbRecord is not a delta-shaped record (leaf/entity tag set) — \
                 drain it via the leaf/entity path, not into_buffered"
            );
        }

        let source_peer = libp2p::PeerId::from_bytes(&self.source_peer)
            .wrap_err("AbsorbRecord.source_peer is not a valid PeerId")?;

        Ok(BufferedDelta {
            id: self.id,
            parents: self.parents,
            hlc: self.hlc,
            payload: self.payload,
            nonce: self.nonce,
            author_id: self.author_id,
            root_hash: self.root_hash,
            events: self.events,
            source_peer,
            key_id: self.key_id,
            governance_position: self.governance_position,
            delta_signature: self.delta_signature,
            governance_drain_attempts: self.governance_drain_attempts,
            producing_app_key: self.producing_app_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_buffered_delta() -> BufferedDelta {
        BufferedDelta {
            id: [7; 32],
            parents: vec![[0; 32]],
            hlc: HybridTimestamp::zero(),
            payload: vec![1, 2, 3],
            nonce: [0; 12],
            author_id: PublicKey::from([0; 32]),
            root_hash: Hash::from([0; 32]),
            events: Some(vec![10, 20, 30]),
            source_peer: libp2p::PeerId::random(),
            key_id: [0; 32],
            // Populate the Some(GovernancePosition) path so the mirror exercises
            // GovernancePosition's hand-written bounds-enforcing BorshDeserialize
            // end-to-end (a straggler's signed governance position must survive
            // persist → restore unchanged, or its replay authorization breaks).
            governance_position: Some(
                GovernancePosition::new(
                    calimero_context_config::types::ContextGroupId::from([3; 32]),
                    [4; 32],
                    vec![[5; 32], [6; 32]],
                )
                .expect("valid governance position fixture"),
            ),
            delta_signature: Some([9; 64]),
            governance_drain_attempts: 0,
            producing_app_key: Some([2; 32]),
        }
    }

    #[test]
    fn into_buffered_rejects_leaf_shaped_record() {
        // A leaf-shaped record has no replayable delta; `into_buffered` must
        // refuse rather than fabricate a garbage `BufferedDelta` from the
        // empty/defaulted delta fields.
        let rec = AbsorbRecord::from_leaf([1; 32], vec![1, 2, 3], [2; 32]);
        let err = rec
            .into_buffered()
            .expect_err("leaf-shaped record must not convert to a BufferedDelta");
        assert!(
            err.to_string().contains("not a delta-shaped"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn into_buffered_rejects_entity_shaped_record() {
        // Same for snapshot-entity-shaped records.
        let rec = AbsorbRecord::from_snapshot_entity([1; 32], vec![1], vec![2], [3; 32]);
        let err = rec
            .into_buffered()
            .expect_err("entity-shaped record must not convert to a BufferedDelta");
        assert!(
            err.to_string().contains("not a delta-shaped"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn absorb_record_round_trips_buffered_delta() {
        let bd = sample_buffered_delta();
        let rec = AbsorbRecord::from_buffered(&bd);
        let bytes = borsh::to_vec(&rec).unwrap();
        let back = AbsorbRecord::try_from_slice(&bytes)
            .unwrap()
            .into_buffered()
            .unwrap();
        // Assert EVERY field survives the round trip, so a future field added to
        // BufferedDelta (or a reorder/typo in the hand-written mirror) is caught.
        assert_eq!(back.id, bd.id);
        assert_eq!(back.parents, bd.parents);
        assert_eq!(back.hlc, bd.hlc);
        assert_eq!(back.payload, bd.payload);
        assert_eq!(back.nonce, bd.nonce);
        assert_eq!(back.author_id, bd.author_id);
        assert_eq!(back.root_hash, bd.root_hash);
        assert_eq!(back.events, bd.events);
        assert_eq!(back.source_peer, bd.source_peer); // PeerId survived to_bytes/from_bytes
        assert_eq!(back.key_id, bd.key_id);
        assert_eq!(back.governance_position, bd.governance_position);
        assert_eq!(back.delta_signature, bd.delta_signature);
        assert_eq!(back.governance_drain_attempts, bd.governance_drain_attempts);
        assert_eq!(back.producing_app_key, bd.producing_app_key);
    }
}
