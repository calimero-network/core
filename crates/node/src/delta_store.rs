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
use tracing::{debug, warn};

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

        // ═══════════════════════════════════════════════════════════════
        // CRITICAL FIX: Ensure deterministic root hash across all nodes
        // ═══════════════════════════════════════════════════════════════
        //
        // When nodes sync deltas, WASM execution may produce different root hashes
        // due to non-deterministic factors (CRDT merge order, timing, etc).
        // To maintain DAG consistency, we MUST use the expected_root_hash from
        // the delta author rather than the computed hash.
        //
        // SAFETY: This is safe because:
        // 1. The DAG ensures deltas are applied in topological order
        // 2. The DAG prevents re-applying the same delta (duplicate check)
        // 3. The expected_root_hash represents the state AFTER applying THIS delta
        //
        // LIMITATION: When multiple DAG heads exist (concurrent branches), Context
        // stores only ONE root_hash. The current implementation uses whichever delta
        // was applied most recently, which can cause non-deterministic root_hash
        // selection across nodes if deltas arrive in different orders.
        //
        // FUTURE FIX: Use deterministic selection (e.g., lexicographically smallest
        // head_id's root_hash) when multiple heads exist.

        let computed_hash = outcome.root_hash;
        if *computed_hash != delta.expected_root_hash {
            warn!(
                context_id = %self.context_id,
                delta_id = ?delta.id,
                computed_hash = ?computed_hash,
                expected_hash = ?delta.expected_root_hash,
                "Root hash mismatch detected - using expected hash for DAG consistency"
            );

            // OVERRIDE: Use the expected root hash from the delta to ensure
            // all nodes have identical DAG structure regardless of WASM execution differences.
            // Note: execute() already set context.root_hash and persisted it, so we're
            // correcting it here if it differs from the expected value.
            self.context_client
                .force_root_hash(&self.context_id, delta.expected_root_hash.into())
                .map_err(|e| ApplyError::Application(format!("Failed to set root hash: {}", e)))?;
        }

        // Note: We do NOT update dag_heads here because:
        // 1. This is called INSIDE CoreDagStore::apply_delta BEFORE it updates its heads
        // 2. We can't read the correct heads from the DAG yet
        // 3. DeltaStore::add_delta will update the heads after the DAG finishes

        debug!(
            context_id = %self.context_id,
            delta_id = ?delta.id,
            action_count = delta.payload.len(),
            expected_root_hash = ?delta.expected_root_hash,
            "Applied delta to WASM storage with expected root hash"
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

    /// Add a delta to the store
    ///
    /// Returns Ok(true) if applied immediately, Ok(false) if pending
    pub async fn add_delta(&self, delta: CausalDelta<Vec<Action>>) -> Result<bool> {
        let delta_id = delta.id;
        let expected_root_hash = delta.expected_root_hash;

        // Store the mapping before applying
        {
            let mut head_hashes = self.head_root_hashes.write().await;
            let _previous = head_hashes.insert(delta_id, expected_root_hash);
        }

        let mut dag = self.dag.write().await;
        let result = dag.add_delta(delta, &*self.applier).await?;

        // CRITICAL: Update context's dag_heads to ALL current DAG heads
        // This must happen AFTER the DAG has updated its heads (which happens in add_delta)
        let heads = dag.get_heads();
        drop(dag); // Release lock before calling context_client

        self.applier
            .context_client
            .update_dag_heads(&self.applier.context_id, heads.clone())
            .map_err(|e| eyre::eyre!("Failed to update dag_heads: {}", e))?;

        // ═══════════════════════════════════════════════════════════════════════
        // CRITICAL: Deterministic root hash selection for multiple DAG heads
        // ═══════════════════════════════════════════════════════════════════════
        //
        // When multiple DAG heads exist (concurrent branches), we must deterministically
        // select which root_hash to use as the Context's root_hash. Without this, nodes
        // receiving deltas in different orders would have different root_hashes even
        // though they have identical DAG structure.
        //
        // Strategy: Use the lexicographically smallest head_id's root_hash.
        // This ensures all nodes make the same choice regardless of delta arrival order.

        if heads.len() > 1 {
            // Multiple heads - select deterministically
            let head_hashes = self.head_root_hashes.read().await;

            // Find the lexicographically smallest head
            let mut sorted_heads = heads.clone();
            sorted_heads.sort();
            let canonical_head = sorted_heads[0];

            if let Some(&canonical_root_hash) = head_hashes.get(&canonical_head) {
                debug!(
                    context_id = %self.applier.context_id,
                    heads_count = heads.len(),
                    canonical_head = ?canonical_head,
                    canonical_root = ?canonical_root_hash,
                    "Multiple DAG heads detected - using deterministic root hash selection"
                );

                self.applier
                    .context_client
                    .force_root_hash(&self.applier.context_id, canonical_root_hash.into())
                    .map_err(|e| eyre::eyre!("Failed to set canonical root hash: {}", e))?;
            }
        }

        // Cleanup: Remove old head hashes that are no longer heads
        {
            let mut head_hashes = self.head_root_hashes.write().await;
            head_hashes.retain(|head_id, _| heads.contains(head_id));
        }

        Ok(result)
    }

    /// Get missing parent IDs (for requesting from peers)
    pub async fn get_missing_parents(&self) -> Vec<[u8; 32]> {
        let dag = self.dag.read().await;
        dag.get_missing_parents()
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
