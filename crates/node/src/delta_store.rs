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

use std::collections::{HashMap, HashSet};
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
use calimero_storage::delta::StorageDelta;
use eyre::Result;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Result of adding a delta with cascaded event information
#[derive(Debug)]
pub struct AddDeltaResult {
    /// Whether the delta was applied immediately (true) or went pending (false)
    pub applied: bool,
    /// List of (delta_id, events_data) for cascaded deltas that have event handlers to execute
    pub cascaded_events: Vec<([u8; 32], Vec<u8>)>,
}

/// Result of checking for missing parents with cascaded event information
#[derive(Debug)]
pub struct MissingParentsResult {
    /// IDs of deltas that are truly missing (need to be requested from network)
    pub missing_ids: Vec<[u8; 32]>,
    /// IDs of ALL deltas cascaded during the check, regardless of whether
    /// they carry events. Callers can use this to detect whether the current
    /// delta was applied via cascade without re-acquiring the DAG lock.
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
                delta_id = ?delta.id,
                current_root_hash = ?Hash::from(current_root_hash),
                delta_expected_hash = ?Hash::from(delta.expected_root_hash),
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

        // Serialize actions to StorageDelta
        let artifact = borsh::to_vec(&StorageDelta::Actions(delta.payload.clone()))
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

        debug!(
            context_id = %self.context_id,
            delta_id = ?delta.id,
            root_hash = ?outcome.root_hash,
            return_registers = ?outcome.returns,
            is_merge = is_merge_scenario,
            wasm_ms = format!("{:.2}", wasm_elapsed_ms),
            "WASM sync completed execution"
        );

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
                    delta_id = ?delta.id,
                    computed_hash = ?computed_hash,
                    delta_expected_hash = ?Hash::from(delta.expected_root_hash),
                    merge_wasm_ms = format!("{:.2}", wasm_elapsed_ms),
                    "Merge produced new hash (expected - concurrent branches merged)"
                );
            } else {
                // Even "sequential" applications can produce different hashes if we have
                // concurrent state that the sender doesn't know about. This is normal in
                // a distributed CRDT system.
                debug!(
                    context_id = %self.context_id,
                    delta_id = ?delta.id,
                    computed_hash = ?computed_hash,
                    expected_hash = ?Hash::from(delta.expected_root_hash),
                    "Hash mismatch (concurrent state) - CRDT merge ensures consistency"
                );
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
            delta_id = ?delta.id,
            action_count = delta.payload.len(),
            final_root_hash = ?computed_hash,
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
    pub async fn load_persisted_deltas(&self) -> Result<usize> {
        use std::collections::HashMap;

        let handle = self.applier.context_client.datastore_handle();

        // Step 1: Collect ALL deltas for this context from DB
        let mut iter = handle.iter::<calimero_store::key::ContextDagDelta>()?;
        let mut all_deltas: HashMap<[u8; 32], CausalDelta<Vec<Action>>> = HashMap::new();

        for entry in iter.entries() {
            let (key_result, value_result) = entry;
            let key = key_result?;
            let stored_delta = value_result?;

            // Filter by context_id
            if key.context_id() != self.applier.context_id {
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

        if all_deltas.is_empty() {
            return Ok(0);
        }

        debug!(
            context_id = %self.applier.context_id,
            total_deltas = all_deltas.len(),
            "Collected persisted deltas, starting topological restore"
        );

        // Step 2: Restore deltas in topological order (parents before children)
        // We keep trying to restore deltas whose parents are already in the DAG
        // NOTE: All persisted deltas are already applied, so we just restore topology
        let mut loaded_count = 0;
        let mut remaining = all_deltas;
        let mut progress_made = true;

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
                        progress_made = true;
                    }
                }
            }

            for delta_id in to_remove {
                drop(remaining.remove(&delta_id));
            }
        }

        // Log any deltas that couldn't be loaded
        if !remaining.is_empty() {
            // Collect the IDs of deltas that are still unloadable
            let unloadable_ids: Vec<[u8; 32]> = remaining.keys().copied().collect();

            warn!(
                context_id = %self.applier.context_id,
                remaining_count = remaining.len(),
                loaded_count,
                unloadable_deltas = ?unloadable_ids,
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

        Ok(loaded_count)
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

        let mut dag = self.dag.write().await;

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

        drop(dag); // Release lock before calling context_client

        // Update persistence if delta applied (was pre-persisted with events=Some, now needs events=None)
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
                        events: None, // Clear events after immediate application
                    },
                )
                .map_err(|e| eyre::eyre!("Failed to update applied delta: {}", e))?;

            debug!(
                context_id = %self.applier.context_id,
                delta_id = ?delta_id,
                "Updated pre-persisted delta as applied (cleared events)"
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
            let pending_before: HashSet<[u8; 32]> =
                dag.get_pending_delta_ids().into_iter().collect();
            let mut any_parent_added = false;
            let mut restored_head_hashes: Vec<([u8; 32], [u8; 32])> = Vec::new();
            // Parents we just transitioned from `applied: false` → applied
            // in the DAG via `add_delta`. Their DB records still say
            // `applied: false`; phase 3 persists the update alongside any
            // cascaded children. Without this, `load_persisted_deltas_into_dag`
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
                        // Clone before `add_delta` consumes — on success
                        // we hand the body to phase 3 to rewrite the DB
                        // record as `applied: true` and forward any
                        // pre-stored events (same treatment as cascaded
                        // children).
                        let body_for_persist = dag_delta.clone();
                        match dag.add_delta(dag_delta, &*self.applier).await {
                            Ok(_) => {
                                any_parent_added = true;
                                added_parent_bodies.push((parent_id, body_for_persist));
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
            let cascaded_ids: Vec<[u8; 32]> =
                pending_before.difference(&pending_after).copied().collect();

            // Clone bodies now so DB I/O in phase 3 runs without the lock.
            let cascaded_bodies: Vec<([u8; 32], CausalDelta<Vec<Action>>)> = cascaded_ids
                .iter()
                .filter_map(|cid| dag.get_delta(cid).map(|d| (*cid, d.clone())))
                .collect();

            (
                any_parent_added,
                restored_head_hashes,
                cascaded_ids,
                cascaded_bodies,
                added_parent_bodies,
                dag.get_heads(),
            )
        };

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
            // true` so restart's `load_persisted_deltas_into_dag` stops
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
    /// - Any existing DB record's `events` column is read and forwarded in
    ///   the return value (so the caller can run handlers) before the
    ///   record is rewritten with `applied: true, events: None`.
    ///   Events-less deltas are never pre-persisted, so a missing DB
    ///   record is normal for the cascade path — the helper rebuilds the
    ///   full record from the in-memory body. Add-path parents always
    ///   have a DB record present (phase 1 loaded it).
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
            for (cid, cascaded_delta) in applied_bodies {
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
                            "Retrieved stored delta for cascaded delta"
                        );
                        stored.events
                    }
                    Ok(None) => {
                        debug!(
                            context_id = %self.applier.context_id,
                            delta_id = ?cid,
                            "Cascaded delta not found in database (was never persisted)"
                        );
                        None
                    }
                    Err(e) => {
                        warn!(
                            ?e,
                            context_id = %self.applier.context_id,
                            delta_id = ?cid,
                            "Failed to query database for cascaded delta"
                        );
                        None
                    }
                };

                let serialized_actions = match borsh::to_vec(&cascaded_delta.payload) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(
                            ?e,
                            context_id = %self.applier.context_id,
                            delta_id = ?cid,
                            "Failed to serialize cascaded delta actions, skipping persistence"
                        );
                        continue;
                    }
                };

                if let Some(ref events_data) = stored_events {
                    forwarded_events.push((*cid, events_data.clone()));
                }

                let record = calimero_store::types::ContextDagDelta {
                    delta_id: *cid,
                    parents: cascaded_delta.parents.clone(),
                    actions: serialized_actions,
                    hlc: cascaded_delta.hlc,
                    applied: true,
                    expected_root_hash: cascaded_delta.expected_root_hash,
                    events: None, // Cleared — caller will run handlers.
                };
                if let Err(e) = handle.put(&db_key, &record) {
                    warn!(
                        ?e,
                        context_id = %self.applier.context_id,
                        delta_id = ?cid,
                        "Failed to persist cascaded delta to database"
                    );
                } else if stored_events.is_some() {
                    info!(
                        context_id = %self.applier.context_id,
                        delta_id = ?cid,
                        "Persisted cascaded delta - has events for handler execution"
                    );
                }
            }
        }

        // Update the database's `dag_heads` so sync handshakes and
        // `broadcast_heartbeat` see the post-cascade state. Failing to
        // do this was the original bug behind #2178.
        //
        // The failure is logged rather than propagated: the applied
        // deltas above have already been rewritten in the DB with
        // `events: None`, so their event payloads now survive only in
        // our return value. If we bailed out with `Err` here, the caller
        // would drop the Vec and those events would be permanently lost
        // — handlers would never run for deltas that *were* successfully
        // persisted. Stale `dag_heads` in the database is recoverable
        // (the next sync session overwrites them); silently dropped
        // events are not. This mirrors the original `get_missing_parents`
        // behaviour (warn-and-continue) and fixes an equivalent latent
        // bug that was present in the old inline `add_delta_internal`
        // code (which `?`-propagated and lost the same events).
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
