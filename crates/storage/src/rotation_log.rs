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
//! The log is materialized as an `UnorderedMap<[u8; 32], RotationLogEntry>`
//! child of the Shared anchor entity (keyed by `delta_id`), so each rotation
//! is a hashed Merkle child that converges through the normal entity-tree
//! sync path. This module owns the entry/log types and the DAG-free
//! local-execution resolver; the node crate's `rotation_log_reader` owns the
//! causal merge-time resolver.
//!
//! # Compaction (shape locked in, not implemented)
//!
//! The [`RotationLog::snapshot`] field is reserved for P6 compaction: a sliding
//! window of recent entries plus a snapshot of the writer set at the cutoff.
//! The threshold (epic suggests 1000 entries) is a P6 measurement; the shape
//! is fixed here so the on-disk format doesn't need a breaking change later.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use crate::entities::OpMask;
use crate::logical_clock::HybridTimestamp;

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
    /// `None` only for legacy / unsigned-bootstrap entries — see the
    /// node-side `rotation_log_reader::writers_at` for how those
    /// participate in ordering (they sort after any `Some(...)` entry
    /// at equal HLC, mirroring the "smaller bytes win" rule with `None`
    /// treated as "larger than any").
    pub signer: Option<PublicKey>,

    /// Ed25519 signature over `signed_payload`, produced by `signer`. `None`
    /// for legacy / unsigned-bootstrap entries.
    ///
    /// P3 (core#2716): once the rotation log rides ordinary sync as hashed
    /// state (a child of the anchor rather than a side store), its entries are
    /// untrusted in transit — a peer can ship any bytes. So each entry carries
    /// its signature and is verified at **resolve time** (`writers_at`) against
    /// the writer set in effect at its causal parents, instead of only at
    /// append time on the apply path. This moves the authentication gate from
    /// "trust what we appended" to "verify what we resolve".
    pub signature: Option<[u8; 64]>,

    /// The exact message `signature` covers: the rotation action's
    /// `payload_for_signing()` (a 32-byte hash). Stored so a resolver can
    /// verify the entry without reconstructing the original action (which it
    /// no longer has — only the log entry survives). `None` iff `signature`
    /// is `None`. See [`signature`](Self::signature).
    pub signed_payload: Option<[u8; 32]>,

    /// Resolved writer set after this rotation, each writer with its
    /// [`OpMask`]. `BTreeMap` so the on-wire representation is canonical
    /// (sorted by key), matching the rest of the `Shared` storage path.
    pub new_writers: BTreeMap<PublicKey, OpMask>,

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
    pub writers: BTreeMap<PublicKey, OpMask>,

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

/// Resolve the writer set for the **local-execution gate** — the set
/// `SharedStorage` checks in `insert` / `rotate_writers` *before* the action
/// syncs (and the settled-state fallback in `Interface::resolve_anchor_writers`).
///
/// Unlike the merge-time resolver (`rotation_log_reader::writers_at` in the
/// node crate), this has **no DAG and no `happens_before`**: it runs inside the
/// WASM guest, which can read the rotation log but not the context DAG. Instead
/// it leans on an invariant the HLC already provides. After the HLC
/// receive-rule fix (#2635) the hybrid clock is **causally monotonic** — a
/// rotation created after applying another always carries a strictly greater
/// `delta_hlc`. So ADR-0001's ordering ("causal precedence, then HLC, then
/// signer") collapses, for a well-formed log, to just **max by
/// `(delta_hlc, signer)`** over the live entries, computable from the log alone.
///
/// Two properties this buys over the old `entries.last()` gate (core#2673):
/// - **Insertion-order invariant.** `max_by` over the entry *set* is
///   independent of the order this node happened to append them, so two nodes
///   that applied the same concurrent rotations resolve the *same* writer set
///   locally — the convergence the insertion-order gate lacked.
/// - **Matches the merge verifier** for any log whose HLCs respect causality
///   (the normal case post-#2635). If a pathological log violates HLC
///   monotonicity (pre-#2635 data, or a clock pushed backwards) this may pick a
///   different winner than the causal `writers_at`, but that only loosens the
///   *local* gate; the merge path stays the security boundary and still rejects
///   anything genuinely unauthorized. It is never weaker than `entries.last()`.
///
/// Returns `None` only when the log has neither entries nor a compaction
/// snapshot.
#[must_use]
pub fn resolve_local(log: &RotationLog) -> Option<BTreeMap<PublicKey, OpMask>> {
    if let Some(entry) = log.entries.iter().max_by(|a, b| {
        // HLC first (causally monotonic post-#2635), then signer as the
        // ADR-0001 tiebreak: smaller signer bytes win, `None` (unsigned legacy)
        // treated as larger so it loses ties. This mirrors the `(delta_hlc,
        // signer)` tail of `rotation_log_reader::writers_at`, minus the
        // `happens_before` steps the guest can't evaluate.
        match a.delta_hlc.cmp(&b.delta_hlc) {
            core::cmp::Ordering::Equal => {}
            non_eq => return non_eq,
        }
        match (&a.signer, &b.signer) {
            (Some(sa), Some(sb)) => sb.digest().cmp(sa.digest()),
            (Some(_), None) => core::cmp::Ordering::Greater,
            (None, Some(_)) => core::cmp::Ordering::Less,
            (None, None) => core::cmp::Ordering::Equal,
        }
    }) {
        return Some(entry.new_writers.clone());
    }
    log.snapshot.as_ref().map(|s| s.writers.clone())
}

/// Resolve the writer set **as of** the causal point of a write at storage-HLC
/// `at` — the writers in effect when that write was authored, NOT the latest
/// set ([`resolve_local`]).
///
/// This is the local resolver for verifying a value entity whose causal DAG
/// position is unavailable (a HashComparison-pushed leaf carries no delta
/// parents, so the node can't run the exact `writers_at(parents)`). A value
/// authored under writer `w` and then rotated out by a LATER rotation must
/// still verify against the set that contained `w` — checking it against the
/// *latest* set wrongly rejects it (the #2716/#2673 concurrent-rotation
/// `SharedMember` split-brain, where the rotation originator could never accept
/// a peer's pre-rotation value).
///
/// `at` is a storage HLC (`Metadata::updated_at` / `SignatureData::nonce`), the
/// same clock a rotation entry's `writers_nonce` records. We consider only
/// **signed** entries (`signer.is_some()`): their `writers_nonce` is the
/// rotation author's real nonce on that clock, so `writers_nonce <= at` is a
/// sound "happened at or before this write" test; an UNSIGNED entry carries no
/// authoritative nonce (and can't authoritatively rotate anyway — the merge
/// verifier requires a signature), so it never forms the cut. Among the
/// eligible entries we pick the latest by the same `(delta_hlc, signer)` order
/// as [`resolve_local`]. If no signed rotation is at/before `at`, fall back to
/// the compaction snapshot (the genesis / floor writer set).
///
/// Soundness: for SEQUENTIAL rotations (one causal chain) this resolves the
/// exact set in effect at `at` — a removed writer's LATER write resolves to the
/// post-removal set and is correctly rejected; a then-valid writer's earlier
/// write resolves to the pre-removal set and is accepted. For genuinely
/// CONCURRENT rotations it is an approximation (the gossip/delta path's
/// `writers_at(parents)` stays the exact boundary) — the design's accepted
/// "liveness wrinkle". It is never weaker than `resolve_local` (latest): it can
/// only authorize a signer against an *earlier* set that genuinely contained
/// them.
#[must_use]
pub fn resolve_local_as_of(log: &RotationLog, at: u64) -> Option<BTreeMap<PublicKey, OpMask>> {
    let eligible = log
        .entries
        .iter()
        .filter(|e| e.signer.is_some() && e.writers_nonce <= at);
    if let Some(entry) = eligible.max_by(|a, b| {
        match a.delta_hlc.cmp(&b.delta_hlc) {
            core::cmp::Ordering::Equal => {}
            non_eq => return non_eq,
        }
        match (&a.signer, &b.signer) {
            (Some(sa), Some(sb)) => sb.digest().cmp(sa.digest()),
            (Some(_), None) => core::cmp::Ordering::Greater,
            (None, Some(_)) => core::cmp::Ordering::Less,
            (None, None) => core::cmp::Ordering::Equal,
        }
    }) {
        return Some(entry.new_writers.clone());
    }
    log.snapshot.as_ref().map(|s| s.writers.clone())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
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

    // ---- resolve_local: the DAG-free local-execution gate (core#2673) ----

    fn log_of(entries: Vec<RotationLogEntry>) -> RotationLog {
        RotationLog {
            snapshot: None,
            entries,
        }
    }

    #[test]
    fn resolve_local_none_when_empty() {
        assert_eq!(resolve_local(&RotationLog::empty()), None);
    }

    #[test]
    fn resolve_local_picks_max_hlc() {
        // Sequential rotations: the later (greater-HLC) one wins, regardless of
        // vector position.
        let log = log_of(vec![
            entry(2, 200, 0xBB, &[0xAA, 0xBB], 2),
            entry(1, 100, 0xAA, &[0xAA], 1),
        ]);
        assert_eq!(
            resolve_local(&log),
            Some(
                [0xAA, 0xBB]
                    .into_iter()
                    .map(|b| (pk(b), OpMask::FULL))
                    .collect()
            )
        );
    }

    #[test]
    fn resolve_local_as_of_returns_the_set_in_effect_at_the_cut() {
        // Genesis writers = {A}; a signed rotation by A at storage-HLC nonce 200
        // rotates A out in favour of {C}. A value authored BEFORE the rotation
        // (nonce 150) must resolve to the pre-rotation set {A} so its signature
        // (by A, a writer then) verifies; a value authored AFTER (nonce 250)
        // must resolve to {C}, so A (removed) is correctly rejected. Resolving
        // the LATEST set ({C}) for the pre-rotation value is the
        // concurrent-rotation `SharedMember` reject bug this resolver fixes.
        let genesis: BTreeMap<PublicKey, OpMask> =
            [0xAA].into_iter().map(|b| (pk(b), OpMask::FULL)).collect();
        let log = RotationLog {
            snapshot: Some(RotationSnapshot {
                writers: genesis.clone(),
                cutoff_index: 0,
            }),
            // writers_nonce 200 is the rotation's storage HLC; signed (signer A).
            entries: vec![entry(1, 200, 0xAA, &[0xCC], 200)],
        };

        // Before the rotation → genesis {A}.
        assert_eq!(resolve_local_as_of(&log, 150), Some(genesis.clone()));
        // At/after the rotation → {C}.
        let post: BTreeMap<PublicKey, OpMask> =
            [0xCC].into_iter().map(|b| (pk(b), OpMask::FULL)).collect();
        assert_eq!(resolve_local_as_of(&log, 200), Some(post.clone()));
        assert_eq!(resolve_local_as_of(&log, 250), Some(post));
    }

    #[test]
    fn resolve_local_as_of_ignores_unsigned_entries_in_the_cut() {
        // An UNSIGNED entry (signer=None) carries no authoritative nonce and
        // cannot authoritatively rotate, so it must never form the as-of cut —
        // otherwise a value would be authorized against (or rejected by) a set
        // no one signed. Here only the genesis {A} is trustworthy at nonce 250.
        let genesis: BTreeMap<PublicKey, OpMask> =
            [0xAA].into_iter().map(|b| (pk(b), OpMask::FULL)).collect();
        let mut unsigned = entry(1, 300, 0xBB, &[0xCC], 0);
        unsigned.signer = None;
        let log = RotationLog {
            snapshot: Some(RotationSnapshot {
                writers: genesis.clone(),
                cutoff_index: 0,
            }),
            entries: vec![unsigned],
        };
        assert_eq!(resolve_local_as_of(&log, 250), Some(genesis));
    }

    #[test]
    fn resolve_local_is_insertion_order_invariant() {
        // The core#2673 property: two nodes append the same two concurrent
        // rotations in opposite orders and must resolve the SAME writer set
        // (the old `entries.last()` gate returned different sets per node).
        let r1 = entry(1, 20, 0xAA, &[0xAA, 0xCC], 1); // {A, C}
        let r2 = entry(2, 21, 0xBB, &[0xBB, 0xCC], 1); // {B, C}, larger HLC

        let node1 = log_of(vec![r1.clone(), r2.clone()]); // self-logged R1, then got R2
        let node2 = log_of(vec![r2, r1]); // self-logged R2, then got R1

        assert_eq!(resolve_local(&node1), resolve_local(&node2));
        // ...and it is the HLC winner R2 = {B, C}.
        assert_eq!(
            resolve_local(&node1),
            Some(
                [0xBB, 0xCC]
                    .into_iter()
                    .map(|b| (pk(b), OpMask::FULL))
                    .collect()
            )
        );
    }

    #[test]
    fn resolve_local_hlc_tie_broken_by_signer() {
        // Equal HLC (same time + same node ID) → smaller signer bytes win,
        // matching `rotation_log_reader::writers_at`.
        let identical = |delta_id: u8, signer: u8, writers: &[u8]| {
            use core::num::NonZeroU128;

            use crate::logical_clock::{Timestamp, ID, NTP64};
            let ts = Timestamp::new(NTP64(50), ID::from(NonZeroU128::new(1).unwrap()));
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
                writers_nonce: 1,
            }
        };
        let log = log_of(vec![
            identical(1, 0xAA, &[0xAA, 0xCC]),
            identical(2, 0xBB, &[0xBB, 0xDD]),
        ]);
        // 0xAA < 0xBB → {A, C} wins; order-independent.
        assert_eq!(
            resolve_local(&log),
            Some(
                [0xAA, 0xCC]
                    .into_iter()
                    .map(|b| (pk(b), OpMask::FULL))
                    .collect()
            )
        );
    }

    #[test]
    fn resolve_local_falls_back_to_snapshot() {
        // Compacted log with no live entries → the snapshot's writer set.
        let snap: BTreeMap<PublicKey, OpMask> =
            [0xEE].into_iter().map(|b| (pk(b), OpMask::FULL)).collect();
        let log = RotationLog {
            snapshot: Some(RotationSnapshot {
                writers: snap.clone(),
                cutoff_index: 3,
            }),
            entries: Vec::new(),
        };
        assert_eq!(resolve_local(&log), Some(snap));
    }
}
