//! DAG-based delta storage and application
//!
//! Wraps calimero-dag and provides context-aware delta application via WASM.

use std::sync::Arc;
use std::time::Duration;

use std::collections::HashMap;

use calimero_context_primitives::client::ContextClient;
use calimero_dag::{ApplyError, CausalDelta, DagStore as CoreDagStore, DeltaApplier, PendingStats};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::delta::StorageDelta;
use eyre::Result;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Applier that applies actions to WASM storage via ContextClient
#[derive(Debug)]
struct ContextStorageApplier {
    context_client: ContextClient,
    context_id: ContextId,
    our_identity: PublicKey,
}

#[async_trait::async_trait]
impl DeltaApplier<Vec<Action>> for ContextStorageApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        // Serialize actions to StorageDelta
        let artifact = borsh::to_vec(&StorageDelta::Actions(delta.payload.clone()))
            .map_err(|e| ApplyError::Application(format!("Failed to serialize delta: {}", e)))?;

        // Get context to access WASM runtime
        let Some(_context) = self
            .context_client
            .get_context(&self.context_id)
            .map_err(|e| ApplyError::Application(format!("Failed to get context: {}", e)))?
        else {
            return Err(ApplyError::Application("Context not found".to_owned()));
        };

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

        if outcome.returns.is_err() {
            return Err(ApplyError::Application(format!(
                "WASM sync returned error: {:?}",
                outcome.returns
            )));
        }

        // Ensure deterministic root hash across all nodes.
        // WASM execution may produce different hashes due to non-deterministic factors;
        // use the delta author's expected_root_hash to maintain DAG consistency.
        let computed_hash = outcome.root_hash;
        if *computed_hash != delta.expected_root_hash {
            warn!(
                context_id = %self.context_id,
                delta_id = ?delta.id,
                computed_hash = ?computed_hash,
                expected_hash = ?delta.expected_root_hash,
                "Root hash mismatch - using expected hash for consistency"
            );

            self.context_client
                .force_root_hash(&self.context_id, delta.expected_root_hash.into())
                .map_err(|e| ApplyError::Application(format!("Failed to set root hash: {}", e)))?;
        }

        debug!(
            context_id = %self.context_id,
            delta_id = ?delta.id,
            action_count = delta.payload.len(),
            expected_root_hash = ?delta.expected_root_hash,
            "Applied delta to WASM storage"
        );

        Ok(())
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
        let applier = Arc::new(ContextStorageApplier {
            context_client,
            context_id,
            our_identity,
        });

        Self {
            dag: Arc::new(RwLock::new(CoreDagStore::new(root))),
            applier,
            head_root_hashes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load all persisted deltas from the database into the in-memory DAG
    ///
    /// CRITICAL: This should be called after creating a DeltaStore to restore
    /// the DAG state from persistent storage. Without this, nodes will "forget"
    /// their DAG history after restart and create disconnected chains.
    ///
    /// IMPORTANT: Deltas must be loaded in topological order (parents before children)
    /// to properly reconstruct the DAG topology.
    pub async fn load_persisted_deltas(&self) -> Result<usize> {
        use std::collections::HashMap;

        let handle = self.applier.context_client.datastore_handle();

        // Step 1: Collect ALL deltas for this context from DB
        let mut iter = handle.iter::<calimero_store::key::ContextDagDelta>()?;
        let mut all_deltas: HashMap<[u8; 32], CausalDelta<Vec<Action>>> =
            HashMap::new();

        for entry in iter.entries() {
            let (key_result, value_result) = entry;
            let key = key_result?;
            let stored_delta = value_result?;

            // Filter by context_id
            if key.context_id() != self.applier.context_id {
                continue;
            }

            // Deserialize actions
            let actions: Vec<Action> =
                match borsh::from_slice(&stored_delta.actions) {
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

            // Store root hash mapping
            {
                let mut head_hashes = self.head_root_hashes.write().await;
                let _ = head_hashes.insert(stored_delta.delta_id, stored_delta.expected_root_hash);
            }

            let _ = all_deltas.insert(stored_delta.delta_id, dag_delta);
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

                // CRITICAL FIX: Check if all parents are APPLIED, not just if they exist
                // A parent might be in the DAG but still pending (not applied yet)
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
                let _ = remaining.remove(&delta_id);
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

    /// Add a delta to the store
    ///
    /// Returns Ok(true) if applied immediately, Ok(false) if pending
    pub async fn add_delta(&self, delta: CausalDelta<Vec<Action>>) -> Result<bool> {
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

        let mut dag = self.dag.write().await;

        // Track which deltas are currently pending BEFORE we add the new delta
        // This lets us detect which pending deltas got applied during the cascade
        let pending_before: std::collections::HashSet<[u8; 32]> =
            dag.get_pending_delta_ids().into_iter().collect();

        let result = dag.add_delta(delta, &*self.applier).await?;

        // CRITICAL: Update context's dag_heads to ALL current DAG heads
        // This must happen AFTER the DAG has updated its heads (which happens in add_delta)
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

        // CRITICAL FIX: Persist APPLIED deltas to RocksDB
        // This includes both the newly added delta (if it applied) AND any deltas
        // that were applied from the pending queue due to the cascade effect
        if result {
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
                    },
                )
                .map_err(|e| eyre::eyre!("Failed to persist delta to database: {}", e))?;

            debug!(
                context_id = %self.applier.context_id,
                delta_id = ?delta_id,
                "Persisted applied delta to database"
            );
        }

        // CRITICAL FIX: Persist cascaded deltas that were applied from pending queue
        // When we add a delta that fills a gap, other pending deltas may become ready
        // and get applied by apply_pending(). We need to persist those too!
        if !cascaded_deltas.is_empty() {
            info!(
                context_id = %self.applier.context_id,
                cascaded_count = cascaded_deltas.len(),
                "Persisting cascaded deltas that were applied from pending queue"
            );

            let dag = self.dag.read().await;
            let mut handle = self.applier.context_client.datastore_handle();

            for cascaded_id in &cascaded_deltas {
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

                    if let Err(e) = handle.put(
                        &calimero_store::key::ContextDagDelta::new(
                            self.applier.context_id,
                            *cascaded_id,
                        ),
                        &calimero_store::types::ContextDagDelta {
                            delta_id: *cascaded_id,
                            parents: cascaded_delta.parents.clone(),
                            actions: serialized_actions,
                            hlc: cascaded_delta.hlc,
                            applied: true,
                            expected_root_hash: cascaded_delta.expected_root_hash,
                        },
                    ) {
                        warn!(
                            ?e,
                            context_id = %self.applier.context_id,
                            delta_id = ?cascaded_id,
                            "Failed to persist cascaded delta to database"
                        );
                    } else {
                        debug!(
                            context_id = %self.applier.context_id,
                            delta_id = ?cascaded_id,
                            "Persisted cascaded delta to database"
                        );
                    }
                }
            }
            drop(dag);
        }

        self.applier
            .context_client
            .update_dag_heads(&self.applier.context_id, heads.clone())
            .map_err(|e| eyre::eyre!("Failed to update dag_heads: {}", e))?;

        // Deterministic root hash selection for concurrent branches.
        // When multiple DAG heads exist, use the lexicographically smallest head's root_hash
        // to ensure all nodes converge to the same root regardless of delta arrival order.
        if heads.len() > 1 {
            let head_hashes = self.head_root_hashes.read().await;
            let mut sorted_heads = heads.clone();
            sorted_heads.sort();
            let canonical_head = sorted_heads[0];

            if let Some(&canonical_root_hash) = head_hashes.get(&canonical_head) {
                debug!(
                    context_id = %self.applier.context_id,
                    heads_count = heads.len(),
                    canonical_head = ?canonical_head,
                    canonical_root = ?canonical_root_hash,
                    "Multiple DAG heads - using deterministic root hash selection"
                );

                self.applier
                    .context_client
                    .force_root_hash(&self.applier.context_id, canonical_root_hash.into())
                    .map_err(|e| eyre::eyre!("Failed to set canonical root hash: {}", e))?;
            }
        }

        // Cleanup old head hashes that are no longer active
        {
            let mut head_hashes = self.head_root_hashes.write().await;
            head_hashes.retain(|head_id, _| heads.contains(head_id));
        }

        Ok(result)
    }

    /// Get missing parent IDs (for requesting from peers)
    ///
    /// This checks both the in-memory DAG and the database to avoid requesting
    /// deltas that are already persisted but not loaded into RAM.
    pub async fn get_missing_parents(&self) -> Vec<[u8; 32]> {
        let dag = self.dag.read().await;
        let potentially_missing = dag.get_missing_parents();
        drop(dag); // Release lock before DB access

        // Filter out parents that exist in the database
        let handle = self.applier.context_client.datastore_handle();
        let mut actually_missing = Vec::new();

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
                    let actions: Vec<Action> =
                        match borsh::from_slice(&stored_delta.actions) {
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

                    // Add to DAG (this might trigger pending deltas to be applied via cascade!)
                    // CRITICAL FIX: Track cascaded deltas here too, not just in main add_delta()
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

                    // Persist cascaded deltas
                    if !cascaded_deltas.is_empty() {
                        info!(
                            context_id = %self.applier.context_id,
                            cascaded_count = cascaded_deltas.len(),
                            "Persisting cascaded deltas triggered by loading parent from DB"
                        );

                        for cascaded_id in &cascaded_deltas {
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

                                if let Err(e) = self.applier.context_client.datastore_handle().put(
                                    &calimero_store::key::ContextDagDelta::new(
                                        self.applier.context_id,
                                        *cascaded_id,
                                    ),
                                    &calimero_store::types::ContextDagDelta {
                                        delta_id: *cascaded_id,
                                        parents: cascaded_delta.parents.clone(),
                                        actions: serialized_actions,
                                        hlc: cascaded_delta.hlc,
                                        applied: true,
                                        expected_root_hash: cascaded_delta.expected_root_hash,
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
                "Filtered missing parents - some were already in database"
            );
        }

        actually_missing
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
}
