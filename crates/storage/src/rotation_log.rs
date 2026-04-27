//! Per-entity writer-set rotation log for `SharedStorage<T>`.
//!
//! Storage owns the log's on-disk shape and the persistence primitives
//! ([`load`], [`save`], [`append`]). DAG-causal *resolution* — "given a
//! delta's parents and a happens-before predicate, what was the writer
//! set as of that causal point?" — lives in the node sync layer
//! (`calimero_node::sync::rotation_log_reader`), where the DAG itself
//! does. This split was made in #2266 (per #2267 Option B) so storage no
//! longer carries DAG-ancestry knowledge.
//!
//! # Design
//!
//! Per [ADR 0001](../../../docs/adr/0001-shared-storage-concurrent-rotation.md):
//! every accepted rotation appends an entry; the node-side reader compares
//! entries by **causal-first → HLC → signer-pubkey** ordering.
//!
//! The log is stored separately from `EntityIndex` under a dedicated
//! [`Key::RotationLog`] so reading the index doesn't pull in the full
//! rotation history.
//!
//! # Compaction (shape locked in, not implemented)
//!
//! The [`RotationLog::snapshot`] field is reserved for P6 compaction: a sliding
//! window of recent entries plus a snapshot of the writer set at the cutoff.
//! The threshold (epic suggests 1000 entries) is a P6 measurement; the shape
//! is fixed here so the on-disk format doesn't need a breaking change later.

use std::collections::BTreeSet;

use borsh::{from_slice, to_vec, BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use crate::address::Id;
use crate::error::StorageError;
use crate::logical_clock::HybridTimestamp;
use crate::store::{Key, StorageAdaptor};

/// One accepted rotation, captured at apply-time.
///
/// **Caller invariant**: at most one entry per `(entity_id, delta_id)`. The
/// log dedups on `delta_id` alone:
/// - Replaying a delta with **identical** entry contents is a no-op (idempotent).
/// - Calling [`append`] a second time with **differing** contents returns
///   [`StorageError::DuplicateRotationInDelta`] — multi-action `CausalDelta`s
///   with two rotations on the same entity are not supported; callers must
///   split them into separate deltas. Tracked in #2233 P3.
///
/// Fields chosen to be sufficient for the ADR ordering rule without needing
/// to consult any other state:
/// - `delta_id` identifies the `CausalDelta` so the node-side reader's
///   DAG-ancestry predicate can locate it.
/// - `delta_hlc` is the sibling tiebreak when two rotations are concurrent.
/// - `signer` is the final tiebreak for HLC ties (vanishingly rare but
///   pinned for spec completeness).
/// - `new_writers` is the resolved writer set after this rotation.
/// - `writers_nonce` is preserved for v2 compatibility / debugging; not
///   consulted by the ADR rule but useful when reading exported logs.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct RotationLogEntry {
    /// Hash of the `CausalDelta` containing this rotation.
    pub delta_id: [u8; 32],

    /// Hybrid timestamp of the `CausalDelta`. Used as the sibling tiebreak
    /// when two rotations are causally concurrent.
    pub delta_hlc: HybridTimestamp,

    /// Public key that signed the rotation action.
    ///
    /// `None` only for legacy / unsigned-bootstrap entries — see
    /// [`writers_at`] for how those participate in ordering (they sort
    /// after any `Some(...)` entry at equal HLC, mirroring the
    /// "smaller bytes win" rule with `None` treated as "larger than any").
    pub signer: Option<PublicKey>,

    /// Resolved writer set after this rotation. `BTreeSet` so the on-wire
    /// representation is canonical (sorted), matching the rest of the
    /// `Shared` storage path.
    pub new_writers: BTreeSet<PublicKey>,

    /// Per-entity monotonic counter at the time of rotation. Preserved for
    /// debugging and v2 compatibility; the ADR rule does not depend on it.
    pub writers_nonce: u64,
}

/// A compacted prefix of the rotation log — the writer set at the boundary
/// plus the index of the entry that boundary corresponds to.
///
/// **Not produced in P2** — the field exists so the on-disk shape doesn't
/// need a migration when P6 turns compaction on.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct RotationSnapshot {
    /// Writer set as-of the boundary. When a query's `causal_parents` only
    /// reach into the compacted region, this is the answer.
    pub writers: BTreeSet<PublicKey>,

    /// Index into the original (uncompacted) entry stream that the snapshot
    /// represents. Compacted entries had indices `[0, cutoff_index)`; live
    /// entries in [`RotationLog::entries`] start at `cutoff_index`. Stored
    /// as `u64` so the field doesn't bottleneck on `usize` portability.
    pub cutoff_index: u64,
}

/// Persistent rotation log for a single Shared entity.
///
/// `entries` is append-only; new rotations push to the end. Order in the
/// vector is **insertion order** (which is the order the receiver applied
/// them), not causal order — the node-side reader resolves causal
/// precedence at read time, since insertion order differs across nodes
/// when sync delivers concurrent rotations in different orders.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct RotationLog {
    /// Compacted prefix; `None` until P6 compaction runs.
    pub snapshot: Option<RotationSnapshot>,

    /// Live entries past the snapshot (or the full history if
    /// `snapshot.is_none()`).
    pub entries: Vec<RotationLogEntry>,
}

impl RotationLog {
    /// Empty log (no rotations recorded yet).
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            snapshot: None,
            entries: Vec::new(),
        }
    }
}

/// Load the rotation log for `id`, if any.
///
/// Returns `Ok(None)` when no log has been written yet (entity exists but
/// no rotation has been recorded). Returns `Err` only on deserialization
/// failure — corruption is loud, never silent.
///
/// # Errors
///
/// Returns [`StorageError::DeserializationError`] if the stored bytes
/// cannot be decoded as a `RotationLog`.
pub fn load<S: StorageAdaptor>(id: Id) -> Result<Option<RotationLog>, StorageError> {
    let Some(bytes) = S::storage_read(Key::RotationLog(id)) else {
        return Ok(None);
    };
    let log = from_slice::<RotationLog>(&bytes).map_err(StorageError::DeserializationError)?;
    Ok(Some(log))
}

/// Persist `log` for `id`, overwriting any existing log.
///
/// Callers should reach for [`append`] in the common case; `save` is exposed
/// for tests and future P6 compaction that rewrites the log shape.
///
/// # Errors
///
/// Returns [`StorageError::SerializationError`] if `log` cannot be encoded.
pub fn save<S: StorageAdaptor>(id: Id, log: &RotationLog) -> Result<(), StorageError> {
    let bytes = to_vec(log).map_err(|e| StorageError::SerializationError(e.into()))?;
    let _ = S::storage_write(Key::RotationLog(id), &bytes);
    Ok(())
}

/// Append a rotation entry.
///
/// Reads the existing log (creating an empty one if absent), pushes the new
/// entry to the live tail, and writes back. Order in storage matches
/// insertion order; causal resolution happens at read time.
///
/// **Idempotent on `delta_id`**: a delta delivered twice (out-of-order sync,
/// retransmit) only produces one log entry, provided the entry contents
/// match. This matches the broader CRDT model where applying the same
/// operation twice is safe.
///
/// **Caller invariant — one rotation per entity per delta**: dedup keys on
/// `delta_id` only. If a single `CausalDelta` carries two rotation actions
/// on the same entity, only the first reaches the log; the second would
/// silently be dropped while `save_internal` still applies its data,
/// diverging log from stored state. To prevent that, a second [`append`]
/// for the same `delta_id` whose entry contents differ from the existing
/// entry returns [`StorageError::DuplicateRotationInDelta`] instead of
/// being silently dropped. Multi-action deltas with multiple rotations on
/// the same entity must be split into separate deltas at construction time.
/// Tracked in #2233 P3.
///
/// # Errors
///
/// - [`StorageError::DuplicateRotationInDelta`] if a prior entry exists for
///   `entry.delta_id` with differing `(new_writers, signer, writers_nonce)`.
/// - Propagates [`load`] / [`save`] errors (deserialization on read,
///   serialization on write).
pub fn append<S: StorageAdaptor>(id: Id, entry: RotationLogEntry) -> Result<(), StorageError> {
    let mut log = load::<S>(id)?.unwrap_or_else(RotationLog::empty);
    if let Some(existing) = log.entries.iter().find(|e| e.delta_id == entry.delta_id) {
        if (
            &existing.new_writers,
            existing.signer,
            existing.writers_nonce,
        ) != (&entry.new_writers, entry.signer, entry.writers_nonce)
        {
            return Err(StorageError::DuplicateRotationInDelta(entry.delta_id));
        }
        return Ok(());
    }
    log.entries.push(entry);
    save::<S>(id, &log)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MockedStorage;

    type Store = MockedStorage<300>;

    fn pk(b: u8) -> PublicKey {
        PublicKey::from([b; 32])
    }

    fn entry(
        delta_id: u8,
        hlc_time: u64,
        signer: u8,
        writers: &[u8],
        nonce: u64,
    ) -> RotationLogEntry {
        use core::num::NonZeroU128;

        use crate::logical_clock::{Timestamp, ID, NTP64};

        // Distinct non-zero ID per signer keeps HLCs ordered by `hlc_time` first
        // (the `time` component dominates the derived `Ord`); the ID component
        // only kicks in when `time` collides, which is exactly the tiebreak case
        // we want to test independently via `signer`.
        let id_u128 = NonZeroU128::new(u128::from(signer) + 1).unwrap();
        let ts = Timestamp::new(NTP64(hlc_time), ID::from(id_u128));
        RotationLogEntry {
            delta_id: [delta_id; 32],
            delta_hlc: HybridTimestamp::new(ts),
            signer: Some(pk(signer)),
            new_writers: writers.iter().copied().map(pk).collect(),
            writers_nonce: nonce,
        }
    }

    fn id(b: u8) -> Id {
        Id::new([b; 32])
    }

    #[test]
    fn empty_when_no_log_written() {
        let id = id(1);
        assert_eq!(load::<Store>(id).unwrap(), None);
    }

    #[test]
    fn append_round_trips_through_storage() {
        let id = id(2);
        let e = entry(1, 100, 0xAA, &[0xAA, 0xBB], 1);
        append::<Store>(id, e.clone()).unwrap();
        let loaded = load::<Store>(id).unwrap().unwrap();
        assert_eq!(loaded.entries, vec![e]);
        assert_eq!(loaded.snapshot, None);
    }

    #[test]
    fn append_is_idempotent_when_replayed_with_identical_contents() {
        // CRDT replay safety: re-appending the exact same entry is a no-op
        // (only one log row, no error).
        let id = id(11);
        let e = entry(1, 100, 0xAA, &[0xAA, 0xBB], 1);
        append::<Store>(id, e.clone()).unwrap();
        append::<Store>(id, e.clone()).unwrap();
        let log = load::<Store>(id).unwrap().unwrap();
        assert_eq!(log.entries, vec![e]);
    }

    #[test]
    fn append_rejects_duplicate_delta_with_divergent_contents() {
        // Caller invariant violation: same delta_id, different new_writers.
        // Was previously a debug_assert; promoted to a hard error so release
        // builds also surface multi-rotation-per-entity-per-delta misuse.
        let id = id(12);
        let e1 = entry(1, 100, 0xAA, &[0xAA], 1);
        let e2 = entry(1, 100, 0xAA, &[0xBB], 1); // same delta_id, different writers
        append::<Store>(id, e1.clone()).unwrap();
        let err = append::<Store>(id, e2).unwrap_err();
        assert!(
            matches!(err, StorageError::DuplicateRotationInDelta(d) if d == [1; 32]),
            "expected DuplicateRotationInDelta, got {err:?}"
        );
        // The original entry stays put; nothing was overwritten.
        let log = load::<Store>(id).unwrap().unwrap();
        assert_eq!(log.entries, vec![e1]);
    }
}
