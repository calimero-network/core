//! Borsh-serializable mirror of [`BufferedDelta`] for durable absorb storage
//! (PR-6b straggler safety).
//!
//! [`BufferedDelta`] is deliberately NOT Borsh-derivable — it carries a
//! `libp2p::PeerId` (no clean Borsh derive) alongside every replay field. To
//! persist an absorbed straggler delta durably we hand-write this mirror, which
//! holds every field in a Borsh-friendly shape (`source_peer` as the raw
//! `PeerId::to_bytes()` vector). `from_buffered` / `into_buffered` convert
//! losslessly; the `PeerId` parse on the way back can fail, so `into_buffered`
//! returns a `Result`.
//!
//! Do NOT add `#[derive(Borsh)]` to `BufferedDelta` itself — keep the
//! serialization concern isolated in this mirror.

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
    /// Set when this record holds a buffered **sync-repair leaf** (PR-6b Task
    /// 6b.7) rather than a full straggler delta. The sync-repair paths
    /// (HashComparison / LevelSync / snapshot) bypass the gossip state-delta
    /// fence, so a receiver on an older reader buffers a future-schema leaf
    /// here instead of LWW-storing unreadable bytes.
    ///
    /// `None` for the delta-absorb path (the gossip fence). The leaf-vs-delta
    /// drain branches on this tag: a delta replays verbatim through
    /// `__calimero_sync_next`; a leaf re-applies through
    /// `apply_leaf_with_crdt_merge` once the reader advances.
    ///
    /// Backward-compatible trailing field — see the borsh note on
    /// [`AbsorbedLeaf`].
    pub leaf: Option<AbsorbedLeaf>,
    /// Set when this record holds a buffered **snapshot entity** (PR-6b Task
    /// 6b.7) rather than a delta or a HashComparison/LevelSync leaf.
    ///
    /// The snapshot apply path decodes its own `SnapshotRecord::Entity` wire
    /// type and persists each verified entity via a raw `handle.put` (it does
    /// NOT route through `apply_leaf_with_crdt_merge`, so it has no
    /// `TreeLeafData` to buffer). A receiver on an older reader buffers the raw
    /// `entry` + `index` blobs here instead of storing unreadable bytes; the
    /// drain re-verifies + `handle.put`s them once the loaded reader advances.
    ///
    /// `None` for the delta-absorb and HashComparison/LevelSync-leaf paths. The
    /// drain branches on this tag (it is mutually exclusive with `leaf`).
    ///
    /// Backward-compatible trailing field — see the borsh note on
    /// [`AbsorbedLeaf`].
    pub entity: Option<AbsorbedEntity>,
}

/// A buffered sync-repair leaf (PR-6b Task 6b.7).
///
/// Holds the original `TreeLeafData` borsh bytes (re-applied verbatim once the
/// reader advances — never translated) plus the `schema_app_key` it was
/// authored under, so the drain only re-applies it when the loaded reader has
/// caught up to that schema.
///
/// Borsh round-trips directly (both fields derive). Because `AbsorbRecord.leaf`
/// is a trailing `Option`, the derived `BorshDeserialize` would normally fail
/// on the pre-#2539 record layout; in practice the `AbsorbBuffer` column is new
/// in this train (PR-6b) so no legacy records exist on disk, and every record
/// is written by this binary. We still keep the field trailing for forward
/// hygiene.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct AbsorbedLeaf {
    /// Borsh-serialized `TreeLeafData` — replayed verbatim on drain.
    pub leaf_bytes: Vec<u8>,
    /// App-schema (loaded-reader) key the leaf was authored under.
    pub schema_app_key: [u8; 32],
}

/// A buffered future-schema **snapshot entity** (PR-6b Task 6b.7).
///
/// The snapshot wire ships an entity as its raw persisted blobs — the `entry`
/// (data) and the borsh-encoded `EntityIndex` (metadata) — verified together
/// and written via `handle.put`. When the receiver's loaded reader can't read
/// the sender's `schema_app_key`, those blobs are held here verbatim (never
/// translated) and re-verified + persisted on drain once the reader advances.
///
/// Trailing-`Option` borsh hygiene is identical to [`AbsorbedLeaf`]: the
/// `AbsorbBuffer` column is new in this train, so no legacy on-disk records
/// exist and every record is written by this binary.
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

    /// Build a leaf-shaped absorb record (PR-6b Task 6b.7). The buffered leaf is
    /// re-applied verbatim through `apply_leaf_with_crdt_merge` once the loaded
    /// reader advances to `schema_app_key`; it is NOT a replayable delta, so the
    /// delta-only fields are left empty / defaulted and the leaf-vs-delta drain
    /// branches on `self.leaf.is_some()`.
    ///
    /// `id` is the leaf's entity key (the absorb-buffer key's `delta_id`
    /// component), giving the same idempotent-overwrite-on-redelivery property
    /// the delta path gets from the real `delta_id`.
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

    /// Build a snapshot-entity-shaped absorb record (PR-6b Task 6b.7). The
    /// buffered entity is re-verified + persisted via `handle.put` once the
    /// loaded reader advances to `schema_app_key`; it is neither a replayable
    /// delta nor a `TreeLeafData`, so the delta/leaf fields are left
    /// empty/defaulted and the drain branches on `self.entity.is_some()`.
    ///
    /// `id` is the entity's key (the absorb-buffer key's `delta_id` component),
    /// giving the same idempotent-overwrite-on-redelivery property the delta
    /// path gets from the real `delta_id`.
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
