//! DAG-causal rotation-log resolution for the node sync layer.
//!
//! Per #2266 (Option B from #2267), the storage crate no longer carries
//! DAG-ancestry knowledge. Storage owns the rotation log's on-disk shape
//! ([`calimero_storage::rotation_log`]) and the `append` write hook;
//! resolution — "given a delta's parents and a DAG-ancestry predicate,
//! what was the writer set as of that causal point?" — lives here, where
//! the DAG itself does.
//!
//! The functions in this module are **pure** (no I/O). Callers load the
//! rotation log via [`calimero_storage::rotation_log::load`] and pass
//! `&RotationLog` in. The DAG-ancestry closure is supplied by the caller
//! (typically wrapping the node's `CoreDagStore::happens_before`).

use std::collections::BTreeSet;

use calimero_primitives::identity::PublicKey;
use calimero_storage::rotation_log::{RotationLog, RotationLogEntry};

/// Returns the writer set from the most recently *appended* entry in
/// `log`.
///
/// "Most recently appended" is **insertion order on this node**, not
/// causal order — concurrent rotations applied in different orders
/// across nodes can produce different answers. Use it only when no
/// causal context is available (snapshot leaf push, local apply).
///
/// Returns `None` if the log has no entries and no snapshot.
#[must_use]
pub fn latest_writers(log: &RotationLog) -> Option<BTreeSet<PublicKey>> {
    if let Some(entry) = log.entries.last() {
        return Some(entry.new_writers.clone());
    }
    log.snapshot.as_ref().map(|s| s.writers.clone())
}

/// Returns the writer set as-of a causal point in the DAG.
///
/// Implements ADR 0001's merge rule end-to-end:
/// 1. Filter entries to those reachable from `causal_parents`.
/// 2. Among reachable entries, pick the causally latest.
/// 3. Truly-concurrent tie → larger `delta_hlc` wins.
/// 4. HLC tie → smaller `signer` pubkey bytes win (`None` treated as larger).
///
/// `happens_before(a, b)` returns true iff delta `a` is in the
/// transitive ancestry of delta `b`. The caller closes over its DAG
/// view (typically `CoreDagStore`) to provide the predicate.
///
/// `causal_parents` is the parent set of the apply context's delta —
/// i.e. `CausalDelta.parents` of the delta being applied.
///
/// # Reachability
///
/// An entry `e` is reachable iff
/// `causal_parents.iter().any(|p| e.delta_id == *p || happens_before(&e.delta_id, p))`.
/// In words: the rotation's delta is one of the apply-context's parents,
/// or in the transitive ancestry of one.
///
/// # When `causal_parents` is empty
///
/// Returns the same answer as [`latest_writers`] — the v2-compatible
/// fallback for paths without DAG context.
#[must_use]
pub fn writers_at<F>(
    log: &RotationLog,
    causal_parents: &[[u8; 32]],
    happens_before: F,
) -> Option<BTreeSet<PublicKey>>
where
    F: Fn(&[u8; 32], &[u8; 32]) -> bool,
{
    if causal_parents.is_empty() {
        return latest_writers(log);
    }

    let reachable: Vec<&RotationLogEntry> = log
        .entries
        .iter()
        .filter(|e| {
            causal_parents
                .iter()
                .any(|p| e.delta_id == *p || happens_before(&e.delta_id, p))
        })
        .collect();

    let latest = reachable.into_iter().max_by(|a, b| {
        // 1. Causal precedence wins.
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
        // 3. HLC tie → smaller signer bytes win. We're picking max, so
        //    return Greater for the entry whose signer is smaller. None
        //    signers are treated as larger than any Some — unsigned
        //    legacy entries lose ties to signed ones.
        match (&a.signer, &b.signer) {
            (Some(sa), Some(sb)) => sb.digest().cmp(sa.digest()),
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });

    if let Some(entry) = latest {
        return Some(entry.new_writers.clone());
    }

    // Nothing in `entries` was reachable. If the log was compacted (P6),
    // the snapshot's writer set is the answer for any apply context whose
    // history pre-dates the cutoff.
    log.snapshot.as_ref().map(|s| s.writers.clone())
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU128;

    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
    use calimero_storage::rotation_log::{RotationLog, RotationLogEntry, RotationSnapshot};

    use super::*;

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
        // Distinct non-zero ID per signer keeps HLCs ordered by `hlc_time`
        // first; the ID component only kicks in on `time` collisions, which
        // is the tiebreak case we test independently via `signer`.
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

    fn log_of(entries: Vec<RotationLogEntry>) -> RotationLog {
        RotationLog {
            snapshot: None,
            entries,
        }
    }

    #[test]
    fn empty_log_returns_none() {
        let log = RotationLog::empty();
        assert_eq!(latest_writers(&log), None);
        assert_eq!(writers_at(&log, &[[0; 32]], |_, _| false), None);
    }

    #[test]
    fn latest_writers_returns_last_appended() {
        let log = log_of(vec![
            entry(1, 100, 0xAA, &[0xAA], 1),
            entry(2, 200, 0xBB, &[0xBB], 2),
        ]);
        assert_eq!(
            latest_writers(&log),
            Some([0xBB].into_iter().map(pk).collect())
        );
    }

    #[test]
    fn writers_at_with_empty_parents_falls_back_to_latest() {
        // ADR: empty causal_parents → match latest_writers (v2 LWW compat).
        let log = log_of(vec![
            entry(1, 100, 0xAA, &[0xAA], 1),
            entry(2, 200, 0xBB, &[0xBB], 2),
        ]);
        assert_eq!(
            writers_at(&log, &[], |_, _| false),
            Some([0xBB].into_iter().map(pk).collect())
        );
    }

    #[test]
    fn writers_at_returns_only_reachable_entries() {
        // ADR Example A: sequential rotations. R1 is the parent, R2 builds
        // on it. Querying with causal_parents = [R1.delta_id] should pick
        // R1, NOT R2 — R2 hasn't happened yet from R1's frontier.
        let log = log_of(vec![
            entry(1, 100, 0xAA, &[0xAA, 0xBB], 1),
            entry(2, 200, 0xBB, &[0xBB, 0xCC], 2),
        ]);

        let happens_before = |a: &[u8; 32], b: &[u8; 32]| a == &[1; 32] && b == &[2; 32];

        let writers = writers_at(&log, &[[1; 32]], happens_before);
        assert_eq!(writers, Some([0xAA, 0xBB].into_iter().map(pk).collect()));

        let writers = writers_at(&log, &[[2; 32]], happens_before);
        assert_eq!(writers, Some([0xBB, 0xCC].into_iter().map(pk).collect()));
    }

    #[test]
    fn writers_at_concurrent_siblings_resolved_by_hlc() {
        // ADR Example B: two concurrent siblings with different HLCs.
        // Larger HLC wins.
        let log = log_of(vec![
            entry(1, 20, 0xAA, &[0xAA], 10),
            entry(2, 21, 0xBB, &[0xBB], 10),
        ]);

        let none_precede = |_: &[u8; 32], _: &[u8; 32]| false;
        let writers = writers_at(&log, &[[1; 32], [2; 32]], none_precede);
        assert_eq!(writers, Some([0xBB].into_iter().map(pk).collect()));
    }

    #[test]
    fn writers_at_hlc_tie_resolved_by_signer_bytes() {
        // ADR Example C: HLC tie (same time AND same node ID).
        // Smaller signer pubkey bytes win.
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
        let log = log_of(vec![mk(1, 0xAA, &[0xAA, 0xCC]), mk(2, 0xBB, &[0xBB, 0xDD])]);

        let none_precede = |_: &[u8; 32], _: &[u8; 32]| false;
        let writers = writers_at(&log, &[[1; 32], [2; 32]], none_precede);
        // 0xAA is smaller → wins.
        assert_eq!(writers, Some([0xAA, 0xCC].into_iter().map(pk).collect()));
    }

    #[test]
    fn writers_at_causal_precedence_overrides_hlc() {
        // R1 happens-before R2, but R1.hlc > R2.hlc (clock skew). Causal
        // wins: R2's author saw R1's reality and chose to rotate from there.
        let log = log_of(vec![
            entry(1, 999, 0xAA, &[0xAA], 1), // huge HLC
            entry(2, 100, 0xBB, &[0xBB], 2), // smaller HLC
        ]);

        let happens_before = |a: &[u8; 32], b: &[u8; 32]| a == &[1; 32] && b == &[2; 32];
        let writers = writers_at(&log, &[[2; 32]], happens_before);
        assert_eq!(writers, Some([0xBB].into_iter().map(pk).collect()));
    }

    #[test]
    fn writers_at_unreachable_entries_ignored() {
        // Entry exists but isn't reachable from the queried parents.
        // Returns None — verifier falls back as it sees fit.
        let log = log_of(vec![entry(1, 100, 0xAA, &[0xAA], 1)]);
        let writers = writers_at(&log, &[[42; 32]], |_, _| false);
        assert_eq!(writers, None);
    }

    #[test]
    fn writers_at_falls_back_to_snapshot_when_entries_unreachable() {
        // P6 compaction wrote a snapshot but no live entries reach the query.
        let snap_writers: BTreeSet<PublicKey> = [0xEE].into_iter().map(pk).collect();
        let log = RotationLog {
            snapshot: Some(RotationSnapshot {
                writers: snap_writers.clone(),
                cutoff_index: 5,
            }),
            entries: vec![entry(99, 100, 0xFF, &[0xFF], 99)],
        };

        // happens_before never matches: live entry at delta 99 is unreachable.
        let writers = writers_at(&log, &[[1; 32]], |_, _| false);
        assert_eq!(writers, Some(snap_writers));
    }
}
