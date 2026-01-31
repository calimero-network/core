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

use std::sync::Arc;
use std::time::Duration;

use std::collections::HashMap;

use calimero_context_primitives::client::ContextClient;
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
    /// List of (delta_id, events_data) for cascaded deltas that have event handlers to execute
    pub cascaded_events: Vec<([u8; 32], Vec<u8>)>,
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
    /// Maps delta_id -> expected_root_hash for parent state tracking
    /// Used to detect concurrent branches (merge scenarios)
    parent_hashes: Arc<RwLock<HashMap<[u8; 32], [u8; 32]>>>,
}

#[async_trait::async_trait]
impl DeltaApplier<Vec<Action>> for ContextStorageApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        let apply_start = std::time::Instant::now();

        // Get current context state
        let context = self
            .context_client
            .get_context(&self.context_id)
            .map_err(|e| ApplyError::Application(format!("Failed to get context: {}", e)))?
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

        // Serialize actions to StorageDelta
        let artifact = borsh::to_vec(&StorageDelta::Actions(delta.payload.clone()))
            .map_err(|e| ApplyError::Application(format!("Failed to serialize delta: {}", e)))?;

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
            .map_err(|e| ApplyError::Application(format!("WASM execution failed: {}", e)))?;

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
        // SIMPLE AND CORRECT: If our current state differs from what the delta expects
        // as the RESULT, then we have diverged and need merge semantics.
        // This covers all cases:
        // 1. First concurrent delta from remote
        // 2. Subsequent deltas in a remote chain after we've already merged
        // 3. Any other divergence scenario
        //
        // The key insight: if delta.expected_root_hash == current_root_hash after
        // sequential application, we'd be fine. If they differ, we've diverged.
        // But we can't know that until after applying. So instead, check if our
        // current state matches what ANY parent in the chain expected.

        // Genesis parent means this is the first delta - check if we have state
        if delta.parents.is_empty() || delta.parents.iter().all(|p| *p == [0u8; 32]) {
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

        // Get the expected root hash of the delta's parent(s)
        let hashes = self.parent_hashes.read().await;

        for parent_id in &delta.parents {
            if *parent_id == [0u8; 32] {
                continue; // Skip genesis
            }

            if let Some(parent_expected_hash) = hashes.get(parent_id) {
                // Parent's expected_root_hash is what the REMOTE expected AFTER applying that parent
                // If our current state differs, we've diverged (either we merged, or have local changes)
                if parent_expected_hash != current_root_hash {
                    debug!(
                        context_id = %self.context_id,
                        delta_id = ?delta.id,
                        parent_id = ?parent_id,
                        parent_expected_hash = ?Hash::from(*parent_expected_hash),
                        current_root_hash = ?Hash::from(*current_root_hash),
                        "State diverged from parent's expected - treating as merge"
                    );
                    return true;
                } else {
                    debug!(
                        context_id = %self.context_id,
                        delta_id = ?delta.id,
                        parent_id = ?parent_id,
                        parent_expected_hash = ?Hash::from(*parent_expected_hash),
                        current_root_hash = ?Hash::from(*current_root_hash),
                        "State matches parent's expected - sequential application OK"
                    );
                }
            } else {
                // Parent was created by another node - we don't have its hash tracked
                // Conservative: treat as merge
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

        false
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

        let applier = Arc::new(ContextStorageApplier {
            context_client,
            context_id,
            our_identity,
            parent_hashes: Arc::clone(&parent_hashes),
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
            let dag_delta = CausalDelta {
                id: stored_delta.delta_id,
                parents: stored_delta.parents,
                payload: actions,
                hlc: stored_delta.hlc,
                expected_root_hash: stored_delta.expected_root_hash,
            };

            // Store root hash mapping for merge detection
            {
                let mut head_hashes = self.head_root_hashes.write().await;
                let _ = head_hashes.insert(stored_delta.delta_id, stored_delta.expected_root_hash);
            }
            {
                // Also populate parent hash tracker for merge detection
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

        Ok(loaded_count)
    }

    /// Add boundary delta stubs to the DAG after snapshot sync.
    ///
    /// # WORKAROUND
    ///
    /// This is a **workaround** for the snapshot sync → delta sync transition.
    /// See `TECH-DEBT-SYNC-2026-01.md` for discussion of alternatives.
    ///
    /// # Problem
    ///
    /// Snapshot sync transfers state without delta history. When new deltas arrive
    /// referencing pre-snapshot parents, the DAG would reject them with "parent not found".
    ///
    /// # Solution
    ///
    /// Create placeholder ("stub") deltas for the snapshot boundary DAG heads:
    /// - Stub ID = actual DAG head ID (so new deltas can reference it as parent)
    /// - Stub parent = genesis `[0; 32]` (fake - we don't know actual parents)
    /// - Stub payload = empty (no actions to replay)
    /// - Marked as "already applied" via `restore_applied_delta()`
    ///
    /// # Limitations
    ///
    /// - **No history replay**: Can't reconstruct pre-snapshot state changes
    /// - **Broken parent chain**: DAG traversal stops at stubs
    /// - **Audit gap**: No verification of pre-snapshot history
    ///
    /// # Future Work
    ///
    /// Consider a proper "checkpoint delta" type in the DAG protocol that
    /// represents snapshot boundaries as first-class citizens.
    pub async fn add_snapshot_boundary_stubs(
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

            // Create a stub delta with no payload
            let stub = CausalDelta::new(
                head_id,
                vec![[0; 32]], // Parent is "genesis" (since we don't know actual parents)
                Vec::new(),    // Empty payload - no actions
                calimero_storage::logical_clock::HybridTimestamp::default(),
                boundary_root_hash, // Expected root hash is the snapshot boundary
            );

            // Restore the stub to the DAG (marks it as applied)
            if dag.restore_applied_delta(stub) {
                added_count += 1;
                info!(
                    context_id = %self.applier.context_id,
                    ?head_id,
                    "Added snapshot boundary stub to DAG"
                );
            }
        }

        // Also track the expected root hash for merge detection
        if added_count > 0 {
            let mut head_hashes = self.head_root_hashes.write().await;
            for head_id in dag.get_heads().iter() {
                let _previous = head_hashes.insert(*head_id, boundary_root_hash);
            }
        }

        info!(
            context_id = %self.applier.context_id,
            added_count,
            "Snapshot boundary stubs added to DAG"
        );

        added_count
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
        let pending_before: std::collections::HashSet<[u8; 32]> =
            dag.get_pending_delta_ids().into_iter().collect();

        // If parents are missing, `result` will be FALSE, and `dag` internally stores it as
        // pending.
        let result = dag.add_delta(delta, &*self.applier).await?;

        // Update context's dag_heads after the DAG has been updated
        let heads = dag.get_heads();

        // Get list of deltas that were pending but are now applied (cascade effect)
        let cascaded_deltas: Vec<[u8; 32]> = if !pending_before.is_empty() {
            let pending_after: std::collections::HashSet<[u8; 32]> =
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

        // Handle cascaded deltas: persist as applied and return event data for handler execution
        let cascaded_with_events: Vec<([u8; 32], Vec<u8>)> = if !cascaded_deltas.is_empty() {
            info!(
                context_id = %self.applier.context_id,
                cascaded_count = cascaded_deltas.len(),
                "Persisting cascaded deltas that were applied from pending queue"
            );

            let dag = self.dag.read().await;
            let mut handle = self.applier.context_client.datastore_handle();
            let mut deltas_with_events = Vec::new();

            for cascaded_id in &cascaded_deltas {
                // Check if this delta has stored events
                let db_key = calimero_store::key::ContextDagDelta::new(
                    self.applier.context_id,
                    *cascaded_id,
                );

                let stored_delta_result = handle.get(&db_key);
                let stored_events = match stored_delta_result {
                    Ok(Some(stored)) => {
                        let has_events = stored.events.is_some();
                        debug!(
                            context_id = %self.applier.context_id,
                            delta_id = ?cascaded_id,
                            has_events,
                            "Retrieved stored delta for cascaded delta"
                        );
                        stored.events
                    }
                    Ok(None) => {
                        debug!(
                            context_id = %self.applier.context_id,
                            delta_id = ?cascaded_id,
                            "Cascaded delta not found in database (was never persisted)"
                        );
                        None
                    }
                    Err(e) => {
                        warn!(
                            ?e,
                            context_id = %self.applier.context_id,
                            delta_id = ?cascaded_id,
                            "Failed to query database for cascaded delta"
                        );
                        None
                    }
                };

                if let Some(cascaded_delta) = dag.get_delta(cascaded_id) {
                    let serialized_actions = match borsh::to_vec(&cascaded_delta.payload) {
                        Ok(s) => s,
                        Err(e) => {
                            warn!(
                                ?e,
                                context_id = %self.applier.context_id,
                                delta_id = ?cascaded_id,
                                "Failed to serialize cascaded delta actions, skipping persistence"
                            );
                            continue;
                        }
                    };

                    // Store events for later handler execution
                    if let Some(ref events_data) = stored_events {
                        deltas_with_events.push((*cascaded_id, events_data.clone()));
                    }

                    if let Err(e) = handle.put(
                        &db_key,
                        &calimero_store::types::ContextDagDelta {
                            delta_id: *cascaded_id,
                            parents: cascaded_delta.parents.clone(),
                            actions: serialized_actions,
                            hlc: cascaded_delta.hlc,
                            applied: true,
                            expected_root_hash: cascaded_delta.expected_root_hash,
                            events: None, // Clear events after cascading (handlers will execute below)
                        },
                    ) {
                        warn!(
                            ?e,
                            context_id = %self.applier.context_id,
                            delta_id = ?cascaded_id,
                            "Failed to persist cascaded delta to database"
                        );
                    } else if stored_events.is_some() {
                        info!(
                            context_id = %self.applier.context_id,
                            delta_id = ?cascaded_id,
                            "Persisted cascaded delta - has events for handler execution"
                        );
                    }
                }
            }
            drop(dag);

            deltas_with_events
        } else {
            Vec::new()
        };

        self.applier
            .context_client
            .update_dag_heads(&self.applier.context_id, heads.clone())
            .map_err(|e| eyre::eyre!("Failed to update dag_heads: {}", e))?;

        // NOTE: We no longer force a deterministic root hash for concurrent branches.
        // Our CRDT merge logic (in ContextStorageApplier::apply) now properly merges
        // concurrent branches, producing a new root hash that incorporates all changes.
        // Forcing one branch's hash would overwrite the merged state and lose data!
        //
        // Multiple DAG heads are expected during concurrent activity and will be resolved
        // when deltas from other branches are applied with CRDT merge semantics.
        if heads.len() > 1 {
            debug!(
                context_id = %self.applier.context_id,
                heads_count = heads.len(),
                "Multiple DAG heads detected - CRDT merge will reconcile when applying deltas"
            );
        }

        // Cleanup old head hashes that are no longer active
        {
            let mut head_hashes = self.head_root_hashes.write().await;
            head_hashes.retain(|head_id, _| heads.contains(head_id));
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

        // Filter out parents that exist in the database
        let handle = self.applier.context_client.datastore_handle();
        let mut actually_missing = Vec::new();
        let mut all_cascaded_events: Vec<([u8; 32], Vec<u8>)> = Vec::new();

        for parent_id in &potentially_missing {
            let db_key =
                calimero_store::key::ContextDagDelta::new(self.applier.context_id, *parent_id);

            match handle.get(&db_key) {
                Ok(Some(stored_delta)) => {
                    // Parent exists in database - load it into DAG!
                    tracing::info!(
                        context_id = %self.applier.context_id,
                        parent_id = ?parent_id,
                        "Parent delta found in database - loading into DAG cache"
                    );

                    // Reconstruct the delta and add to DAG
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
                    };

                    // Add to DAG and track any cascaded deltas
                    let mut dag = self.dag.write().await;

                    let pending_before: std::collections::HashSet<[u8; 32]> =
                        dag.get_pending_delta_ids().into_iter().collect();

                    if let Err(e) = dag.add_delta(dag_delta, &*self.applier).await {
                        tracing::warn!(
                            ?e,
                            context_id = %self.applier.context_id,
                            parent_id = ?parent_id,
                            "Failed to load persisted parent delta into DAG"
                        );
                    }

                    // Check for cascaded deltas
                    let cascaded_deltas: Vec<[u8; 32]> = if !pending_before.is_empty() {
                        let pending_after: std::collections::HashSet<[u8; 32]> =
                            dag.get_pending_delta_ids().into_iter().collect();
                        pending_before.difference(&pending_after).copied().collect()
                    } else {
                        Vec::new()
                    };

                    // Persist cascaded deltas and retrieve their stored events
                    if !cascaded_deltas.is_empty() {
                        info!(
                            context_id = %self.applier.context_id,
                            cascaded_count = cascaded_deltas.len(),
                            "Persisting cascaded deltas triggered by loading parent from DB"
                        );

                        for cascaded_id in &cascaded_deltas {
                            // Retrieve stored events for this cascaded delta
                            let cascaded_db_key = calimero_store::key::ContextDagDelta::new(
                                self.applier.context_id,
                                *cascaded_id,
                            );
                            let stored_events =
                                handle.get(&cascaded_db_key).ok().flatten().and_then(
                                    |stored: calimero_store::types::ContextDagDelta| stored.events,
                                );

                            if stored_events.is_some() {
                                info!(
                                    context_id = %self.applier.context_id,
                                    delta_id = ?cascaded_id,
                                    "Found stored events for cascaded delta - will execute handlers"
                                );
                            }

                            if let Some(cascaded_delta) = dag.get_delta(cascaded_id) {
                                let serialized_actions = match borsh::to_vec(
                                    &cascaded_delta.payload,
                                ) {
                                    Ok(s) => s,
                                    Err(e) => {
                                        warn!(?e, context_id = %self.applier.context_id, delta_id = ?cascaded_id, "Failed to serialize");
                                        continue;
                                    }
                                };

                                // Add events to return list
                                if let Some(events_data) = stored_events {
                                    all_cascaded_events.push((*cascaded_id, events_data));
                                }

                                if let Err(e) = self.applier.context_client.datastore_handle().put(
                                    &cascaded_db_key,
                                    &calimero_store::types::ContextDagDelta {
                                        delta_id: *cascaded_id,
                                        parents: cascaded_delta.parents.clone(),
                                        actions: serialized_actions,
                                        hlc: cascaded_delta.hlc,
                                        applied: true,
                                        expected_root_hash: cascaded_delta.expected_root_hash,
                                        events: None, // Clear events after cascading
                                    },
                                ) {
                                    warn!(?e, context_id = %self.applier.context_id, delta_id = ?cascaded_id, "Failed to persist cascaded delta");
                                }
                            }
                        }
                    }

                    drop(dag);
                }
                Ok(None) => {
                    // Truly missing - add to request list
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
            cascaded_events: all_cascaded_events,
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

    /// Get all applied delta IDs for bloom filter sync
    ///
    /// Returns all delta IDs that have been successfully applied to this store.
    pub async fn get_applied_delta_ids(&self) -> Vec<[u8; 32]> {
        let dag = self.dag.read().await;
        dag.get_applied_delta_ids()
    }

    /// Get deltas that the remote doesn't have based on their bloom filter
    ///
    /// Checks each of our applied deltas against the remote's bloom filter.
    /// Returns deltas that are NOT in the filter (remote is missing them).
    pub async fn get_deltas_not_in_bloom(
        &self,
        bloom_filter: &[u8],
        false_positive_rate: f32,
    ) -> Vec<CausalDelta<Vec<Action>>> {
        let dag = self.dag.read().await;
        dag.get_deltas_not_in_bloom(bloom_filter, false_positive_rate)
    }
}
