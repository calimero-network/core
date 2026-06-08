//! DAG-based delta storage and application
//!
//! Wraps calimero-dag and provides context-aware delta application via WASM.
//!
//! # Merge Handling
//!
//! When concurrent deltas are detected (deltas from different branches of the DAG),
//! the applier uses CRDT merge semantics instead of failing on hash mismatch.
//! This ensures that all nodes converge to the same state regardless of the
//! order in which they receive concurrent deltas.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use calimero_context_client::client::ContextClient;
use calimero_context_client::{ContextAtomic, ContextAtomicKey};
use calimero_dag::{
    ApplyError, CausalDelta, DagStore as CoreDagStore, DeltaApplier, PendingStats,
    MAX_DELTA_QUERY_LIMIT,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::delta::StorageDelta;
use calimero_storage::entities::{OpMask, StorageType};
use calimero_storage::rotation_log::{RotationLog, RotationLogEntry, RotationSnapshot};
use calimero_storage::store::Key as StorageKey;
use eyre::Result;
use indexmap::IndexMap;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::sync::rotation_log_reader;

/// Maximum entries in the applier-local topology mirror. Exceeding this
/// triggers oldest-first eviction (insertion-ordered via `IndexMap`)
/// down to 90% of the cap. Applied both inside `apply()` (steady state)
/// and at the end of `restore_topology` (startup seed) so a long-lived
/// node persisting >10K deltas doesn't carry the full set in memory
/// across restarts. Per #2272 review.
const MAX_TOPOLOGY_ENTRIES: usize = 10_000;

/// Soft budget for `context_client.execute(...)` inside `apply()`. The
/// caller holds the DAG write lock during this await; a slow WASM
/// merge-apply (#2199) therefore pins it. This is *not* a hard cap: the
/// apply runs on a `spawn_blocking` thread that cannot be cancelled, and
/// abandoning it here would release the DAG write lock while the apply
/// completes and `commit()`s its storage writes anyway — late, racing the
/// next delta (the delta would be in storage but absent from the DAG, then
/// re-synced and re-applied → divergent root hash). The merge-apply is
/// gas-bounded so it terminates, and post-#2238 is fast enough that holding
/// the lock for its duration is acceptable; exceeding this threshold only
/// produces a warning. See #2199 / #2238.
const WASM_APPLY_TIMEOUT: Duration = Duration::from_secs(15);

/// Upper bound on how many deltas `add_deltas_batch` will register under a
/// single DAG write lock. Bounding the batch caps the worst-case lock-hold
/// time: in-order inputs each run WASM under that one lock, so an unbounded
/// batch could starve concurrent gossip-delta application and head lookups.
/// Callers with more deltas than this must chunk their input.
pub(crate) const DELTA_BATCH_MAX: usize = 100;

/// Result of adding a delta with cascaded event information
#[derive(Debug)]
pub struct AddDeltaResult {
    /// Whether the delta was applied immediately (true) or went pending (false)
    pub applied: bool,
    /// List of (delta_id, events_data) for cascaded deltas that have event handlers to execute
    pub cascaded_events: Vec<([u8; 32], Vec<u8>)>,
}

/// One verified delta to feed into [`DeltaStore::add_deltas_batch`].
///
/// Mirrors the per-delta arguments of [`DeltaStore::add_delta_with_events`].
/// Signature and governance verification are the caller's responsibility and
/// must happen *before* the delta is placed in a batch — the batch path only
/// registers and persists, exactly like the single-delta path.
#[derive(Debug)]
pub struct BatchDeltaInput {
    pub delta: CausalDelta<Vec<Action>>,
    pub events: Option<Vec<u8>>,
    pub author_id: Option<PublicKey>,
    pub governance_position_blob: Option<Vec<u8>>,
    pub delta_signature: Option<[u8; 64]>,
}

/// Result of [`DeltaStore::add_deltas_batch`].
#[derive(Debug, Default)]
pub struct BatchAddResult {
    /// Input deltas that are applied in the DAG once the batch settles —
    /// includes deltas that went pending mid-batch but were then unblocked by
    /// a later input in the same batch.
    pub applied: Vec<[u8; 32]>,
    /// Input deltas still waiting on missing parents after the batch settled.
    pub pending: Vec<[u8; 32]>,
    /// Input deltas whose `dag.add_delta` returned an error and were skipped.
    /// These are neither applied nor pending (they may not be in the DAG at
    /// all); they are re-fetched on the next sync. Kept distinct so callers
    /// and metrics don't misreport a failed delta as pending.
    pub failed: Vec<[u8; 32]>,
    /// (delta_id, events_data) for *pre-existing* pending deltas that this
    /// batch unblocked and whose handlers must run. Events carried by the
    /// batch inputs themselves are not forwarded here — they ride in the
    /// inputs' `applied: true` records and follow the same restart-replay
    /// safety net as a single `add_delta`'s primary.
    pub forwarded_events: Vec<([u8; 32], Vec<u8>)>,
}

/// Max orphaned member deltas held across all anchors before the buffer stops
/// accepting new ones (and they fall back to the DAG / HashComparison re-fetch
/// path). Bounds the memory a peer can pin by sending members whose anchor never
/// arrives (e.g. a forged anchor id).
const MAX_ANCHOR_PENDING: usize = 256;

/// Per-anchor cap on buffered orphan members. Bounds the blast radius of a
/// single forged anchor id: without it, one bogus anchor with 256 distinct
/// delta ids could exhaust the global buffer and starve buffering for every
/// legitimate anchor. 32 comfortably covers a real cold-join burst for one
/// domain while leaving global headroom for many anchors.
const MAX_PENDING_PER_ANCHOR: usize = 32;

/// In-memory buffer for **orphaned member deltas**: a delta carrying a
/// `StorageType::SharedMember { anchor }` action that arrived before its
/// `anchor` (the `Shared` wrapper entity / rotation log) had synced. Without
/// the anchor the member's writers can't be resolved, so applying it now would
/// fail closed. Rather than drop it and wait for a HashComparison re-fetch, we
/// hold it here keyed by the missing anchor id and re-inject it the moment that
/// anchor applies.
///
/// This is a pure **liveness** optimization layered on the already-correct
/// fail-closed path: anything not buffered (or evicted on overflow, or lost on
/// restart since this is in-memory) still converges via the re-fetch fallback.
///
/// Self-contained and lock-free so it can be unit-tested directly; the
/// `DeltaStore` wraps it in an `RwLock`.
#[derive(Debug, Default)]
struct AnchorPendingBuffer {
    /// anchor id → FIFO queue of deltas waiting on it.
    by_anchor: HashMap<Id, VecDeque<BatchDeltaInput>>,
    /// delta ids currently buffered, for O(1) dedup across anchors.
    seen: HashSet<[u8; 32]>,
}

impl AnchorPendingBuffer {
    /// Buffer `input` under `anchor`. Returns `Ok(())` if buffered; returns
    /// `Err(input)` (handing ownership back) if it was already buffered or the
    /// global cap is reached, so the caller can fall back to the normal path.
    fn buffer(&mut self, anchor: Id, input: BatchDeltaInput) -> Result<(), BatchDeltaInput> {
        if self.seen.contains(&input.delta.id) {
            return Err(input);
        }
        if self.seen.len() >= MAX_ANCHOR_PENDING {
            return Err(input);
        }
        // Per-anchor cap: a single (possibly forged) anchor id can't fill the
        // global buffer and starve other anchors.
        if self
            .by_anchor
            .get(&anchor)
            .is_some_and(|q| q.len() >= MAX_PENDING_PER_ANCHOR)
        {
            return Err(input);
        }
        let _inserted = self.seen.insert(input.delta.id);
        self.by_anchor.entry(anchor).or_default().push_back(input);
        Ok(())
    }

    /// Remove and return every delta waiting on `anchor` (FIFO order).
    fn take_for(&mut self, anchor: &Id) -> Vec<BatchDeltaInput> {
        let Some(queue) = self.by_anchor.remove(anchor) else {
            return Vec::new();
        };
        for input in &queue {
            let _removed = self.seen.remove(&input.delta.id);
        }
        queue.into_iter().collect()
    }

    /// Total buffered deltas across all anchors.
    fn len(&self) -> usize {
        self.seen.len()
    }
}

/// Result of `load_persisted_deltas`.
#[derive(Debug, Default)]
pub struct LoadPersistedResult {
    /// Number of deltas restored into the DAG.
    pub loaded_count: usize,
    /// Deltas that still have `events: Some(..)` on disk with
    /// `applied: true` — handlers for these were interrupted before
    /// `execute_cascaded_events` could clear them (#2185). Caller is
    /// expected to feed these through `execute_cascaded_events` which
    /// will clear them on success.
    pub pending_handler_events: Vec<([u8; 32], Vec<u8>)>,
}

/// Result of checking for missing parents with cascaded event information
#[derive(Debug)]
pub struct MissingParentsResult {
    /// IDs of deltas that are truly missing (need to be requested from network)
    pub missing_ids: Vec<[u8; 32]>,
    /// IDs of ALL deltas newly applied during the check, regardless of
    /// whether they carry events. Includes both cascade-from-pending
    /// applications and parents loaded from the DB and applied via
    /// `ParentPlan::Add`. Callers can use this to detect whether the
    /// current delta was applied as a side effect of the call without
    /// re-acquiring the DAG lock.
    pub cascaded_ids: Vec<[u8; 32]>,
    /// Subset of `cascaded_ids` that have events to forward to handlers,
    /// as (delta_id, events_data) pairs.
    pub cascaded_events: Vec<([u8; 32], Vec<u8>)>,
}

/// Internal plan describing what to do with one potentially-missing parent
/// during `get_missing_parents`. Built in the DB-lookup phase with no DAG
/// lock held, then consumed in the write-lock phase.
enum ParentPlan {
    /// Parent is already marked `applied=true` in the database (typically
    /// because *this* node authored it). Restore topology only; do not
    /// re-apply.
    Restore {
        parent_id: [u8; 32],
        dag_delta: CausalDelta<Vec<Action>>,
        expected_root_hash: [u8; 32],
    },
    /// Parent is persisted but not applied. Run through `add_delta` which
    /// executes WASM and may cascade children via `apply_pending`.
    Add {
        parent_id: [u8; 32],
        dag_delta: CausalDelta<Vec<Action>>,
    },
}

/// Applier that applies actions to WASM storage via ContextClient
///
/// Supports two application modes:
/// 1. **Sequential**: When delta's parent matches our current state - verify hash
/// 2. **Merge**: When concurrent branches detected - CRDT merge, skip hash check
#[derive(Debug)]
struct ContextStorageApplier {
    context_client: ContextClient,
    context_id: ContextId,
    our_identity: PublicKey,
    /// Maps delta_id -> actual_computed_root_hash for parent state tracking
    /// Used to detect concurrent branches (merge scenarios)
    /// CRITICAL: This stores the ACTUAL computed hash, NOT expected_root_hash!
    parent_hashes: Arc<RwLock<HashMap<[u8; 32], [u8; 32]>>>,
    /// Set of delta IDs that were applied as merges on our node.
    /// Any child of a merged delta must also be treated as a potential merge,
    /// because the computed hash on our node differs from what the author computed.
    /// This prevents the case where Node-A applies delta X as merge → hash H1,
    /// while Node-B applies X sequentially → hash H2. When Node-B's child delta
    /// arrives, we must recognize that our parent hash differs from the author's.
    merged_deltas: Arc<RwLock<HashSet<[u8; 32]>>>,
    /// `delta_id → parents` for every applied delta this node has seen.
    /// Maintained inside `apply()` (after WASM success) and seeded by
    /// `restore_topology` from `load_persisted_deltas`. Read-only inside
    /// `apply()` to derive the `happens_before` predicate that
    /// [`rotation_log_reader::writers_at`] needs — kept separate from
    /// the `Arc<RwLock<CoreDagStore>>` so reads here don't deadlock
    /// against the dag write lock the caller holds during `add_delta`.
    ///
    /// Stored as `IndexMap` so eviction at the `MAX_TOPOLOGY_ENTRIES`
    /// cap iterates insertion order (oldest-first) — `HashMap`'s
    /// iteration order is non-deterministic and could evict recent
    /// ancestry links still needed by buffered children, which is
    /// security-adjacent: a missing link makes `happens_before` return
    /// false, `writers_at` returns `None`, and the verifier falls back
    /// to v2 stored-writers, potentially admitting a revoked writer.
    /// (#2266 + #2272 review)
    ///
    /// Wrapped in an inner `Arc` for copy-on-write reads: the
    /// `resolve_effective_writers_for_delta` read path needs a snapshot
    /// that outlives the lock guard (the `happens_before` closure holds
    /// it across the per-entity loop), but deep-cloning the whole map on
    /// every Shared-touching apply allocated up to ~1MB at the cap on the
    /// hot sync path. With the inner `Arc`, reads bump a refcount and
    /// writers `Arc::make_mut` to clone-on-first-write only when a reader
    /// snapshot is still live.
    topology: Arc<RwLock<Arc<IndexMap<[u8; 32], Vec<[u8; 32]>>>>>,
    /// Armed by [`DeltaStore::add_delta_internal`] around its `dag.add_delta`
    /// call so the inbound `apply()` *retains* the per-context execution lock
    /// (stashing the guard in [`Self::apply_lock_slot`]) instead of releasing
    /// it when the WASM apply returns. The caller then holds that lock across
    /// the subsequent `dag_heads` commit.
    ///
    /// Without this, the lock is released between the WASM apply — which makes
    /// a just-rotated-in writer authoritative in *storage* — and the
    /// `dag_heads` commit. A local write that runs in that window reads the
    /// pre-apply heads and forks the DAG: its parents exclude the rotation,
    /// so every peer's `writers_at(parents)` resolves the *old* writer set and
    /// rejects it as `InvalidSignature`, an unconvergeable split-brain.
    ///
    /// Read/written only under the `dag` write lock (held across
    /// `add_delta_internal`'s `dag.add_delta`), which serializes all access.
    retain_apply_lock: std::sync::atomic::AtomicBool,
    /// Relays the retained execution-lock guard from `apply()` up to
    /// `add_delta_internal` when [`Self::retain_apply_lock`] is set. Holds at
    /// most one guard at a time (the per-context lock is not re-entrant); a
    /// cascaded buffered-child apply reuses it via `ContextAtomic::Held`.
    apply_lock_slot: std::sync::Mutex<Option<ContextAtomicKey>>,
}

#[async_trait::async_trait]
impl DeltaApplier<Vec<Action>> for ContextStorageApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        let apply_start = std::time::Instant::now();

        // Get current context state
        let context = self
            .context_client
            .get_context(&self.context_id)
            .map_err(|e| ApplyError::Application(format!("Failed to get context: {e}")))?
            .ok_or_else(|| ApplyError::Application("Context not found".to_owned()))?;

        let current_root_hash = *context.root_hash;

        // Detect if this is a merge scenario (concurrent branches)
        // A merge is needed when our current state differs from what the delta's parent expects
        let is_merge_scenario = self.is_merge_scenario(delta, &current_root_hash).await;

        if is_merge_scenario {
            info!(
                context_id = %self.context_id,
                delta_id = %Hash::from(delta.id),
                current_root_hash = %Hash::from(current_root_hash),
                delta_expected_hash = %Hash::from(delta.expected_root_hash),
                "Concurrent branch detected - applying with CRDT merge semantics"
            );
        }

        // Log action types — debug! because this fires per-delta on the hot
        // path and the body already carries an internal `DELTA_DEBUG` marker
        // indicating it's diagnostic, not operationally significant.
        if tracing::enabled!(tracing::Level::DEBUG) {
            let action_types: Vec<&str> = delta
                .payload
                .iter()
                .map(|a| match a {
                    Action::Compare { .. } => "Compare",
                    Action::Add { .. } => "Add",
                    Action::Update { .. } => "Update",
                    Action::DeleteRef { .. } => "DeleteRef",
                })
                .collect();
            debug!(
                context_id = %self.context_id,
                delta_id = ?delta.id,
                action_types = ?action_types,
                "DELTA_DEBUG: Actions in delta before WASM execution"
            );
        }

        // #2266: resolve `effective_writers` for every Shared entity
        // touched by this delta, then ship the artifact as
        // `StorageDelta::CausalActions` so the receiver's verifier
        // validates Shared signatures against the pre-resolved set
        // (per ADR 0001 / writers_at(delta.parents)) instead of
        // falling back to v2 stored writers. Resolution reads `topology`
        // (this applier's local copy of the DAG parent links) so it
        // doesn't contend with the dag write lock held by our caller.
        let effective_writers = self
            .resolve_effective_writers_for_delta(delta)
            .await
            .map_err(|e| {
                ApplyError::Application(format!("Failed to resolve effective writers: {e}"))
            })?;

        let artifact = borsh::to_vec(&StorageDelta::CausalActions {
            actions: delta.payload.clone(),
            delta_id: delta.id,
            delta_hlc: delta.hlc,
            effective_writers,
        })
        .map_err(|e| ApplyError::Application(format!("Failed to serialize delta: {e}")))?;

        let wasm_start = std::time::Instant::now();

        // Do NOT bound this with a hard `timeout`: `context_client.execute`
        // runs the WASM merge-apply on a `spawn_blocking` thread, which
        // can't be cancelled. If we timed out and dropped this future, the
        // caller's DAG write lock would be released and the delta recorded
        // as not-applied, while the blocking apply ran to completion and
        // then `commit()`d its storage writes + bumped `context.root_hash`
        // anyway — late, racing the next delta's apply. Storage would hold
        // a delta the DAG doesn't know about (re-synced, re-applied,
        // divergent root hash). So we keep waiting; the apply is gas-bounded
        // (it terminates) and post-#2238 is fast. Only warn if it runs long.
        // See #2199 / #2238.
        // When the caller (`add_delta_internal` / `add_deltas_batch`) has armed
        // `retain_apply_lock`, run this inbound apply with `ContextAtomic::Lock`
        // so the executor hands the per-context execution-lock guard back in
        // `outcome.atomic`; we stash it in `apply_lock_slot` for the caller to
        // hold across its `dag_heads` commit. A cascaded buffered-child apply in
        // the same `dag.add_delta` reuses the already-held guard via
        // `ContextAtomic::Held` (the lock is not re-entrant). When not armed
        // (e.g. the local path's `try_process_pending`), behavior is unchanged
        // (`None`).
        //
        // The slot is empty only between the `take()` here and the stash-back
        // after the await. That window cannot be observed by another `apply()`:
        // a `dag.add_delta` processes the primary and any cascaded children
        // strictly sequentially on this task (its `apply_pending` runs only
        // after the current apply returns), so there is never a concurrent
        // `apply()` to find the empty slot and issue a second `Lock`. The
        // armed/disarmed flag is set and read on this same task under the `dag`
        // write lock; `Acquire`/`Release` is used for defensive clarity (the
        // WASM body runs on a `spawn_blocking` thread inside `execute`).
        let retain_lock = self
            .retain_apply_lock
            .load(std::sync::atomic::Ordering::Acquire);
        let atomic = if retain_lock {
            Some(
                match self
                    .apply_lock_slot
                    .lock()
                    .expect("apply_lock_slot poisoned")
                    .take()
                {
                    Some(key) => ContextAtomic::Held(key),
                    None => ContextAtomic::Lock,
                },
            )
        } else {
            None
        };

        let execute = self.context_client.execute(
            &self.context_id,
            &self.our_identity,
            "__calimero_sync_next".to_owned(),
            artifact,
            vec![],
            atomic,
        );
        tokio::pin!(execute);
        let mut outcome = match tokio::time::timeout(WASM_APPLY_TIMEOUT, &mut execute).await {
            Ok(res) => res,
            Err(_elapsed) => {
                warn!(
                    context_id = %self.context_id,
                    delta_id = %Hash::from(delta.id),
                    over_budget_secs = WASM_APPLY_TIMEOUT.as_secs(),
                    is_merge = is_merge_scenario,
                    "WASM merge-apply over soft budget — still waiting (a spawn_blocking apply can't be cancelled without leaving storage/DAG divergent); see #2199/#2238"
                );
                execute.await
            }
        }
        .map_err(|e| ApplyError::Application(format!("WASM execution failed: {e}")))?;

        // Stash the retained guard for `add_delta_internal` to hold across its
        // `dag_heads` commit. On the error path above we already returned via
        // `?`; the executor dropped the guard when the failed message
        // completed, so the slot stays empty and the caller commits heads
        // unlocked — safe, because a failed apply advances no heads.
        if retain_lock {
            *self
                .apply_lock_slot
                .lock()
                .expect("apply_lock_slot poisoned") = outcome.atomic.take();
        }

        let wasm_elapsed_ms = wasm_start.elapsed().as_secs_f64() * 1000.0;

        // Hot path — prefer Display over Debug to skip per-byte formatting.
        // Gate the returns_ok/returns_len derivation behind the same
        // level check the debug! expansion uses internally, matching
        // the `tracing::enabled!` convention used above at line 143.
        if tracing::enabled!(tracing::Level::DEBUG) {
            let (returns_ok, returns_len) = match &outcome.returns {
                Ok(Some(v)) => (true, v.len()),
                Ok(None) => (true, 0),
                Err(_) => (false, 0),
            };
            debug!(
                context_id = %self.context_id,
                delta_id = %Hash::from(delta.id),
                root_hash = %outcome.root_hash,
                returns_ok,
                returns_len,
                is_merge = is_merge_scenario,
                wasm_ms = format!("{:.2}", wasm_elapsed_ms),
                "WASM sync completed execution"
            );
        }

        if outcome.returns.is_err() {
            return Err(ApplyError::Application(format!(
                "WASM sync returned error: {:?}",
                outcome.returns
            )));
        }

        let computed_hash = outcome.root_hash;

        // In a CRDT environment, hash mismatches are EXPECTED when there are concurrent writes.
        // The delta's expected_root_hash is based on the sender's linear history, but we may have
        // additional data from concurrent writes (our own or from other nodes).
        //
        // We NEVER reject deltas due to hash mismatch - CRDT merge semantics ensure eventual
        // consistency. The hash mismatch just means we have concurrent state.
        //
        // Log the mismatch for debugging, but always accept the delta.
        if *computed_hash != delta.expected_root_hash {
            if is_merge_scenario {
                info!(
                    context_id = %self.context_id,
                    delta_id = %Hash::from(delta.id),
                    computed_hash = %computed_hash,
                    delta_expected_hash = %Hash::from(delta.expected_root_hash),
                    merge_wasm_ms = format!("{:.2}", wasm_elapsed_ms),
                    "Merge produced new hash (expected - concurrent branches merged)"
                );
            } else {
                // Even "sequential" applications can produce different hashes if we have
                // concurrent state that the sender doesn't know about. This is normal in
                // a distributed CRDT system.
                debug!(
                    context_id = %self.context_id,
                    delta_id = %Hash::from(delta.id),
                    computed_hash = %computed_hash,
                    expected_hash = %Hash::from(delta.expected_root_hash),
                    "Hash mismatch (concurrent state) - CRDT merge ensures consistency"
                );
            }
        }

        // #2266: record this delta's parent links into the applier's
        // topology mirror so cascaded children's `apply()` can resolve
        // happens_before against an up-to-date view. Done after WASM
        // success so a failed apply doesn't pollute the topology with
        // an unapplied delta. Bounded the same way as `parent_hashes`
        // below so it can't grow without limit on long-lived nodes.
        {
            let mut topology = self.topology.write().await;
            // `make_mut` clones the inner map only if a reader snapshot is
            // still holding the previous `Arc`; otherwise it mutates in
            // place.
            let topology = Arc::make_mut(&mut topology);
            let _previous = topology.insert(delta.id, delta.parents.clone());

            // Evict oldest-first (insertion order) so recently-applied
            // deltas — whose ancestry links are still needed by buffered
            // children — survive the cap. Cf. #2272 review on
            // non-deterministic eviction.
            cap_topology(topology);
        }

        // Store the ACTUAL computed hash after applying this delta for future merge detection
        // This is what OUR state actually is, not what the remote expected.
        // Child deltas will check if our current state matches the parent's result.
        //
        // CRITICAL: We must store the computed hash, NOT delta.expected_root_hash!
        // In merge scenarios, computed_hash differs from expected_root_hash.
        // If we stored expected_root_hash, sequential child deltas would incorrectly
        // appear to be merge scenarios because our state wouldn't match.
        {
            let mut hashes = self.parent_hashes.write().await;
            hashes.insert(delta.id, *computed_hash);

            // CLEANUP: Prevent unbounded memory growth
            // Keep only the most recent entries. Old delta hashes are rarely needed
            // since merge detection mainly looks at recent parent-child relationships.
            // 10,000 entries = ~640KB (64 bytes per entry), sufficient for most scenarios.
            const MAX_PARENT_HASH_ENTRIES: usize = 10_000;
            if hashes.len() > MAX_PARENT_HASH_ENTRIES {
                // Remove ~10% of oldest entries when threshold exceeded
                // Since HashMap doesn't track insertion order, we do a simple drain
                // This is rare (only when threshold exceeded) so perf impact is minimal
                let excess = hashes.len() - (MAX_PARENT_HASH_ENTRIES * 9 / 10);
                let keys_to_remove: Vec<_> = hashes.keys().take(excess).copied().collect();
                for key in keys_to_remove {
                    hashes.remove(&key);
                }
                debug!(
                    context_id = %self.context_id,
                    removed = excess,
                    remaining = hashes.len(),
                    "Pruned parent_hashes cache to prevent memory growth"
                );
            }
        }

        // Track if this delta was applied as a merge
        // Child deltas of merged deltas need special handling because:
        // - Our computed hash differs from what the delta author computed
        // - Child deltas from other nodes are based on THEIR computed hash, not ours
        // - So children of merged deltas should also be treated as merges
        if is_merge_scenario || *computed_hash != delta.expected_root_hash {
            let mut merged = self.merged_deltas.write().await;
            merged.insert(delta.id);

            // CLEANUP: Same strategy as parent_hashes
            const MAX_MERGED_ENTRIES: usize = 10_000;
            if merged.len() > MAX_MERGED_ENTRIES {
                let excess = merged.len() - (MAX_MERGED_ENTRIES * 9 / 10);
                let keys_to_remove: Vec<_> = merged.iter().take(excess).copied().collect();
                for key in keys_to_remove {
                    merged.remove(&key);
                }
            }
        }

        let total_elapsed_ms = apply_start.elapsed().as_secs_f64() * 1000.0;

        // Log with unique marker for parsing: DELTA_APPLY_TIMING
        info!(
            context_id = %self.context_id,
            delta_id = %Hash::from(delta.id),
            action_count = delta.payload.len(),
            final_root_hash = %computed_hash,
            was_merge = is_merge_scenario,
            wasm_ms = format!("{:.2}", wasm_elapsed_ms),
            total_ms = format!("{:.2}", total_elapsed_ms),
            "DELTA_APPLY_TIMING"
        );

        Ok(())
    }
}

impl ContextStorageApplier {
    /// Determine if this delta application is a merge scenario.
    ///
    /// A merge is needed when:
    /// 1. The delta has a non-genesis parent, AND
    /// 2. Our current state has diverged from that parent's expected state
    ///
    /// This happens when concurrent deltas were applied before this one.
    ///
    /// Detection strategies (in order):
    /// 1. If parent hash is tracked, compare directly
    /// 2. If the delta expects a different state than we have, it's a merge
    /// 3. If parent is unknown and we're not at genesis, assume merge (conservative)
    async fn is_merge_scenario(
        &self,
        delta: &CausalDelta<Vec<Action>>,
        current_root_hash: &[u8; 32],
    ) -> bool {
        // Genesis parent means this is a first delta - no merge needed
        let has_only_genesis_parent = delta.parents.iter().all(|p| *p == [0u8; 32]);
        if has_only_genesis_parent {
            // But if we have state and delta expects genesis state, we've diverged
            if *current_root_hash != [0u8; 32] {
                debug!(
                    context_id = %self.context_id,
                    delta_id = ?delta.id,
                    current_root_hash = ?Hash::from(*current_root_hash),
                    "Delta from genesis but we have state - concurrent branch detected"
                );
                return true;
            }
            return false;
        }

        // Check if any parent was applied as a merge on our node
        // If so, child deltas from other nodes are based on THEIR computed hash for that parent,
        // which differs from ours. So we must treat this as a merge.
        let merged = self.merged_deltas.read().await;
        for parent_id in &delta.parents {
            if *parent_id == [0u8; 32] {
                continue; // Skip genesis
            }

            if merged.contains(parent_id) {
                debug!(
                    context_id = %self.context_id,
                    delta_id = ?delta.id,
                    parent_id = ?parent_id,
                    current_root_hash = ?Hash::from(*current_root_hash),
                    "Parent was previously merged - child inherits merge requirement"
                );
                return true;
            }
        }
        drop(merged);

        // Get the expected root hash of the delta's parent(s)
        let hashes = self.parent_hashes.read().await;

        for parent_id in &delta.parents {
            if *parent_id == [0u8; 32] {
                continue; // Skip genesis
            }

            if let Some(parent_computed_hash) = hashes.get(parent_id) {
                // Parent's computed hash is what OUR state was AFTER applying that parent
                // If our current state differs, we've diverged (either we merged, or have local changes)
                if parent_computed_hash != current_root_hash {
                    debug!(
                        context_id = %self.context_id,
                        delta_id = ?delta.id,
                        parent_id = ?parent_id,
                        parent_computed_hash = ?Hash::from(*parent_computed_hash),
                        current_root_hash = ?Hash::from(*current_root_hash),
                        "State diverged from parent's computed hash - treating as merge"
                    );
                    return true;
                } else {
                    // State matches, but we CANNOT know if the delta's author was working
                    // on the same state we have. They might have applied the parent as a
                    // CRDT merge (resulting in different state than us) and then created
                    // this child delta based on THEIR computed state.
                    //
                    // Key insight: We created the parent locally with hash H1.
                    // The remote node received our parent, applied it as merge → hash H2.
                    // They created this child delta expecting H2 state.
                    // We have H1 state. If we apply sequentially, we get WRONG result.
                    //
                    // Detection: Check if delta's expected_root_hash is "reasonable" given
                    // our current state. If it's very different, the author was on different state.
                    //
                    // CONSERVATIVE APPROACH: Always treat remote deltas as potential merges
                    // when they build on parents we created locally. The CRDT merge is
                    // idempotent - if states were identical, merge produces same result.
                    //
                    // For now, check if delta.expected_root_hash matches what we'd expect.
                    // If our current_root_hash is H1 and delta expects H3 (significantly different),
                    // it's a merge scenario.

                    // Since we can't pre-compute what hash we'd get, use the heuristic:
                    // If we're not the same as the delta's parent according to head_root_hashes,
                    // treat as merge.
                    debug!(
                        context_id = %self.context_id,
                        delta_id = ?delta.id,
                        parent_id = ?parent_id,
                        parent_computed_hash = ?Hash::from(*parent_computed_hash),
                        current_root_hash = ?Hash::from(*current_root_hash),
                        "State matches parent's computed hash - checking if truly sequential"
                    );

                    // Additional check: was this parent created by us locally (not through
                    // the normal add_delta flow)? If so, the parent might have been merged
                    // by the remote node, meaning this child was created on different state.
                    //
                    // We detect locally-created parents by checking if they were added
                    // through add_delta_internal (which sets a flag) vs loaded from storage.
                    // For now, we use a simpler check: if parent is in head_root_hashes with
                    // the same hash as parent_computed_hash, it was processed normally.
                    // If not, it might have been created locally and needs merge treatment.
                }
            } else {
                // Parent was created by another node - we don't have its hash tracked
                // This is common when receiving deltas from other nodes.
                // Conservative: treat as merge since we can't verify sequential application
                debug!(
                    context_id = %self.context_id,
                    delta_id = ?delta.id,
                    parent_id = ?parent_id,
                    current_root_hash = ?Hash::from(*current_root_hash),
                    "Unknown parent (not in our tracking) - treating as merge"
                );
                return true;
            }
        }

        // If we reach this point, every parent of this delta was:
        //   1. present in our `parent_hashes` map (we know its computed hash);
        //   2. computed by us to the same hash that matches our current state;
        //   3. NOT in `merged_deltas` (was applied sequentially, not as a merge).
        //
        // (1) ensures the parent isn't an opaque "from elsewhere" delta whose
        // applied state we can't reason about. (2) ensures our current state
        // is exactly the state the delta's author was working against — we
        // haven't drifted via concurrent local writes. (3) ensures the
        // parent's apply was deterministic for both us and the author —
        // neither side took the CRDT-merge path that can produce different
        // hashes for the same input.
        //
        // Under those three conditions the delta is sequential by every
        // available signal and direct apply will produce the same result the
        // author got. The previous code returned `true` here on a "CRDT merge
        // is idempotent so always-merge is safe" theory; that's false in
        // practice — the merge-apply path (`Interface::save_internal` →
        // `try_merge_data` / `try_merge_non_root`) routes through different
        // metadata-stamping and ancestor-resolution code than the direct
        // apply path, so two nodes that BOTH had the exact same state
        // pre-delta could compute different post-delta root hashes if one
        // direct-applied and the other merge-applied. That divergence then
        // cascades: the `merged_deltas` write below records the
        // divergent-applying side, so every subsequent child of this delta
        // also takes the merge path (line ~465 above) and never reconverges
        // — the "same DAG heads, different root hash" symptom in #2319.
        //
        // Cases where merge is still needed are caught above and return
        // `true` before we get here: legitimate concurrent branches (line
        // ~498), unknown parents (line ~554), and parents previously
        // applied via merge (line ~473).
        debug!(
            context_id = %self.context_id,
            delta_id = ?delta.id,
            current_root_hash = ?Hash::from(*current_root_hash),
            delta_expected_hash = ?Hash::from(delta.expected_root_hash),
            "All parent checks passed and state matches — treating as sequential apply"
        );
        false
    }

    /// Resolve the writer set for every Shared entity touched by `delta`.
    ///
    /// Iterates the action payload, picks out Shared `Add`/`Update`/
    /// `DeleteRef`s, dedups by entity, and resolves each via
    /// [`rotation_log_reader::writers_at`] against this applier's
    /// `topology` view of the DAG.
    ///
    /// Returns a map keyed by entity id; non-Shared entities are absent.
    /// An empty result is normal — a delta with only User/Frozen/Public
    /// actions has nothing to resolve.
    ///
    /// No caching: each delta is applied at most once (the DAG dedups
    /// by content-addressed `delta.id`), so a per-`(entity, delta_id)`
    /// cache could never hit. Removed per #2272 review.
    async fn resolve_effective_writers_for_delta(
        &self,
        delta: &CausalDelta<Vec<Action>>,
    ) -> Result<BTreeMap<Id, BTreeMap<PublicKey, OpMask>>> {
        let mut shared_entities: BTreeSet<Id> = BTreeSet::new();
        // (member entity id, its anchor id). A `SharedMember` carries no writer
        // set of its own; it resolves the ANCHOR's writers and the result is
        // keyed by the MEMBER id so `apply_action` finds it under the member's
        // own `effective_writers` lookup.
        let mut members: Vec<(Id, Id)> = Vec::new();
        for action in &delta.payload {
            let metadata = match action {
                Action::Add { metadata, .. }
                | Action::Update { metadata, .. }
                | Action::DeleteRef { metadata, .. } => metadata,
                Action::Compare { .. } => continue,
            };
            match metadata.storage_type {
                StorageType::Shared { .. } => {
                    let _inserted = shared_entities.insert(action.id());
                }
                StorageType::SharedMember { anchor, .. } => {
                    members.push((action.id(), anchor));
                }
                StorageType::Public | StorageType::User { .. } | StorageType::Frozen => {}
            }
        }

        let mut out: BTreeMap<Id, BTreeMap<PublicKey, OpMask>> = BTreeMap::new();
        if shared_entities.is_empty() && members.is_empty() {
            return Ok(out);
        }

        // Snapshot topology once per delta apply. The `happens_before`
        // closure consults this snapshot for every reachability test
        // inside `writers_at`, avoiding repeated lock acquisitions. The
        // inner `Arc` makes this a refcount bump rather than a deep clone
        // of the whole map; the guard is released immediately.
        let topology_snapshot = Arc::clone(&*self.topology.read().await);

        for entity_id in shared_entities {
            // Read the rotation log directly from the datastore rather
            // than via `MainStorage::storage_read`. `MainStorage` routes
            // through the `RUNTIME_ENV` thread-local which is only
            // installed inside `context_client.execute()` — i.e. *after*
            // this function runs. See `load_rotation_log_direct` doc.
            let log =
                match load_rotation_log_direct(&self.context_client, self.context_id, entity_id) {
                    Ok(Some(log)) => log,
                    Ok(None) => continue, // No log → verifier falls back to v2 stored-writers.
                    Err(e) => {
                        return Err(eyre::eyre!(
                            "rotation_log direct read for entity {entity_id:?} failed: {e}"
                        ))
                    }
                };

            let resolved = rotation_log_reader::writers_at(&log, &delta.parents, |a, b| {
                happens_before_in_topology(&topology_snapshot, a, b)
            });

            if let Some(set) = resolved {
                let _replaced = out.insert(entity_id, set);
            }
        }

        // Members resolve from their anchor's rotation log at the same causal
        // cut. Cache per anchor — one anchor commonly gates many members. When
        // the anchor has no rotation log (never rotated), leave the member
        // unset: `apply_action` then falls back to the anchor's settled local
        // state (genesis writers), which is correct precisely because nothing
        // has rotated. When the anchor is absent entirely, the fallback yields
        // the empty set and verification fails closed (the member is retried
        // once the anchor syncs).
        let mut anchor_cache: BTreeMap<Id, Option<BTreeMap<PublicKey, OpMask>>> = BTreeMap::new();
        for (member_id, anchor) in members {
            let resolved = match anchor_cache.get(&anchor) {
                Some(cached) => cached.clone(),
                None => {
                    let set = match load_rotation_log_direct(
                        &self.context_client,
                        self.context_id,
                        anchor,
                    ) {
                        Ok(Some(log)) => {
                            rotation_log_reader::writers_at(&log, &delta.parents, |a, b| {
                                happens_before_in_topology(&topology_snapshot, a, b)
                            })
                        }
                        Ok(None) => None,
                        Err(e) => {
                            return Err(eyre::eyre!(
                                "rotation_log direct read for anchor {anchor:?} failed: {e}"
                            ))
                        }
                    };
                    let _cached = anchor_cache.insert(anchor, set.clone());
                    set
                }
            };
            if let Some(set) = resolved {
                let _replaced = out.insert(member_id, set);
            }
        }

        Ok(out)
    }

    /// Seed `topology` with deltas restored by `load_persisted_deltas`.
    ///
    /// Persisted deltas are restored into the dag via
    /// `restore_applied_delta` without going through `apply()`, so the
    /// topology mirror would otherwise miss them and `happens_before`
    /// would incorrectly return false for ancestry that crosses the
    /// pre-restart boundary. Call this once after persisted-delta
    /// restoration completes.
    async fn restore_topology(&self, deltas: impl IntoIterator<Item = ([u8; 32], Vec<[u8; 32]>)>) {
        let mut topology = self.topology.write().await;
        let topology = Arc::make_mut(&mut topology);
        seed_topology(topology, deltas);
        // #2272 review: enforce the same `MAX_TOPOLOGY_ENTRIES` cap that
        // `apply()` uses. Without this, a context with 100K persisted
        // deltas would seed all of them at startup and consume ~10MB+
        // until enough new deltas triggered the steady-state cap.
        cap_topology(topology);
    }
}

/// Pure seed of a topology mirror. Extracted so the seeding semantics
/// (later entries overwrite earlier ones with the same delta id) can be
/// unit-tested without standing up a `ContextStorageApplier`.
fn seed_topology(
    topology: &mut IndexMap<[u8; 32], Vec<[u8; 32]>>,
    deltas: impl IntoIterator<Item = ([u8; 32], Vec<[u8; 32]>)>,
) {
    for (delta_id, parents) in deltas {
        let _previous = topology.insert(delta_id, parents);
    }
}

/// Trim a topology mirror to `MAX_TOPOLOGY_ENTRIES` by evicting the
/// oldest-inserted entries first (the `IndexMap` insertion-order
/// invariant is what makes this deterministic). Targets 90% of the cap
/// after eviction so we don't thrash on every insert near the boundary.
///
/// Uses `IndexMap::drain(0..excess)` for an O(n) eviction in a single
/// memmove pass. A naive `shift_remove_index(0)` loop would be O(excess
/// × n) — at 100K persisted deltas during `restore_topology`, that's
/// ~9 billion shifts and a multi-second startup stall under the
/// topology write lock. Cf. PR #2272 review on quadratic eviction cost.
fn cap_topology(topology: &mut IndexMap<[u8; 32], Vec<[u8; 32]>>) {
    if topology.len() <= MAX_TOPOLOGY_ENTRIES {
        return;
    }
    let excess = topology.len() - (MAX_TOPOLOGY_ENTRIES * 9 / 10);
    // The `Drain` iterator's destructor removes the front-range entries
    // and shifts the tail left by `excess` in a single memmove. Letting
    // it drop at end of statement is sufficient.
    drop(topology.drain(0..excess));
}

/// Read a `RotationLog` directly from the datastore for a given context
/// + entity, bypassing `calimero_storage::rotation_log::load` (which
/// goes through the `RUNTIME_ENV` thread-local that's only installed
/// inside `context_client.execute()`).
///
/// The on-disk shape mirrors what
/// `calimero_node_primitives::sync::storage_bridge::create_runtime_env`
/// writes inside the WASM execute scope: `Key::RotationLog(entity_id)`
/// is hashed to a 32-byte state key, and the value lives under
/// `ContextState::new(context_id, state_key)`. Decoded via Borsh.
///
/// Returns `Ok(None)` for entities with no rotation log yet (every
/// pre-rotation Shared entity), which is fine — the receiver verifier
/// then falls back to v2 stored-writers, matching pre-#2266 behavior.
pub(crate) fn load_rotation_log_direct(
    context_client: &ContextClient,
    context_id: ContextId,
    entity_id: Id,
) -> Result<Option<RotationLog>> {
    // P3: the rotation log's synced source of truth is the hashed collection
    // child — a parent entity with one child PER delta_id. Walk it directly:
    // read the parent's Index for the child list, then union each per-delta
    // child (a single-entry RotationLog). Falls back to the legacy side store
    // (`Key::RotationLog`) for anchors with no collection yet (pre-P3 /
    // mid-transition).
    let map_id = calimero_storage::interface::Interface::<
        calimero_storage::store::MainStorage,
    >::rotation_log_child_id(entity_id);
    if let Some(index) = read_entity_index_direct(context_client, context_id, map_id)? {
        let mut entries = Vec::new();
        if let Some(children) = index.children() {
            for child in children {
                // P3: each child is an `UnorderedMap` entry, so its stored value
                // is `borsh(Entry<([u8;32], RotationLogEntry)>)` — decode the
                // single entry it holds (NOT a bare `RotationLog` blob).
                if let Some(bytes) = read_entity_value_direct(
                    context_client,
                    context_id,
                    StorageKey::Entry(child.id()),
                )? {
                    if let Some(entry) =
                        calimero_storage::collections::decode_rotation_log_entry_child(&bytes)
                    {
                        entries.push(entry);
                    }
                }
            }
        }
        // Canonical order so resolution is insertion-order invariant.
        entries.sort_by(|a, b| a.delta_id.cmp(&b.delta_id));
        return Ok(Some(RotationLog {
            snapshot: None,
            entries,
        }));
    }
    load_rotation_log_bytes_direct(
        context_client,
        context_id,
        StorageKey::RotationLog(entity_id),
    )
}

/// Read + Borsh-decode an entity's `EntityIndex` (the child list etc.) via a
/// direct datastore lookup (no `RUNTIME_ENV`). Used by
/// [`load_rotation_log_direct`] to walk the rotation-log collection's children.
fn read_entity_index_direct(
    context_client: &ContextClient,
    context_id: ContextId,
    id: Id,
) -> Result<Option<calimero_storage::index::EntityIndex>> {
    let state_key =
        calimero_store::key::ContextState::new(context_id, StorageKey::Index(id).to_bytes());
    let handle = context_client.datastore_handle();
    let bytes: Option<Vec<u8>> = match handle.get(&state_key) {
        Ok(Some(state)) => Some(state.value.into_boxed().into_vec()),
        Ok(None) => None,
        Err(e) => return Err(eyre::eyre!("rotation_log index read failed: {e:?}")),
    };
    drop(handle);
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let index = borsh::from_slice::<calimero_storage::index::EntityIndex>(&bytes)
        .map_err(|e| eyre::eyre!("rotation_log index decode failed: {e}"))?;
    Ok(Some(index))
}

/// Read an entity's raw stored VALUE bytes via a direct datastore lookup (no
/// `RUNTIME_ENV`). Used by [`load_rotation_log_direct`] to read each rotation-log
/// map child, whose value is decoded by
/// [`calimero_storage::collections::decode_rotation_log_entry_child`].
fn read_entity_value_direct(
    context_client: &ContextClient,
    context_id: ContextId,
    key: StorageKey,
) -> Result<Option<Vec<u8>>> {
    let state_key = calimero_store::key::ContextState::new(context_id, key.to_bytes());
    let handle = context_client.datastore_handle();
    let bytes: Option<Vec<u8>> = match handle.get(&state_key) {
        Ok(Some(state)) => Some(state.value.into_boxed().into_vec()),
        Ok(None) => None,
        Err(e) => return Err(eyre::eyre!("rotation_log value read failed: {e:?}")),
    };
    drop(handle);
    Ok(bytes)
}

/// Read + Borsh-decode a `RotationLog` at an arbitrary `StorageKey` via a direct
/// datastore lookup (no `RUNTIME_ENV`). Shared by the P3 child read and the
/// legacy side-store read in [`load_rotation_log_direct`].
fn load_rotation_log_bytes_direct(
    context_client: &ContextClient,
    context_id: ContextId,
    key: StorageKey,
) -> Result<Option<RotationLog>> {
    let state_key = calimero_store::key::ContextState::new(context_id, key.to_bytes());
    let handle = context_client.datastore_handle();
    // Copy the bytes out before `handle` is dropped — `state.value` is a
    // `Slice<'_>` borrowed from the handle's read snapshot.
    let bytes: Option<Vec<u8>> = match handle.get(&state_key) {
        Ok(Some(state)) => Some(state.value.into_boxed().into_vec()),
        Ok(None) => None,
        Err(e) => return Err(eyre::eyre!("rotation_log datastore read failed: {e:?}")),
    };
    drop(handle);
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let log = borsh::from_slice::<RotationLog>(&bytes)
        .map_err(|e| eyre::eyre!("rotation_log decode failed: {e}"))?;
    Ok(Some(log))
}

/// Write `log` for `entity_id` via a DIRECT datastore write (no WASM
/// `RUNTIME_ENV`). The byte format is identical to
/// [`rotation_log::save`](calimero_storage::rotation_log) (borsh `RotationLog`
/// under `StorageKey::RotationLog`), so a log written here and one written by
/// the receive-path `MainStorage` hook are interchangeable. Used to self-log a
/// locally-created delta's own rotations, which happens after execute returns —
/// outside the env where `MainStorage` is valid.
fn save_rotation_log_direct(
    context_client: &ContextClient,
    context_id: ContextId,
    entity_id: Id,
    log: &RotationLog,
) -> Result<()> {
    let bytes = borsh::to_vec(log).map_err(|e| eyre::eyre!("rotation_log encode failed: {e}"))?;
    let storage_key = StorageKey::RotationLog(entity_id).to_bytes();
    let state_key = calimero_store::key::ContextState::new(context_id, storage_key);
    let mut handle = context_client.datastore_handle();
    handle
        .put(
            &state_key,
            &calimero_store::types::ContextState::from(calimero_store::slice::Slice::from(bytes)),
        )
        .map_err(|e| eyre::eyre!("rotation_log datastore write failed: {e:?}"))?;
    Ok(())
}

/// Seed `entity_id`'s rotation log with `writers` as its genesis/floor snapshot
/// via a direct datastore write, **if no log exists yet** (idempotent — never
/// clobbers real history). Datastore-direct counterpart of
/// [`rotation_log::seed_genesis`](calimero_storage::rotation_log::seed_genesis)
/// for the cold-join snapshot path (no `RUNTIME_ENV`): a joiner that receives
/// settled state needs the boundary writer set as its causal floor, so
/// `writers_at` is total for any post-join cut (it never applies deltas
/// predating its snapshot boundary).
pub(crate) fn seed_rotation_log_genesis_direct(
    context_client: &ContextClient,
    context_id: ContextId,
    entity_id: Id,
    writers: BTreeMap<PublicKey, OpMask>,
) -> Result<()> {
    if load_rotation_log_direct(context_client, context_id, entity_id)?.is_some() {
        return Ok(());
    }
    let log = RotationLog {
        snapshot: Some(RotationSnapshot {
            writers,
            cutoff_index: 0,
        }),
        entries: Vec::new(),
    };
    save_rotation_log_direct(context_client, context_id, entity_id, &log)
}

/// Self-log the `Shared` rotations carried by a locally-originated delta's
/// `actions` into their entities' rotation logs, via direct datastore writes.
///
/// The receive path records rotations in `Interface::maybe_append_rotation_log`
/// (inside the WASM sync-apply); a locally-created delta never goes through that
/// path, so without this a node is missing its OWN rotations and `writers_at`
/// stays asymmetric across nodes under concurrent rotation. Mirrors the
/// receive-path logic: append only on a real rotation — bootstrap (no prior
/// writers) or a changed writer set — never on a plain value-write. Idempotent
/// (dedup on `delta_id`), so it is safe to run on both the live notify
/// (`add_local_applied_delta`) and crash/restart recovery
/// (`load_persisted_deltas`); the latter backstops the case where the live
/// notify was skipped (DeltaStore not yet up) and the delta is later restored
/// as already-applied.
///
/// Best-effort: a per-entity read/write failure is logged and skipped, leaving
/// that entry for the next restart's restore to re-attempt — it never aborts
/// the (already-committed) delta.
fn self_log_rotations_direct(
    context_client: &ContextClient,
    context_id: ContextId,
    delta_id: [u8; 32],
    delta_hlc: calimero_storage::logical_clock::HybridTimestamp,
    actions: &[Action],
) {
    let mut appended: Vec<Id> = Vec::new();
    for action in actions {
        let (entity_id, metadata) = match action {
            Action::Add { id, metadata, .. } | Action::Update { id, metadata, .. } => {
                (*id, metadata)
            }
            Action::DeleteRef { .. } | Action::Compare { .. } => continue,
        };
        // Only `Shared` anchors own a rotation log; members/others don't.
        let StorageType::Shared {
            writers,
            signature_data,
        } = &metadata.storage_type
        else {
            continue;
        };
        let mut log = match load_rotation_log_direct(context_client, context_id, entity_id) {
            Ok(existing) => existing.unwrap_or_else(RotationLog::empty),
            Err(e) => {
                warn!(?e, %context_id, %entity_id,
                    "self-log: failed to read rotation log — skipping (restore will retry)");
                continue;
            }
        };
        // Append only on an actual rotation — bootstrap (no prior writers) OR
        // the writer set changed. Mirrors `maybe_append_rotation_log`.
        let prior = log
            .entries
            .last()
            .map(|e| &e.new_writers)
            .or_else(|| log.snapshot.as_ref().map(|s| &s.writers));
        if !prior.map_or(true, |p| p != writers) {
            continue;
        }
        // Idempotent on delta_id (one rotation per entity per delta).
        if log.entries.iter().any(|e| e.delta_id == delta_id) {
            continue;
        }
        log.entries.push(RotationLogEntry {
            delta_id,
            delta_hlc,
            signer: signature_data.as_ref().and_then(|s| s.signer),
            signature: signature_data.as_ref().map(|s| s.signature),
            signed_payload: signature_data
                .as_ref()
                .map(|_| action.payload_for_signing()),
            new_writers: writers.clone(),
            writers_nonce: signature_data.as_ref().map(|s| s.nonce).unwrap_or(0),
        });
        if let Err(e) = save_rotation_log_direct(context_client, context_id, entity_id, &log) {
            warn!(?e, %context_id, %entity_id,
                "self-log: failed to write rotation log — leaving for restore retry");
            continue;
        }
        appended.push(entity_id);
    }

    // Phase 2 of core#2716: self-logging just changed these anchors' resolved
    // writer sets WITHOUT an entity write, so their folded `own_hash` (which now
    // commits to the ACL) is stale — the originator would otherwise keep a root
    // that reflects the pre-rotation set and never converge with peers that
    // applied the rotation as a delta. Recompute + propagate, mirroring the
    // `union_received_rotation_logs` rehash on the receive path.
    //
    // NOTE: this corrects the originator's LOCAL root. The delta's
    // `expected_root_hash` was already computed at `commit_root` (before this
    // self-log), so it still reflects the stale fold; making the *gossiped*
    // expected root correct requires running self-log + rehash before
    // `commit_root` in the execute pipeline (tracked follow-up — see
    // docs/design/unified-causal-log.md).
    if !appended.is_empty() {
        let store = context_client.datastore_handle().into_inner();
        let identity = calimero_primitives::identity::PublicKey::from([0u8; 32]);
        let env = calimero_node_primitives::sync::create_runtime_env(&store, context_id, identity);
        calimero_storage::env::with_runtime_env(env, || {
            for entity_id in &appended {
                // `rehash_shared_anchor` mirrors the side-store log into the
                // hashed child (P3) before folding, so the originator's own
                // rotation rides ordinary sync and folds into the anchor's root.
                if let Err(e) = calimero_storage::interface::Interface::<
                    calimero_storage::store::MainStorage,
                >::rehash_shared_anchor(*entity_id)
                {
                    warn!(?e, %context_id, %entity_id,
                        "self-log: rehash_shared_anchor failed — root may lag until next sync");
                }
            }
        });
    }
}

/// Whether the anchor entity `anchor` has synced to this node, by a direct
/// datastore read (no WASM env). True if either its rotation log
/// (`StorageKey::RotationLog`) or its index entry (`StorageKey::Index`, written
/// the moment its `Shared` action applies) exists. Mirrors the two sources
/// `Interface::resolve_anchor_writers` consults — so "present" here means the
/// member's writers WILL resolve at apply time. A presence check only (no
/// decode): we just need to know the anchor arrived.
fn anchor_present_direct(
    context_client: &ContextClient,
    context_id: ContextId,
    anchor: Id,
) -> bool {
    let handle = context_client.datastore_handle();
    for key_bytes in [
        StorageKey::RotationLog(anchor).to_bytes(),
        StorageKey::Index(anchor).to_bytes(),
    ] {
        let state_key = calimero_store::key::ContextState::new(context_id, key_bytes);
        if matches!(handle.get(&state_key), Ok(Some(_))) {
            return true;
        }
    }
    false
}

/// Reverse-BFS reachability over a `delta_id → parents` mirror of the
/// DAG: returns true iff `a` is in the transitive ancestry of `b`. Pure
/// over the snapshot — `happens_before(x, x) == false` (strict ancestry).
///
/// Re-exported under `calimero_node::sync::happens_before_in_topology`
/// for integration tests so they can mirror the production resolve
/// flow without copying the function (#2272 review).
pub fn happens_before_in_topology(
    topology: &IndexMap<[u8; 32], Vec<[u8; 32]>>,
    a: &[u8; 32],
    b: &[u8; 32],
) -> bool {
    if a == b {
        return false;
    }
    let mut frontier: Vec<[u8; 32]> = topology.get(b).cloned().unwrap_or_default();
    let mut seen: HashSet<[u8; 32]> = HashSet::new();
    while let Some(node) = frontier.pop() {
        if !seen.insert(node) {
            continue;
        }
        if &node == a {
            return true;
        }
        if let Some(parents) = topology.get(&node) {
            frontier.extend(parents.iter().copied());
        }
    }
    false
}

/// Node-level delta store that wraps calimero-dag
#[derive(Clone, Debug)]
pub struct DeltaStore {
    /// Core DAG logic (topology, ordering, buffering)
    dag: Arc<RwLock<CoreDagStore<Vec<Action>>>>,

    /// Applier for applying deltas to WASM storage
    applier: Arc<ContextStorageApplier>,

    /// Maps delta_id -> expected_root_hash for deterministic selection
    /// when multiple DAG heads exist (concurrent branches)
    head_root_hashes: Arc<RwLock<HashMap<[u8; 32], [u8; 32]>>>,

    /// Orphaned member deltas (a `SharedMember` whose `anchor` hasn't synced
    /// yet), keyed by the missing anchor id. Re-injected when that anchor
    /// applies. Liveness only — see [`AnchorPendingBuffer`].
    anchor_pending: Arc<RwLock<AnchorPendingBuffer>>,
}

/// Outcome of [`DeltaStore::persist_cascaded_deltas_and_update_heads`].
///
/// `committed` is `true` only if the atomic batch actually landed (or there
/// was nothing to write). Callers that included a freshly-applied `primary`
/// delta MUST check it: `false` means the delta's `applied: true` record
/// never reached the DB, so its event handlers must not run — a restart
/// would re-apply the delta and run them again (a duplicate). Cascade-only
/// callers may ignore it and keep their warn-and-continue behaviour (the
/// next sync corrects the heads).
struct CascadePersistOutcome {
    committed: bool,
    forwarded_events: Vec<([u8; 32], Vec<u8>)>,
}

impl DeltaStore {
    /// Creates a new delta store
    pub fn new(
        root: [u8; 32],
        context_client: ContextClient,
        context_id: ContextId,
        our_identity: PublicKey,
    ) -> Self {
        // Shared parent hash tracking for merge detection
        let parent_hashes = Arc::new(RwLock::new(HashMap::new()));
        // Track deltas that were applied as merges (their children need merge too)
        let merged_deltas = Arc::new(RwLock::new(HashSet::new()));

        let applier = Arc::new(ContextStorageApplier {
            context_client,
            context_id,
            our_identity,
            parent_hashes: Arc::clone(&parent_hashes),
            merged_deltas: Arc::clone(&merged_deltas),
            // #2266: applier-local DAG topology mirror for the
            // rotation-log-driven writer-set resolution. Populated by
            // `apply()` and seeded by `load_persisted_deltas` →
            // `restore_topology` so cross-restart ancestry is preserved.
            // Insertion-ordered (IndexMap) so the eviction at the
            // `MAX_TOPOLOGY_ENTRIES` cap is deterministic (oldest-first).
            topology: Arc::new(RwLock::new(Arc::new(IndexMap::new()))),
            retain_apply_lock: std::sync::atomic::AtomicBool::new(false),
            apply_lock_slot: std::sync::Mutex::new(None),
        });

        Self {
            dag: Arc::new(RwLock::new(CoreDagStore::new(root))),
            applier,
            head_root_hashes: Arc::new(RwLock::new(HashMap::new())),
            anchor_pending: Arc::new(RwLock::new(AnchorPendingBuffer::default())),
        }
    }

    /// Load all persisted deltas from the database into the in-memory DAG
    ///
    /// This restores the DAG state from persistent storage. Should be called after
    /// creating a DeltaStore to prevent nodes from losing DAG history after restart.
    ///
    /// Deltas are loaded in topological order (parents before children) to properly
    /// reconstruct the DAG topology.
    pub async fn load_persisted_deltas(&self) -> Result<LoadPersistedResult> {
        use std::collections::HashMap;

        let handle = self.applier.context_client.datastore_handle();

        // Step 1: Collect deltas for this context from DB.
        //
        // Scoped via prefix seek (keys are `context_id || delta_id`),
        // then streams key+value together via `iter.entries()` so the
        // value decode shares the iterator's block buffer rather than
        // doing an extra point-lookup per row. The seek result itself
        // needs a manual value read since `entries()` advances past it.
        //
        // Event harvesting runs for every applied row to preserve the
        // pre-refactor contract (crash-leftover retry until the caller
        // clears `events` on disk). The `is_applied` short-circuit
        // skips only the expensive work — nested actions decode and
        // HashMap / topology rebuilds.
        let start_key =
            calimero_store::key::ContextDagDelta::new(self.applier.context_id, [0u8; 32]);
        let mut iter = handle.iter::<calimero_store::key::ContextDagDelta>()?;
        let first_key = iter.seek(start_key)?;

        let mut all_deltas: HashMap<[u8; 32], CausalDelta<Vec<Action>>> = HashMap::new();
        let mut pending_handler_events: Vec<([u8; 32], Vec<u8>)> = Vec::new();

        // Process the seek result's (key, value) manually — entries()
        // advances past the cursor's current position. One handle.get
        // for the first row is the only non-buffered value read.
        let first_entry = if let Some(key) = first_key {
            if key.context_id() == self.applier.context_id {
                handle.get(&key)?.map(|v| (key, v))
            } else {
                None
            }
        } else {
            None
        };

        // Combined stream: first (manual) entry + subsequent
        // value-buffered entries from the iterator.
        let mut stream: Box<dyn Iterator<Item = Result<_>>> = match first_entry {
            Some(entry) => Box::new(
                std::iter::once(Ok(entry))
                    .chain(iter.entries().map(|(k, v)| -> Result<_> { Ok((k?, v?)) })),
            ),
            None => Box::new(iter.entries().map(|(k, v)| -> Result<_> { Ok((k?, v?)) })),
        };

        while let Some(entry) = stream.next() {
            let (key, stored_delta) = entry?;

            // Sorted by context_id first — once the prefix changes we're
            // past our context's range and can stop.
            if key.context_id() != self.applier.context_id {
                break;
            }

            let delta_id = key.delta_id();

            // Event harvest runs for every applied row regardless of
            // DAG membership, preserving retry of crash-leftovers
            // until `execute_cascaded_events` clears `events` on disk.
            if stored_delta.applied {
                if let Some(ref events_data) = stored_delta.events {
                    pending_handler_events.push((stored_delta.delta_id, events_data.clone()));
                }
            }

            // Skip the expensive work (actions decode + map inserts)
            // if the DAG already has topology for this delta.
            let already_in_dag = {
                let dag = self.dag.read().await;
                dag.is_applied(&delta_id)
            };
            if already_in_dag {
                continue;
            }

            // Deserialize actions
            let actions: Vec<Action> = match borsh::from_slice(&stored_delta.actions) {
                Ok(actions) => actions,
                Err(e) => {
                    warn!(
                        ?e,
                        context_id = %self.applier.context_id,
                        delta_id = ?stored_delta.delta_id,
                        "Failed to deserialize persisted delta actions, skipping"
                    );
                    continue;
                }
            };

            // P4 backstop: re-self-log this delta's `Shared` rotations on
            // restore. If the live notify (`add_local_applied_delta`) was
            // skipped — e.g. the DeltaStore wasn't up when the delta was created
            // and persisted — the originator's own rotation would otherwise be
            // missing from its log forever (this delta is restored as
            // already-applied, so `add_local_applied_delta` won't re-run for
            // it). Idempotent (dedup on delta_id): a no-op for deltas already
            // logged at creation or by the receive path. Only applied deltas
            // represent rotations that actually took effect.
            if stored_delta.applied {
                self_log_rotations_direct(
                    &self.applier.context_client,
                    self.applier.context_id,
                    stored_delta.delta_id,
                    stored_delta.hlc,
                    &actions,
                );
            }

            // Reconstruct the delta
            // Infer checkpoint status: checkpoints have genesis as parent and empty payload
            let is_checkpoint = stored_delta.parents.len() == 1
                && stored_delta.parents[0] == [0u8; 32]
                && actions.is_empty();

            let dag_delta = CausalDelta {
                id: stored_delta.delta_id,
                parents: stored_delta.parents,
                payload: actions,
                hlc: stored_delta.hlc,
                expected_root_hash: stored_delta.expected_root_hash,
                kind: if is_checkpoint {
                    calimero_dag::DeltaKind::Checkpoint
                } else {
                    calimero_dag::DeltaKind::Regular
                },
            };

            // Store root hash mapping for both head_root_hashes and parent_hashes
            // Note: For persisted deltas that were already applied, we use expected_root_hash
            // as the computed hash (they should be the same for non-merge deltas).
            // For merge deltas, the actual computed hash may have differed, but we don't
            // persist that - this is a minor approximation that works for most cases.
            {
                let mut head_hashes = self.head_root_hashes.write().await;
                let _ = head_hashes.insert(stored_delta.delta_id, stored_delta.expected_root_hash);
            }
            {
                let mut parent_hashes = self.applier.parent_hashes.write().await;
                let _ =
                    parent_hashes.insert(stored_delta.delta_id, stored_delta.expected_root_hash);
            }

            drop(all_deltas.insert(stored_delta.delta_id, dag_delta));
        }

        // Historically this function bailed early when `all_deltas`
        // was empty, skipping the `try_process_pending` call below.
        // That was harmless when every in-context DB row was always
        // collected (pre-#2244), because any non-empty context made
        // `all_deltas` non-empty. After #2244 introduced the
        // skip-if-already-in-DAG path, `all_deltas` is empty whenever
        // a warmed-up node scans a context whose rows are all already
        // in the DAG — i.e. the steady state. The early return then
        // permanently prevents pending deltas (received via gossip
        // before their parents were available) from being retried,
        // which strands late joiners whose seed deltas arrived before
        // their parents (root-hash stuck at the wrong value until
        // restart). Keep going; step 2's while-loop is a no-op on
        // empty input.
        if !all_deltas.is_empty() {
            debug!(
                context_id = %self.applier.context_id,
                total_deltas = all_deltas.len(),
                "Collected persisted deltas, starting topological restore"
            );
        }

        // Step 2: Restore deltas in topological order (parents before children)
        // We keep trying to restore deltas whose parents are already in the DAG
        // NOTE: All persisted deltas are already applied, so we just restore topology
        let mut loaded_count = 0;
        let mut remaining = all_deltas;
        let mut progress_made = true;
        // #2266: collect (delta_id, parents) for the applier-local
        // topology mirror. Persisted deltas bypass `apply()` (they're
        // restored as already-applied), so without this seed the mirror
        // would be empty after restart and `happens_before` would
        // incorrectly return false for any cross-restart ancestry.
        let mut topology_seed: Vec<([u8; 32], Vec<[u8; 32]>)> = Vec::new();

        while progress_made && !remaining.is_empty() {
            progress_made = false;
            let mut to_remove = Vec::new();

            for (delta_id, dag_delta) in &remaining {
                let mut dag = self.dag.write().await;

                // Check if all parents have been applied before restoring
                let can_restore = dag_delta
                    .parents
                    .iter()
                    .all(|p| *p == [0u8; 32] || dag.is_applied(p));

                if can_restore {
                    // Restore topology WITHOUT re-applying (delta was already applied)
                    if dag.restore_applied_delta(dag_delta.clone()) {
                        loaded_count += 1;
                        to_remove.push(*delta_id);
                        topology_seed.push((*delta_id, dag_delta.parents.clone()));
                        progress_made = true;
                    }
                }
            }

            for delta_id in to_remove {
                drop(remaining.remove(&delta_id));
            }
        }

        // Seed the applier topology with everything we just restored.
        if !topology_seed.is_empty() {
            self.applier.restore_topology(topology_seed).await;
        }

        // Count + small bs58 sample rather than full-list Debug —
        // this warn fires every interval sync during mesh bootstrap.
        // Match the bs58 encoding used by delta_id elsewhere so
        // operators can cross-reference sample IDs against other logs.
        if !remaining.is_empty() {
            let sample = remaining
                .keys()
                .take(3)
                .map(|id| Hash::from(*id).to_base58())
                .collect::<Vec<_>>()
                .join(",");

            warn!(
                context_id = %self.applier.context_id,
                remaining_count = remaining.len(),
                loaded_count,
                sample_unloadable = %sample,
                "Some deltas could not be loaded - they will remain pending until parents arrive"
            );

            // These deltas are still persisted and will be in the pending queue
            // They'll be applied when their parents arrive via network sync
        }

        if loaded_count > 0 {
            debug!(
                context_id = %self.applier.context_id,
                loaded_count,
                "Loaded persisted deltas into DAG from database"
            );
        }

        // Try to process any pending deltas that might now have their parents available
        // This handles cases where deltas were received via gossip before their parents
        // were loaded from the database.
        {
            let mut dag = self.dag.write().await;
            match dag.try_process_pending(&*self.applier).await {
                Ok(processed) if processed > 0 => {
                    info!(
                        context_id = %self.applier.context_id,
                        processed,
                        "Processed pending deltas after loading from database"
                    );
                }
                Err(e) => {
                    warn!(
                        ?e,
                        context_id = %self.applier.context_id,
                        "Failed to process pending deltas after database load"
                    );
                }
                _ => {}
            }
        }

        Ok(LoadPersistedResult {
            loaded_count,
            pending_handler_events,
        })
    }

    /// Add a delta with optional event data to the store
    ///
    /// If events are provided and the delta goes pending, events are persisted
    /// so handlers can execute when the delta cascades later.
    ///
    /// Returns applied status and any cascaded events that need handler execution
    pub async fn add_delta_with_events(
        &self,
        delta: CausalDelta<Vec<Action>>,
        events: Option<Vec<u8>>,
        author_id: Option<calimero_primitives::identity::PublicKey>,
        governance_position_blob: Option<Vec<u8>>,
        delta_signature: Option<[u8; 64]>,
    ) -> Result<AddDeltaResult> {
        self.add_delta_internal(
            delta,
            events,
            author_id,
            governance_position_blob,
            delta_signature,
        )
        .await
    }

    /// Add a delta to the store (without event data)
    ///
    /// Returns Ok(true) if applied immediately, Ok(false) if pending
    pub async fn add_delta(
        &self,
        delta: CausalDelta<Vec<Action>>,
        author_id: Option<calimero_primitives::identity::PublicKey>,
        governance_position_blob: Option<Vec<u8>>,
        delta_signature: Option<[u8; 64]>,
    ) -> Result<bool> {
        let result = self
            .add_delta_internal(
                delta,
                None,
                author_id,
                governance_position_blob,
                delta_signature,
            )
            .await?;
        Ok(result.applied)
    }

    /// Register a batch of pre-verified deltas under a *single* DAG write-lock
    /// scope and commit every newly-applied delta together with the post-batch
    /// `dag_heads` as one atomic write.
    ///
    /// This is the bulk counterpart of [`Self::add_delta`] for catchup paths
    /// that receive many deltas at once. Versus calling `add_delta` in a loop
    /// it (a) takes the `dag` write lock once instead of once per delta, and
    /// (b) collapses N per-delta atomic persists + N `dag_heads` writes into a
    /// single atomic batch. Per-delta signature and governance verification is
    /// the caller's job and must already have happened — this method only
    /// registers and persists, exactly like `add_delta`.
    ///
    /// Ordering and cascades match the single-delta path: an input that
    /// arrives before its parent goes pending and is unblocked by a later
    /// input in the same batch (its internal `apply_pending`). The final
    /// applied set is therefore read back from the DAG after the whole batch
    /// settles, not inferred from per-call return values.
    ///
    /// Inputs must not exceed [`DELTA_BATCH_MAX`]; the caller chunks larger
    /// runs to bound the lock-hold window.
    pub async fn add_deltas_batch(&self, inputs: Vec<BatchDeltaInput>) -> Result<BatchAddResult> {
        if inputs.is_empty() {
            return Ok(BatchAddResult::default());
        }
        debug_assert!(
            inputs.len() <= DELTA_BATCH_MAX,
            "add_deltas_batch called with {} inputs (> DELTA_BATCH_MAX {}); caller must chunk",
            inputs.len(),
            DELTA_BATCH_MAX,
        );

        // Phase 0: pre-persist every event-carrying input as `applied: false`
        // BEFORE touching the DAG, so a within-batch cascade can recover those
        // events from the DB during apply (mirrors `add_delta_internal`'s
        // events branch). No-op for the events-less catchup path that is the
        // primary caller today.
        for input in &inputs {
            if input.events.is_some() {
                let mut handle = self.applier.context_client.datastore_handle();
                let serialized_actions = borsh::to_vec(&input.delta.payload)
                    .map_err(|e| eyre::eyre!("Failed to serialize delta actions: {}", e))?;
                handle
                    .put(
                        &calimero_store::key::ContextDagDelta::new(
                            self.applier.context_id,
                            input.delta.id,
                        ),
                        &calimero_store::types::ContextDagDelta {
                            delta_id: input.delta.id,
                            parents: input.delta.parents.clone(),
                            actions: serialized_actions,
                            hlc: input.delta.hlc,
                            applied: false,
                            expected_root_hash: input.delta.expected_root_hash,
                            events: input.events.clone(),
                            author_id: input.author_id,
                            governance_position_blob: input.governance_position_blob.clone(),
                            delta_signature: input.delta_signature,
                        },
                    )
                    .map_err(|e| eyre::eyre!("Failed to pre-persist delta with events: {}", e))?;
            }
        }

        // Phase 1: record head-root-hash mappings for every input, once.
        {
            let mut head_hashes = self.head_root_hashes.write().await;
            for input in &inputs {
                let _ = head_hashes.insert(input.delta.id, input.delta.expected_root_hash);
            }
        }

        // `add_delta` consumes the delta, and we retain `inputs` to build the
        // applied records after the batch settles — so the DAG needs its own
        // copies. Clone them HERE, before taking the write lock, to keep the
        // payload allocations off the lock-hold window the batch exists to
        // minimize. Mirrors the payload/parents clone the single path does.
        let dag_deltas: Vec<CausalDelta<Vec<Action>>> =
            inputs.iter().map(|i| i.delta.clone()).collect();

        // Phase 2: register every input into the DAG under one write lock.
        // `lock_start` is captured AFTER `.write().await` so we measure hold
        // time only, not acquire-wait (same rationale as add_delta_internal).
        let mut dag = self.dag.write().await;
        let lock_start = std::time::Instant::now();

        let pending_before: HashSet<[u8; 32]> = dag.get_pending_delta_ids().into_iter().collect();

        // A per-delta apply error must NOT abort the whole chunk. The deltas
        // that already applied are real and are persisted below — exactly as
        // the single-delta catchup path persisted each success independently.
        // `?`-propagating here instead would strand those already-applied,
        // not-yet-persisted deltas in memory, where `has_delta` would then
        // suppress their re-fetch for the rest of the session. So we log and
        // skip the failing delta (it is re-fetched on the next sync) and keep
        // registering the rest, matching the old per-call skip-and-continue.
        // Hold the per-context execution lock across the whole batch apply AND
        // the `dag_heads` commit below — same race the single-delta path closes
        // (`add_delta_internal`): a hash-neutral writer-set rotation applied
        // here must not leave a window where its storage effect is visible but
        // `dag_heads` still predates it, or a concurrent local write forks the
        // DAG into an unconvergeable delta. The first apply takes
        // `ContextAtomic::Lock`; the rest reuse the guard via
        // `ContextAtomic::Held`. Unlike `add_delta_internal`, the batch path has
        // no orphan-member re-injection, so holding the guard to the commit
        // cannot self-deadlock.
        self.applier
            .retain_apply_lock
            .store(true, std::sync::atomic::Ordering::Release);
        let mut failed_ids: HashSet<[u8; 32]> = HashSet::new();
        for (input, dag_delta) in inputs.iter().zip(dag_deltas) {
            if let Err(e) = dag.add_delta(dag_delta, &*self.applier).await {
                warn!(
                    ?e,
                    context_id = %self.applier.context_id,
                    delta_id = ?input.delta.id,
                    "Skipping delta that failed to apply during batch; will re-fetch on next sync"
                );
                let _ = failed_ids.insert(input.delta.id);
            }
        }
        self.applier
            .retain_apply_lock
            .store(false, std::sync::atomic::Ordering::Release);
        // Take the retained guard (if any apply acquired one) to hold across the
        // `dag_heads` commit below; dropped right after the persist.
        let batch_apply_lock_guard = self
            .applier
            .apply_lock_slot
            .lock()
            .expect("apply_lock_slot poisoned")
            .take();

        let heads = dag.get_heads();
        let heads_count = heads.len();
        let pending_after: HashSet<[u8; 32]> = dag.get_pending_delta_ids().into_iter().collect();

        // Partition inputs by their FINAL state: an input that went pending
        // mid-batch but was unblocked by a later input is `is_applied` now, so
        // querying the settled DAG is the authoritative classification (the
        // per-call return values are stale under within-batch cascades). A
        // delta that errored is neither applied nor pending — it may not be in
        // the DAG at all — so it goes in its own `failed` bucket rather than
        // being miscounted as pending.
        let mut applied_input_ids: Vec<[u8; 32]> = Vec::new();
        let mut pending_input_ids: Vec<[u8; 32]> = Vec::new();
        for input in &inputs {
            let id = input.delta.id;
            if failed_ids.contains(&id) {
                continue;
            }
            if dag.is_applied(&id) {
                applied_input_ids.push(id);
            } else {
                pending_input_ids.push(id);
            }
        }

        // Pre-existing pending deltas (not part of this batch) that the batch
        // unblocked. These go through the `applied_bodies` path, which nulls
        // their author/gov/sig — the same lossy-but-acceptable treatment the
        // single path gives cascaded children (they keep whatever envelope was
        // pre-persisted when they first arrived as pending).
        //
        // Batch inputs are explicitly excluded: a delta can be both
        // pre-existing-pending AND a re-fetched input (a duplicate), in which
        // case it must be persisted once, as an envelope-preserving primary —
        // not also via the envelope-nulling `applied_bodies` path.
        let input_id_set: HashSet<[u8; 32]> = inputs.iter().map(|i| i.delta.id).collect();
        let cascaded_other_ids: Vec<[u8; 32]> = pending_before
            .difference(&pending_after)
            .copied()
            .filter(|id| !input_id_set.contains(id))
            .collect();
        let cascaded_bodies: Vec<([u8; 32], CausalDelta<Vec<Action>>)> = cascaded_other_ids
            .iter()
            .filter_map(|cid| dag.get_delta(cid).map(|d| (*cid, d.clone())))
            .collect();

        let hold = lock_start.elapsed();
        drop(dag);
        self.record_dag_write_lock_hold("add_deltas_batch", hold, None, cascaded_other_ids.len());
        crate::node_metrics::observe_dag_heads_count(heads_count);

        // Prune head-root-hash tracking down to the actual current heads.
        {
            let heads_set: HashSet<[u8; 32]> = heads.iter().copied().collect();
            let mut head_hashes = self.head_root_hashes.write().await;
            head_hashes.retain(|id, _| heads_set.contains(id));
        }

        // Build one fully-formed `applied: true` record per applied input so
        // each keeps its author/gov/sig envelope (so this node can in turn
        // serve verifiable catchup for them). Routed as `primaries`, NOT
        // `applied_bodies`, precisely to preserve that envelope.
        let applied_set: HashSet<[u8; 32]> = applied_input_ids.iter().copied().collect();
        let mut primaries: Vec<(
            calimero_store::key::ContextDagDelta,
            calimero_store::types::ContextDagDelta,
        )> = Vec::with_capacity(applied_set.len());
        for input in &inputs {
            if !applied_set.contains(&input.delta.id) {
                continue;
            }
            let serialized_actions = borsh::to_vec(&input.delta.payload)
                .map_err(|e| eyre::eyre!("Failed to serialize delta actions: {}", e))?;
            primaries.push((
                calimero_store::key::ContextDagDelta::new(self.applier.context_id, input.delta.id),
                calimero_store::types::ContextDagDelta {
                    delta_id: input.delta.id,
                    parents: input.delta.parents.clone(),
                    actions: serialized_actions,
                    hlc: input.delta.hlc,
                    applied: true,
                    expected_root_hash: input.delta.expected_root_hash,
                    events: input.events.clone(),
                    author_id: input.author_id,
                    governance_position_blob: input.governance_position_blob.clone(),
                    delta_signature: input.delta_signature,
                },
            ));
        }

        // Commit applied inputs + unblocked pre-existing pendings + new heads
        // as one atomic batch. Only persist when something actually applied
        // (pending-only batches leave heads unchanged — same gate as the
        // single path).
        //
        // If the commit carried applied inputs but did not land, return `Err`:
        // the durable outcome is that those deltas did not happen (the DAG is
        // ahead of the DB in memory, which a restart reconciles by rebuilding
        // from the DB). The catchup caller (`flush_delta_batch`) treats that
        // `Err` as warn-and-continue and relies on the next sync to re-fetch
        // and re-apply — the same recovery the pre-batch per-delta path used
        // when its `add_delta` returned an error. Cascade-only failures stay
        // best-effort (the next sync corrects the heads).
        let forwarded_events: Vec<([u8; 32], Vec<u8>)> =
            if !primaries.is_empty() || !cascaded_bodies.is_empty() {
                let primaries_present = !primaries.is_empty();
                let outcome = self
                    .persist_cascaded_deltas_and_update_heads(&cascaded_bodies, primaries, heads)
                    .await;
                if primaries_present && !outcome.committed {
                    eyre::bail!("failed to atomically persist applied batch deltas and dag_heads");
                }
                outcome.forwarded_events
            } else {
                Vec::new()
            };

        // Heads are persisted — release the retained context lock so blocked
        // local writes can proceed and observe the committed heads.
        drop(batch_apply_lock_guard);

        // Metrics — keep per-delta granularity so dashboards read the same as
        // the single path. Within-batch-unblocked inputs count as `applied`
        // (their final state); only pre-existing pendings count as `cascaded`;
        // errored inputs count as `failed`, never `pending`.
        for _ in &applied_input_ids {
            crate::node_metrics::record_delta_outcome("applied");
        }
        for _ in &pending_input_ids {
            crate::node_metrics::record_delta_outcome("pending");
        }
        for _ in &failed_ids {
            crate::node_metrics::record_delta_outcome("failed");
        }
        let cascade_size = cascaded_other_ids.len();
        if cascade_size > 0 {
            crate::node_metrics::observe_delta_cascade(cascade_size);
            for _ in 0..cascade_size {
                crate::node_metrics::record_delta_outcome("cascaded");
            }
        }

        Ok(BatchAddResult {
            applied: applied_input_ids,
            pending: pending_input_ids,
            failed: failed_ids.into_iter().collect(),
            forwarded_events,
        })
    }

    /// Incrementally register a locally-generated delta that the execute
    /// path has just persisted to the DB. Updates the in-memory DAG
    /// topology + hash tracking **without** re-applying the delta (the
    /// WASM pass inside `execute.rs` already did that).
    ///
    /// Equivalent to one row of `load_persisted_deltas`'s main loop.
    /// Previously the execute path wrote only to RocksDB and relied on
    /// the next `perform_interval_sync` to rescan and catch up; this
    /// method lets us keep the DAG in sync at write time and drop the
    /// per-sync rescan.
    pub async fn add_local_applied_delta(
        &self,
        delta: CausalDelta<Vec<Action>>,
    ) -> Result<Vec<([u8; 32], Vec<u8>)>> {
        let delta_id = delta.id;
        let expected_root_hash = delta.expected_root_hash;

        // Already known — nothing to do (handles the benign race where
        // the same delta arrives via sync before the local notify lands).
        {
            let dag = self.dag.read().await;
            if dag.is_applied(&delta_id) {
                return Ok(Vec::new());
            }
        }

        // P4: self-log this delta's own `Shared` rotations (the receive path
        // logs received rotations; locally-created deltas need this so the node
        // records its OWN, keeping `writers_at` symmetric across nodes). Runs
        // after the `is_applied` dedup, so a live local delta is logged once;
        // `load_persisted_deltas` re-runs the same idempotent helper on restore,
        // backstopping the case where this live notify was skipped.
        self_log_rotations_direct(
            &self.applier.context_client,
            self.applier.context_id,
            delta_id,
            delta.hlc,
            &delta.payload,
        );

        // Mirror the hash-tracking writes load_persisted_deltas does.
        {
            let mut head_hashes = self.head_root_hashes.write().await;
            let _ = head_hashes.insert(delta_id, expected_root_hash);
        }
        {
            let mut parent_hashes = self.applier.parent_hashes.write().await;
            let _ = parent_hashes.insert(delta_id, expected_root_hash);
        }

        // Register topology, nudge cascades, collect cascaded IDs + heads
        // all under one write-lock scope (matches add_delta_internal).
        let (cascaded_ids, heads) = {
            let mut dag = self.dag.write().await;

            let pending_before: HashSet<[u8; 32]> =
                dag.get_pending_delta_ids().into_iter().collect();

            let added = dag.restore_applied_delta(delta);

            // `restore_applied_delta` does not call `apply_pending` —
            // mirror the explicit nudge documented in `get_missing_parents`
            // (#2238 review). Without this, a sync-received child that
            // went pending because its locally-created parent wasn't yet
            // visible in the DAG stays stranded in the pending queue
            // until restart or an unrelated remote-delta application
            // happens to trigger `apply_pending`.
            let mut cascaded: Vec<[u8; 32]> = Vec::new();
            if added {
                match dag.try_process_pending(&*self.applier).await {
                    Ok(0) => {}
                    Ok(n) => {
                        let pending_after: HashSet<[u8; 32]> =
                            dag.get_pending_delta_ids().into_iter().collect();
                        cascaded = pending_before.difference(&pending_after).copied().collect();
                        tracing::info!(
                            context_id = %self.applier.context_id,
                            cascaded_count = n,
                            "Cascaded pending deltas after registering local applied delta"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            ?e,
                            context_id = %self.applier.context_id,
                            "Failed to process pending deltas after local applied delta"
                        );
                    }
                }
            }
            (cascaded, dag.get_heads())
        };

        // Prune head_root_hashes down to the actual current DAG heads.
        // `add_delta_internal` does the same after its own add (line ~1063);
        // without this mirror, ancestors accumulate here over the lifetime
        // of a DeltaStore and head-state lookups by peers can return a
        // non-head's root hash. Safe to run unconditionally: retain is a
        // no-op when the map is already a subset of `heads`.
        {
            let heads_set: HashSet<[u8; 32]> = heads.iter().copied().collect();
            let mut head_hashes = self.head_root_hashes.write().await;
            head_hashes.retain(|id, _| heads_set.contains(id));
        }

        // Persist cascaded children's DB state + updated dag_heads. Without
        // this, cascaded children that ran through WASM via the applier
        // above would leave the DB record at `applied: false, events: Some(..)`,
        // and `dag_heads` would still reference pre-cascade heads. On restart
        // `load_persisted_deltas` would re-execute WASM for these rows
        // (correctness is preserved — CRDT merge is idempotent — but it's
        // wasted work), and any peer reading `dag_heads` between now and the
        // next sync would see stale heads. See #2248 reviewer comment.
        //
        // Events returned here flow back to the drainer; today they sit in
        // the DB as `applied: true, events: Some(..)` until the next startup
        // `load_persisted_deltas` surfaces them via `pending_handler_events`.
        // Immediate handler dispatch from the drainer would need plumbing to
        // NodeManager / NodeClients and is tracked as a follow-up — the
        // restart-replay path is the existing safety net for cascaded events
        // whose handlers couldn't run synchronously (#2185 contract).
        if cascaded_ids.is_empty() {
            return Ok(Vec::new());
        }

        let cascaded_bodies: Vec<([u8; 32], CausalDelta<Vec<Action>>)> = {
            let dag = self.dag.read().await;
            cascaded_ids
                .iter()
                .filter_map(|cid| dag.get_delta(cid).map(|d| (*cid, d.clone())))
                .collect()
        };

        // Cascade-only path: no `primary`, so ignore `committed` and keep the
        // warn-and-continue behaviour (a failed heads write is corrected by
        // the next sync).
        let cascaded_events = self
            .persist_cascaded_deltas_and_update_heads(&cascaded_bodies, Vec::new(), heads)
            .await
            .forwarded_events;

        Ok(cascaded_events)
    }

    /// Internal add_delta implementation
    /// If this delta writes a `SharedMember` whose anchor has not synced to
    /// this node, return that anchor id (the delta is an "orphan" — its
    /// member's writers can't be resolved yet). Returns `None` when every
    /// member's anchor is present, or is being created by a `Shared` action in
    /// THIS same delta (then anchor + members apply together, so it isn't an
    /// orphan). First missing anchor only: a delta orphaned on several anchors
    /// re-checks the rest after the first one drains.
    fn first_missing_anchor(&self, delta: &CausalDelta<Vec<Action>>) -> Option<Id> {
        // Anchors created/updated in THIS delta.
        let mut shared_here: HashSet<Id> = HashSet::new();
        for action in &delta.payload {
            if let Action::Add { metadata, .. } | Action::Update { metadata, .. } = action {
                if matches!(metadata.storage_type, StorageType::Shared { .. }) {
                    let _inserted = shared_here.insert(action.id());
                }
            }
        }
        for action in &delta.payload {
            let anchor = match action {
                Action::Add { metadata, .. }
                | Action::Update { metadata, .. }
                | Action::DeleteRef { metadata, .. } => match metadata.storage_type {
                    StorageType::SharedMember { anchor, .. } => anchor,
                    _ => continue,
                },
                Action::Compare { .. } => continue,
            };
            if shared_here.contains(&anchor) {
                continue;
            }
            if !anchor_present_direct(
                &self.applier.context_client,
                self.applier.context_id,
                anchor,
            ) {
                return Some(anchor);
            }
        }
        None
    }

    async fn add_delta_internal(
        &self,
        delta: CausalDelta<Vec<Action>>,
        events: Option<Vec<u8>>,
        author_id: Option<calimero_primitives::identity::PublicKey>,
        governance_position_blob: Option<Vec<u8>>,
        delta_signature: Option<[u8; 64]>,
    ) -> Result<AddDeltaResult> {
        // Orphan-member buffering (liveness): if this delta writes a
        // `SharedMember` whose anchor hasn't synced, the member's writers can't
        // be resolved and applying now would fail closed (then drop, awaiting a
        // HashComparison re-fetch). Instead hold it keyed by the missing anchor
        // and re-inject the moment that anchor applies (drain at the end of this
        // fn). Done BEFORE any persist/DAG work so a buffered delta leaves no
        // half-state. Best-effort: if it's already buffered or the buffer is
        // full, `buffer` hands ownership back and we fall through to the normal
        // (fail-closed + re-fetch) path, which still converges.
        let (delta, events, author_id, governance_position_blob, delta_signature) =
            match self.first_missing_anchor(&delta) {
                Some(anchor) => {
                    let input = BatchDeltaInput {
                        delta,
                        events,
                        author_id,
                        governance_position_blob,
                        delta_signature,
                    };
                    match self.anchor_pending.write().await.buffer(anchor, input) {
                        Ok(()) => {
                            debug!(
                                context_id = %self.applier.context_id,
                                %anchor,
                                "Buffered orphan member delta awaiting its anchor"
                            );
                            return Ok(AddDeltaResult {
                                applied: false,
                                cascaded_events: Vec::new(),
                            });
                        }
                        Err(returned) => (
                            returned.delta,
                            returned.events,
                            returned.author_id,
                            returned.governance_position_blob,
                            returned.delta_signature,
                        ),
                    }
                }
                None => (
                    delta,
                    events,
                    author_id,
                    governance_position_blob,
                    delta_signature,
                ),
            };

        let delta_id = delta.id;
        let expected_root_hash = delta.expected_root_hash;
        let parents = delta.parents.clone();
        let actions_for_db = delta.payload.clone();
        let hlc = delta.hlc;

        // Store the mapping before applying
        {
            let mut head_hashes = self.head_root_hashes.write().await;
            let _previous = head_hashes.insert(delta_id, expected_root_hash);
        }

        // CRITICAL: If this delta has events, persist it BEFORE adding to DAG
        // This ensures events are available if the delta cascades during add_delta()
        if events.is_some() {
            let mut handle = self.applier.context_client.datastore_handle();
            let serialized_actions = borsh::to_vec(&actions_for_db)
                .map_err(|e| eyre::eyre!("Failed to serialize delta actions: {}", e))?;

            handle
                .put(
                    &calimero_store::key::ContextDagDelta::new(self.applier.context_id, delta_id),
                    &calimero_store::types::ContextDagDelta {
                        delta_id,
                        parents: parents.clone(),
                        actions: serialized_actions,
                        hlc,
                        applied: false, // Not applied yet, will update if it applies
                        expected_root_hash,
                        events: events.clone(), // Store events for potential cascade
                        author_id,
                        governance_position_blob: governance_position_blob.clone(),
                        delta_signature,
                    },
                )
                .map_err(|e| eyre::eyre!("Failed to pre-persist delta with events: {}", e))?;

            info!(
                context_id = %self.applier.context_id,
                delta_id = ?delta_id,
                "Pre-persisted pending delta WITH events (before DAG add)"
            );
        }

        // Instrument the DAG write-lock scope (#2186). The `add_delta`
        // call below runs WASM `__calimero_sync_next` via the applier,
        // and its internal `apply_pending` may cascade additional
        // pending children — each also running WASM. All of that
        // happens under this single write lock, serializing concurrent
        // callers on the same DeltaStore. Emit the hold time so we
        // can decide from real data whether throttling (issue #2186
        // options 1/2) is needed. Threshold-gated warn for the tail.
        //
        // `lock_start` is captured AFTER `.write().await` so we measure
        // hold time only, not acquire-wait. Under contention the two
        // are distinct signals; conflating them would inflate hold
        // numbers purely because other callers are slow, defeating the
        // measurement (#2196 review).
        let mut dag = self.dag.write().await;
        let lock_start = std::time::Instant::now();

        // Track which deltas are currently pending BEFORE we add the new delta
        // This lets us detect which pending deltas got applied during the cascade
        let pending_before: HashSet<[u8; 32]> = dag.get_pending_delta_ids().into_iter().collect();

        // If parents are missing, `result` will be FALSE, and `dag` internally stores it as
        // pending.
        //
        // Arm lock-retention so the inbound WASM apply inside `dag.add_delta`
        // keeps the per-context execution lock held (its guard lands in
        // `apply_lock_slot`); we capture it into `_apply_lock_guard` below and
        // hold it across the `dag_heads` commit. This closes the window where a
        // concurrent local write observes this delta's storage effect (e.g. a
        // writer-set rotation) but still reads the pre-apply DAG heads —
        // forking the DAG into an unconvergeable delta. The lock order stays
        // `dag` write lock → context lock (the apply acquires the context lock
        // from under this `dag` guard), so no inversion is introduced. Setting
        // the flag here, under the `dag` write lock, serializes it against
        // every other `add_delta_internal`/`try_process_pending` caller.
        self.applier
            .retain_apply_lock
            .store(true, std::sync::atomic::Ordering::Release);
        let add_outcome = dag.add_delta(delta, &*self.applier).await;
        self.applier
            .retain_apply_lock
            .store(false, std::sync::atomic::Ordering::Release);
        // Take the retained guard (if the apply acquired one). Binding it to a
        // local keeps the context lock held across the `dag_heads` commit below
        // and releases it on every exit path — explicitly via `drop` once the
        // heads are persisted, or (on the `?`/`bail!` paths before that) when
        // the local goes out of scope. It MUST be released before the
        // orphan-member re-injection loop further down: that path recurses into
        // `add_delta_internal`, whose own inbound apply takes `ContextAtomic::
        // Lock` and would block forever on the lock we still hold (self-
        // deadlock). Releasing after the persist is correct — the heads are
        // committed by then, which is all a concurrent local write needs to
        // observe. While held, the only work is the direct-datastore head
        // persist (no executor re-entry), so holding it cannot deadlock.
        let apply_lock_guard = self
            .applier
            .apply_lock_slot
            .lock()
            .expect("apply_lock_slot poisoned")
            .take();
        let result = add_outcome?;

        // Update context's dag_heads after the DAG has been updated
        let heads = dag.get_heads();
        let heads_count = heads.len();

        // Get list of deltas that were pending but are now applied (cascade effect)
        let cascaded_deltas: Vec<[u8; 32]> = if !pending_before.is_empty() {
            let pending_after: HashSet<[u8; 32]> =
                dag.get_pending_delta_ids().into_iter().collect();
            pending_before.difference(&pending_after).copied().collect()
        } else {
            Vec::new()
        };

        let hold = lock_start.elapsed();
        drop(dag); // Release lock before calling context_client
        self.record_dag_write_lock_hold("add_delta_internal", hold, None, cascaded_deltas.len());

        // Observe DAG head fan-out for #2356 item 2. The histogram drives
        // the cap-mechanism decision (checkpoint vs periodic consolidation
        // vs no cap) — currently we have no production data for what
        // heads_count actually looks like post-#2238 / #2465.
        //
        // Recorded after `drop(dag)` so the (sub-microsecond) atomic
        // observe doesn't extend the write-lock critical section every
        // caller waits on.
        crate::node_metrics::observe_dag_heads_count(heads_count);

        // Build the primary delta's applied-record so it commits in the SAME
        // atomic batch as any cascaded deltas and the new `dag_heads`, via
        // `persist_cascaded_deltas_and_update_heads` below. A standalone `put`
        // here with the heads written separately afterwards left a crash
        // window where the delta could be on disk as `applied: true` while
        // `dag_heads` still pointed before it (or the reverse) — the same
        // divergence the cascade path was hardened against.
        //
        // Events (when present) ride along in the record and are preserved
        // until the caller confirms handler execution via
        // `mark_events_executed(&delta_id)`; if we crash before that, the
        // next init's `load_persisted_deltas` resurfaces the record through
        // `pending_handler_events` and replays the handler.
        //
        // A pending delta (`!result`) keeps its `applied: false` pre-persisted
        // record (written before the DAG add, if it had events) — nothing to
        // persist here.
        let primary_record: Option<(
            calimero_store::key::ContextDagDelta,
            calimero_store::types::ContextDagDelta,
        )> = if result {
            let serialized_actions = borsh::to_vec(&actions_for_db)
                .map_err(|e| eyre::eyre!("Failed to serialize delta actions: {}", e))?;
            Some((
                calimero_store::key::ContextDagDelta::new(self.applier.context_id, delta_id),
                calimero_store::types::ContextDagDelta {
                    delta_id,
                    parents,
                    actions: serialized_actions,
                    hlc,
                    applied: true,
                    expected_root_hash,
                    events,
                    author_id,
                    governance_position_blob,
                    delta_signature,
                },
            ))
        } else {
            // Pending: already pre-persisted as `applied: false` (with events
            // if any) before the DAG add, so there's nothing to write now.
            None
        };

        // When multiple DAG heads exist, compute the actual root hash from storage.
        // With CRDT merge semantics, the state after applying all deltas is deterministic
        // regardless of application order, so computing from storage gives the correct hash.
        // NOTE: The root hash is already updated during delta application via __calimero_sync_next,
        // so we just log for debugging purposes.
        if heads.len() > 1 {
            debug!(
                context_id = %self.applier.context_id,
                heads_count = heads.len(),
                "Multiple DAG heads detected - state hash determined by CRDT merge semantics"
            );

            // Optionally verify the stored hash matches computed
            // (the hash should already be correct from WASM execution)
            match self
                .applier
                .context_client
                .compute_root_hash(&self.applier.context_id)
            {
                Ok(computed_hash) => {
                    debug!(
                        context_id = %self.applier.context_id,
                        computed_root = ?computed_hash,
                        "Verified root hash from storage after multi-head DAG merge"
                    );
                }
                Err(e) => {
                    warn!(
                        context_id = %self.applier.context_id,
                        error = %e,
                        "Failed to compute root hash for verification (non-fatal)"
                    );
                }
            }
        }

        // Cleanup old head hashes that are no longer active
        {
            let mut head_hashes = self.head_root_hashes.write().await;
            head_hashes.retain(|head_id, _| heads.contains(head_id));
        }

        // Clone any cascaded delta bodies out of the DAG under a short
        // read lock, so the persist helper below can run its DB I/O
        // without holding any DAG lock. Matches the pattern already used
        // in `get_missing_parents` after the #2178 TOCTOU refactor.
        let cascaded_bodies: Vec<([u8; 32], CausalDelta<Vec<Action>>)> =
            if cascaded_deltas.is_empty() {
                Vec::new()
            } else {
                let dag = self.dag.read().await;
                cascaded_deltas
                    .iter()
                    .filter_map(|cid| dag.get_delta(cid).map(|d| (*cid, d.clone())))
                    .collect()
            };

        // Persist the primary delta (if applied) + cascaded deltas +
        // `dag_heads` together via the shared helper, as one atomic batch.
        // Gate the call on "we actually changed the DAG" so we don't write
        // unchanged heads for a delta that went straight to pending without
        // cascading (in which case `primary_record` is `None` too).
        let cascaded_with_events: Vec<([u8; 32], Vec<u8>)> =
            if result || !cascaded_deltas.is_empty() {
                // When this call carries the just-applied primary delta, a failed
                // commit must surface as an error so the caller does NOT run its
                // event handlers: the `applied: true` record never landed, and
                // the atomic batch wrote nothing, so the durable outcome is that
                // this delta did not happen.
                //
                // Post-bail the in-memory DAG is ahead of the DB (the delta is
                // applied in memory, absent on disk, heads stale). That is the
                // intended, recoverable state: a restart rebuilds the DAG from the
                // DB via `load_persisted_deltas`, which won't find this record, so
                // the delta is simply re-delivered by sync and re-applied then —
                // handlers run exactly once, on the durable apply. Running them now
                // would double-run them after that re-apply. Cascade-only commit
                // failures stay best-effort (the next sync corrects the heads).
                let primary_present = primary_record.is_some();
                let outcome = self
                    .persist_cascaded_deltas_and_update_heads(
                        &cascaded_bodies,
                        primary_record.into_iter().collect(),
                        heads,
                    )
                    .await;
                if primary_present && !outcome.committed {
                    eyre::bail!("failed to atomically persist applied delta and dag_heads");
                }
                outcome.forwarded_events
            } else {
                Vec::new()
            };

        // Heads are now persisted, so a concurrent local write that was
        // blocked on the context lock will observe them. Release the retained
        // guard HERE — before the orphan-member re-injection below, which
        // recurses into `add_delta_internal` and would self-deadlock on a lock
        // we still held (its inbound apply takes `ContextAtomic::Lock`).
        drop(apply_lock_guard);

        // Metrics — one increment per add_delta call so dashboards can
        // chart raw apply rate, plus a separate `cascaded` increment per
        // delta unblocked by this call's cascade. Cascade size is also
        // recorded as a histogram so tail values are visible.
        if result {
            crate::node_metrics::record_delta_outcome("applied");
        } else {
            crate::node_metrics::record_delta_outcome("pending");
        }
        let cascade_size = cascaded_deltas.len();
        if cascade_size > 0 {
            crate::node_metrics::observe_delta_cascade(cascade_size);
            for _ in 0..cascade_size {
                crate::node_metrics::record_delta_outcome("cascaded");
            }
        }

        // Drain orphan members whose anchor just applied (liveness). If this
        // delta applied any `Shared` anchor, its writers/rotation-log are now on
        // disk (the WASM apply committed before `dag.add_delta` returned), so
        // members that were buffered awaiting it will now verify — re-inject
        // them. Recursive (a re-injected member could itself unblock a nested
        // anchor); bounded by the buffer size + per-delta dedup. Best-effort:
        // a re-injected delta that still can't apply re-buffers or falls back
        // to the HashComparison re-fetch path. The DAG lock was already
        // released above, so the recursive call re-acquires it cleanly.
        if result {
            let anchors_applied: Vec<Id> = actions_for_db
                .iter()
                .filter_map(|a| match a {
                    Action::Add { metadata, .. } | Action::Update { metadata, .. }
                        if matches!(metadata.storage_type, StorageType::Shared { .. }) =>
                    {
                        Some(a.id())
                    }
                    _ => None,
                })
                .collect();
            if !anchors_applied.is_empty() {
                let to_reinject: Vec<BatchDeltaInput> = {
                    let mut buf = self.anchor_pending.write().await;
                    let mut out = Vec::new();
                    for anchor in anchors_applied {
                        out.append(&mut buf.take_for(&anchor));
                    }
                    out
                };
                for input in to_reinject {
                    debug!(
                        context_id = %self.applier.context_id,
                        delta_id = ?input.delta.id,
                        "Re-injecting buffered orphan member delta (anchor applied)"
                    );
                    if let Err(e) = Box::pin(self.add_delta_internal(
                        input.delta,
                        input.events,
                        input.author_id,
                        input.governance_position_blob,
                        input.delta_signature,
                    ))
                    .await
                    {
                        debug!(
                            context_id = %self.applier.context_id,
                            ?e,
                            "Re-injected orphan member delta failed; relying on re-fetch"
                        );
                    }
                }
            }
        }

        Ok(AddDeltaResult {
            applied: result,
            cascaded_events: cascaded_with_events,
        })
    }

    /// Get missing parent IDs and handle any cascades from DB loads
    ///
    /// This checks both the in-memory DAG and the database to avoid requesting
    /// deltas that are already persisted but not loaded into RAM.
    ///
    /// Returns missing IDs and any cascaded events that need handler execution.
    pub async fn get_missing_parents(&self) -> MissingParentsResult {
        let dag = self.dag.read().await;
        let potentially_missing = dag.get_missing_parents(MAX_DELTA_QUERY_LIMIT);
        drop(dag); // Release lock before DB access

        // Phase 1: classify each potentially-missing parent via a DB lookup.
        // Runs with no DAG lock held. Output feeds the write-lock scope below.
        // See `ParentPlan` above for what each variant represents.
        let handle = self.applier.context_client.datastore_handle();
        let mut actually_missing = Vec::new();
        let mut plans: Vec<ParentPlan> = Vec::new();

        for parent_id in &potentially_missing {
            let db_key =
                calimero_store::key::ContextDagDelta::new(self.applier.context_id, *parent_id);

            match handle.get(&db_key) {
                Ok(Some(stored_delta)) => {
                    tracing::info!(
                        context_id = %self.applier.context_id,
                        parent_id = ?parent_id,
                        "Parent delta found in database - loading into DAG cache"
                    );

                    let actions: Vec<Action> = match borsh::from_slice(&stored_delta.actions) {
                        Ok(actions) => actions,
                        Err(e) => {
                            tracing::warn!(
                                ?e,
                                context_id = %self.applier.context_id,
                                parent_id = ?parent_id,
                                "Failed to deserialize parent delta actions"
                            );
                            actually_missing.push(*parent_id);
                            continue;
                        }
                    };

                    let dag_delta = CausalDelta {
                        id: stored_delta.delta_id,
                        parents: stored_delta.parents,
                        payload: actions,
                        hlc: stored_delta.hlc,
                        expected_root_hash: stored_delta.expected_root_hash,
                        kind: calimero_dag::DeltaKind::Regular,
                    };

                    if stored_delta.applied {
                        tracing::info!(
                            context_id = %self.applier.context_id,
                            parent_id = ?parent_id,
                            "Parent delta already applied in DB - restoring to DAG without re-applying"
                        );
                        plans.push(ParentPlan::Restore {
                            parent_id: *parent_id,
                            dag_delta,
                            expected_root_hash: stored_delta.expected_root_hash,
                        });
                    } else {
                        plans.push(ParentPlan::Add {
                            parent_id: *parent_id,
                            dag_delta,
                        });
                    }
                }
                Ok(None) => {
                    actually_missing.push(*parent_id);
                }
                Err(e) => {
                    tracing::warn!(
                        ?e,
                        context_id = %self.applier.context_id,
                        parent_id = ?parent_id,
                        "Error checking database for parent delta, treating as missing"
                    );
                    actually_missing.push(*parent_id);
                }
            }
        }

        // Phase 2: run every DAG mutation under a *single* write-lock scope
        // and capture `pending_before` inside the same critical section.
        //
        // Prior iterations of this code captured `pending_before` under a
        // separate read lock and then re-acquired a write lock per
        // parent-plan iteration; concurrent callers on the same DeltaStore
        // could interleave between the pending_before snapshot and our
        // later `try_process_pending`, causing their cascades to show up
        // in our diff.
        //
        // Holding the write lock for the whole mutation section serializes
        // us with any concurrent `add_delta_with_events` / `get_missing_parents`
        // call, so the diff reliably reflects only cascades we triggered —
        // either inside the per-plan loop (via `add_delta`'s internal
        // `apply_pending`) or via the explicit `try_process_pending`
        // below.
        //
        // DB I/O is deliberately hoisted out of this scope (phase 1 above,
        // phase 3 below) to keep the lock window short and deterministic.
        let mut all_cascaded_events: Vec<([u8; 32], Vec<u8>)> = Vec::new();
        // Instrument the phase-2 write-lock scope (#2186). Under this
        // single critical section we run `add_delta` (→ WASM) for each
        // `ParentPlan::Add`, `restore_applied_delta` for Restore plans,
        // then a final `try_process_pending` that may cascade a long
        // chain of pending children — each also running WASM. The
        // total work can be unbounded if a restored parent unblocks
        // many pending children. Emit hold time + plans/cascade
        // counts so we can decide from real data whether throttling
        // (issue #2186 options 1/2) is warranted.
        //
        // `hold` is captured inside the `else` branch only (where the
        // lock is actually acquired) and after `.write().await` resolves
        // — so we measure hold time only, not acquire-wait nor the
        // lock-never-taken case (#2196 review).
        let plans_count = plans.len();
        let mut hold: Option<Duration> = None;
        let (
            any_parent_added,
            restored_head_hashes,
            cascaded_ids,
            cascaded_bodies,
            added_parent_bodies,
            heads_after_cascade,
        ) = if plans.is_empty() {
            (
                false,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
        } else {
            let mut dag = self.dag.write().await;
            let lock_start = std::time::Instant::now();
            let pending_before: HashSet<[u8; 32]> =
                dag.get_pending_delta_ids().into_iter().collect();
            let mut any_parent_added = false;
            let mut restored_head_hashes: Vec<([u8; 32], [u8; 32])> = Vec::new();
            // Parents we just transitioned from `applied: false` → applied
            // in the DAG via `add_delta`. Their DB records still say
            // `applied: false`; phase 3 persists the update alongside any
            // cascaded children. Without this, `load_persisted_deltas`
            // re-runs WASM for these parents on every restart (#2187).
            let mut added_parent_bodies: Vec<([u8; 32], CausalDelta<Vec<Action>>)> = Vec::new();

            for plan in plans {
                match plan {
                    ParentPlan::Restore {
                        parent_id,
                        dag_delta,
                        expected_root_hash,
                    } => {
                        if dag.restore_applied_delta(dag_delta) {
                            any_parent_added = true;
                            restored_head_hashes.push((parent_id, expected_root_hash));
                        } else {
                            tracing::debug!(
                                context_id = %self.applier.context_id,
                                parent_id = ?parent_id,
                                "Parent delta already in DAG"
                            );
                        }
                    }
                    ParentPlan::Add {
                        parent_id,
                        dag_delta,
                    } => {
                        // `add_delta` consumes `dag_delta`, so we can't
                        // reuse the value after the call. Instead of
                        // pre-cloning (which pays the payload-sized clone
                        // on every iteration even for Ok(false) / Err
                        // where we don't need the body), we re-fetch
                        // from the DAG after `Ok(true)` — `add_delta`
                        // stores a clone at insert time, so the body is
                        // already there and the lock is still ours.
                        //
                        // `Ok(false)` means either a duplicate (already
                        // in the DAG; nothing changed) OR the delta went
                        // pending because its own ancestors are missing
                        // from the DAG. Persisting `applied: true` in
                        // the latter case would be catastrophic:
                        // `load_persisted_deltas` on restart takes the
                        // `restore_applied_delta` path for `applied=true`
                        // records, which skips WASM — the delta's actions
                        // would never run on this node, causing state
                        // divergence.
                        match dag.add_delta(dag_delta, &*self.applier).await {
                            Ok(true) => {
                                any_parent_added = true;
                                if let Some(body) = dag.get_delta(&parent_id) {
                                    added_parent_bodies.push((parent_id, body.clone()));
                                }
                            }
                            Ok(false) => {
                                tracing::debug!(
                                    context_id = %self.applier.context_id,
                                    parent_id = ?parent_id,
                                    "ParentPlan::Add did not apply (duplicate or went pending); DB record stays applied=false"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    ?e,
                                    context_id = %self.applier.context_id,
                                    parent_id = ?parent_id,
                                    "Failed to load persisted parent delta into DAG"
                                );
                            }
                        }
                    }
                }
            }

            if any_parent_added {
                // `restore_applied_delta` does not call `apply_pending`,
                // so any pending children whose parents we just restored
                // stay stuck without this explicit nudge. Closes the
                // up-to-DEFAULT_SYNC_FREQUENCY_SECS (10 s) stall
                // otherwise incurred before the periodic sync fires.
                match dag.try_process_pending(&*self.applier).await {
                    Ok(0) => {}
                    Ok(n) => {
                        tracing::info!(
                            context_id = %self.applier.context_id,
                            cascaded_count = n,
                            "Cascaded pending deltas after restoring parent from database"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            ?e,
                            context_id = %self.applier.context_id,
                            "try_process_pending failed after parent restore"
                        );
                    }
                }
            }

            let pending_after: HashSet<[u8; 32]> =
                dag.get_pending_delta_ids().into_iter().collect();
            let mut cascaded_ids: Vec<[u8; 32]> =
                pending_before.difference(&pending_after).copied().collect();

            // Clone bodies now so DB I/O in phase 3 runs without the lock.
            let cascaded_bodies: Vec<([u8; 32], CausalDelta<Vec<Action>>)> = cascaded_ids
                .iter()
                .filter_map(|cid| dag.get_delta(cid).map(|d| (*cid, d.clone())))
                .collect();

            // Add-path parents are also newly-applied deltas; include
            // their IDs so `cascaded_events` stays a subset of
            // `cascaded_ids` (documented invariant of
            // `MissingParentsResult`).
            cascaded_ids.extend(added_parent_bodies.iter().map(|(id, _)| *id));

            let heads = dag.get_heads();
            hold = Some(lock_start.elapsed());
            (
                any_parent_added,
                restored_head_hashes,
                cascaded_ids,
                cascaded_bodies,
                added_parent_bodies,
                heads,
            )
        };
        if let Some(hold) = hold {
            self.record_dag_write_lock_hold(
                "get_missing_parents",
                hold,
                Some(plans_count),
                cascaded_ids.len(),
            );
        }

        // Phase 3: post-DAG work — head-root-hash tracking + cascaded delta
        // persistence + dag_heads write. Runs with no DAG lock held.
        if !restored_head_hashes.is_empty() {
            let mut head_hashes = self.head_root_hashes.write().await;
            for (parent_id, expected) in restored_head_hashes {
                head_hashes.insert(parent_id, expected);
            }
        }

        if any_parent_added {
            // Persist cascaded deltas, newly-applied parents (Add path),
            // and push updated `dag_heads` via the shared helper. Helper
            // logs-and-continues on `dag_heads` write failure, so events
            // are preserved even if the heads write fails (next sync will
            // correct the heads).
            //
            // Parents and cascaded children get the same per-entry
            // treatment: read any pre-stored `events` and forward them in
            // the return value, then rewrite the DB record as
            // `applied: true, events: None`. For a `ParentPlan::Add` that
            // had events pre-persisted on this node, this means its
            // handlers run on the next `execute_cascaded_events` call.
            // For one without events, it simply flips `applied: false →
            // true` so restart's `load_persisted_deltas` stops
            // re-running its WASM (#2187).
            let mut bodies_to_persist = added_parent_bodies;
            bodies_to_persist.extend(cascaded_bodies);

            // Cascade-only path (no `primary`): ignore `committed` and keep
            // warn-and-continue — the next sync corrects a failed heads write.
            all_cascaded_events.extend(
                self.persist_cascaded_deltas_and_update_heads(
                    &bodies_to_persist,
                    Vec::new(),
                    heads_after_cascade,
                )
                .await
                .forwarded_events,
            );
        }

        if !actually_missing.is_empty() && actually_missing.len() < potentially_missing.len() {
            tracing::info!(
                context_id = %self.applier.context_id,
                total_checked = potentially_missing.len(),
                in_database = potentially_missing.len() - actually_missing.len(),
                truly_missing = actually_missing.len(),
                cascaded_with_events = all_cascaded_events.len(),
                "Filtered missing parents - some were already in database"
            );
        }

        MissingParentsResult {
            missing_ids: actually_missing,
            cascaded_ids,
            cascaded_events: all_cascaded_events,
        }
    }

    /// Persist newly-applied deltas to the database and push the updated
    /// DAG heads — the two writes that must always land together after
    /// any code path that flips deltas from "not applied on this node"
    /// to applied.
    ///
    /// Used by both `add_delta_internal` and `get_missing_parents`. Before
    /// this helper existed, each site inlined its own copy of the persist
    /// loop and a separate `update_dag_heads` call; the first version of
    /// #2178's cascade fix shipped without the `update_dag_heads` call
    /// because one of the two sites forgot it. Consolidating here removes
    /// that class of bug — neither caller can forget either step.
    ///
    /// # Semantics
    ///
    /// For each delta body passed in (cascaded-from-pending child *or*
    /// parent just applied via `get_missing_parents`'s Add path):
    /// - The in-memory DAG body (in `applied_bodies`) is the authoritative
    ///   source; the DB may or may not already contain a record.
    /// - Any existing DB record's `events` column is read, forwarded in
    ///   the return value (so the caller can run handlers), AND kept in
    ///   the rewritten DB record as `events: Some(..)`. Events are only
    ///   cleared (via `mark_events_executed`) after the caller confirms
    ///   handler execution succeeded — this is the crash-safety contract
    ///   from #2185. Events-less deltas are never pre-persisted, so a
    ///   missing DB record is normal for the cascade path — the helper
    ///   rebuilds the full record from the in-memory body. Add-path
    ///   parents always have a DB record present (phase 1 loaded it).
    ///
    /// After the loop, `update_dag_heads` is called unconditionally with
    /// the supplied `heads`. Pass an empty slice when you only need the
    /// heads write (e.g. a plain `add_delta` with no cascades).
    ///
    /// # Locking
    ///
    /// Holds no DAG lock. The caller is expected to have pre-cloned the
    /// `CausalDelta` bodies out of the DAG under whatever lock the caller
    /// chose. This keeps DB I/O out of the DAG critical section.
    async fn persist_cascaded_deltas_and_update_heads(
        &self,
        applied_bodies: &[([u8; 32], CausalDelta<Vec<Action>>)],
        primaries: Vec<(
            calimero_store::key::ContextDagDelta,
            calimero_store::types::ContextDagDelta,
        )>,
        heads: Vec<[u8; 32]>,
    ) -> CascadePersistOutcome {
        let mut forwarded_events: Vec<([u8; 32], Vec<u8>)> = Vec::new();

        // Build the records to persist first, then commit them and the
        // updated `dag_heads` as one atomic batch below. Writing each delta
        // with its own `put` and then updating `dag_heads` separately left a
        // window where a crash or I/O error midway persisted some cascaded
        // deltas as `applied: true` while `dag_heads` stayed stale — on
        // restart the delta-load path would miss the unpersisted deltas and
        // the in-memory DAG and the DB could diverge permanently.
        //
        // `primaries` are the deltas the caller just applied directly (the
        // `add_delta` Add-path, or one per applied input on the batch path),
        // already fully formed. They ride in the same
        // batch so their `applied: true` record and the heads that now point at
        // it commit together — a standalone `put` for it would reopen the
        // exact tear this batch closes. Its events (if any) are handled by the
        // caller, so it is *not* added to `forwarded_events`.
        let mut records: Vec<(
            calimero_store::key::ContextDagDelta,
            calimero_store::types::ContextDagDelta,
        )> = Vec::with_capacity(applied_bodies.len() + primaries.len());

        if !applied_bodies.is_empty() {
            info!(
                context_id = %self.applier.context_id,
                applied_count = applied_bodies.len(),
                "Persisting newly-applied deltas (cascades and/or Add-path parents)"
            );

            let handle = self.applier.context_client.datastore_handle();
            for (cid, applied_delta) in applied_bodies {
                let db_key =
                    calimero_store::key::ContextDagDelta::new(self.applier.context_id, *cid);

                // Recover any stored events. Absent = delta was never
                // pre-persisted (events-less pending path) = nothing to
                // forward to handlers.
                let stored_events = match handle.get(&db_key) {
                    Ok(Some(stored)) => {
                        debug!(
                            context_id = %self.applier.context_id,
                            delta_id = ?cid,
                            has_events = stored.events.is_some(),
                            "Retrieved stored delta for applied delta"
                        );
                        stored.events
                    }
                    Ok(None) => {
                        debug!(
                            context_id = %self.applier.context_id,
                            delta_id = ?cid,
                            "Applied delta not found in database (was never persisted)"
                        );
                        None
                    }
                    Err(e) => {
                        warn!(
                            ?e,
                            context_id = %self.applier.context_id,
                            delta_id = ?cid,
                            "Failed to query database for applied delta"
                        );
                        None
                    }
                };

                let serialized_actions = match borsh::to_vec(&applied_delta.payload) {
                    Ok(s) => s,
                    Err(e) => {
                        // Aborting the whole batch — not `continue` — is the
                        // point of the atomic path. Skipping just this delta
                        // would still commit `heads` below, advancing them
                        // past a delta we never persisted: exactly the
                        // divergence this code prevents. Bail with no events
                        // forwarded and no heads write; the in-memory DAG
                        // keeps the delta applied and the next sync re-runs
                        // this persist cleanly.
                        warn!(
                            ?e,
                            context_id = %self.applier.context_id,
                            delta_id = ?cid,
                            "Failed to serialize applied delta actions; \
                             aborting persist so dag_heads can't advance past \
                             an unpersisted delta"
                        );
                        return CascadePersistOutcome {
                            committed: false,
                            forwarded_events: Vec::new(),
                        };
                    }
                };

                if let Some(ref events_data) = stored_events {
                    forwarded_events.push((*cid, events_data.clone()));
                }

                // Preserve `events` in the DB until handler execution is
                // confirmed by the caller. If we crash between this write
                // and `execute_cascaded_events` succeeding, the next
                // `load_persisted_deltas` / cascade scan will find
                // `applied: true, events: Some(..)` and replay the handlers.
                // `mark_events_executed` clears the column once they run.
                let record = calimero_store::types::ContextDagDelta {
                    delta_id: *cid,
                    parents: applied_delta.parents.clone(),
                    actions: serialized_actions,
                    hlc: applied_delta.hlc,
                    applied: true,
                    expected_root_hash: applied_delta.expected_root_hash,
                    events: stored_events,
                    author_id: None,
                    governance_position_blob: None,
                    delta_signature: None,
                };
                records.push((db_key, record));
            }
        }

        // Fold the caller's just-applied primary delta(s) into the same batch
        // so their records and the heads land together (see the note above).
        // The single-delta paths pass zero or one primary; the batch path
        // passes one fully-formed `applied: true` record per input that
        // applied immediately, so each keeps its author / governance-position
        // / signature envelope (the `applied_bodies` path below would null
        // those out, which is only acceptable for already-pre-persisted
        // cascaded children).
        records.extend(primaries);

        // Commit the delta records and the post-cascade `dag_heads` as one
        // atomic write so sync handshakes and `broadcast_heartbeat` never
        // observe heads that point past deltas the DB doesn't hold (and vice
        // versa). The failure is logged rather than propagated to match
        // `get_missing_parents`'s warn-and-continue behaviour: on failure
        // nothing is written, so the DB stays at its pre-cascade state and
        // the next sync replays cleanly.
        match self.applier.context_client.persist_deltas_and_dag_heads(
            &self.applier.context_id,
            &records,
            heads.clone(),
        ) {
            Ok(()) => {
                debug!(
                    context_id = %self.applier.context_id,
                    new_heads = ?heads,
                    persisted_deltas = records.len(),
                    "Atomically persisted cascaded deltas and dag_heads"
                );
                CascadePersistOutcome {
                    committed: true,
                    forwarded_events,
                }
            }
            Err(e) => {
                // The commit failed, so none of these records landed. Don't
                // forward their events: running handlers for deltas whose
                // `applied: true` record was never written would break the
                // crash-safety contract (a restart's `load_persisted_deltas`
                // wouldn't find them to replay). The next sync re-applies the
                // deltas and re-forwards from the durable DB records instead.
                // `committed: false` lets an Add-path caller propagate the
                // failure instead of running the primary delta's handlers.
                warn!(
                    ?e,
                    context_id = %self.applier.context_id,
                    "Failed to persist cascaded deltas and dag_heads atomically; \
                     DB left at pre-cascade state, events not forwarded, next \
                     sync will correct it"
                );
                CascadePersistOutcome {
                    committed: false,
                    forwarded_events: Vec::new(),
                }
            }
        }
    }

    /// Mark a delta's events as executed by clearing its `events` column
    /// in the DB. Called by `execute_cascaded_events` after the handler
    /// for this delta runs successfully (#2185).
    ///
    /// Failures are logged, not propagated: if the clear fails, the
    /// worst case is a duplicate handler run on the next restart (the
    /// record still shows `applied: true, events: Some(..)` and the
    /// replay path will pick it up). That's strictly less bad than
    /// losing events, which is the bug #2185 fixes.
    pub fn mark_events_executed(&self, delta_id: &[u8; 32]) {
        let mut handle = self.applier.context_client.datastore_handle();
        let db_key = calimero_store::key::ContextDagDelta::new(self.applier.context_id, *delta_id);

        let stored = match handle.get(&db_key) {
            Ok(Some(record)) => record,
            Ok(None) => {
                // Events-less deltas aren't pre-persisted, so the helper
                // would have rebuilt the record before this point. Absent
                // here means another code path raced us; nothing to do.
                return;
            }
            Err(e) => {
                warn!(
                    ?e,
                    context_id = %self.applier.context_id,
                    delta_id = ?delta_id,
                    "Failed to read DB for events clear; next restart will replay"
                );
                return;
            }
        };

        if stored.events.is_none() {
            return;
        }

        // Safety guard against a stale read (#2194 review): if `applied`
        // is false in the snapshot we just read, something else is
        // mid-write on this record — our `..stored` spread would clobber
        // any concurrent `applied: true` write. Bail out; the stored
        // `events: Some(..)` stays in the DB and the next restart
        // replays via `load_persisted_deltas`. The race is narrow (same
        // delta id being cascaded + handler-executed twice in parallel),
        // but silently downgrading `applied: true → false` would be a
        // correctness bug, not just a lost clear.
        if !stored.applied {
            debug!(
                context_id = %self.applier.context_id,
                delta_id = ?delta_id,
                "mark_events_executed observed applied=false; skipping clear to avoid clobbering concurrent write"
            );
            return;
        }

        let record = calimero_store::types::ContextDagDelta {
            events: None,
            ..stored
        };
        if let Err(e) = handle.put(&db_key, &record) {
            warn!(
                ?e,
                context_id = %self.applier.context_id,
                delta_id = ?delta_id,
                "Failed to clear events after handler execution; next restart will replay"
            );
        }
    }

    /// Emit a structured log for the DAG write-lock hold time at a
    /// given call site (#2186). Used by `add_delta_internal` and
    /// `get_missing_parents` to surface observability data for the
    /// "long-tail WASM-under-write-lock" concern without committing to
    /// the larger throttling refactor.
    ///
    /// Emits at `debug!` so aggregators/dashboards can scrape; bumps
    /// to `warn!` above 500ms so long tails are visible in production
    /// logs without dashboard plumbing. The threshold is intentionally
    /// generous — WASM execution is often 50-200ms on its own, so
    /// anything under 500ms is within-budget.
    ///
    /// `plans_count` is `Some(n)` for `get_missing_parents` (each plan
    /// runs WASM for the `Add` variant) and `None` for
    /// `add_delta_internal` where the notion doesn't apply. The pair
    /// `(plans, cascaded)` is what a future throttling fix would need
    /// to bound, so surfacing both matters for the Add-path call site.
    fn record_dag_write_lock_hold(
        &self,
        site: &'static str,
        hold: Duration,
        plans_count: Option<usize>,
        cascaded_count: usize,
    ) {
        let hold_ms = hold.as_secs_f64() * 1000.0;
        let hold_ms_s = format!("{:.2}", hold_ms);
        let plans = plans_count.unwrap_or(0);
        if hold.as_millis() >= 500 {
            warn!(
                context_id = %self.applier.context_id,
                site = site,
                hold_ms = %hold_ms_s,
                plans_count = plans,
                cascaded_count,
                "DAG write lock held for long tail (#2186)"
            );
        } else {
            debug!(
                context_id = %self.applier.context_id,
                site = site,
                hold_ms = %hold_ms_s,
                plans_count = plans,
                cascaded_count,
                "DAG write lock hold time"
            );
        }
    }

    /// Check if a delta has been applied to the DAG
    pub async fn dag_has_delta_applied(&self, delta_id: &[u8; 32]) -> bool {
        let dag = self.dag.read().await;
        dag.is_applied(delta_id)
    }

    /// Get current DAG heads
    pub async fn get_heads(&self) -> Vec<[u8; 32]> {
        let dag = self.dag.read().await;
        dag.get_heads()
    }

    /// Snapshot of the `head_root_hashes` map keys. Test-only accessor for
    /// asserting that the `add_local_applied_delta` path prunes non-head
    /// ancestors, matching `add_delta_internal`'s `retain(...)` at the end.
    #[cfg(test)]
    pub async fn head_root_hash_ids(&self) -> Vec<[u8; 32]> {
        let head_hashes = self.head_root_hashes.read().await;
        head_hashes.keys().copied().collect()
    }

    /// Cleanup stale pending deltas (timeout eviction)
    pub async fn cleanup_stale(&self, max_age: Duration) -> usize {
        let mut dag = self.dag.write().await;
        dag.cleanup_stale(max_age)
    }

    /// Get statistics for pending deltas
    pub async fn pending_stats(&self) -> PendingStats {
        let dag = self.dag.read().await;
        dag.pending_stats()
    }

    /// Check if we have a specific delta
    pub async fn has_delta(&self, id: &[u8; 32]) -> bool {
        let dag = self.dag.read().await;
        dag.has_delta(id)
    }

    /// Get a specific delta (for sending to peers)
    pub async fn get_delta(&self, id: &[u8; 32]) -> Option<CausalDelta<Vec<Action>>> {
        let dag = self.dag.read().await;
        dag.get_delta(id).cloned()
    }

    /// Add snapshot boundary checkpoints to the DAG.
    ///
    /// After a snapshot sync, we need to inform the DAG about the boundary state
    /// so it knows what "heads" the snapshot represents. This allows subsequent
    /// delta sync to continue from the correct point.
    ///
    /// Creates proper checkpoint deltas (with DeltaKind::Checkpoint) that mark
    /// the snapshot boundary. These checkpoints have empty payloads and are
    /// treated as "already applied" by the DAG.
    ///
    /// CRITICAL: Checkpoints are persisted to the database so that peers can
    /// request them during delta sync. Without persistence, a peer requesting
    /// a delta whose parent is a checkpoint would get "delta not found".
    pub async fn add_snapshot_checkpoints(
        &self,
        boundary_dag_heads: Vec<[u8; 32]>,
        boundary_root_hash: [u8; 32],
    ) -> usize {
        let mut added_count = 0;
        let mut dag = self.dag.write().await;

        // Collect every checkpoint record and persist them in one atomic
        // batch after the loop. Writing each with its own `put` left a window
        // where a crash mid-loop persisted some boundary checkpoints but not
        // others — a peer requesting a delta whose parent is a missing
        // checkpoint then gets "delta not found". One batch is all-or-nothing.
        let mut checkpoint_records: Vec<(
            calimero_store::key::ContextDagDelta,
            calimero_store::types::ContextDagDelta,
        )> = Vec::new();

        for head_id in boundary_dag_heads {
            // Skip genesis (zero hash)
            if head_id == [0; 32] {
                continue;
            }

            // Create a proper checkpoint delta using the architecture-defined constructor
            let checkpoint = CausalDelta::checkpoint(head_id, boundary_root_hash);

            // Serialize BEFORE inserting into the DAG. A checkpoint must be
            // persisted so peers can request it during delta sync (a peer
            // asking for a delta whose parent is this checkpoint would
            // otherwise get "delta not found"). Serializing first means a
            // failure skips both the DAG insertion and the persist, so the
            // in-memory DAG never holds a checkpoint the DB will lack.
            let serialized_actions = match borsh::to_vec(&checkpoint.payload) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        ?e,
                        context_id = %self.applier.context_id,
                        ?head_id,
                        "Failed to serialize checkpoint payload; skipping checkpoint"
                    );
                    continue;
                }
            };

            // Restore the checkpoint to the in-memory DAG (marks it applied).
            // `restore_applied_delta` is idempotent: it returns false when the
            // checkpoint is already present, which `added_count` reflects.
            if dag.restore_applied_delta(checkpoint.clone()) {
                added_count += 1;
            }

            // Stage the checkpoint for persistence UNCONDITIONALLY — not gated
            // on the restore result. If a previous call already put it in the
            // DAG but its persist failed, `restore_applied_delta` now returns
            // false; gating the write on it would strand the DB copy (the DAG
            // has the checkpoint, the DB doesn't, and peers requesting it get
            // "delta not found") until a process restart. Re-staging every
            // time makes the write self-healing across retries; rewriting an
            // already-persisted checkpoint is an idempotent same-bytes put.
            checkpoint_records.push((
                calimero_store::key::ContextDagDelta::new(self.applier.context_id, head_id),
                calimero_store::types::ContextDagDelta {
                    delta_id: head_id,
                    parents: checkpoint.parents.clone(),
                    actions: serialized_actions,
                    hlc: checkpoint.hlc,
                    applied: true, // Checkpoints are always "applied"
                    expected_root_hash: checkpoint.expected_root_hash,
                    events: None,
                    // Snapshot checkpoints are receiver-side derived
                    // (boundary heads from a snapshot transfer), not
                    // peer-authored deltas; no author claim to verify.
                    author_id: None,
                    governance_position_blob: None,
                    delta_signature: None,
                },
            ));
        }

        // Commit all staged checkpoints atomically. Logged, not propagated, to
        // match the rest of this best-effort path: on failure none land and
        // the next snapshot sync re-stages and retries them (records are now
        // staged unconditionally above, so the retry heals a prior failure).
        //
        // This runs while the DAG write lock is still held — deliberately, and
        // not just for parity with the old per-checkpoint `put` loop. The
        // head-hash bookkeeping below tags *every current head* with
        // `boundary_root_hash`, which is only correct while the heads are
        // exactly the checkpoints just restored. Dropping the lock for the
        // persist (as `add_delta_internal` does) would let a concurrent
        // `add_delta` inject a non-boundary head in the gap, which the
        // bookkeeping would then mis-tag with `boundary_root_hash`. So the
        // restore → persist → head-hash → `try_process_pending` span is one
        // critical section. The cost is bounded: this is a single batched
        // `apply` (less in-lock I/O than the prior N puts), on the infrequent
        // snapshot-sync path.
        match self
            .applier
            .context_client
            .persist_delta_records(&checkpoint_records)
        {
            Ok(()) => tracing::info!(
                context_id = %self.applier.context_id,
                count = checkpoint_records.len(),
                "Persisted snapshot checkpoints to DAG and database"
            ),
            Err(e) => tracing::warn!(
                ?e,
                context_id = %self.applier.context_id,
                count = checkpoint_records.len(),
                "Failed to persist snapshot checkpoints to database; \
                 next snapshot sync will re-add them"
            ),
        }

        // Track the expected root hash for merge detection, and process any
        // now-unblocked pending deltas. Both are gated on `added_count > 0`
        // (a checkpoint NEWLY restored this call) rather than on persistence,
        // and that is deliberate:
        //
        // - These reflect in-memory DAG state, so they must run whenever the
        //   DAG actually changed — even if the DB write below failed. Skipping
        //   `try_process_pending` on a persist failure would strand pending
        //   children whose parent checkpoint IS present in memory.
        // - `added_count == 0` means every checkpoint was already in the DAG.
        //   The maps were therefore populated on the earlier call that first
        //   restored them (when `added_count > 0`), so re-running this block is
        //   unnecessary — including in the persist-retry/self-heal case, where
        //   the DB write is redone but the in-memory bookkeeping already holds.
        //   (A restart clears the in-memory DAG, so a post-restart re-add is
        //   `added_count > 0` again and re-populates the maps.)
        if added_count > 0 {
            let mut head_hashes = self.head_root_hashes.write().await;
            for head_id in dag.get_heads().iter() {
                let _previous = head_hashes.insert(*head_id, boundary_root_hash);
            }

            // Also update parent_hashes for merge detection
            let mut parent_hashes = self.applier.parent_hashes.write().await;
            for head_id in dag.get_heads().iter() {
                let _previous = parent_hashes.insert(*head_id, boundary_root_hash);
            }
        }

        // Try to process any pending deltas that might now have their parents available
        // This is critical because deltas received via gossip before the checkpoint was added
        // would be stuck in pending state waiting for the checkpoint parent.
        if added_count > 0 {
            match dag.try_process_pending(&*self.applier).await {
                Ok(processed) if processed > 0 => {
                    tracing::info!(
                        context_id = %self.applier.context_id,
                        processed,
                        "Processed pending deltas after adding snapshot checkpoints"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        ?e,
                        context_id = %self.applier.context_id,
                        "Failed to process pending deltas after checkpoint"
                    );
                }
                _ => {}
            }
        }

        tracing::info!(
            context_id = %self.applier.context_id,
            added_count,
            "Snapshot checkpoints added to DAG"
        );

        added_count
    }

    /// Compact this context's DAG history, bounding on-disk delta growth
    /// (issue #2026).
    ///
    /// When the in-memory DAG holds more than `min_deltas_before_compact`
    /// deltas, history older than the most-recent `retain_recent_count` is
    /// dropped from both the in-memory DAG and the durable delta column. The
    /// retained window keeps cheap incremental delta catch-up working for
    /// peers with small gaps; a peer that needs older history will request a
    /// pruned delta, get "not found", and fall back to HashComparison — which
    /// reconciles current state without the delta log, so the pruned history
    /// is never required for convergence.
    ///
    /// The whole operation runs under the DAG write lock so it serialises
    /// against `get_delta`/`has_delta` (the responder send path) and the apply
    /// path: a peer either sees a delta or sees it gone, never a torn view.
    /// Pruning is skipped while pending deltas exist — a mid-sync DAG whose
    /// heads are about to advance is not a good moment to draw the retain
    /// window.
    ///
    /// The in-memory prune happens first, then the DB delete. Order is not
    /// correctness-critical: a crash between them leaves extra delta rows that
    /// the next sweep re-prunes (and that `load_persisted_deltas` would simply
    /// reload), never lost state — context state lives in the storage tree,
    /// not the delta log.
    ///
    /// Returns the number of deltas pruned (0 when not eligible or skipped).
    pub async fn compact(
        &self,
        min_deltas_before_compact: usize,
        retain_recent_count: usize,
    ) -> usize {
        let mut dag = self.dag.write().await;

        // `delta_count()` is the in-memory DAG size (applied + pending). It is
        // NOT the number of DB rows: after a restart `load_persisted_deltas`
        // is bounded by the in-memory caps (`MAX_TOPOLOGY_ENTRIES`), so a
        // context with far more rows on disk can report a smaller count here
        // and under-prune the DB. Bounding cold/large contexts' on-disk rows
        // is the separate "cold-context compaction" follow-up; this sweep only
        // bounds the live working set.
        let total = dag.delta_count();
        if total <= min_deltas_before_compact {
            return 0;
        }

        // Don't compact mid-catch-up: pending deltas mean heads are still
        // advancing, so the retain window would be drawn against a moving
        // target. `prune_to_recent` already refuses to drop pending deltas,
        // but skipping wholesale here also avoids needless churn. A delta
        // stuck pending blocks its context's compaction until it resolves or
        // the existing stale-pending eviction (PENDING_DELTA_MAX_AGE) clears
        // it — so this never wedges a context permanently.
        let pending = dag.pending_stats().count;
        if pending > 0 {
            debug!(
                context_id = %self.applier.context_id,
                total,
                pending,
                "Skipping DAG compaction: pending deltas present (mid-sync)"
            );
            return 0;
        }

        let pruned_ids = dag.prune_to_recent(retain_recent_count);
        if pruned_ids.is_empty() {
            return 0;
        }

        // Mirror the prune to durable storage. Done while the write lock is
        // still held so the in-memory and on-disk views can't diverge under a
        // concurrent responder read.
        let delta_keys: Vec<calimero_store::key::ContextDagDelta> = pruned_ids
            .iter()
            .map(|id| calimero_store::key::ContextDagDelta::new(self.applier.context_id, *id))
            .collect();

        match self.applier.context_client.prune_delta_records(&delta_keys) {
            Ok(()) => {
                let remaining = dag.delta_count();
                tracing::info!(
                    context_id = %self.applier.context_id,
                    pruned = pruned_ids.len(),
                    remaining,
                    "Compacted DAG history"
                );
            }
            Err(e) => {
                // In-memory is already pruned; the DB still carries the rows.
                // That's the safe direction — the next sweep re-deletes them,
                // and a restart reloads them into the DAG (no lost state). Log
                // and report the in-memory prune count regardless.
                tracing::warn!(
                    ?e,
                    context_id = %self.applier.context_id,
                    pruned = pruned_ids.len(),
                    "DAG compaction pruned in-memory but failed to delete rows; next sweep retries"
                );
            }
        }

        pruned_ids.len()
    }
}

#[cfg(test)]
mod anchor_pending_tests {
    //! Unit tests for the orphan-member buffer in isolation (no DAG / WASM):
    //! dedup, FIFO drain, the global cap, and re-buffer-after-drain.

    use super::*;

    fn anchor(b: u8) -> Id {
        Id::new([b; 32])
    }

    fn input(id_byte: u8) -> BatchDeltaInput {
        BatchDeltaInput {
            delta: CausalDelta {
                id: [id_byte; 32],
                parents: vec![],
                payload: vec![],
                hlc: calimero_storage::logical_clock::HybridTimestamp::default(),
                expected_root_hash: [0u8; 32],
                kind: calimero_dag::DeltaKind::Regular,
            },
            events: None,
            author_id: None,
            governance_position_blob: None,
            delta_signature: None,
        }
    }

    #[test]
    fn buffer_and_drain_fifo() {
        let mut buf = AnchorPendingBuffer::default();
        let a = anchor(0xA0);
        assert!(buf.buffer(a, input(1)).is_ok());
        assert!(buf.buffer(a, input(2)).is_ok());
        assert_eq!(buf.len(), 2);

        let drained = buf.take_for(&a);
        let ids: Vec<u8> = drained.iter().map(|d| d.delta.id[0]).collect();
        assert_eq!(ids, vec![1, 2], "drain must preserve FIFO order");
        assert_eq!(buf.len(), 0, "drain empties the buffer");
        assert!(buf.take_for(&a).is_empty(), "second drain is empty");
    }

    #[test]
    fn dedup_same_delta_id() {
        let mut buf = AnchorPendingBuffer::default();
        let a = anchor(0xA0);
        assert!(buf.buffer(a, input(1)).is_ok());
        // Same delta id under the same anchor — rejected (handed back).
        assert!(buf.buffer(a, input(1)).is_err());
        // Same delta id under a DIFFERENT anchor — still rejected (global dedup).
        assert!(buf.buffer(anchor(0xB0), input(1)).is_err());
        assert_eq!(buf.len(), 1);
    }

    // Distinct delta ids via the first two bytes (a single byte only gives 256
    // values — exactly the global cap — leaving no room for an "extra").
    fn id_for(n: usize) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[0] = u8::try_from(n & 0xff).unwrap();
        b[1] = u8::try_from((n >> 8) & 0xff).unwrap();
        b
    }

    #[test]
    fn rejects_when_global_cap_reached_then_accepts_after_drain() {
        // Spread across many anchors (each under the per-anchor cap) to reach
        // the GLOBAL cap without tripping the per-anchor limit.
        let mut buf = AnchorPendingBuffer::default();
        for n in 0..MAX_ANCHOR_PENDING {
            let mut inp = input(0);
            inp.delta.id = id_for(n);
            let a = anchor(u8::try_from(n / MAX_PENDING_PER_ANCHOR).unwrap());
            assert!(buf.buffer(a, inp).is_ok());
        }
        assert_eq!(buf.len(), MAX_ANCHOR_PENDING);
        // Global cap reached: a further distinct delta (fresh anchor) is handed
        // back so the caller falls through to fail-closed + re-fetch.
        let mut extra = input(0);
        extra.delta.id = id_for(MAX_ANCHOR_PENDING);
        assert!(buf.buffer(anchor(0xFE), extra).is_err());
        // Draining one anchor frees its slots; a new delta is accepted again.
        let _ = buf.take_for(&anchor(0));
        let mut extra2 = input(0);
        extra2.delta.id = id_for(MAX_ANCHOR_PENDING);
        assert!(buf.buffer(anchor(0xFE), extra2).is_ok());
    }

    #[test]
    fn per_anchor_cap_limits_one_anchor_without_starving_others() {
        let mut buf = AnchorPendingBuffer::default();
        let hot = anchor(0xA0);
        for n in 0..MAX_PENDING_PER_ANCHOR {
            let mut inp = input(0);
            inp.delta.id = id_for(n);
            assert!(buf.buffer(hot, inp).is_ok());
        }
        // One more under the SAME anchor is rejected (per-anchor cap)...
        let mut over = input(0);
        over.delta.id = id_for(MAX_PENDING_PER_ANCHOR);
        assert!(buf.buffer(hot, over).is_err());
        // ...but a different anchor is unaffected (no global starvation).
        let mut other = input(0);
        other.delta.id = id_for(MAX_PENDING_PER_ANCHOR + 1);
        assert!(buf.buffer(anchor(0xB0), other).is_ok());
    }

    #[test]
    fn drain_removes_from_dedup_set() {
        let mut buf = AnchorPendingBuffer::default();
        let a = anchor(0xA0);
        assert!(buf.buffer(a, input(1)).is_ok());
        let _ = buf.take_for(&a);
        // After draining, the same delta id can be buffered again (e.g. a
        // re-injected delta that orphaned on a different anchor).
        assert!(buf.buffer(anchor(0xB0), input(1)).is_ok());
        assert_eq!(buf.len(), 1);
    }
}

#[cfg(test)]
mod happens_before_tests {
    //! Direct unit tests for the topology-snapshot reachability primitive
    //! that drives Shared writer-set resolution at apply time. Covered
    //! cases mirror the ADR 0001 examples and the bounded-cache reasoning
    //! in `apply()`.

    use super::*;

    fn id(b: u8) -> [u8; 32] {
        [b; 32]
    }

    fn topology(edges: &[(u8, &[u8])]) -> IndexMap<[u8; 32], Vec<[u8; 32]>> {
        edges
            .iter()
            .map(|(child, parents)| (id(*child), parents.iter().copied().map(id).collect()))
            .collect()
    }

    #[test]
    fn self_is_not_strict_ancestor() {
        let t = topology(&[(2, &[1])]);
        assert!(!happens_before_in_topology(&t, &id(1), &id(1)));
        assert!(!happens_before_in_topology(&t, &id(2), &id(2)));
    }

    #[test]
    fn single_hop_ancestry() {
        // 1 → 2
        let t = topology(&[(2, &[1])]);
        assert!(happens_before_in_topology(&t, &id(1), &id(2)));
        assert!(!happens_before_in_topology(&t, &id(2), &id(1)));
    }

    #[test]
    fn transitive_ancestry() {
        // 1 → 2 → 3 → 4
        let t = topology(&[(2, &[1]), (3, &[2]), (4, &[3])]);
        assert!(happens_before_in_topology(&t, &id(1), &id(4)));
        assert!(happens_before_in_topology(&t, &id(2), &id(4)));
        assert!(!happens_before_in_topology(&t, &id(4), &id(1)));
    }

    #[test]
    fn diamond_merge() {
        //   1
        //  / \
        // 2   3
        //  \ /
        //   4
        let t = topology(&[(2, &[1]), (3, &[1]), (4, &[2, 3])]);
        assert!(happens_before_in_topology(&t, &id(1), &id(4)));
        assert!(happens_before_in_topology(&t, &id(2), &id(4)));
        assert!(happens_before_in_topology(&t, &id(3), &id(4)));
        // Siblings 2 and 3 are concurrent — neither precedes the other.
        assert!(!happens_before_in_topology(&t, &id(2), &id(3)));
        assert!(!happens_before_in_topology(&t, &id(3), &id(2)));
    }

    #[test]
    fn unknown_node_returns_false() {
        // Querying a node the topology has never seen must be a clean
        // false, not a panic — happens during sync when peers reference
        // deltas we haven't received yet.
        let t = topology(&[(2, &[1])]);
        assert!(!happens_before_in_topology(&t, &id(1), &id(99)));
        assert!(!happens_before_in_topology(&t, &id(99), &id(2)));
    }

    #[test]
    fn missing_parent_terminates() {
        // Topology references a parent it doesn't list (42 has no own
        // entry). BFS must terminate cleanly at that leaf — and report
        // 42 as a direct ancestor of 2, since 2's parent list contains
        // it. Anything *behind* 42 is unknown and reports false.
        let t = topology(&[(2, &[42])]);
        assert!(happens_before_in_topology(&t, &id(42), &id(2)));
        assert!(!happens_before_in_topology(&t, &id(1), &id(2)));
        assert!(!happens_before_in_topology(&t, &id(99), &id(42)));
    }

    #[test]
    fn cycle_is_resilient() {
        // Causal DAGs cannot have cycles, but the BFS must still
        // terminate if a malformed mirror ever produced one — the
        // `seen` set is the load-bearing guard.
        let t = topology(&[(1, &[2]), (2, &[1])]);
        // The query terminates rather than spinning; ancestry result
        // for cyclic input is best-effort.
        let _ = happens_before_in_topology(&t, &id(1), &id(2));
        let _ = happens_before_in_topology(&t, &id(2), &id(1));
    }
}

#[cfg(test)]
mod seed_topology_tests {
    //! `seed_topology` is the cross-restart correctness hinge: without
    //! the seeding step in `load_persisted_deltas`, the topology mirror
    //! is empty after restart and `happens_before` returns false for all
    //! ancestry that pre-dates the restart.

    use super::*;

    fn id(b: u8) -> [u8; 32] {
        [b; 32]
    }

    #[test]
    fn seeded_chain_is_visible_to_happens_before() {
        // Pretend we restored a 1 → 2 → 3 chain from disk.
        let mut topology = IndexMap::new();
        seed_topology(
            &mut topology,
            vec![(id(2), vec![id(1)]), (id(3), vec![id(2)])],
        );

        assert!(happens_before_in_topology(&topology, &id(1), &id(3)));
        assert!(happens_before_in_topology(&topology, &id(2), &id(3)));
        assert!(!happens_before_in_topology(&topology, &id(3), &id(1)));
    }

    #[test]
    fn seed_overwrites_existing_entries() {
        // If the same id appears twice (shouldn't, but defensive), the
        // later one wins — same semantic as the `apply()` path's
        // `topology.insert(...)`.
        let mut topology = IndexMap::new();
        seed_topology(&mut topology, vec![(id(2), vec![id(1)])]);
        seed_topology(&mut topology, vec![(id(2), vec![id(7)])]);

        assert_eq!(topology.get(&id(2)), Some(&vec![id(7)]));
    }

    #[test]
    fn seed_with_empty_iter_is_noop() {
        let mut topology = IndexMap::new();
        let _ = topology.insert(id(2), vec![id(1)]);
        seed_topology(&mut topology, std::iter::empty());
        assert_eq!(topology.len(), 1);
        assert_eq!(topology.get(&id(2)), Some(&vec![id(1)]));
    }

    #[test]
    fn cap_topology_evicts_oldest_first() {
        // Insert MAX + extra entries; cap to 90% of MAX. The oldest
        // inserts must be the ones evicted; the most recent must
        // survive — that's the load-bearing security property
        // (see comment on `topology` field).
        let mut topology: IndexMap<[u8; 32], Vec<[u8; 32]>> = IndexMap::new();
        let total = MAX_TOPOLOGY_ENTRIES + 50;
        for i in 0..total {
            let key = u32::try_from(i).unwrap().to_le_bytes();
            let mut k32 = [0_u8; 32];
            k32[..4].copy_from_slice(&key);
            let _ = topology.insert(k32, vec![]);
        }
        cap_topology(&mut topology);

        let target = MAX_TOPOLOGY_ENTRIES * 9 / 10;
        assert_eq!(topology.len(), target);

        // The first `total - target` inserts must be gone; the rest
        // must be present (insertion order, deterministic).
        let evicted_count = total - target;
        for i in 0..evicted_count {
            let mut k32 = [0_u8; 32];
            k32[..4].copy_from_slice(&u32::try_from(i).unwrap().to_le_bytes());
            assert!(
                !topology.contains_key(&k32),
                "expected oldest entry {i} to be evicted"
            );
        }
        for i in evicted_count..total {
            let mut k32 = [0_u8; 32];
            k32[..4].copy_from_slice(&u32::try_from(i).unwrap().to_le_bytes());
            assert!(
                topology.contains_key(&k32),
                "expected recent entry {i} to survive"
            );
        }
    }

    /// Stress test for the eviction's asymptotic cost.
    ///
    /// `restore_topology` may seed up to N entries from disk in one shot.
    /// A naive `loop { shift_remove_index(0) }` would be O(excess × n)
    /// — at the values exercised here (n=10× cap, excess=91% of n) that
    /// reduces to ~9 billion shifts and a multi-second startup stall
    /// under the topology write lock. The `drain(0..excess)` form is
    /// O(n). This test runs in well under a second; if a future change
    /// reverts to per-element eviction, the timeout makes it loud.
    /// Cf. PR #2272 review on quadratic eviction cost.
    #[test]
    fn cap_topology_evicts_in_linear_time() {
        let mut topology: IndexMap<[u8; 32], Vec<[u8; 32]>> = IndexMap::new();
        let total = MAX_TOPOLOGY_ENTRIES * 10;
        for i in 0..total {
            let mut k32 = [0_u8; 32];
            k32[..8].copy_from_slice(&u64::try_from(i).unwrap().to_le_bytes());
            let _ = topology.insert(k32, vec![]);
        }

        let start = std::time::Instant::now();
        cap_topology(&mut topology);
        let elapsed = start.elapsed();

        let target = MAX_TOPOLOGY_ENTRIES * 9 / 10;
        assert_eq!(topology.len(), target);
        // Generous bound — this should take milliseconds, not seconds.
        // A regression to O(excess × n) at this size would take many
        // seconds and trip this assertion.
        assert!(
            elapsed.as_secs() < 2,
            "cap_topology({total} entries) took {elapsed:?} — likely a \
             regression to O(excess × n) eviction"
        );
    }

    #[test]
    fn cap_topology_under_cap_is_noop() {
        let mut topology: IndexMap<[u8; 32], Vec<[u8; 32]>> = IndexMap::new();
        for i in 0..100_u8 {
            let _ = topology.insert([i; 32], vec![]);
        }
        cap_topology(&mut topology);
        assert_eq!(topology.len(), 100);
    }
}
