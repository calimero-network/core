//! DAG-based delta storage and application
//!
//! Wraps calimero-dag and provides context-aware delta application via WASM.

use std::sync::Arc;
use std::time::Duration;

use std::collections::HashMap;

use calimero_context_primitives::client::ContextClient;
use calimero_dag::{CausalDelta, DagStore as CoreDagStore, PendingStats};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use eyre::Result;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::services::ContextDeltaApplier;

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

// NOTE: ContextStorageApplier moved to services/delta_applier.rs as ContextDeltaApplier
// for better separation of concerns and testability.

/// Node-level delta store that wraps calimero-dag
#[derive(Clone, Debug)]
pub struct DeltaStore {
    /// Core DAG logic (topology, ordering, buffering)
    dag: Arc<RwLock<CoreDagStore<Vec<Action>>>>,

    /// Applier for applying deltas to WASM storage
    /// Uses ContextDeltaApplier from services module for better separation
    applier: Arc<ContextDeltaApplier>,

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
        // Create applier using the extracted ContextDeltaApplier from services module
        let applier = Arc::new(ContextDeltaApplier::new(
            context_client,
            context_id,
            our_identity,
        ));

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

        let handle = self.applier.context_client().datastore_handle();

        // Step 1: Collect ALL deltas for this context from DB
        let mut iter = handle.iter::<calimero_store::key::ContextDagDelta>()?;
        let mut all_deltas: HashMap<[u8; 32], CausalDelta<Vec<Action>>> = HashMap::new();

        for entry in iter.entries() {
            let (key_result, value_result) = entry;
            let key = key_result?;
            let stored_delta = value_result?;

            // Filter by context_id
            if key.context_id() != *self.applier.context_id() {
                continue;
            }

            // Deserialize actions
            let actions: Vec<Action> = match borsh::from_slice(&stored_delta.actions) {
                Ok(actions) => actions,
                Err(e) => {
                    warn!(
                        ?e,
                        context_id = %*self.applier.context_id(),
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

            drop(all_deltas.insert(stored_delta.delta_id, dag_delta));
        }

        if all_deltas.is_empty() {
            return Ok(0);
        }

        debug!(
            context_id = %*self.applier.context_id(),
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
                context_id = %*self.applier.context_id(),
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
                context_id = %*self.applier.context_id(),
                loaded_count,
                "Loaded persisted deltas into DAG from database"
            );
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
            let mut handle = self.applier.context_client().datastore_handle();
            let serialized_actions = borsh::to_vec(&actions_for_db)
                .map_err(|e| eyre::eyre!("Failed to serialize delta actions: {}", e))?;

            handle
                .put(
                    &calimero_store::key::ContextDagDelta::new(*self.applier.context_id(), delta_id),
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
                context_id = %*self.applier.context_id(),
                delta_id = ?delta_id,
                "Pre-persisted pending delta WITH events (before DAG add)"
            );
        }

        let mut dag = self.dag.write().await;

        // Track which deltas are currently pending BEFORE we add the new delta
        // This lets us detect which pending deltas got applied during the cascade
        let pending_before: std::collections::HashSet<[u8; 32]> =
            dag.get_pending_delta_ids().into_iter().collect();

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
            let mut handle = self.applier.context_client().datastore_handle();
            let serialized_actions = borsh::to_vec(&actions_for_db)
                .map_err(|e| eyre::eyre!("Failed to serialize delta actions: {}", e))?;

            handle
                .put(
                    &calimero_store::key::ContextDagDelta::new(*self.applier.context_id(), delta_id),
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
                context_id = %*self.applier.context_id(),
                delta_id = ?delta_id,
                "Updated pre-persisted delta as applied (cleared events)"
            );
        } else if result {
            // Delta applied and had no events - just persist normally
            let mut handle = self.applier.context_client().datastore_handle();
            let serialized_actions = borsh::to_vec(&actions_for_db)
                .map_err(|e| eyre::eyre!("Failed to serialize delta actions: {}", e))?;

            handle
                .put(
                    &calimero_store::key::ContextDagDelta::new(*self.applier.context_id(), delta_id),
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
                context_id = %*self.applier.context_id(),
                delta_id = ?delta_id,
                "Persisted applied delta to database"
            );
        }
        // If !result, delta is pending and was already pre-persisted with events (if any)

        // Handle cascaded deltas: persist as applied and return event data for handler execution
        let cascaded_with_events: Vec<([u8; 32], Vec<u8>)> = if !cascaded_deltas.is_empty() {
            info!(
                context_id = %*self.applier.context_id(),
                cascaded_count = cascaded_deltas.len(),
                "Persisting cascaded deltas that were applied from pending queue"
            );

            let dag = self.dag.read().await;
            let mut handle = self.applier.context_client().datastore_handle();
            let mut deltas_with_events = Vec::new();

            for cascaded_id in &cascaded_deltas {
                // Check if this delta has stored events
                let db_key = calimero_store::key::ContextDagDelta::new(
                    *self.applier.context_id(),
                    *cascaded_id,
                );

                let stored_delta_result = handle.get(&db_key);
                let stored_events = match stored_delta_result {
                    Ok(Some(stored)) => {
                        let has_events = stored.events.is_some();
                        debug!(
                            context_id = %*self.applier.context_id(),
                            delta_id = ?cascaded_id,
                            has_events,
                            "Retrieved stored delta for cascaded delta"
                        );
                        stored.events
                    }
                    Ok(None) => {
                        debug!(
                            context_id = %*self.applier.context_id(),
                            delta_id = ?cascaded_id,
                            "Cascaded delta not found in database (was never persisted)"
                        );
                        None
                    }
                    Err(e) => {
                        warn!(
                            ?e,
                            context_id = %*self.applier.context_id(),
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
                                context_id = %*self.applier.context_id(),
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
                            context_id = %*self.applier.context_id(),
                            delta_id = ?cascaded_id,
                            "Failed to persist cascaded delta to database"
                        );
                    } else if stored_events.is_some() {
                        info!(
                            context_id = %*self.applier.context_id(),
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
            .context_client()
            .update_dag_heads(&*self.applier.context_id(), heads.clone())
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
                    context_id = %*self.applier.context_id(),
                    heads_count = heads.len(),
                    canonical_head = ?canonical_head,
                    canonical_root = ?canonical_root_hash,
                    "Multiple DAG heads - using deterministic root hash selection"
                );

                self.applier
                    .context_client()
                    .force_root_hash(&*self.applier.context_id(), canonical_root_hash.into())
                    .map_err(|e| eyre::eyre!("Failed to set canonical root hash: {}", e))?;
            }
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
        let potentially_missing = dag.get_missing_parents();
        drop(dag); // Release lock before DB access

        // Filter out parents that exist in the database
        let handle = self.applier.context_client().datastore_handle();
        let mut actually_missing = Vec::new();
        let mut all_cascaded_events: Vec<([u8; 32], Vec<u8>)> = Vec::new();

        for parent_id in &potentially_missing {
            let db_key =
                calimero_store::key::ContextDagDelta::new(*self.applier.context_id(), *parent_id);

            match handle.get(&db_key) {
                Ok(Some(stored_delta)) => {
                    // Parent exists in database - load it into DAG!
                    tracing::info!(
                        context_id = %*self.applier.context_id(),
                        parent_id = ?parent_id,
                        "Parent delta found in database - loading into DAG cache"
                    );

                    // Reconstruct the delta and add to DAG
                    let actions: Vec<Action> = match borsh::from_slice(&stored_delta.actions) {
                        Ok(actions) => actions,
                        Err(e) => {
                            tracing::warn!(
                                ?e,
                                context_id = %*self.applier.context_id(),
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
                            context_id = %*self.applier.context_id(),
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
                            context_id = %*self.applier.context_id(),
                            cascaded_count = cascaded_deltas.len(),
                            "Persisting cascaded deltas triggered by loading parent from DB"
                        );

                        for cascaded_id in &cascaded_deltas {
                            // Retrieve stored events for this cascaded delta
                            let cascaded_db_key = calimero_store::key::ContextDagDelta::new(
                                *self.applier.context_id(),
                                *cascaded_id,
                            );
                            let stored_events =
                                handle.get(&cascaded_db_key).ok().flatten().and_then(
                                    |stored: calimero_store::types::ContextDagDelta| stored.events,
                                );

                            if stored_events.is_some() {
                                info!(
                                    context_id = %*self.applier.context_id(),
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
                                        warn!(?e, context_id = %*self.applier.context_id(), delta_id = ?cascaded_id, "Failed to serialize");
                                        continue;
                                    }
                                };

                                // Add events to return list
                                if let Some(events_data) = stored_events {
                                    all_cascaded_events.push((*cascaded_id, events_data));
                                }

                                if let Err(e) = self.applier.context_client().datastore_handle().put(
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
                                    warn!(?e, context_id = %*self.applier.context_id(), delta_id = ?cascaded_id, "Failed to persist cascaded delta");
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
                        context_id = %*self.applier.context_id(),
                        parent_id = ?parent_id,
                        "Error checking database for parent delta, treating as missing"
                    );
                    actually_missing.push(*parent_id);
                }
            }
        }

        if !actually_missing.is_empty() && actually_missing.len() < potentially_missing.len() {
            tracing::info!(
                context_id = %*self.applier.context_id(),
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
}

// ═══════════════════════════════════════════════════════════════════════════
// Implement DeltaStore trait from calimero-protocols
// ═══════════════════════════════════════════════════════════════════════════

#[async_trait::async_trait(?Send)]
impl calimero_protocols::p2p::delta_request::DeltaStore for DeltaStore {
    async fn has_delta(&self, delta_id: &[u8; 32]) -> bool {
        self.has_delta(delta_id).await
    }
    
    async fn add_delta(
        &self,
        delta: calimero_dag::CausalDelta<Vec<calimero_storage::interface::Action>>,
    ) -> Result<()> {
        // Use the simple add_delta (no events)
        self.add_delta(delta).await?;
        Ok(())
    }
    
    async fn add_delta_with_events(
        &self,
        delta: calimero_dag::CausalDelta<Vec<calimero_storage::interface::Action>>,
        events: Option<Vec<u8>>,
    ) -> Result<calimero_protocols::p2p::delta_request::AddDeltaResult> {
        // Use the full add_delta_with_events
        let result = self.add_delta_with_events(delta, events).await?;
        
        // Convert our AddDeltaResult to protocol's AddDeltaResult
        Ok(calimero_protocols::p2p::delta_request::AddDeltaResult {
            applied: result.applied,
            cascaded_events: result.cascaded_events,
        })
    }
    
    async fn get_delta(
        &self,
        delta_id: &[u8; 32],
    ) -> Option<calimero_dag::CausalDelta<Vec<calimero_storage::interface::Action>>> {
        self.get_delta(delta_id).await
    }
    
    async fn get_missing_parents(&self) -> calimero_protocols::p2p::delta_request::MissingParentsResult {
        let result = self.get_missing_parents().await;
        
        // Convert our MissingParentsResult to protocol's MissingParentsResult
        calimero_protocols::p2p::delta_request::MissingParentsResult {
            missing_ids: result.missing_ids,
            cascaded_events: result.cascaded_events,
        }
    }
    
    async fn dag_has_delta_applied(&self, delta_id: &[u8; 32]) -> bool {
        self.dag_has_delta_applied(delta_id).await
    }
}
