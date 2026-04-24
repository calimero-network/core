//! Per-entity writer-set rotation log for `SharedStorage<T>`.
//!
//! Phase **P2** of [#2233](https://github.com/calimero-network/core/issues/2233)
//! (DAG-causal Shared verification). Provides the storage shape and read API
//! that P3's verifier uses to resolve "what was the writer set as of this
//! causal point in the DAG?" — answering the partition-race scenarios called
//! out in #2197.
//!
//! # Design
//!
//! Per [ADR 0001](../../../docs/adr/0001-shared-storage-concurrent-rotation.md):
//! every accepted rotation appends an entry; reads compare entries by
//! **causal-first → HLC → signer-pubkey** ordering.
//!
//! The log is stored separately from `EntityIndex` under a dedicated
//! [`Key::RotationLog`] so reading the index doesn't pull in the full
//! rotation history.
//!
//! # P2 scope (this module)
//!
//! - Schema: [`RotationLogEntry`], [`RotationSnapshot`], [`RotationLog`]
//! - Persistence: [`load`], [`save`], [`append`]
//! - Read APIs: [`latest_writers`], [`writers_at`]
//!
//! Wiring into [`Interface::apply_action`](crate::interface::Interface::apply_action)
//! and the verifier is **P3**. In P2 the module is callable but not yet invoked
//! by the apply pipeline; standalone unit tests cover the schema and ordering.
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
/// Fields chosen to be sufficient for the ADR ordering rule without needing
/// to consult any other state:
/// - `delta_id` identifies the `CausalDelta` so the caller's DAG-ancestry
///   predicate can locate it.
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
/// them), not causal order — `writers_at` is responsible for resolving
/// causal precedence at read time, since insertion order differs across
/// nodes when sync delivers concurrent rotations in different orders.
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
/// retransmit) only produces one log entry. The duplicate is silently
/// dropped — no error. This matches the broader CRDT model where applying
/// the same operation twice is safe.
pub fn append<S: StorageAdaptor>(id: Id, entry: RotationLogEntry) -> Result<(), StorageError> {
    let mut log = load::<S>(id)?.unwrap_or_else(RotationLog::empty);
    if log.entries.iter().any(|e| e.delta_id == entry.delta_id) {
        return Ok(());
    }
    log.entries.push(entry);
    save::<S>(id, &log)
}

/// Returns the writer set from the most recently *appended* entry.
///
/// "Most recently appended" is **insertion order on this node**, not causal
/// order — concurrent rotations applied in different orders across nodes
/// can produce different answers from this function. Use it only when no
/// causal context is available:
/// - snapshot leaf push (no `CausalDelta` in scope)
/// - local apply paths
/// - P1-era code that hasn't been wired through P3 yet
///
/// Matches v2's LWW-by-`writers_nonce` behavior in practice (the latest
/// inserted entry tends to also be the highest nonce on the local node),
/// preserving backward compatibility.
///
/// Returns `Ok(None)` if the log is empty or absent.
pub fn latest_writers<S: StorageAdaptor>(
    id: Id,
) -> Result<Option<BTreeSet<PublicKey>>, StorageError> {
    let Some(log) = load::<S>(id)? else {
        return Ok(None);
    };
    if let Some(entry) = log.entries.last() {
        return Ok(Some(entry.new_writers.clone()));
    }
    if let Some(snap) = log.snapshot {
        return Ok(Some(snap.writers));
    }
    Ok(None)
}

/// Returns the writer set as-of a causal point in the DAG.
///
/// Implements ADR 0001's merge rule end-to-end:
/// 1. Filter entries to those reachable from `causal_parents`.
/// 2. Among reachable entries, pick the causally latest.
/// 3. Truly-concurrent tie → larger `delta_hlc` wins.
/// 4. HLC tie → smaller `signer` pubkey bytes win (`None` treated as larger).
///
/// **P4-impl** of #2233 is a no-op for the chosen rule — there's no
/// intermediate state to materialise, no convergence reduction step.
/// The whole rule lives here, with the per-entity write hook in
/// [`Interface::apply_action`](crate::interface::Interface::apply_action)
/// (P3) keeping the log fed.
///
/// `happens_before(a, b)` is the caller-provided DAG-ancestry predicate:
/// returns true iff delta `a` is in the transitive ancestry of delta `b`
/// (i.e., `a` happens-before `b` in the DAG-causal sense). Storage cannot
/// answer this on its own — it doesn't have the DAG — so the node sync
/// layer (which does) provides the closure at the call site.
///
/// `causal_parents` is the parent set of the apply context's delta (i.e.,
/// `CausalDelta.parents` of the delta whose action is being applied).
///
/// # Reachability
///
/// An entry `e` is reachable iff
/// `causal_parents.iter().any(|p| e.delta_id == *p || happens_before(&e.delta_id, p))`.
/// In words: the rotation's delta is one of the apply-context's parents, or
/// is in the transitive ancestry of one of them.
///
/// # When `causal_parents` is empty
///
/// Returns the same answer as [`latest_writers`] — the v2-compatible
/// fallback for paths without DAG context.
///
/// Returns `Ok(None)` if no log entry is reachable (or the log is absent).
pub fn writers_at<S: StorageAdaptor, F>(
    id: Id,
    causal_parents: &[[u8; 32]],
    happens_before: F,
) -> Result<Option<BTreeSet<PublicKey>>, StorageError>
where
    F: Fn(&[u8; 32], &[u8; 32]) -> bool,
{
    if causal_parents.is_empty() {
        return latest_writers::<S>(id);
    }

    let Some(log) = load::<S>(id)? else {
        return Ok(None);
    };

    // Filter to entries reachable from any of the apply context's parents.
    let reachable: Vec<&RotationLogEntry> = log
        .entries
        .iter()
        .filter(|e| {
            causal_parents
                .iter()
                .any(|p| e.delta_id == *p || happens_before(&e.delta_id, p))
        })
        .collect();

    // ADR 0001 ordering. We want the *latest* entry, so define cmp such that
    // "later" is `Greater`, then take the max.
    let latest = reachable.into_iter().max_by(|a, b| {
        // 1. Causal precedence wins: if a happens-before b, b is later.
        if happens_before(&a.delta_id, &b.delta_id) {
            return std::cmp::Ordering::Less;
        }
        if happens_before(&b.delta_id, &a.delta_id) {
            return std::cmp::Ordering::Greater;
        }
        // 2. Truly concurrent → larger HLC wins.
        match a.delta_hlc.cmp(&b.delta_hlc) {
            std::cmp::Ordering::Equal => {}
            non_eq => return non_eq,
        }
        // 3. HLC tie → smaller signer bytes win, so the smaller one is "later"
        //    in our cmp (since we take max). `None` is treated as larger than
        //    any `Some(_)` so it sorts last and loses ties.
        match (&a.signer, &b.signer) {
            (Some(sa), Some(sb)) => sb.digest().cmp(sa.digest()),
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });

    if let Some(entry) = latest {
        return Ok(Some(entry.new_writers.clone()));
    }

    // Nothing in `entries` was reachable. If the log has been compacted
    // (P6), the snapshot's writer set is the answer for any apply context
    // whose history pre-dates the cutoff. For now `snapshot` is always
    // `None`, so this falls through to `Ok(None)`.
    if let Some(snap) = log.snapshot {
        return Ok(Some(snap.writers));
    }
    Ok(None)
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
        assert_eq!(latest_writers::<Store>(id).unwrap(), None);
        assert_eq!(
            writers_at::<Store, _>(id, &[[0; 32]], |_, _| false).unwrap(),
            None
        );
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
    fn latest_writers_returns_last_appended() {
        let id = id(3);
        append::<Store>(id, entry(1, 100, 0xAA, &[0xAA], 1)).unwrap();
        append::<Store>(id, entry(2, 200, 0xBB, &[0xBB], 2)).unwrap();
        assert_eq!(
            latest_writers::<Store>(id).unwrap(),
            Some([0xBB].into_iter().map(pk).collect())
        );
    }

    #[test]
    fn writers_at_with_empty_parents_falls_back_to_latest() {
        // ADR: empty causal_parents → match latest_writers (v2 LWW compat).
        let id = id(4);
        append::<Store>(id, entry(1, 100, 0xAA, &[0xAA], 1)).unwrap();
        append::<Store>(id, entry(2, 200, 0xBB, &[0xBB], 2)).unwrap();
        assert_eq!(
            writers_at::<Store, _>(id, &[], |_, _| false).unwrap(),
            Some([0xBB].into_iter().map(pk).collect())
        );
    }

    #[test]
    fn writers_at_returns_only_reachable_entries() {
        // ADR Example A: sequential rotations. R1 is the parent, R2 builds on it.
        // Querying with causal_parents = [R1.delta_id] should pick R1, NOT R2,
        // because R2 hasn't happened yet from the perspective of someone
        // looking at R1's frontier.
        let id = id(5);
        append::<Store>(id, entry(1, 100, 0xAA, &[0xAA, 0xBB], 1)).unwrap();
        append::<Store>(id, entry(2, 200, 0xBB, &[0xBB, 0xCC], 2)).unwrap();

        // happens_before(a, b): R1 (id=[1;32]) happens-before R2 (id=[2;32]).
        let happens_before = |a: &[u8; 32], b: &[u8; 32]| a == &[1; 32] && b == &[2; 32];

        // Query as-of R1: only R1 is reachable.
        let writers = writers_at::<Store, _>(id, &[[1; 32]], happens_before).unwrap();
        assert_eq!(writers, Some([0xAA, 0xBB].into_iter().map(pk).collect()));

        // Query as-of R2: both reachable, R2 is causally later → R2 wins.
        let writers = writers_at::<Store, _>(id, &[[2; 32]], happens_before).unwrap();
        assert_eq!(writers, Some([0xBB, 0xCC].into_iter().map(pk).collect()));
    }

    #[test]
    fn writers_at_concurrent_siblings_resolved_by_hlc() {
        // ADR Example B: two concurrent siblings with different HLCs.
        // Larger HLC wins.
        let id = id(6);
        // R1 by Alice at HLC=20, removes Bob.
        append::<Store>(id, entry(1, 20, 0xAA, &[0xAA], 10)).unwrap();
        // R2 by Bob at HLC=21, removes Alice. Concurrent with R1.
        append::<Store>(id, entry(2, 21, 0xBB, &[0xBB], 10)).unwrap();

        // Apply context's parents reference both rotations as siblings of D_root.
        // Our happens_before says neither precedes the other.
        let none_precede = |_: &[u8; 32], _: &[u8; 32]| false;

        // Query reaching both: HLC=21 > HLC=20 → R2 (writers={Bob}) wins.
        let writers = writers_at::<Store, _>(id, &[[1; 32], [2; 32]], none_precede).unwrap();
        assert_eq!(writers, Some([0xBB].into_iter().map(pk).collect()));
    }

    #[test]
    fn writers_at_hlc_tie_resolved_by_signer_bytes() {
        // ADR Example C: HLC tie (same time AND same node ID, which is
        // vanishingly rare in production but pinned in the spec).
        // Smaller signer pubkey bytes win.
        use core::num::NonZeroU128;

        use crate::logical_clock::{Timestamp, ID, NTP64};

        let id = id(7);
        // Build two entries with IDENTICAL HLCs (same time, same node ID).
        let identical_ts = HybridTimestamp::new(Timestamp::new(
            NTP64(50),
            ID::from(NonZeroU128::new(1).unwrap()),
        ));
        let mk = |delta_id: u8, signer: u8, writers: &[u8]| RotationLogEntry {
            delta_id: [delta_id; 32],
            delta_hlc: identical_ts,
            signer: Some(pk(signer)),
            new_writers: writers.iter().copied().map(pk).collect(),
            writers_nonce: 10,
        };
        // signer 0xAA < signer 0xBB byte-wise.
        append::<Store>(id, mk(1, 0xAA, &[0xAA, 0xCC])).unwrap();
        append::<Store>(id, mk(2, 0xBB, &[0xBB, 0xDD])).unwrap();

        let none_precede = |_: &[u8; 32], _: &[u8; 32]| false;
        let writers = writers_at::<Store, _>(id, &[[1; 32], [2; 32]], none_precede).unwrap();
        // 0xAA is smaller → wins.
        assert_eq!(writers, Some([0xAA, 0xCC].into_iter().map(pk).collect()));
    }

    #[test]
    fn writers_at_causal_precedence_overrides_hlc() {
        // R1 happens-before R2, but R1.hlc > R2.hlc (clock skew). Causal
        // wins: R2's author saw R1's reality and chose to rotate from there.
        let id = id(8);
        append::<Store>(id, entry(1, 999, 0xAA, &[0xAA], 1)).unwrap(); // huge HLC
        append::<Store>(id, entry(2, 100, 0xBB, &[0xBB], 2)).unwrap(); // smaller HLC

        // R1 happens-before R2 in the DAG.
        let happens_before = |a: &[u8; 32], b: &[u8; 32]| a == &[1; 32] && b == &[2; 32];

        let writers = writers_at::<Store, _>(id, &[[2; 32]], happens_before).unwrap();
        // R2 wins despite smaller HLC, because it's causally later.
        assert_eq!(writers, Some([0xBB].into_iter().map(pk).collect()));
    }

    #[test]
    fn writers_at_unreachable_entries_ignored() {
        // Entry exists but isn't reachable from the queried parents.
        // Returns None (not the latest) — verifier can fall back as it sees fit.
        let id = id(9);
        append::<Store>(id, entry(1, 100, 0xAA, &[0xAA], 1)).unwrap();

        // Different parent that doesn't reach delta 1.
        let writers = writers_at::<Store, _>(id, &[[42; 32]], |_, _| false).unwrap();
        assert_eq!(writers, None);
    }

    #[test]
    fn writers_at_falls_back_to_snapshot_when_entries_unreachable() {
        // P6 compaction wrote a snapshot but no live entries reach the query.
        let id = id(10);
        let snap_writers: BTreeSet<PublicKey> = [0xEE].into_iter().map(pk).collect();
        let log = RotationLog {
            snapshot: Some(RotationSnapshot {
                writers: snap_writers.clone(),
                cutoff_index: 5,
            }),
            entries: vec![entry(99, 100, 0xFF, &[0xFF], 99)],
        };
        save::<Store>(id, &log).unwrap();

        // happens_before never matches: live entry at delta 99 is unreachable.
        let writers = writers_at::<Store, _>(id, &[[1; 32]], |_, _| false).unwrap();
        assert_eq!(writers, Some(snap_writers));
    }
}
