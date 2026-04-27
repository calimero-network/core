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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use calimero_context_client::client::ContextClient;
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
use calimero_storage::entities::StorageType;
use calimero_storage::store::MainStorage;
use eyre::Result;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::sync::rotation_log_reader;

/// Result of adding a delta with cascaded event information
#[derive(Debug)]
pub struct AddDeltaResult {
    /// Whether the delta was applied immediately (true) or went pending (false)
    pub applied: bool,
    /// List of (delta_id, events_data) for cascaded deltas that have event handlers to execute
    pub cascaded_events: Vec<([u8; 32], Vec<u8>)>,
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
    /// (#2266)
    topology: Arc<RwLock<HashMap<[u8; 32], Vec<[u8; 32]>>>>,
    /// Cache for resolved writer sets keyed on `(entity_id, delta_id)`.
    /// Each entry is the answer to "what was the writer set for this
    /// Shared entity as of this delta's causal frontier?" Cache hits
    /// dominate when chains of deltas all reference the same entity
    /// against the same parent set. (#2266)
    effective_writers_cache: Arc<RwLock<HashMap<(Id, [u8; 32]), BTreeSet<PublicKey>>>>,
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

        // Execute __calimero_sync_next via WASM to apply actions to storage
        let outcome = self
            .context_client
            .execute(
                &self.context_id,
                &self.our_identity,
                "__calimero_sync_next".to_owned(),
                artifact,
                vec![],
                None,
            )
            .await
            .map_err(|e| ApplyError::Application(format!("WASM execution failed: {e}")))?;

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
            let _previous = topology.insert(delta.id, delta.parents.clone());

            const MAX_TOPOLOGY_ENTRIES: usize = 10_000;
            if topology.len() > MAX_TOPOLOGY_ENTRIES {
                let excess = topology.len() - (MAX_TOPOLOGY_ENTRIES * 9 / 10);
                let keys_to_remove: Vec<_> = topology.keys().take(excess).copied().collect();
                for key in keys_to_remove {
                    let _removed = topology.remove(&key);
                }
            }
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

        // CRITICAL FIX: Even if all parent checks passed, we STILL might need merge.
        // The issue is: we don't know if the remote node applied our parent as a merge.
        // Their child delta's actions are based on THEIR computed state, not ours.
        //
        // CONSERVATIVE FINAL CHECK: Always use merge semantics. CRDT merge is idempotent,
        // so if states were identical, we get the same result. If not, we merge correctly.
        //
        // This is the safest approach and ensures eventual consistency.
        debug!(
            context_id = %self.context_id,
            delta_id = ?delta.id,
            current_root_hash = ?Hash::from(*current_root_hash),
            delta_expected_hash = ?Hash::from(delta.expected_root_hash),
            "Using CRDT merge semantics for all remote deltas (conservative)"
        );
        true
    }

    /// Resolve the writer set for every Shared entity touched by `delta`.
    ///
    /// Iterates the action payload, picks out Shared `Add`/`Update`/
    /// `DeleteRef`s, dedups by entity, and resolves each via
    /// [`rotation_log_reader::writers_at`] against this applier's
    /// `topology` view of the DAG. Cache-keyed on `(entity_id, delta_id)`.
    ///
    /// Returns a map keyed by entity id; non-Shared entities are absent.
    /// An empty result is normal — a delta with only User/Frozen/Public
    /// actions has nothing to resolve.
    async fn resolve_effective_writers_for_delta(
        &self,
        delta: &CausalDelta<Vec<Action>>,
    ) -> Result<BTreeMap<Id, BTreeSet<PublicKey>>> {
        let mut shared_entities: BTreeSet<Id> = BTreeSet::new();
        for action in &delta.payload {
            let metadata = match action {
                Action::Add { metadata, .. }
                | Action::Update { metadata, .. }
                | Action::DeleteRef { metadata, .. } => metadata,
                Action::Compare { .. } => continue,
            };
            if matches!(metadata.storage_type, StorageType::Shared { .. }) {
                let _inserted = shared_entities.insert(action.id());
            }
        }

        let mut out: BTreeMap<Id, BTreeSet<PublicKey>> = BTreeMap::new();
        if shared_entities.is_empty() {
            return Ok(out);
        }

        // Snapshot topology once per delta apply. The `happens_before`
        // closure consults this snapshot for every reachability test
        // inside `writers_at`, avoiding repeated lock acquisitions.
        let topology_snapshot = self.topology.read().await.clone();

        for entity_id in shared_entities {
            let cache_key = (entity_id, delta.id);
            if let Some(cached) = self.effective_writers_cache.read().await.get(&cache_key) {
                let _replaced = out.insert(entity_id, cached.clone());
                continue;
            }

            let log = match calimero_storage::rotation_log::load::<MainStorage>(entity_id) {
                Ok(Some(log)) => log,
                Ok(None) => continue, // No log → verifier falls back to v2 stored-writers.
                Err(e) => {
                    return Err(eyre::eyre!(
                        "rotation_log::load for entity {entity_id:?} failed: {e}"
                    ))
                }
            };

            let resolved = rotation_log_reader::writers_at(&log, &delta.parents, |a, b| {
                happens_before_in_topology(&topology_snapshot, a, b)
            });

            if let Some(set) = resolved {
                let _replaced = out.insert(entity_id, set.clone());

                let mut cache = self.effective_writers_cache.write().await;
                let _previous = cache.insert(cache_key, set);

                // Bound the cache the same way as parent_hashes/topology
                // to keep long-lived nodes from growing unboundedly.
                const MAX_EFFECTIVE_WRITERS_ENTRIES: usize = 10_000;
                if cache.len() > MAX_EFFECTIVE_WRITERS_ENTRIES {
                    let excess = cache.len() - (MAX_EFFECTIVE_WRITERS_ENTRIES * 9 / 10);
                    let keys_to_remove: Vec<_> = cache.keys().take(excess).copied().collect();
                    for key in keys_to_remove {
                        let _removed = cache.remove(&key);
                    }
                }
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
        for (delta_id, parents) in deltas {
            let _previous = topology.insert(delta_id, parents);
        }
    }
}

/// Reverse-BFS reachability over a `delta_id → parents` mirror of the
/// DAG: returns true iff `a` is in the transitive ancestry of `b`. Pure
/// over the snapshot — `happens_before(x, x) == false` (strict ancestry).
fn happens_before_in_topology(
    topology: &HashMap<[u8; 32], Vec<[u8; 32]>>,
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
            // #2266: applier-local DAG topology + per-(entity,delta) cache
            // for the rotation-log-driven writer-set resolution. Populated
            // by `apply()` and seeded by `load_persisted_deltas` →
            // `restore_topology` so cross-restart ancestry is preserved.
            topology: Arc::new(RwLock::new(HashMap::new())),
            effective_writers_cache: Arc::new(RwLock::new(HashMap::new())),
        });

        Self {
            dag: Arc::new(RwLock::new(CoreDagStore::new(root))),
            applier,
            head_root_hashes: Arc::new(RwLock::new(HashMap::new())),
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
    ) -> Result<AddDeltaResult> {
        self.add_delta_internal(delta, events).await
    }

    /// Add a delta to the store (without event data)
    ///
    /// Returns Ok(true) if applied immediately, Ok(false) if pending
    pub async fn add_delta(&self, delta: CausalDelta<Vec<Action>>) -> Result<bool> {
        let result = self.add_delta_internal(delta, None).await?;
        Ok(result.applied)
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

        let cascaded_events = self
            .persist_cascaded_deltas_and_update_heads(&cascaded_bodies, heads)
            .await;

        Ok(cascaded_events)
    }

    /// Internal add_delta implementation
    async fn add_delta_internal(
        &self,
        delta: CausalDelta<Vec<Action>>,
        events: Option<Vec<u8>>,
    ) -> Result<AddDeltaResult> {
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
        let result = dag.add_delta(delta, &*self.applier).await?;

        // Update context's dag_heads after the DAG has been updated
        let heads = dag.get_heads();

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

        // Update persistence if delta applied. Preserve events until
        // the caller confirms handler execution via
        // `mark_events_executed(&delta_id)` — same crash-safety contract
        // as the cascade path (#2185, #2194 review). If we crash between
        // this write and the caller's `execute_event_handlers_parsed`
        // success, the next init's `load_persisted_deltas` surfaces the
        // record via `pending_handler_events` and replays the handler.
        if result && events.is_some() {
            let mut handle = self.applier.context_client.datastore_handle();
            let serialized_actions = borsh::to_vec(&actions_for_db)
                .map_err(|e| eyre::eyre!("Failed to serialize delta actions: {}", e))?;

            handle
                .put(
                    &calimero_store::key::ContextDagDelta::new(self.applier.context_id, delta_id),
                    &calimero_store::types::ContextDagDelta {
                        delta_id,
                        parents,
                        actions: serialized_actions,
                        hlc,
                        applied: true,
                        expected_root_hash,
                        events: events.clone(),
                    },
                )
                .map_err(|e| eyre::eyre!("Failed to update applied delta: {}", e))?;

            debug!(
                context_id = %self.applier.context_id,
                delta_id = ?delta_id,
                "Updated pre-persisted delta as applied (events preserved until handler success)"
            );
        } else if result {
            // Delta applied and had no events - just persist normally
            let mut handle = self.applier.context_client.datastore_handle();
            let serialized_actions = borsh::to_vec(&actions_for_db)
                .map_err(|e| eyre::eyre!("Failed to serialize delta actions: {}", e))?;

            handle
                .put(
                    &calimero_store::key::ContextDagDelta::new(self.applier.context_id, delta_id),
                    &calimero_store::types::ContextDagDelta {
                        delta_id,
                        parents,
                        actions: serialized_actions,
                        hlc,
                        applied: true,
                        expected_root_hash,
                        events: None,
                    },
                )
                .map_err(|e| eyre::eyre!("Failed to persist applied delta: {}", e))?;

            debug!(
                context_id = %self.applier.context_id,
                delta_id = ?delta_id,
                "Persisted applied delta to database"
            );
        }
        // If !result, delta is pending and was already pre-persisted with events (if any)

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

        // Persist cascaded deltas + `dag_heads` together via the shared
        // helper. Gate the call on "we actually changed the DAG" so we
        // don't write unchanged heads for a delta that went straight to
        // pending without cascading.
        let cascaded_with_events: Vec<([u8; 32], Vec<u8>)> =
            if result || !cascaded_deltas.is_empty() {
                self.persist_cascaded_deltas_and_update_heads(&cascaded_bodies, heads)
                    .await
            } else {
                Vec::new()
            };

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

            all_cascaded_events.extend(
                self.persist_cascaded_deltas_and_update_heads(
                    &bodies_to_persist,
                    heads_after_cascade,
                )
                .await,
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
        heads: Vec<[u8; 32]>,
    ) -> Vec<([u8; 32], Vec<u8>)> {
        let mut forwarded_events: Vec<([u8; 32], Vec<u8>)> = Vec::new();

        if !applied_bodies.is_empty() {
            info!(
                context_id = %self.applier.context_id,
                applied_count = applied_bodies.len(),
                "Persisting newly-applied deltas (cascades and/or Add-path parents)"
            );

            let mut handle = self.applier.context_client.datastore_handle();
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
                        warn!(
                            ?e,
                            context_id = %self.applier.context_id,
                            delta_id = ?cid,
                            "Failed to serialize applied delta actions, skipping persistence"
                        );
                        continue;
                    }
                };

                if let Some(ref events_data) = stored_events {
                    forwarded_events.push((*cid, events_data.clone()));
                }

                // Preserve `events` in the DB until handler execution is
                // confirmed by the caller (#2185). If we crash between
                // this write and `execute_cascaded_events` succeeding,
                // the next `load_persisted_deltas` / cascade scan will
                // find `applied: true, events: Some(..)` and replay the
                // handlers. `mark_events_executed` clears the column
                // once handlers have run.
                let record = calimero_store::types::ContextDagDelta {
                    delta_id: *cid,
                    parents: applied_delta.parents.clone(),
                    actions: serialized_actions,
                    hlc: applied_delta.hlc,
                    applied: true,
                    expected_root_hash: applied_delta.expected_root_hash,
                    events: stored_events.clone(),
                };
                if let Err(e) = handle.put(&db_key, &record) {
                    warn!(
                        ?e,
                        context_id = %self.applier.context_id,
                        delta_id = ?cid,
                        "Failed to persist applied delta to database"
                    );
                } else if stored_events.is_some() {
                    info!(
                        context_id = %self.applier.context_id,
                        delta_id = ?cid,
                        "Persisted applied delta - has events for handler execution"
                    );
                }
            }
        }

        // Update the database's `dag_heads` so sync handshakes and
        // `broadcast_heartbeat` see the post-cascade state. Failing to
        // do this was the original bug behind #2178.
        //
        // The failure is logged rather than propagated to match
        // `get_missing_parents`'s warn-and-continue behaviour. Stale
        // `dag_heads` is recoverable (the next sync session overwrites
        // them). Since #2185, events are preserved in the DB record
        // until the caller confirms handler execution, so a failure
        // here no longer risks losing the event payloads — they still
        // live in both the in-memory Vec we return *and* in the DB.
        match self
            .applier
            .context_client
            .update_dag_heads(&self.applier.context_id, heads.clone())
        {
            Ok(()) => debug!(
                context_id = %self.applier.context_id,
                new_heads = ?heads,
                "Updated database dag_heads"
            ),
            Err(e) => warn!(
                ?e,
                context_id = %self.applier.context_id,
                "Failed to update dag_heads in database; next sync will correct it"
            ),
        }

        forwarded_events
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

        for head_id in boundary_dag_heads {
            // Skip genesis (zero hash)
            if head_id == [0; 32] {
                continue;
            }

            // Create a proper checkpoint delta using the architecture-defined constructor
            let checkpoint = CausalDelta::checkpoint(head_id, boundary_root_hash);

            // Restore the checkpoint to the DAG (marks it as applied)
            if dag.restore_applied_delta(checkpoint.clone()) {
                added_count += 1;

                // CRITICAL: Persist the checkpoint to the database so peers can request it
                // Without this, delta sync fails because the parent delta (checkpoint) is "not found"
                let mut handle = self.applier.context_client.datastore_handle();
                let serialized_actions = match borsh::to_vec(&checkpoint.payload) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(
                            ?e,
                            context_id = %self.applier.context_id,
                            ?head_id,
                            "Failed to serialize checkpoint payload"
                        );
                        continue;
                    }
                };

                if let Err(e) = handle.put(
                    &calimero_store::key::ContextDagDelta::new(self.applier.context_id, head_id),
                    &calimero_store::types::ContextDagDelta {
                        delta_id: head_id,
                        parents: checkpoint.parents.clone(),
                        actions: serialized_actions,
                        hlc: checkpoint.hlc,
                        applied: true, // Checkpoints are always "applied"
                        expected_root_hash: checkpoint.expected_root_hash,
                        events: None,
                    },
                ) {
                    tracing::warn!(
                        ?e,
                        context_id = %self.applier.context_id,
                        ?head_id,
                        "Failed to persist checkpoint to database"
                    );
                } else {
                    tracing::info!(
                        context_id = %self.applier.context_id,
                        ?head_id,
                        "Persisted snapshot checkpoint to DAG and database"
                    );
                }
            }
        }

        // Also track the expected root hash for merge detection
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
}
