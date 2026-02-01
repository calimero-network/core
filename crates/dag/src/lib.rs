//! DAG (Directed Acyclic Graph) for causal delta tracking
//!
//! This crate provides a pure DAG implementation for managing causal relationships
//! between deltas. It's independent of storage and network layers, making it
//! easy to test and reuse.
//!
//! ## Core Concepts
//!
//! - **CausalDelta**: A delta with parent references (content-addressed)
//! - **DagStore**: Manages DAG topology and applies deltas in topological order
//! - **DeltaApplier**: Trait for applying deltas (dependency injection)

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

/// Maximum number of items returned by query methods to prevent resource exhaustion.
/// Even if a caller requests more, the DAG will cap the result at this size.
/// The value selected as ~96 KB.
pub const MAX_DELTA_QUERY_LIMIT: usize = 3000;

/// Type of delta - regular operation or checkpoint (snapshot boundary)
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum DeltaKind {
    /// Regular delta with operations to apply
    Regular,
    /// Checkpoint delta representing a snapshot boundary
    ///
    /// Checkpoints are created after snapshot sync to mark a known-good state.
    /// They have no payload to apply but provide parent IDs for future deltas.
    ///
    /// # Properties
    /// - `payload` is empty (no operations)
    /// - `expected_root_hash` is the snapshot's root hash
    /// - Treated as "already applied" by the DAG
    Checkpoint,
}

impl Default for DeltaKind {
    fn default() -> Self {
        Self::Regular
    }
}

/// A causal delta with parent references
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct CausalDelta<T> {
    /// Unique delta ID (content hash)
    pub id: [u8; 32],

    /// Parent delta IDs (for causal ordering)
    pub parents: Vec<[u8; 32]>,

    /// The actual delta payload (generic)
    pub payload: T,

    /// Hybrid Logical Clock timestamp for fine-grained causal ordering
    pub hlc: calimero_storage::logical_clock::HybridTimestamp,

    /// Expected root hash after applying this delta
    pub expected_root_hash: [u8; 32],

    /// Kind of delta (regular or checkpoint)
    #[serde(default)]
    pub kind: DeltaKind,
}

impl<T> CausalDelta<T> {
    pub fn new(
        id: [u8; 32],
        parents: Vec<[u8; 32]>,
        payload: T,
        hlc: calimero_storage::logical_clock::HybridTimestamp,
        expected_root_hash: [u8; 32],
    ) -> Self {
        Self {
            id,
            parents,
            payload,
            hlc,
            expected_root_hash,
            kind: DeltaKind::Regular,
        }
    }

    /// Create a checkpoint delta for snapshot boundary
    ///
    /// Checkpoints mark the boundary after a snapshot sync. They have:
    /// - The DAG head IDs from the snapshot as their ID
    /// - Genesis as parent (since we don't know actual history)
    /// - Empty payload (no operations to apply)
    /// - The snapshot's root hash as expected_root_hash
    pub fn checkpoint(id: [u8; 32], expected_root_hash: [u8; 32]) -> Self
    where
        T: Default,
    {
        Self {
            id,
            parents: vec![[0; 32]], // Genesis parent
            payload: T::default(),  // Empty payload
            hlc: calimero_storage::logical_clock::HybridTimestamp::default(),
            expected_root_hash,
            kind: DeltaKind::Checkpoint,
        }
    }

    /// Returns true if this is a checkpoint (snapshot boundary) delta
    pub fn is_checkpoint(&self) -> bool {
        self.kind == DeltaKind::Checkpoint
    }

    /// Convenience constructor for tests that uses a default HLC
    #[cfg(any(test, feature = "testing"))]
    pub fn new_test(id: [u8; 32], parents: Vec<[u8; 32]>, payload: T) -> Self {
        Self {
            id,
            parents,
            payload,
            hlc: calimero_storage::logical_clock::HybridTimestamp::default(),
            expected_root_hash: [0; 32],
            kind: DeltaKind::Regular,
        }
    }
}

/// Trait for applying deltas to underlying storage
///
/// The DAG doesn't know how to apply deltas - it delegates to this trait.
/// This allows testing with mock appliers and using real storage in production.
#[async_trait::async_trait]
pub trait DeltaApplier<T> {
    async fn apply(&self, delta: &CausalDelta<T>) -> Result<(), ApplyError>;
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ApplyError {
    #[error("Failed to apply delta: {0}")]
    Application(String),

    /// Root hash mismatch - delta was based on different state
    ///
    /// This happens when concurrent updates create divergent histories.
    /// The caller should trigger a proper state sync/merge instead of
    /// blindly applying the delta.
    #[error("Root hash mismatch: computed {computed:?}, expected {expected:?}")]
    RootHashMismatch {
        /// Hash computed after applying delta to current state
        computed: [u8; 32],
        /// Hash the delta author expected (based on their state)
        expected: [u8; 32],
    },
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DagError {
    #[error("Delta already exists: {0:?}")]
    DuplicateDelta([u8; 32]),

    #[error("Failed to apply delta: {0}")]
    ApplyFailed(#[from] ApplyError),
}

/// Tracks a pending delta with timeout metadata
#[derive(Debug, Clone)]
struct PendingDelta<T> {
    delta: CausalDelta<T>,
    received_at: Instant,
}

impl<T> PendingDelta<T> {
    fn new(delta: CausalDelta<T>) -> Self {
        Self {
            delta,
            received_at: Instant::now(),
        }
    }

    fn age(&self) -> Duration {
        self.received_at.elapsed()
    }
}

/// Statistics about the DAG store
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DagStats {
    pub total_deltas: usize,
    pub applied_deltas: usize,
    pub pending_deltas: usize,
    pub head_count: usize,
}

/// Statistics about pending deltas
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingStats {
    pub count: usize,
    pub oldest_age_secs: u64,
    pub total_missing_parents: usize,
}

/// DAG-based delta store
///
/// Manages causal deltas in a DAG structure, applying them in topological order.
/// Generic over the delta payload type.
#[derive(Debug)]
pub struct DagStore<T> {
    /// All deltas we've seen
    deltas: HashMap<[u8; 32], CausalDelta<T>>,

    /// Deltas we've successfully applied
    applied: HashSet<[u8; 32]>,

    /// Deltas waiting for parents
    pending: HashMap<[u8; 32], PendingDelta<T>>,

    /// Current heads (deltas with no children yet)
    heads: HashSet<[u8; 32]>,

    /// Root delta (genesis)
    #[allow(dead_code)]
    root: [u8; 32],

    /// Maximum number of items returned by query methods to prevent resource exhaustion.
    /// Even if a caller requests more, the DAG will cap the result at this size.
    /// By default, equal to `MAX_DELTA_QUERY_SIZE`.
    delta_query_limit: usize,
}

impl<T: Clone> DagStore<T> {
    /// Creates a new DAG store with the given root
    pub fn new(root: [u8; 32]) -> Self {
        let mut applied = HashSet::new();
        applied.insert(root);

        let mut heads = HashSet::new();
        heads.insert(root);

        Self {
            deltas: HashMap::new(),
            applied,
            pending: HashMap::new(),
            heads,
            root,
            delta_query_limit: MAX_DELTA_QUERY_LIMIT,
        }
    }

    /// Test-only ctor for more convenient testing of delta query limits.
    #[cfg(any(test, feature = "testing"))]
    pub fn new_with_delta_query_limit(root: [u8; 32], delta_query_limit: usize) -> Self {
        let mut dag = Self::new(root);
        dag.set_delta_query_limit(delta_query_limit);
        dag
    }

    /// Sets the new delta query limit to be used by query methods.
    pub fn set_delta_query_limit(&mut self, delta_query_limit: usize) {
        info!(
            %delta_query_limit,
            old_delta_query_limit = %self.delta_query_limit,
            "Updated DAG Delta query limit"
        );

        self.delta_query_limit = delta_query_limit;
    }

    /// Restore an already-applied delta from persistent storage
    ///
    /// This adds the delta to the DAG topology WITHOUT applying it again.
    /// Use this when loading deltas from the database that were previously applied.
    ///
    /// Returns true if the delta was restored, false if it was already in the DAG.
    pub fn restore_applied_delta(&mut self, delta: CausalDelta<T>) -> bool {
        let delta_id = delta.id;

        // Skip if already seen
        if self.deltas.contains_key(&delta_id) {
            return false;
        }

        // Store delta in memory
        self.deltas.insert(delta_id, delta.clone());

        // Mark as applied (it was already applied when it was persisted)
        self.applied.insert(delta_id);

        // Update heads: remove parents (if they're heads), add this delta
        for parent in &delta.parents {
            self.heads.remove(parent);
        }
        self.heads.insert(delta_id);

        true
    }

    /// Add a delta to the DAG
    ///
    /// Returns:
    /// - `Ok(true)` if applied immediately
    /// - `Ok(false)` if pending (waiting for parents)
    /// - `Err(DagError)` if delta already exists or application fails
    pub async fn add_delta(
        &mut self,
        delta: CausalDelta<T>,
        applier: &impl DeltaApplier<T>,
    ) -> Result<bool, DagError> {
        let delta_id = delta.id;

        // Skip if already seen
        if self.deltas.contains_key(&delta_id) {
            return Ok(false); // Silently skip duplicates
        }

        // Store delta in memory
        self.deltas.insert(delta_id, delta.clone());

        // Check if we can apply immediately
        if self.can_apply(&delta) {
            self.apply_delta(delta, applier).await?;
            Ok(true)
        } else {
            // Missing parents - store as pending
            self.pending.insert(delta_id, PendingDelta::new(delta));
            Ok(false)
        }
    }

    /// Check if a delta can be applied
    ///
    /// Returns true if all parent deltas have been applied and exist in the DAG.
    /// This ensures topological ordering and prevents phantom references.
    fn can_apply(&self, delta: &CausalDelta<T>) -> bool {
        delta.parents.iter().all(|p| {
            // Genesis (zero hash) is always considered applied
            if *p == [0; 32] {
                return true;
            }

            // Parent must be both applied and exist in the DAG
            self.applied.contains(p) && self.deltas.contains_key(p)
        })
    }

    /// Apply a delta using the provided applier
    fn apply_delta<'a>(
        &'a mut self,
        delta: CausalDelta<T>,
        applier: &'a impl DeltaApplier<T>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), DagError>> + 'a>> {
        Box::pin(async move {
            // Apply via the applier
            applier.apply(&delta).await?;

            // Mark as applied
            self.applied.insert(delta.id);

            // Update heads: remove parents, add this delta
            for parent in &delta.parents {
                self.heads.remove(parent);
            }
            self.heads.insert(delta.id);

            // Try to apply pending deltas
            self.apply_pending(applier).await?;

            Ok(())
        })
    }

    /// Apply pending deltas whose parents are now available
    async fn apply_pending(&mut self, applier: &impl DeltaApplier<T>) -> Result<(), DagError> {
        let mut applied_any = true;

        while applied_any {
            applied_any = false;

            // Find deltas that are now ready
            let ready: Vec<[u8; 32]> = self
                .pending
                .iter()
                .filter(|(_, pending)| self.can_apply(&pending.delta))
                .map(|(id, _)| *id)
                .collect();

            for id in ready {
                if let Some(pending) = self.pending.remove(&id) {
                    self.apply_delta(pending.delta, applier).await?;
                    applied_any = true;
                }
            }
        }

        Ok(())
    }

    /// Get missing parent IDs (parents that are not in the DAG at all)
    ///
    /// Returns parents that aren't in the DAG yet and need to be fetched from peers.
    ///
    /// Parents that are already in the DAG but still pending will cascade and apply
    /// automatically when their own missing parents arrive.
    ///
    /// Note: With proper eviction (removing from both pending AND deltas), stale
    /// pending deltas are fully removed, allowing them to be re-fetched in future syncs.
    ///
    /// # Arguments
    /// * `limit` - maximum number of deltas to return. Capped at `dag.delta_query_limit`.
    ///
    /// # Returns
    ///
    /// * Vec of causal deltas that have the following `ancestor`.
    ///   NOTE: if the client requested more than a `limit` - the node will return only that
    ///   amount, without raising an error. Only a warning log will be produced.
    ///   In future, we might change it, if needed, but for now looks like a clean behaviour, the
    ///   client should be responsible for the pagination himself.
    pub fn get_missing_parents(&self, query_limit: usize) -> Vec<[u8; 32]> {
        // Enforce hard cap on the delta query limit
        let delta_query_limit = std::cmp::min(query_limit, self.delta_query_limit);

        let mut missing_ids = HashSet::new();

        'outer: for (_pending_id, pending) in &self.pending {
            for parent in &pending.delta.parents {
                if missing_ids.len() >= delta_query_limit {
                    warn!(
                        %query_limit,
                        max_query_limit = %self.delta_query_limit,
                        "The requested amount of deltas for missing parents reached limit, only limited amount of deltas returned"
                    );
                    break 'outer;
                }

                // Skip genesis
                if *parent == [0; 32] {
                    continue;
                }

                // Only return parents that aren't in the DAG at all
                // Parents that are in the DAG but pending will cascade when ready
                if !self.deltas.contains_key(parent) {
                    missing_ids.insert(*parent);
                }
            }
        }

        missing_ids.into_iter().collect()
    }

    /// Get IDs of deltas that are currently pending (not yet applied)
    pub fn get_pending_delta_ids(&self) -> Vec<[u8; 32]> {
        self.pending.keys().copied().collect()
    }

    /// Cleanup stale pending deltas (timeout eviction)
    ///
    /// Removes pending deltas older than max_age from both pending map AND deltas map.
    /// This allows them to be re-fetched in future syncs instead of being stuck as
    /// zombie deltas (in deltas but not in pending or applied).
    pub fn cleanup_stale(&mut self, max_age: Duration) -> usize {
        let initial_count = self.pending.len();

        // Collect IDs to evict
        let to_evict: Vec<[u8; 32]> = self
            .pending
            .iter()
            .filter(|(_id, pending)| pending.age() > max_age)
            .map(|(id, _)| *id)
            .collect();

        // Remove from both pending AND deltas maps
        for id in &to_evict {
            self.pending.remove(id);
            self.deltas.remove(id);
        }

        to_evict.len()
    }

    /// Get statistics for pending deltas
    pub fn pending_stats(&self) -> PendingStats {
        if self.pending.is_empty() {
            return PendingStats::default();
        }

        let oldest_age = self
            .pending
            .values()
            .map(|p| p.age())
            .max()
            .unwrap_or(Duration::ZERO);

        let total_missing: usize = self
            .pending
            .values()
            .map(|p| {
                p.delta
                    .parents
                    .iter()
                    .filter(|&parent| !self.applied.contains(parent))
                    .count()
            })
            .sum();

        PendingStats {
            count: self.pending.len(),
            oldest_age_secs: oldest_age.as_secs(),
            total_missing_parents: total_missing,
        }
    }

    /// Get current heads (for creating new deltas)
    pub fn get_heads(&self) -> Vec<[u8; 32]> {
        self.heads.iter().copied().collect()
    }

    /// Get all deltas since a common ancestor (for sync)
    ///
    /// # Arguments
    /// * `ancestor` - the ID of the delta to stop at.
    /// * `start_ids` - Optional list of IDs to start traversal from. If `None`, starts from current DAG heads.
    /// * `limit` - maximum number of deltas to return. Capped at `dag.delta_query_limit`.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// 1. The list of deltas found (`Vec<CausalDelta>`).
    /// 2. A "cursor" (`Vec<[u8; 32]>`) containing the next IDs to fetch. Client can pass this to `start_id` in
    ///   the next call to resume the process. If the cursor is empty, the traversal is complete.
    ///
    ///   NOTE: if the client requested more than a `limit` - the node will return only that
    ///   amount, without raising an error. Only a warning log will be produced.
    ///   In future, we might change it, if needed, but for now looks like a clean behaviour, the
    ///   client should be responsible for the pagination himself.
    pub fn get_deltas_since(
        &self,
        ancestor: [u8; 32],
        start_ids: Option<Vec<[u8; 32]>>,
        query_limit: usize,
    ) -> (Vec<CausalDelta<T>>, Vec<[u8; 32]>) {
        // Enforce hard cap on the delta query limit
        let delta_query_limit = std::cmp::min(query_limit, self.delta_query_limit);

        let mut result = Vec::new();
        let mut visited = HashSet::new();

        // Initialize queue: use provided 'start_ids' cursor or default to all current heads
        let mut queue = if let Some(start_ids) = start_ids {
            VecDeque::from(start_ids)
        } else {
            VecDeque::from_iter(self.heads.iter().copied())
        };

        while let Some(id) = queue.pop_front() {
            if result.len() >= delta_query_limit {
                warn!(
                    %query_limit,
                    max_query_limit = %self.delta_query_limit,
                    ?ancestor,
                    "The requested amount of deltas for ancestor reached limit, only limited amount of deltas returned"
                );

                // Push the unprocessed ID back to the front.
                // The current state of the queue represents the cursor for the next page.
                queue.push_front(id);
                break;
            }

            if visited.contains(&id) || id == ancestor {
                continue;
            }

            visited.insert(id);

            if let Some(delta) = self.deltas.get(&id) {
                result.push(delta.clone());

                for parent in &delta.parents {
                    // Only queue parents if we haven't visited them and they aren't the stop-ancestor.
                    // This is required to eliminate diamond dependencies in a DAG, when different branches can
                    // merge back into a common ancestor.
                    if !visited.contains(parent) && *parent != ancestor {
                        queue.push_back(*parent);
                    }
                }
            }
        }

        // The remaining items in the queue represent the cursor for the next page.
        // We deduplicate them to keep the cursor clean and efficient.
        let cursor: Vec<[u8; 32]> = queue
            .into_iter()
            // Deduplicate items using HashSet
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        (result, cursor)
    }

    /// Check if we have a specific delta
    pub fn has_delta(&self, id: &[u8; 32]) -> bool {
        self.deltas.contains_key(id)
    }

    /// Check if a specific delta has been applied (not just exists)
    pub fn is_applied(&self, id: &[u8; 32]) -> bool {
        self.applied.contains(id)
    }

    /// Get a delta by ID
    pub fn get_delta(&self, id: &[u8; 32]) -> Option<&CausalDelta<T>> {
        self.deltas.get(id)
    }

    /// Get all applied delta IDs
    ///
    /// Returns all delta IDs that have been successfully applied.
    /// Used by bloom filter sync to build a filter of known deltas.
    pub fn get_applied_delta_ids(&self) -> Vec<[u8; 32]> {
        self.applied.iter().copied().collect()
    }

    /// FNV-1a hash for bloom filter bit position calculation.
    ///
    /// CRITICAL: This MUST match `DeltaIdBloomFilter::hash` in sync_protocol.rs
    /// to ensure bloom filter checks work correctly.
    fn bloom_hash(data: &[u8; 32], seed: u8) -> usize {
        let mut hash: u64 = 0xcbf29ce484222325_u64; // FNV offset basis
        hash = hash.wrapping_add(u64::from(seed));
        for byte in data {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3); // FNV prime
        }
        hash as usize
    }

    /// Get deltas that the remote doesn't have based on a bloom filter
    ///
    /// Checks each of our applied deltas against the bloom filter.
    /// Returns deltas that are NOT in the filter (remote is missing them).
    pub fn get_deltas_not_in_bloom(
        &self,
        bloom_filter: &[u8],
        _false_positive_rate: f32, // Note: Currently unused, kept for API compatibility
    ) -> Vec<CausalDelta<T>> {
        if bloom_filter.len() < 5 {
            // Invalid filter, return all deltas
            return self
                .applied
                .iter()
                .filter_map(|id| self.deltas.get(id).cloned())
                .collect();
        }

        // Parse bloom filter metadata
        let num_bits = u32::from_le_bytes([
            bloom_filter[0],
            bloom_filter[1],
            bloom_filter[2],
            bloom_filter[3],
        ]) as usize;

        // SECURITY: Prevent division by zero from malformed bloom filter
        if num_bits == 0 {
            tracing::warn!("Malformed bloom filter: num_bits is 0, returning all deltas");
            return self
                .applied
                .iter()
                .filter_map(|id| self.deltas.get(id).cloned())
                .collect();
        }

        let num_hashes = bloom_filter[4] as usize;
        let bits = &bloom_filter[5..];

        let mut missing = Vec::new();

        for delta_id in &self.applied {
            // Check if delta_id is in bloom filter
            // CRITICAL: Must use same hash function as DeltaIdBloomFilter::hash (FNV-1a)
            // Previous bug: was using DefaultHasher (SipHash) which produced different bit positions
            let mut in_filter = true;
            for i in 0..num_hashes {
                let bit_index = Self::bloom_hash(delta_id, i as u8) % num_bits;

                if bit_index / 8 >= bits.len()
                    || (bits[bit_index / 8] & (1 << (bit_index % 8))) == 0
                {
                    in_filter = false;
                    break;
                }
            }

            if !in_filter {
                // Remote doesn't have this delta
                if let Some(delta) = self.deltas.get(delta_id) {
                    missing.push(delta.clone());
                }
            }
        }

        missing
    }

    /// Get statistics
    pub fn stats(&self) -> DagStats {
        DagStats {
            total_deltas: self.deltas.len(),
            applied_deltas: self.applied.len(),
            pending_deltas: self.pending.len(),
            head_count: self.heads.len(),
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod basic_tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;

    #[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
    struct TestPayload {
        value: u32,
    }

    struct TestApplier {
        applied: Arc<Mutex<Vec<[u8; 32]>>>,
    }

    #[async_trait::async_trait]
    impl DeltaApplier<TestPayload> for TestApplier {
        async fn apply(&self, delta: &CausalDelta<TestPayload>) -> Result<(), ApplyError> {
            self.applied.lock().await.push(delta.id);
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_dag_linear_sequence() {
        let applier = TestApplier {
            applied: Arc::new(Mutex::new(Vec::new())),
        };

        let mut dag = DagStore::new([0; 32]);

        // Create linear chain: root -> delta1 -> delta2
        let delta1 = CausalDelta::new_test(
            [1; 32],
            vec![[0; 32]], // parent: root
            TestPayload { value: 1 },
        );

        let delta2 = CausalDelta::new_test(
            [2; 32],
            vec![[1; 32]], // parent: delta1
            TestPayload { value: 2 },
        );

        // Apply in order
        let applied1 = dag.add_delta(delta1, &applier).await.unwrap();
        assert!(applied1, "Delta1 should be applied immediately");

        let applied2 = dag.add_delta(delta2, &applier).await.unwrap();
        assert!(applied2, "Delta2 should be applied immediately");

        // Check heads
        let heads = dag.get_heads();
        assert_eq!(heads, vec![[2; 32]], "Head should be delta2");

        // Check applier received both
        let applied = applier.applied.lock().await;
        assert_eq!(applied.len(), 2);
        assert_eq!(applied[0], [1; 32]);
        assert_eq!(applied[1], [2; 32]);
    }

    #[tokio::test]
    async fn test_dag_out_of_order() {
        let applier = TestApplier {
            applied: Arc::new(Mutex::new(Vec::new())),
        };

        let mut dag = DagStore::new([0; 32]);

        // Create chain but receive out of order
        let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });

        let delta2 = CausalDelta::new_test(
            [2; 32],
            vec![[1; 32]], // depends on delta1
            TestPayload { value: 2 },
        );

        // Receive delta2 first (out of order)
        let applied2_first = dag.add_delta(delta2.clone(), &applier).await.unwrap();
        assert!(!applied2_first, "Delta2 should be pending (missing parent)");

        // Check pending
        assert_eq!(dag.pending_stats().count, 1);
        assert_eq!(
            dag.get_missing_parents(MAX_DELTA_QUERY_LIMIT),
            vec![[1; 32]]
        );

        // Now receive delta1
        let applied1 = dag.add_delta(delta1, &applier).await.unwrap();
        assert!(applied1, "Delta1 should be applied immediately");

        // Delta2 should now be applied automatically
        let applied = applier.applied.lock().await;
        assert_eq!(applied.len(), 2);
        assert_eq!(applied[0], [1; 32]); // delta1
        assert_eq!(applied[1], [2; 32]); // delta2 (auto-applied)

        // No more pending
        assert_eq!(dag.pending_stats().count, 0);
    }

    #[tokio::test]
    async fn test_dag_concurrent_updates() {
        let applier = TestApplier {
            applied: Arc::new(Mutex::new(Vec::new())),
        };

        let mut dag = DagStore::new([0; 32]);

        // Two nodes create concurrent deltas from same parent
        let delta_a = CausalDelta::new_test(
            [10; 32],
            vec![[0; 32]], // both from root
            TestPayload { value: 10 },
        );

        let delta_b = CausalDelta::new_test(
            [20; 32],
            vec![[0; 32]], // both from root
            TestPayload { value: 20 },
        );

        // Apply both
        dag.add_delta(delta_a, &applier).await.unwrap();
        dag.add_delta(delta_b, &applier).await.unwrap();

        // Should have TWO heads (concurrent updates)
        let mut heads = dag.get_heads();
        heads.sort();
        assert_eq!(heads.len(), 2);
        assert!(heads.contains(&[10; 32]));
        assert!(heads.contains(&[20; 32]));

        // Merge delta
        let delta_merge = CausalDelta::new_test(
            [30; 32],
            vec![[10; 32], [20; 32]], // merge both
            TestPayload { value: 30 },
        );

        dag.add_delta(delta_merge, &applier).await.unwrap();

        // Now should have ONE head (merged)
        let heads = dag.get_heads();
        assert_eq!(heads, vec![[30; 32]]);
    }

    #[tokio::test]
    async fn test_dag_cleanup_stale() {
        let applier = TestApplier {
            applied: Arc::new(Mutex::new(Vec::new())),
        };

        let mut dag = DagStore::new([0; 32]);

        // Add a pending delta
        let delta_pending = CausalDelta::new_test(
            [99; 32],
            vec![[88; 32]], // missing parent
            TestPayload { value: 99 },
        );

        dag.add_delta(delta_pending, &applier).await.unwrap();
        assert_eq!(dag.pending_stats().count, 1);

        // Wait a bit
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Cleanup with very short timeout
        let evicted = dag.cleanup_stale(Duration::from_millis(50));
        assert_eq!(evicted, 1, "Should evict the stale delta");
        assert_eq!(dag.pending_stats().count, 0);
    }
}
