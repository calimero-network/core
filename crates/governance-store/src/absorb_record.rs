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
        }
    }

    /// Reconstruct a [`BufferedDelta`] from this mirror. The `PeerId` parse can
    /// fail (corrupt on-disk bytes), so this returns a `Result`.
    ///
    /// Only valid for delta-shaped records (`leaf.is_none()`); leaf-shaped
    /// records have no replayable delta and must be drained via the leaf path.
    pub fn into_buffered(self) -> EyreResult<BufferedDelta> {
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
            events: None,
            source_peer: libp2p::PeerId::random(),
            key_id: [0; 32],
            governance_position: None,
            delta_signature: Some([9; 64]),
            governance_drain_attempts: 0,
            producing_app_key: Some([2; 32]),
        }
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
        assert_eq!(back.id, bd.id);
        assert_eq!(back.source_peer, bd.source_peer); // PeerId survived to_bytes/from_bytes
        assert_eq!(back.producing_app_key, bd.producing_app_key);
        assert_eq!(back.delta_signature, bd.delta_signature);
    }
}
