//! Comprehensive unit tests for DAG functionality
//!
//! Tests cover:
//! - Linear sequences
//! - Out-of-order delivery
//! - Concurrent updates
//! - Multi-way merges
//! - Error handling
//! - Edge cases
//! - Stress testing

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use super::*;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
struct TestPayload {
    value: u32,
}

struct TestApplier {
    applied: Arc<Mutex<Vec<[u8; 32]>>>,
    should_fail: Arc<Mutex<bool>>,
}

impl TestApplier {
    fn new() -> Self {
        Self {
            applied: Arc::new(Mutex::new(Vec::new())),
            should_fail: Arc::new(Mutex::new(false)),
        }
    }

    fn with_failure() -> Self {
        Self {
            applied: Arc::new(Mutex::new(Vec::new())),
            should_fail: Arc::new(Mutex::new(true)),
        }
    }

    async fn get_applied(&self) -> Vec<[u8; 32]> {
        self.applied.lock().await.clone()
    }

    async fn set_should_fail(&self, value: bool) {
        *self.should_fail.lock().await = value;
    }
}

#[async_trait::async_trait]
impl DeltaApplier<TestPayload> for TestApplier {
    async fn apply(&self, delta: &CausalDelta<TestPayload>) -> Result<(), ApplyError> {
        if *self.should_fail.lock().await {
            return Err(ApplyError::Application("Simulated failure".to_string()));
        }
        self.applied.lock().await.push(delta.id);
        Ok(())
    }
}

// ============================================================
// Basic Functionality Tests
// ============================================================

#[tokio::test]
async fn test_dag_new() {
    let root = [0; 32];
    let dag = DagStore::<TestPayload>::new(root);

    // Root should be in applied set
    let stats = dag.stats();
    assert_eq!(stats.applied_deltas, 1);
    assert_eq!(stats.pending_deltas, 0);
    assert_eq!(stats.head_count, 1);
    assert_eq!(dag.get_heads(), vec![root]);
}

#[tokio::test]
async fn test_dag_linear_sequence() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create linear chain: root -> delta1 -> delta2 -> delta3
    let delta1 = CausalDelta::new([1; 32], vec![[0; 32]], TestPayload { value: 1 }, 1000);

    let delta2 = CausalDelta::new([2; 32], vec![[1; 32]], TestPayload { value: 2 }, 2000);

    let delta3 = CausalDelta::new([3; 32], vec![[2; 32]], TestPayload { value: 3 }, 3000);

    // Apply in order
    let applied1 = dag.add_delta(delta1, &applier).await.unwrap();
    assert!(applied1, "Delta1 should be applied immediately");

    let applied2 = dag.add_delta(delta2, &applier).await.unwrap();
    assert!(applied2, "Delta2 should be applied immediately");

    let applied3 = dag.add_delta(delta3, &applier).await.unwrap();
    assert!(applied3, "Delta3 should be applied immediately");

    // Check heads
    assert_eq!(dag.get_heads(), vec![[3; 32]], "Head should be delta3");

    // Check applier received all deltas in order
    let applied = applier.get_applied().await;
    assert_eq!(applied, vec![[1; 32], [2; 32], [3; 32]]);

    // Check stats
    let stats = dag.stats();
    assert_eq!(stats.total_deltas, 3);
    assert_eq!(stats.applied_deltas, 4); // root + 3 deltas
    assert_eq!(stats.pending_deltas, 0);
}

#[tokio::test]
async fn test_dag_duplicate_delta() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let delta = CausalDelta::new([1; 32], vec![[0; 32]], TestPayload { value: 1 }, 1000);

    // Add delta first time
    let result1 = dag.add_delta(delta.clone(), &applier).await.unwrap();
    assert!(result1, "First add should apply");

    // Add same delta again (should be silently ignored)
    let result2 = dag.add_delta(delta, &applier).await.unwrap();
    assert!(!result2, "Duplicate should be ignored");

    // Only applied once
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0], [1; 32]);
}

// ============================================================
// Out-of-Order Delivery Tests
// ============================================================

#[tokio::test]
async fn test_dag_out_of_order() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let delta1 = CausalDelta::new([1; 32], vec![[0; 32]], TestPayload { value: 1 }, 1000);

    let delta2 = CausalDelta::new([2; 32], vec![[1; 32]], TestPayload { value: 2 }, 2000);

    // Receive delta2 first (out of order)
    let applied2_first = dag.add_delta(delta2.clone(), &applier).await.unwrap();
    assert!(!applied2_first, "Delta2 should be pending");

    // Check pending
    assert_eq!(dag.pending_stats().count, 1);
    assert_eq!(dag.get_missing_parents(), vec![[1; 32]]);

    // No deltas applied yet
    assert_eq!(applier.get_applied().await.len(), 0);

    // Now receive delta1
    let applied1 = dag.add_delta(delta1, &applier).await.unwrap();
    assert!(applied1, "Delta1 should be applied");

    // Both should now be applied
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), 2);
    assert_eq!(applied[0], [1; 32]);
    assert_eq!(applied[1], [2; 32]);

    // No more pending
    assert_eq!(dag.pending_stats().count, 0);
    assert_eq!(dag.get_heads(), vec![[2; 32]]);
}

#[tokio::test]
async fn test_dag_multiple_pending_sequential() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create chain: root -> d1 -> d2 -> d3
    let delta1 = CausalDelta::new([1; 32], vec![[0; 32]], TestPayload { value: 1 }, 1000);
    let delta2 = CausalDelta::new([2; 32], vec![[1; 32]], TestPayload { value: 2 }, 2000);
    let delta3 = CausalDelta::new([3; 32], vec![[2; 32]], TestPayload { value: 3 }, 3000);

    // Receive completely out of order: d3, d2, then d1
    dag.add_delta(delta3.clone(), &applier).await.unwrap();
    dag.add_delta(delta2.clone(), &applier).await.unwrap();

    // Both should be pending
    assert_eq!(dag.pending_stats().count, 2);
    assert_eq!(applier.get_applied().await.len(), 0);

    // Receive delta1 - should trigger cascade
    dag.add_delta(delta1, &applier).await.unwrap();

    // All should be applied in correct order
    let applied = applier.get_applied().await;
    assert_eq!(applied, vec![[1; 32], [2; 32], [3; 32]]);
    assert_eq!(dag.pending_stats().count, 0);
    assert_eq!(dag.get_heads(), vec![[3; 32]]);
}

#[tokio::test]
async fn test_dag_deep_pending_chain() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create a long chain
    let deltas: Vec<_> = (1..=10)
        .map(|i| {
            CausalDelta::new(
                [i; 32],
                vec![[i - 1; 32]],
                TestPayload { value: i as u32 },
                i as u64 * 1000,
            )
        })
        .collect();

    // Add them in reverse order (10, 9, 8, ..., 1)
    for delta in deltas.iter().rev() {
        dag.add_delta(delta.clone(), &applier).await.unwrap();
    }

    // All should be applied in correct order
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), 10);
    for i in 1..=10 {
        assert_eq!(applied[i - 1], [i as u8; 32]);
    }

    assert_eq!(dag.pending_stats().count, 0);
    assert_eq!(dag.get_heads(), vec![[10; 32]]);
}

// ============================================================
// Concurrent Updates & Merges
// ============================================================

#[tokio::test]
async fn test_dag_concurrent_updates() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Two nodes create concurrent deltas from same parent
    let delta_a = CausalDelta::new([10; 32], vec![[0; 32]], TestPayload { value: 10 }, 1000);

    let delta_b = CausalDelta::new([20; 32], vec![[0; 32]], TestPayload { value: 20 }, 1001);

    dag.add_delta(delta_a, &applier).await.unwrap();
    dag.add_delta(delta_b, &applier).await.unwrap();

    // Should have TWO heads (concurrent updates)
    let mut heads = dag.get_heads();
    heads.sort();
    assert_eq!(heads.len(), 2);
    assert!(heads.contains(&[10; 32]));
    assert!(heads.contains(&[20; 32]));

    // Both applied
    assert_eq!(applier.get_applied().await.len(), 2);
}

#[tokio::test]
async fn test_dag_merge_concurrent_branches() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Concurrent branches
    let delta_a = CausalDelta::new([10; 32], vec![[0; 32]], TestPayload { value: 10 }, 1000);
    let delta_b = CausalDelta::new([20; 32], vec![[0; 32]], TestPayload { value: 20 }, 1001);

    dag.add_delta(delta_a, &applier).await.unwrap();
    dag.add_delta(delta_b, &applier).await.unwrap();

    // Two heads
    assert_eq!(dag.get_heads().len(), 2);

    // Merge delta
    let delta_merge = CausalDelta::new(
        [30; 32],
        vec![[10; 32], [20; 32]],
        TestPayload { value: 30 },
        2000,
    );

    dag.add_delta(delta_merge, &applier).await.unwrap();

    // Single head now
    assert_eq!(dag.get_heads(), vec![[30; 32]]);

    // All deltas applied
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), 3);
    assert!(applied.contains(&[10; 32]));
    assert!(applied.contains(&[20; 32]));
    assert!(applied.contains(&[30; 32]));
}

#[tokio::test]
async fn test_dag_three_way_merge() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Three concurrent branches
    let delta_a = CausalDelta::new([1; 32], vec![[0; 32]], TestPayload { value: 1 }, 1000);
    let delta_b = CausalDelta::new([2; 32], vec![[0; 32]], TestPayload { value: 2 }, 1001);
    let delta_c = CausalDelta::new([3; 32], vec![[0; 32]], TestPayload { value: 3 }, 1002);

    dag.add_delta(delta_a, &applier).await.unwrap();
    dag.add_delta(delta_b, &applier).await.unwrap();
    dag.add_delta(delta_c, &applier).await.unwrap();

    // Three heads
    let mut heads = dag.get_heads();
    heads.sort();
    assert_eq!(heads.len(), 3);

    // Three-way merge
    let merge = CausalDelta::new(
        [99; 32],
        vec![[1; 32], [2; 32], [3; 32]],
        TestPayload { value: 99 },
        3000,
    );

    dag.add_delta(merge, &applier).await.unwrap();

    // Single head
    assert_eq!(dag.get_heads(), vec![[99; 32]]);
}

#[tokio::test]
async fn test_dag_complex_topology() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Complex DAG:
    //       0
    //      / \
    //     1   2
    //     |   |
    //     3   4
    //      \ /
    //       5

    let d1 = CausalDelta::new([1; 32], vec![[0; 32]], TestPayload { value: 1 }, 1000);
    let d2 = CausalDelta::new([2; 32], vec![[0; 32]], TestPayload { value: 2 }, 1001);
    let d3 = CausalDelta::new([3; 32], vec![[1; 32]], TestPayload { value: 3 }, 2000);
    let d4 = CausalDelta::new([4; 32], vec![[2; 32]], TestPayload { value: 4 }, 2001);
    let d5 = CausalDelta::new(
        [5; 32],
        vec![[3; 32], [4; 32]],
        TestPayload { value: 5 },
        3000,
    );

    dag.add_delta(d1, &applier).await.unwrap();
    dag.add_delta(d2, &applier).await.unwrap();
    dag.add_delta(d3, &applier).await.unwrap();
    dag.add_delta(d4, &applier).await.unwrap();
    dag.add_delta(d5, &applier).await.unwrap();

    assert_eq!(dag.get_heads(), vec![[5; 32]]);
    assert_eq!(dag.stats().applied_deltas, 6); // root + 5 deltas
}

// ============================================================
// Error Handling Tests
// ============================================================

#[tokio::test]
async fn test_dag_apply_failure() {
    let applier = TestApplier::with_failure();
    let mut dag = DagStore::new([0; 32]);

    let delta = CausalDelta::new([1; 32], vec![[0; 32]], TestPayload { value: 1 }, 1000);

    // Should fail due to applier
    let result = dag.add_delta(delta, &applier).await;
    assert!(result.is_err());

    // Delta should not be in applied set
    let stats = dag.stats();
    assert_eq!(stats.applied_deltas, 1); // Only root
    assert_eq!(stats.total_deltas, 1); // Delta was stored
}

#[tokio::test]
async fn test_dag_apply_failure_recovery() {
    let applier = TestApplier::new();
    applier.set_should_fail(true).await;

    let mut dag = DagStore::new([0; 32]);

    let delta = CausalDelta::new([1; 32], vec![[0; 32]], TestPayload { value: 1 }, 1000);

    // First attempt fails
    let result = dag.add_delta(delta.clone(), &applier).await;
    assert!(result.is_err());

    // Delta is stored but not applied
    assert!(dag.has_delta(&[1; 32]));
    assert_eq!(dag.stats().applied_deltas, 1); // Only root

    // Note: Recovery would require manual retry - the DAG doesn't auto-retry
}

// ============================================================
// Pending Delta Management
// ============================================================

#[tokio::test]
async fn test_dag_pending_stats() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Add pending delta
    let delta = CausalDelta::new(
        [99; 32],
        vec![[88; 32]], // missing parent
        TestPayload { value: 99 },
        1000,
    );

    dag.add_delta(delta, &applier).await.unwrap();

    let stats = dag.pending_stats();
    assert_eq!(stats.count, 1);
    assert_eq!(stats.total_missing_parents, 1);
    // Age should be a reasonable value (not checking >= 0 as u64 is always >= 0)
    assert!(stats.oldest_age_secs < 10); // Should be very recent
}

#[tokio::test]
async fn test_dag_cleanup_stale() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Add pending delta
    let delta = CausalDelta::new([99; 32], vec![[88; 32]], TestPayload { value: 99 }, 1000);

    dag.add_delta(delta, &applier).await.unwrap();
    assert_eq!(dag.pending_stats().count, 1);

    // Wait a bit
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Cleanup with very short timeout
    let evicted = dag.cleanup_stale(Duration::from_millis(50));
    assert_eq!(evicted, 1);
    assert_eq!(dag.pending_stats().count, 0);
}

#[tokio::test]
async fn test_dag_cleanup_stale_keeps_recent() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let delta = CausalDelta::new([99; 32], vec![[88; 32]], TestPayload { value: 99 }, 1000);

    dag.add_delta(delta, &applier).await.unwrap();

    // Cleanup with long timeout (keeps recent)
    let evicted = dag.cleanup_stale(Duration::from_secs(10));
    assert_eq!(evicted, 0);
    assert_eq!(dag.pending_stats().count, 1);
}

#[tokio::test]
async fn test_dag_get_missing_parents() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Add delta with missing parent
    let delta1 = CausalDelta::new(
        [2; 32],
        vec![[1; 32]], // missing
        TestPayload { value: 2 },
        2000,
    );

    // Add delta with multiple missing parents
    let delta2 = CausalDelta::new(
        [4; 32],
        vec![[3; 32], [1; 32]], // both missing, [1; 32] already tracked
        TestPayload { value: 4 },
        4000,
    );

    dag.add_delta(delta1, &applier).await.unwrap();
    dag.add_delta(delta2, &applier).await.unwrap();

    let missing = dag.get_missing_parents();
    assert_eq!(missing.len(), 2);
    assert!(missing.contains(&[1; 32]));
    assert!(missing.contains(&[3; 32]));
}

// ============================================================
// Query & Inspection Tests
// ============================================================

#[tokio::test]
async fn test_dag_has_delta() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let delta = CausalDelta::new([1; 32], vec![[0; 32]], TestPayload { value: 1 }, 1000);

    assert!(!dag.has_delta(&[1; 32]));

    dag.add_delta(delta, &applier).await.unwrap();

    assert!(dag.has_delta(&[1; 32]));
    assert!(!dag.has_delta(&[2; 32]));
}

#[tokio::test]
async fn test_dag_get_delta() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let delta = CausalDelta::new([1; 32], vec![[0; 32]], TestPayload { value: 1 }, 1000);

    assert!(dag.get_delta(&[1; 32]).is_none());

    dag.add_delta(delta.clone(), &applier).await.unwrap();

    let retrieved = dag.get_delta(&[1; 32]);
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap(), &delta);
}

#[tokio::test]
async fn test_dag_get_deltas_since() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Linear chain
    let d1 = CausalDelta::new([1; 32], vec![[0; 32]], TestPayload { value: 1 }, 1000);
    let d2 = CausalDelta::new([2; 32], vec![[1; 32]], TestPayload { value: 2 }, 2000);
    let d3 = CausalDelta::new([3; 32], vec![[2; 32]], TestPayload { value: 3 }, 3000);

    dag.add_delta(d1.clone(), &applier).await.unwrap();
    dag.add_delta(d2.clone(), &applier).await.unwrap();
    dag.add_delta(d3.clone(), &applier).await.unwrap();

    // Get all deltas since root
    let deltas = dag.get_deltas_since([0; 32]);
    assert_eq!(deltas.len(), 3);

    // Get deltas since d1
    let deltas = dag.get_deltas_since([1; 32]);
    assert_eq!(deltas.len(), 2);

    // Get deltas since d3 (none)
    let deltas = dag.get_deltas_since([3; 32]);
    assert_eq!(deltas.len(), 0);
}

#[tokio::test]
async fn test_dag_get_deltas_since_branched() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Branched structure
    //      0
    //     / \
    //    1   2
    //    |
    //    3

    let d1 = CausalDelta::new([1; 32], vec![[0; 32]], TestPayload { value: 1 }, 1000);
    let d2 = CausalDelta::new([2; 32], vec![[0; 32]], TestPayload { value: 2 }, 1001);
    let d3 = CausalDelta::new([3; 32], vec![[1; 32]], TestPayload { value: 3 }, 2000);

    dag.add_delta(d1, &applier).await.unwrap();
    dag.add_delta(d2, &applier).await.unwrap();
    dag.add_delta(d3, &applier).await.unwrap();

    // Get all since root
    let deltas = dag.get_deltas_since([0; 32]);
    assert_eq!(deltas.len(), 3);

    let ids: Vec<_> = deltas.iter().map(|d| d.id).collect();
    assert!(ids.contains(&[1; 32]));
    assert!(ids.contains(&[2; 32]));
    assert!(ids.contains(&[3; 32]));
}

// ============================================================
// Stress Tests
// ============================================================

#[tokio::test]
async fn test_dag_many_concurrent_branches() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create 100 concurrent branches from root
    for i in 1..=100 {
        let delta = CausalDelta::new(
            [i; 32],
            vec![[0; 32]],
            TestPayload { value: i as u32 },
            i as u64 * 1000,
        );
        dag.add_delta(delta, &applier).await.unwrap();
    }

    // 100 heads
    assert_eq!(dag.get_heads().len(), 100);

    // Merge all with single delta
    let parents: Vec<_> = (1..=100).map(|i| [i; 32]).collect();
    let merge = CausalDelta::new([200; 32], parents, TestPayload { value: 200 }, 200_000);

    dag.add_delta(merge, &applier).await.unwrap();

    // Single head now
    assert_eq!(dag.get_heads(), vec![[200; 32]]);
    assert_eq!(dag.stats().applied_deltas, 102); // root + 100 + merge
}

#[tokio::test]
async fn test_dag_long_chain() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create chain of 100 deltas (reduced from 1000 to fit in u8)
    for i in 1..=100 {
        let delta = CausalDelta::new(
            [i as u8; 32],
            vec![[(i - 1) as u8; 32]],
            TestPayload { value: i as u32 },
            i as u64 * 1000,
        );
        dag.add_delta(delta, &applier).await.unwrap();
    }

    assert_eq!(dag.get_heads(), vec![[100_u8; 32]]);
    assert_eq!(dag.stats().applied_deltas, 101); // root + 100
    assert_eq!(applier.get_applied().await.len(), 100);
}

#[tokio::test]
async fn test_dag_stress_out_of_order() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create 100-delta chain
    let mut deltas = Vec::new();
    for i in 1..=100 {
        deltas.push(CausalDelta::new(
            [i; 32],
            vec![[i - 1; 32]],
            TestPayload { value: i as u32 },
            i as u64 * 1000,
        ));
    }

    // Add in reverse order
    for delta in deltas.iter().rev() {
        dag.add_delta(delta.clone(), &applier).await.unwrap();
    }

    // All applied in correct order
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), 100);
    for i in 1..=100 {
        assert_eq!(applied[i - 1], [i as u8; 32]);
    }

    assert_eq!(dag.pending_stats().count, 0);
    assert_eq!(dag.get_heads(), vec![[100; 32]]);
}

// ============================================================
// Extreme Stress Tests - Bulletproof for E2E
// ============================================================

#[tokio::test]
async fn test_extreme_pending_chain_500_deltas() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create 500-delta chain
    let mut deltas = Vec::new();
    for i in 1..=500 {
        let id = {
            let mut bytes = [0u8; 32];
            bytes[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            bytes
        };
        let parent_id = {
            let mut bytes = [0u8; 32];
            bytes[0..8].copy_from_slice(&((i - 1) as u64).to_le_bytes());
            bytes
        };

        deltas.push(CausalDelta::new(
            id,
            vec![parent_id],
            TestPayload { value: i as u32 },
            i as u64 * 1000,
        ));
    }

    // Add ALL in reverse (worst case - all pending initially)
    for delta in deltas.iter().rev() {
        dag.add_delta(delta.clone(), &applier).await.unwrap();
    }

    // All should be resolved
    assert_eq!(dag.pending_stats().count, 0, "All deltas should be applied");
    assert_eq!(dag.stats().applied_deltas, 501); // root + 500
    assert_eq!(applier.get_applied().await.len(), 500);
}

#[tokio::test]
async fn test_extreme_concurrent_branches_200() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create 200 concurrent branches from root
    for i in 1..=200 {
        let id = {
            let mut bytes = [0u8; 32];
            bytes[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            bytes
        };

        let delta = CausalDelta::new(
            id,
            vec![[0; 32]],
            TestPayload { value: i as u32 },
            i as u64 * 1000,
        );
        dag.add_delta(delta, &applier).await.unwrap();
    }

    // 200 heads
    assert_eq!(
        dag.get_heads().len(),
        200,
        "Should have 200 concurrent heads"
    );

    // Merge all with single delta
    let parents: Vec<_> = (1..=200)
        .map(|i| {
            let mut bytes = [0u8; 32];
            bytes[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            bytes
        })
        .collect();

    let merge = CausalDelta::new([255; 32], parents, TestPayload { value: 999 }, 300_000);

    dag.add_delta(merge, &applier).await.unwrap();

    // Single head now
    assert_eq!(dag.get_heads(), vec![[255; 32]]);
    assert_eq!(dag.stats().applied_deltas, 202); // root + 200 + merge
}

#[tokio::test]
async fn test_extreme_random_order_1000_deltas() {
    use rand::seq::SliceRandom;

    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create 1000-delta chain
    let mut deltas = Vec::new();
    for i in 1..=1000 {
        let id = {
            let mut bytes = [0u8; 32];
            bytes[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            bytes
        };
        let parent_id = {
            let mut bytes = [0u8; 32];
            bytes[0..8].copy_from_slice(&((i - 1) as u64).to_le_bytes());
            bytes
        };

        deltas.push(CausalDelta::new(
            id,
            vec![parent_id],
            TestPayload { value: i as u32 },
            i as u64 * 1000,
        ));
    }

    // Completely shuffle
    let mut shuffled = deltas.clone();
    shuffled.shuffle(&mut rand::thread_rng());

    // Apply in random order
    for delta in shuffled {
        dag.add_delta(delta, &applier).await.unwrap();
    }

    // All applied correctly
    assert_eq!(dag.pending_stats().count, 0);
    assert_eq!(dag.stats().applied_deltas, 1001); // root + 1000

    let final_head = {
        let mut bytes = [0u8; 32];
        bytes[0..8].copy_from_slice(&(1000_u64).to_le_bytes());
        bytes
    };
    assert_eq!(dag.get_heads(), vec![final_head]);
}

#[tokio::test]
async fn test_extreme_mixed_topology_500_deltas() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Pattern: 10 concurrent branches, each with 50 deltas deep
    let mut all_deltas = Vec::new();

    for branch in 0..10 {
        for depth in 1..=50 {
            let id = {
                let mut bytes = [0u8; 32];
                bytes[0] = branch as u8;
                bytes[1..9].copy_from_slice(&(depth as u64).to_le_bytes());
                bytes
            };

            let parent_id = if depth == 1 {
                [0; 32] // Branch from root
            } else {
                let mut bytes = [0u8; 32];
                bytes[0] = branch as u8;
                bytes[1..9].copy_from_slice(&((depth - 1) as u64).to_le_bytes());
                bytes
            };

            all_deltas.push(CausalDelta::new(
                id,
                vec![parent_id],
                TestPayload {
                    value: (branch * 1000 + depth) as u32,
                },
                (branch * 1000 + depth) as u64 * 1000,
            ));
        }
    }

    // Apply all
    for delta in all_deltas {
        dag.add_delta(delta, &applier).await.unwrap();
    }

    // 10 heads (one per branch)
    assert_eq!(dag.get_heads().len(), 10);
    assert_eq!(dag.stats().applied_deltas, 501); // root + 500
    assert_eq!(dag.pending_stats().count, 0);
}

#[tokio::test]
async fn test_extreme_late_parent_arrival() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Add 100 deltas that all depend on missing parent [99; 32]
    for i in 1..=100 {
        let id = {
            let mut bytes = [0u8; 32];
            bytes[0..8].copy_from_slice(&((i + 100) as u64).to_le_bytes());
            bytes
        };

        let delta = CausalDelta::new(
            id,
            vec![[99; 32]], // All depend on same missing parent
            TestPayload { value: i as u32 },
            i as u64 * 1000,
        );
        dag.add_delta(delta, &applier).await.unwrap();
    }

    // All pending
    assert_eq!(dag.pending_stats().count, 100);
    assert_eq!(dag.get_missing_parents(), vec![[99; 32]]);

    // Parent finally arrives
    let parent = CausalDelta::new([99; 32], vec![[0; 32]], TestPayload { value: 99 }, 99_000);

    dag.add_delta(parent, &applier).await.unwrap();

    // All 100 should cascade apply
    assert_eq!(dag.pending_stats().count, 0);
    assert_eq!(dag.stats().applied_deltas, 102); // root + parent + 100
}
