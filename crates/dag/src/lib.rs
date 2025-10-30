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
        }
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
        }
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

    /// Check if a delta can be applied (all parents applied)
    fn can_apply(&self, delta: &CausalDelta<T>) -> bool {
        delta.parents.iter().all(|p| self.applied.contains(p))
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

    /// Get missing parent IDs
    pub fn get_missing_parents(&self) -> Vec<[u8; 32]> {
        let mut missing = HashSet::new();

        for pending in self.pending.values() {
            for parent in &pending.delta.parents {
                if !self.deltas.contains_key(parent) {
                    missing.insert(*parent);
                }
            }
        }

        missing.into_iter().collect()
    }

    /// Cleanup stale pending deltas (timeout eviction)
    pub fn cleanup_stale(&mut self, max_age: Duration) -> usize {
        let initial_count = self.pending.len();

        self.pending.retain(|_id, pending| pending.age() <= max_age);

        initial_count - self.pending.len()
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
    pub fn get_deltas_since(&self, ancestor: [u8; 32]) -> Vec<CausalDelta<T>> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::from_iter(self.heads.iter().copied());

        while let Some(id) = queue.pop_front() {
            if visited.contains(&id) || id == ancestor {
                continue;
            }

            visited.insert(id);

            if let Some(delta) = self.deltas.get(&id) {
                result.push(delta.clone());

                for parent in &delta.parents {
                    queue.push_back(*parent);
                }
            }
        }

        result
    }

    /// Check if we have a specific delta
    pub fn has_delta(&self, id: &[u8; 32]) -> bool {
        self.deltas.contains_key(id)
    }

    /// Get a delta by ID
    pub fn get_delta(&self, id: &[u8; 32]) -> Option<&CausalDelta<T>> {
        self.deltas.get(id)
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
        assert_eq!(dag.get_missing_parents(), vec![[1; 32]]);

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
