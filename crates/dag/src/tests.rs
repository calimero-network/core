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
    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });

    let delta2 = CausalDelta::new_test([2; 32], vec![[1; 32]], TestPayload { value: 2 });

    let delta3 = CausalDelta::new_test([3; 32], vec![[2; 32]], TestPayload { value: 3 });

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

    let delta = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });

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

    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });

    let delta2 = CausalDelta::new_test([2; 32], vec![[1; 32]], TestPayload { value: 2 });

    // Receive delta2 first (out of order)
    let applied2_first = dag.add_delta(delta2.clone(), &applier).await.unwrap();
    assert!(!applied2_first, "Delta2 should be pending");

    // Check pending
    assert_eq!(dag.pending_stats().count, 1);
    assert_eq!(
        dag.get_missing_parents(MAX_DELTA_QUERY_LIMIT),
        vec![[1; 32]]
    );

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
    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });
    let delta2 = CausalDelta::new_test([2; 32], vec![[1; 32]], TestPayload { value: 2 });
    let delta3 = CausalDelta::new_test([3; 32], vec![[2; 32]], TestPayload { value: 3 });

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
        .map(|i| CausalDelta::new_test([i; 32], vec![[i - 1; 32]], TestPayload { value: i as u32 }))
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
    let delta_a = CausalDelta::new_test([10; 32], vec![[0; 32]], TestPayload { value: 10 });

    let delta_b = CausalDelta::new_test([20; 32], vec![[0; 32]], TestPayload { value: 20 });

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
    let delta_a = CausalDelta::new_test([10; 32], vec![[0; 32]], TestPayload { value: 10 });
    let delta_b = CausalDelta::new_test([20; 32], vec![[0; 32]], TestPayload { value: 20 });

    dag.add_delta(delta_a, &applier).await.unwrap();
    dag.add_delta(delta_b, &applier).await.unwrap();

    // Two heads
    assert_eq!(dag.get_heads().len(), 2);

    // Merge delta
    let delta_merge = CausalDelta::new_test(
        [30; 32],
        vec![[10; 32], [20; 32]],
        TestPayload { value: 30 },
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
    let delta_a = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });
    let delta_b = CausalDelta::new_test([2; 32], vec![[0; 32]], TestPayload { value: 2 });
    let delta_c = CausalDelta::new_test([3; 32], vec![[0; 32]], TestPayload { value: 3 });

    dag.add_delta(delta_a, &applier).await.unwrap();
    dag.add_delta(delta_b, &applier).await.unwrap();
    dag.add_delta(delta_c, &applier).await.unwrap();

    // Three heads
    let mut heads = dag.get_heads();
    heads.sort();
    assert_eq!(heads.len(), 3);

    // Three-way merge
    let merge = CausalDelta::new_test(
        [99; 32],
        vec![[1; 32], [2; 32], [3; 32]],
        TestPayload { value: 99 },
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

    let d1 = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });
    let d2 = CausalDelta::new_test([2; 32], vec![[0; 32]], TestPayload { value: 2 });
    let d3 = CausalDelta::new_test([3; 32], vec![[1; 32]], TestPayload { value: 3 });
    let d4 = CausalDelta::new_test([4; 32], vec![[2; 32]], TestPayload { value: 4 });
    let d5 = CausalDelta::new_test([5; 32], vec![[3; 32], [4; 32]], TestPayload { value: 5 });

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

    let delta = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });

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

    let delta = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });

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
    let delta = CausalDelta::new_test(
        [99; 32],
        vec![[88; 32]], // missing parent
        TestPayload { value: 99 },
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
    let delta = CausalDelta::new_test([99; 32], vec![[88; 32]], TestPayload { value: 99 });

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

    let delta = CausalDelta::new_test([99; 32], vec![[88; 32]], TestPayload { value: 99 });

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
    let delta1 = CausalDelta::new_test(
        [2; 32],
        vec![[1; 32]], // missing
        TestPayload { value: 2 },
    );

    // Add delta with multiple missing parents
    let delta2 = CausalDelta::new_test(
        [4; 32],
        vec![[3; 32], [1; 32]], // both missing, [1; 32] already tracked
        TestPayload { value: 4 },
    );

    dag.add_delta(delta1, &applier).await.unwrap();
    dag.add_delta(delta2, &applier).await.unwrap();

    let missing = dag.get_missing_parents(MAX_DELTA_QUERY_LIMIT);
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

    let delta = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });

    assert!(!dag.has_delta(&[1; 32]));

    dag.add_delta(delta, &applier).await.unwrap();

    assert!(dag.has_delta(&[1; 32]));
    assert!(!dag.has_delta(&[2; 32]));
}

#[tokio::test]
async fn test_dag_get_delta() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    let delta = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });

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
    let d1 = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });
    let d2 = CausalDelta::new_test([2; 32], vec![[1; 32]], TestPayload { value: 2 });
    let d3 = CausalDelta::new_test([3; 32], vec![[2; 32]], TestPayload { value: 3 });

    dag.add_delta(d1.clone(), &applier).await.unwrap();
    dag.add_delta(d2.clone(), &applier).await.unwrap();
    dag.add_delta(d3.clone(), &applier).await.unwrap();

    // Get all deltas since root
    let (deltas, _) = dag.get_deltas_since([0; 32], None, MAX_DELTA_QUERY_LIMIT);
    assert_eq!(deltas.len(), 3);

    // Get deltas since d1
    let (deltas, _) = dag.get_deltas_since([1; 32], None, MAX_DELTA_QUERY_LIMIT);
    assert_eq!(deltas.len(), 2);

    // Get deltas since d3 (none)
    let (deltas, _) = dag.get_deltas_since([3; 32], None, MAX_DELTA_QUERY_LIMIT);
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

    let d1 = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });
    let d2 = CausalDelta::new_test([2; 32], vec![[0; 32]], TestPayload { value: 2 });
    let d3 = CausalDelta::new_test([3; 32], vec![[1; 32]], TestPayload { value: 3 });

    dag.add_delta(d1, &applier).await.unwrap();
    dag.add_delta(d2, &applier).await.unwrap();
    dag.add_delta(d3, &applier).await.unwrap();

    // Get all since root
    let (deltas, _) = dag.get_deltas_since([0; 32], None, MAX_DELTA_QUERY_LIMIT);
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
        let delta = CausalDelta::new_test([i; 32], vec![[0; 32]], TestPayload { value: i as u32 });
        dag.add_delta(delta, &applier).await.unwrap();
    }

    // 100 heads
    assert_eq!(dag.get_heads().len(), 100);

    // Merge all with single delta
    let parents: Vec<_> = (1..=100).map(|i| [i; 32]).collect();
    let merge = CausalDelta::new_test([200; 32], parents, TestPayload { value: 200 });

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
        let delta = CausalDelta::new_test(
            [i as u8; 32],
            vec![[(i - 1) as u8; 32]],
            TestPayload { value: i as u32 },
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
        deltas.push(CausalDelta::new_test(
            [i; 32],
            vec![[i - 1; 32]],
            TestPayload { value: i as u32 },
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
// Pagination & Limit Tests for query methods
// ============================================================

#[tokio::test]
async fn test_get_missing_parents_pagination() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Add 5 deltas, each missing a unique parent
    let num_deltas_added = 5_usize;
    for i in 1..=num_deltas_added {
        let delta = CausalDelta::new_test(
            [100 + i as u8; 32],
            // missing parent
            vec![[i as u8; 32]],
            TestPayload { value: i as u32 },
        );
        dag.add_delta(delta, &applier).await.unwrap();
    }

    assert_eq!(dag.pending_stats().count, num_deltas_added);

    // Request only 3
    let missing = dag.get_missing_parents(3);
    assert_eq!(missing.len(), 3);

    // Request all
    let missing_all = dag.get_missing_parents(MAX_DELTA_QUERY_LIMIT);
    assert_eq!(missing_all.len(), num_deltas_added);
}

#[tokio::test]
async fn test_get_deltas_since_pagination() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create chain 1..10
    let num_deltas_added = 10_usize;
    let mut prev = [0; 32];
    for i in 1..=num_deltas_added {
        let id = [i as u8; 32];
        let delta = CausalDelta::new_test(id, vec![prev], TestPayload { value: i as u32 });
        dag.add_delta(delta, &applier).await.unwrap();
        prev = id;
    }

    // Request recent history with limit 3
    // Note: get_deltas_since performs BFS from heads (reverse chronological-ish)
    // So we expect to see the latest deltas first (10, 9, 8...) or parents of heads.
    let (deltas, _) = dag.get_deltas_since([0; 32], None, 3);

    assert_eq!(deltas.len(), 3);

    // The current implementation uses VecDeque and pushes parents to back.
    // It starts with Head (10).
    // Result should be [10, 9, 8].
    let ids: Vec<u8> = deltas.iter().map(|d| d.id[0]).collect();
    assert_eq!(ids, vec![10, 9, 8]);
}

#[tokio::test]
async fn test_get_deltas_since_manual_pagination() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create linear chain 1..10:  10 -> 9 -> ... -> 1 -> Root
    let mut prev = [0; 32];
    for i in 1..=10 {
        let id = [i; 32];
        let delta = CausalDelta::new_test(id, vec![prev], TestPayload { value: i as u32 });
        dag.add_delta(delta, &applier).await.unwrap();
        prev = id;
    }

    // Page 1: Start from head (None), Limit 3
    // Expect: 10, 9, 8
    let (page1, _) = dag.get_deltas_since([0; 32], None, 3);
    assert_eq!(page1.len(), 3);
    assert_eq!(page1[0].id, [10; 32]);
    assert_eq!(page1.last().unwrap().id, [8; 32]);

    // Manual Step: Client identifies the last received ID (8)
    // and asks for the NEXT batch starting from 8's parent (7).
    // Note: Clients must interpret the 'parents' field of the last delta to know where to
    // continue.
    let next_start = page1.last().unwrap().parents[0];
    assert_eq!(next_start, [7; 32]);

    // Page 2: Start from [7], Limit 3
    // Expect: 7, 6, 5
    let (page2, _) = dag.get_deltas_since([0; 32], Some(vec![next_start]), 3);
    assert_eq!(page2.len(), 3);
    assert_eq!(page2[0].id, [7; 32]);
    assert_eq!(page2.last().unwrap().id, [5; 32]);

    // Page 3: Finish the rest
    let next_start = page2.last().unwrap().parents[0];
    let (page3, _) = dag.get_deltas_since([0; 32], Some(vec![next_start]), 10);
    assert_eq!(page3.len(), 4); // 4, 3, 2, 1
    assert_eq!(page3[0].id, [4; 32]);
    assert_eq!(page3.last().unwrap().id, [1; 32]);
}

#[tokio::test]
async fn test_pagination_hard_limits() {
    let applier = TestApplier::new();
    let delta_query_limit = 10;
    // Create chain longer than `delta_query_limit` (10)
    let over_delta_query_limit = delta_query_limit * 2;

    let mut dag = DagStore::new_with_delta_query_limit([0; 32], delta_query_limit);

    // Test `get_deltas_since()` hard cap
    let mut prev = [0; 32];
    for i in 1..=over_delta_query_limit {
        let id = {
            let mut bytes = [0u8; 32];
            bytes[0..4].copy_from_slice(&(i as u32).to_le_bytes());
            bytes
        };
        let delta = CausalDelta::new_test(id, vec![prev], TestPayload { value: i as u32 });
        dag.add_delta(delta, &applier).await.unwrap();
        prev = id;
    }

    // Request MORE than the hard cap
    let (deltas, cursor) = dag.get_deltas_since([0; 32], None, over_delta_query_limit);

    // Assert it was capped at 1000
    assert_eq!(
        deltas.len(),
        delta_query_limit,
        "Should be capped at delta_query_limit (10)"
    );
    // Assert cursor is not empty
    assert!(
        !cursor.is_empty(),
        "Cursor shoudn't be empty when pagination is not finished"
    );

    // Test `get_missing_parents` hard cap
    // Clear dag for cleanliness
    let mut dag = DagStore::new_with_delta_query_limit([0; 32], delta_query_limit);

    // Add 1001 pending deltas with unique missing parents
    for i in 1..=over_delta_query_limit {
        let id = {
            let mut bytes = [0u8; 32];
            bytes[0] = 1; // distinct from parents
            bytes[4..8].copy_from_slice(&(i as u32).to_le_bytes());
            bytes
        };
        let parent_id = {
            let mut bytes = [0u8; 32];
            bytes[0] = 2; // distinct
            bytes[4..8].copy_from_slice(&(i as u32).to_le_bytes());
            bytes
        };

        let delta = CausalDelta::new_test(id, vec![parent_id], TestPayload { value: i as u32 });
        dag.add_delta(delta, &applier).await.unwrap();
    }

    // Request MORE than the hard cap
    let missing = dag.get_missing_parents(over_delta_query_limit);

    // Assert it was capped at 10
    assert_eq!(
        missing.len(),
        delta_query_limit,
        "Should be capped at delta_query_limit (10)"
    );
}

#[tokio::test]
async fn test_pagination_preserves_branches() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Setup:
    //       Root (0)
    //      /   \
    //  BranchA  BranchB
    //     |        |
    //     A1       B1
    //      \      /
    //       Merge (M)

    // Branch A
    let delta_a1 = CausalDelta::new_test([10; 32], vec![[0; 32]], TestPayload { value: 10 });
    // Branch B
    let delta_b1 = CausalDelta::new_test([20; 32], vec![[0; 32]], TestPayload { value: 20 });
    // Merge
    let delta_merge = CausalDelta::new_test(
        [30; 32],
        vec![[10; 32], [20; 32]],
        TestPayload { value: 30 },
    );

    dag.add_delta(delta_a1, &applier).await.unwrap();
    dag.add_delta(delta_b1, &applier).await.unwrap();
    dag.add_delta(delta_merge, &applier).await.unwrap();

    // The DAG head is [30].

    // Request Page 1 with Limit 1.
    // Flow: Start at [30]. Pop [30]. Result=[30]. Push parents [10, 20]. Limit reached.
    // Cursor should contain [10, 20] (or whatever order the queue had).
    let query_limit = 1;
    let (page1, cursor1) = dag.get_deltas_since([0; 32], None, query_limit);

    assert_eq!(page1.len(), 1);
    assert_eq!(page1[0].id, [30; 32]);

    // Verify cursor contains BOTH branches
    assert_eq!(cursor1.len(), 2);
    assert!(cursor1.contains(&[10; 32]));
    assert!(cursor1.contains(&[20; 32]));

    // Request Page 2 using cursor1, using max limits
    // Flow: Start at [10, 20]. Pop 10. Result=[10]. Pop 20. Result=[10, 20].
    // Both hit ancestor [0]. Cursor should be empty.
    let query_limit = MAX_DELTA_QUERY_LIMIT;
    let (page2, cursor2) = dag.get_deltas_since([0; 32], Some(cursor1), query_limit);
    assert_eq!(page2.len(), 2);

    // Ensure we got both branches
    let ids: Vec<u8> = page2.iter().map(|d| d.id[0]).collect();
    assert!(ids.contains(&10));
    assert!(ids.contains(&20));

    // Ensure cursor is empty
    assert!(cursor2.is_empty());
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

        deltas.push(CausalDelta::new_test(
            id,
            vec![parent_id],
            TestPayload { value: i as u32 },
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

        let delta = CausalDelta::new_test(id, vec![[0; 32]], TestPayload { value: i as u32 });
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

    let merge = CausalDelta::new_test([255; 32], parents, TestPayload { value: 999 });

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

        deltas.push(CausalDelta::new_test(
            id,
            vec![parent_id],
            TestPayload { value: i as u32 },
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

            all_deltas.push(CausalDelta::new_test(
                id,
                vec![parent_id],
                TestPayload {
                    value: (branch * 1000 + depth) as u32,
                },
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

        let delta = CausalDelta::new_test(
            id,
            vec![[99; 32]], // All depend on same missing parent
            TestPayload { value: i as u32 },
        );
        dag.add_delta(delta, &applier).await.unwrap();
    }

    // All pending
    assert_eq!(dag.pending_stats().count, 100);
    assert_eq!(
        dag.get_missing_parents(MAX_DELTA_QUERY_LIMIT),
        vec![[99; 32]]
    );

    // Parent finally arrives
    let parent = CausalDelta::new_test([99; 32], vec![[0; 32]], TestPayload { value: 99 });

    dag.add_delta(parent, &applier).await.unwrap();

    // All 100 should cascade apply
    assert_eq!(dag.pending_stats().count, 0);
    assert_eq!(dag.stats().applied_deltas, 102); // root + parent + 100
}
