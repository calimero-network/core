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

use std::collections::BTreeMap;

use calimero_primitives::identity::PublicKey;
use calimero_storage::entities::OpMask;
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
pub fn latest_writers(log: &RotationLog) -> Option<BTreeMap<PublicKey, OpMask>> {
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
) -> Option<BTreeMap<PublicKey, OpMask>>
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

/// Resolve the writer set at `causal_parents` with **per-entry authentication**
/// — the P3 security boundary (core#2716 S2).
///
/// The rotation-log child rides ordinary sync, so any peer can inject entries
/// (untrusted in transit). This fold makes each rotation earn its place: walking
/// the reachable entries in causal order, a rotation is applied only when its
/// `signer` held [`OpMask::ADMIN`] in the writer set resolved *just before* it
/// AND `verify(entry)` confirms the signature. An unauthorized or forged entry
/// is skipped (the prior set carries forward), so a fabricated child cannot
/// grant writer rights. The genesis entry — the one with no reachable causal
/// ancestor in the log — is self-authorizing: its signature establishes the
/// initial set (the context creator bootstraps the writers).
///
/// `verify` is injected (rather than calling `ed25519_verify` directly) so the
/// fold is pure and unit-testable, and so the caller owns the
/// commitment-binding policy (the entry's signature must commit to its
/// `new_writers`/nonce — see the wiring site). It receives each candidate entry
/// and returns whether the signature is cryptographically valid.
///
/// Returns `None` only when there is neither a reachable entry nor a snapshot.
pub fn writers_at_authenticated<H, V>(
    log: &RotationLog,
    causal_parents: &[[u8; 32]],
    happens_before: H,
    verify: V,
) -> Option<BTreeMap<PublicKey, OpMask>>
where
    H: Fn(&[u8; 32], &[u8; 32]) -> bool,
    V: Fn(&RotationLogEntry) -> bool,
{
    let mut reachable: Vec<&RotationLogEntry> = if causal_parents.is_empty() {
        log.entries.iter().collect()
    } else {
        log.entries
            .iter()
            .filter(|e| {
                causal_parents
                    .iter()
                    .any(|p| e.delta_id == *p || happens_before(&e.delta_id, p))
            })
            .collect()
    };

    // Ascending causal order: causal precedence first, then HLC, then signer.
    // (The mirror of `writers_at`'s `max_by`, applied as a total sort so the
    // fold visits ancestors before descendants.)
    reachable.sort_by(|a, b| {
        if happens_before(&a.delta_id, &b.delta_id) {
            return std::cmp::Ordering::Less;
        }
        if happens_before(&b.delta_id, &a.delta_id) {
            return std::cmp::Ordering::Greater;
        }
        match a.delta_hlc.cmp(&b.delta_hlc) {
            std::cmp::Ordering::Equal => {}
            non_eq => return non_eq,
        }
        // Smaller signer bytes sort first; `None` (unsigned) sorts last.
        match (&a.signer, &b.signer) {
            (Some(sa), Some(sb)) => sa.digest().cmp(sb.digest()),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });

    let mut current: Option<BTreeMap<PublicKey, OpMask>> =
        log.snapshot.as_ref().map(|s| s.writers.clone());

    for entry in reachable {
        let authorized = match &current {
            // Genesis: self-authorizing — the bootstrap establishes the set.
            None => verify(entry),
            // Rotation: signer must have held ADMIN in the prior set, and the
            // signature must verify. Otherwise the entry is forged/unauthorized
            // and the prior set carries forward.
            Some(prior) => {
                entry
                    .signer
                    .as_ref()
                    .and_then(|s| prior.get(s))
                    .is_some_and(|mask| mask.contains(OpMask::ADMIN))
                    && verify(entry)
            }
        };
        if authorized {
            current = Some(entry.new_writers.clone());
        }
    }

    current
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU128;

    use calimero_storage::entities::OpMask;
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
            signature: None,
            signed_payload: None,
            new_writers: writers
                .iter()
                .copied()
                .map(|b| (pk(b), OpMask::FULL))
                .collect(),
            writers_nonce: nonce,
        }
    }

    fn log_of(entries: Vec<RotationLogEntry>) -> RotationLog {
        RotationLog {
            snapshot: None,
            entries,
        }
    }

    fn sorted_keys(writers: &BTreeMap<PublicKey, OpMask>) -> Vec<PublicKey> {
        let mut k: Vec<PublicKey> = writers.keys().copied().collect();
        k.sort();
        k
    }

    fn expect(bytes: &[u8]) -> Vec<PublicKey> {
        let mut v: Vec<PublicKey> = bytes.iter().copied().map(pk).collect();
        v.sort();
        v
    }

    #[test]
    fn empty_log_returns_none() {
        let log = RotationLog::empty();
        assert_eq!(latest_writers(&log), None);
        assert_eq!(writers_at(&log, &[[0; 32]], |_, _| false), None);
    }

    /// P3 S2 security boundary: a rotation authored by a non-writer (here Carol,
    /// who was never granted ADMIN) must be REJECTED by the authenticated fold,
    /// even when its signature is cryptographically valid — authorization is
    /// against the writer set resolved just before the entry.
    #[test]
    fn authenticated_fold_rejects_unauthorized_rotation() {
        let hb = |a: &[u8; 32], b: &[u8; 32]| a[0] < b[0];
        let log = log_of(vec![
            entry(1, 100, 0xAA, &[0xAA], 1), // genesis: Alice establishes {Alice}
            entry(2, 200, 0xAA, &[0xAA, 0xBB], 2), // Alice (ADMIN) adds Bob — OK
            entry(3, 300, 0xCC, &[0xCC], 3), // Carol (NOT a writer) forges a rotation
        ]);
        // All signatures "valid"; the fold still rejects Carol's rotation.
        let writers = writers_at_authenticated(&log, &[], hb, |_| true).unwrap();
        assert_eq!(
            sorted_keys(&writers),
            expect(&[0xAA, 0xBB]),
            "Carol's unauthorized rotation must be excluded"
        );
    }

    /// A rotation whose signature does NOT verify (forged / replayed) is rejected
    /// even though its claimed signer IS an authorized writer.
    #[test]
    fn authenticated_fold_rejects_bad_signature() {
        let hb = |a: &[u8; 32], b: &[u8; 32]| a[0] < b[0];
        let log = log_of(vec![
            entry(1, 100, 0xAA, &[0xAA], 1),       // genesis
            entry(2, 200, 0xAA, &[0xAA, 0xBB], 2), // Alice authorized as signer...
        ]);
        // ...but `verify` rejects entry 2's signature → rotation dropped.
        let writers = writers_at_authenticated(&log, &[], hb, |e| e.delta_id[0] == 1).unwrap();
        assert_eq!(
            sorted_keys(&writers),
            expect(&[0xAA]),
            "a rotation with an invalid signature must be rejected"
        );
    }

    /// The authorized linear chain folds to the latest set (sanity: the fold
    /// doesn't over-reject legitimate rotations).
    #[test]
    fn authenticated_fold_accepts_authorized_chain() {
        let hb = |a: &[u8; 32], b: &[u8; 32]| a[0] < b[0];
        let log = log_of(vec![
            entry(1, 100, 0xAA, &[0xAA], 1),
            entry(2, 200, 0xAA, &[0xAA, 0xBB], 2),
            entry(3, 300, 0xBB, &[0xBB], 3), // Bob (ADMIN in {Alice,Bob}) rotates to {Bob}
        ]);
        let writers = writers_at_authenticated(&log, &[], hb, |_| true).unwrap();
        assert_eq!(sorted_keys(&writers), expect(&[0xBB]));
    }

    #[test]
    fn latest_writers_returns_last_appended() {
        let log = log_of(vec![
            entry(1, 100, 0xAA, &[0xAA], 1),
            entry(2, 200, 0xBB, &[0xBB], 2),
        ]);
        assert_eq!(
            latest_writers(&log),
            Some([0xBB].into_iter().map(|b| (pk(b), OpMask::FULL)).collect())
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
            Some([0xBB].into_iter().map(|b| (pk(b), OpMask::FULL)).collect())
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
        assert_eq!(
            writers,
            Some(
                [0xAA, 0xBB]
                    .into_iter()
                    .map(|b| (pk(b), OpMask::FULL))
                    .collect()
            )
        );

        let writers = writers_at(&log, &[[2; 32]], happens_before);
        assert_eq!(
            writers,
            Some(
                [0xBB, 0xCC]
                    .into_iter()
                    .map(|b| (pk(b), OpMask::FULL))
                    .collect()
            )
        );
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
        assert_eq!(
            writers,
            Some([0xBB].into_iter().map(|b| (pk(b), OpMask::FULL)).collect())
        );
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
            signature: None,
            signed_payload: None,
            new_writers: writers
                .iter()
                .copied()
                .map(|b| (pk(b), OpMask::FULL))
                .collect(),
            writers_nonce: 10,
        };
        let log = log_of(vec![mk(1, 0xAA, &[0xAA, 0xCC]), mk(2, 0xBB, &[0xBB, 0xDD])]);

        let none_precede = |_: &[u8; 32], _: &[u8; 32]| false;
        let writers = writers_at(&log, &[[1; 32], [2; 32]], none_precede);
        // 0xAA is smaller → wins.
        assert_eq!(
            writers,
            Some(
                [0xAA, 0xCC]
                    .into_iter()
                    .map(|b| (pk(b), OpMask::FULL))
                    .collect()
            )
        );
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
        assert_eq!(
            writers,
            Some([0xBB].into_iter().map(|b| (pk(b), OpMask::FULL)).collect())
        );
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
        let snap_writers: BTreeMap<PublicKey, OpMask> =
            [0xEE].into_iter().map(|b| (pk(b), OpMask::FULL)).collect();
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

    // ---- Cross-node concurrent-rotation convergence (the #2668 guarantee) ----
    //
    // #2665 makes each node self-log its OWN rotations alongside received ones,
    // so after sync every node holds the SAME SET of rotation-log entries — but
    // in its own *insertion order* (a node appends its locally-created rotation
    // when it executes, and a peer's when sync delivers it; two nodes that
    // rotate concurrently append the pair in opposite orders). The convergence
    // guarantee therefore rests on `writers_at` being a deterministic function
    // of (entry set, causal parents) that is INVARIANT to insertion order. The
    // tests above resolve a single ordering; these assert order-invariance
    // directly — the property that makes two nodes agree end-to-end.
    //
    // Scenario mirrors the merobox e2e: genesis writers {A, B}; node-1 rotates
    // -> {A, C} (R1) and node-2 rotates -> {B, C} (R2) concurrently; then a
    // settling rotation -> {A, C} (R3) causally after both. A = 0xAA,
    // B = 0xBB, C = 0xCC.

    #[test]
    fn writers_at_is_insertion_order_invariant_under_concurrent_rotations() {
        // R1 {A,C} and R2 {B,C} are causally concurrent (neither precedes the
        // other); R2 has the larger HLC, so it wins the tie.
        let r1 = entry(1, 20, 0xAA, &[0xAA, 0xCC], 1);
        let r2 = entry(2, 21, 0xBB, &[0xBB, 0xCC], 1);

        // node-1 self-logged R1 first, then received R2; node-2 the reverse.
        let log_node1 = log_of(vec![r1.clone(), r2.clone()]);
        let log_node2 = log_of(vec![r2, r1]);

        // Merged frontier sees both; they are concurrent, so nothing precedes.
        let parents = [[1; 32], [2; 32]];
        let none_precede = |_: &[u8; 32], _: &[u8; 32]| false;

        let from_node1 = writers_at(&log_node1, &parents, none_precede);
        let from_node2 = writers_at(&log_node2, &parents, none_precede);

        // Both nodes resolve the SAME set despite opposite insertion order —
        // this is what makes them converge.
        assert_eq!(from_node1, from_node2);
        // ...and it is the HLC winner R2 = {B, C}.
        assert_eq!(
            from_node1,
            Some(
                [0xBB, 0xCC]
                    .into_iter()
                    .map(|b| (pk(b), OpMask::FULL))
                    .collect()
            )
        );
        // C is in both concurrent sets, so it survives whichever wins — the
        // property the e2e relies on to pick a guaranteed-authorized settler.
        assert!(from_node1.unwrap().contains_key(&pk(0xCC)));
    }

    #[test]
    fn settling_rotation_converges_and_revokes_under_concurrency() {
        // After the concurrent pair, a settling rotation R3 -> {A, C} is issued
        // causally after BOTH (it builds on the merged frontier).
        let r1 = entry(1, 20, 0xAA, &[0xAA, 0xCC], 1);
        let r2 = entry(2, 21, 0xBB, &[0xBB, 0xCC], 1);
        let r3 = entry(3, 30, 0xCC, &[0xAA, 0xCC], 2);

        // Same entry set, opposite insertion order for the concurrent pair; R3
        // is last on both (it is causally newest, so it is appended last
        // everywhere).
        let log_node1 = log_of(vec![r1.clone(), r2.clone(), r3.clone()]);
        let log_node2 = log_of(vec![r2, r1, r3]);

        // R1 and R2 both happen-before R3; R1 and R2 are concurrent.
        let happens_before = |a: &[u8; 32], b: &[u8; 32]| {
            (a == &[1; 32] && b == &[3; 32]) || (a == &[2; 32] && b == &[3; 32])
        };
        let parents = [[3; 32]];

        let from_node1 = writers_at(&log_node1, &parents, happens_before);
        let from_node2 = writers_at(&log_node2, &parents, happens_before);

        // Deterministic convergence to the settled set on every node...
        assert_eq!(from_node1, from_node2);
        let resolved = from_node1.expect("settled writer set");
        assert_eq!(
            resolved,
            [0xAA, 0xCC]
                .into_iter()
                .map(|b| (pk(b), OpMask::FULL))
                .collect()
        );
        // ...and B is retroactively revoked everywhere (the bonus the e2e
        // asserts by rejecting B's post-settle write).
        assert!(!resolved.contains_key(&pk(0xBB)));
        assert!(resolved.contains_key(&pk(0xAA)));
        assert!(resolved.contains_key(&pk(0xCC)));
    }
}
