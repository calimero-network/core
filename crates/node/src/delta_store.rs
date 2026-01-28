//! DAG-based delta storage and application
//!
//! Wraps calimero-dag and provides context-aware delta application via WASM.

use std::sync::Arc;
use std::time::Duration;

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
#[derive(Debug)]
struct ContextStorageApplier {
    context_client: ContextClient,
    context_id: ContextId,
    our_identity: PublicKey,
    /// State for conditional root hash validation.
    /// Set before each add_delta call to detect linear-base scenarios.
    validation_state: RwLock<ValidationState>,
}

/// State used to determine if a delta should be validated for root hash mismatch.
#[derive(Debug, Default)]
struct ValidationState {
    /// DAG heads captured before delta application begins.
    pre_apply_heads: Vec<[u8; 32]>,
    /// ID of the last successfully applied delta (for cascaded validation).
    last_applied_id: Option<[u8; 32]>,
    /// Whether the original base (pre_apply_heads) was deterministic.
    /// Only set true for linear (single head) or clean merge bases.
    /// Cascaded validation only allowed when this is true.
    base_is_deterministic: bool,
}

impl ValidationState {
    /// Check if this delta should be validated for root hash mismatch.
    fn should_validate(&self, delta_parents: &[[u8; 32]]) -> bool {
        // Cascaded linear: single parent matches the just-applied delta.
        // Only validate if the original base was deterministic (linear or clean merge).
        if delta_parents.len() == 1 {
            if let Some(last) = self.last_applied_id {
                if delta_parents[0] == last && self.base_is_deterministic {
                    return true;
                }
            }
        }

        // Linear base: single head matches single parent
        if let ([head], [parent]) = (self.pre_apply_heads.as_slice(), delta_parents) {
            if head == parent {
                return true;
            }
        }

        // Clean merge: heads exactly equals parents (order-independent)
        if !self.pre_apply_heads.is_empty() && self.pre_apply_heads.len() == delta_parents.len() {
            let mut heads = self.pre_apply_heads.clone();
            let mut parents = delta_parents.to_vec();
            heads.sort();
            parents.sort();
            if heads == parents {
                return true;
            }
        }

        false
    }
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

        debug!(
            context_id = %self.context_id,
            delta_id = ?delta.id,
            root_hash = ?outcome.root_hash,
            return_registers = ?outcome.returns,
            "WASM sync completed execution"
        );

        if outcome.returns.is_err() {
            return Err(ApplyError::Application(format!(
                "WASM sync returned error: {:?}",
                outcome.returns
            )));
        }

        // Validate root hash only on deterministic bases (linear, cascaded, clean merge).
        // For concurrent-head cases, mismatches are expected and not validated.
        let computed_hash = outcome.root_hash;
        if *computed_hash != delta.expected_root_hash {
            let state = self.validation_state.read().await;
            if state.should_validate(&delta.parents) {
                warn!(
                    context_id = %self.context_id,
                    delta_id = ?delta.id,
                    computed_hash = ?computed_hash,
                    expected_hash = ?Hash::from(delta.expected_root_hash),
                    "Root hash mismatch - possible non-determinism or state divergence"
                );
            } else {
                debug!(
                    context_id = %self.context_id,
                    delta_id = ?delta.id,
                    computed_hash = ?computed_hash,
                    expected_hash = ?Hash::from(delta.expected_root_hash),
                    "Root hash differs (concurrent heads - not validating)"
                );
            }
        }

        // Track last applied for cascaded validation
        self.validation_state.write().await.last_applied_id = Some(delta.id);

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
            validation_state: RwLock::new(ValidationState::default()),
        });

        Self {
            dag: Arc::new(RwLock::new(CoreDagStore::new(root))),
            applier,
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

        // Snapshot DAG heads before applying for conditional root hash validation.
        {
            let mut state = self.applier.validation_state.write().await;
            let heads = dag.get_heads();

            // Check if this delta's application is on a deterministic base:
            // - Linear: single head that matches the delta's single parent
            // - Clean merge: delta parents exactly match all current heads
            let is_linear = heads.len() == 1 && delta.parents.len() == 1 && heads[0] == delta.parents[0];
            let is_clean_merge = if !heads.is_empty() && heads.len() == delta.parents.len() {
                let mut sorted_heads = heads.clone();
                let mut sorted_parents = delta.parents.clone();
                sorted_heads.sort();
                sorted_parents.sort();
                sorted_heads == sorted_parents
            } else {
                false
            };

            state.pre_apply_heads = heads;
            state.last_applied_id = None;
            state.base_is_deterministic = is_linear || is_clean_merge;
        }

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
            .update_dag_heads(&self.applier.context_id, heads)
            .map_err(|e| eyre::eyre!("Failed to update dag_heads: {}", e))?;

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

                    // Snapshot heads for validation
                    {
                        let mut state = self.applier.validation_state.write().await;
                        let heads = dag.get_heads();

                        // Check if this delta's application is on a deterministic base
                        let is_linear = heads.len() == 1
                            && dag_delta.parents.len() == 1
                            && heads[0] == dag_delta.parents[0];
                        let is_clean_merge =
                            if !heads.is_empty() && heads.len() == dag_delta.parents.len() {
                                let mut sorted_heads = heads.clone();
                                let mut sorted_parents = dag_delta.parents.clone();
                                sorted_heads.sort();
                                sorted_parents.sort();
                                sorted_heads == sorted_parents
                            } else {
                                false
                            };

                        state.pre_apply_heads = heads;
                        state.last_applied_id = None;
                        state.base_is_deterministic = is_linear || is_clean_merge;
                    }

                    let pending_before: std::collections::HashSet<[u8; 32]> =
                        dag.get_pending_delta_ids().into_iter().collect();

                    if let Err(e) = dag.add_delta(dag_delta.clone(), &*self.applier).await {
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
}
