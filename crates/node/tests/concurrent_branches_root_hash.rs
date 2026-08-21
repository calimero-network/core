//! Integration test for concurrent branches in the DAG.
//!
//! These tests used to assert that each delta carried an `expected_root_hash`
//! through to the applier. That field is gone: it was a sender-declared value no
//! receiver could verify, and every node computes its own root hash on apply. What
//! remains worth pinning is the DAG topology itself — concurrent branches produce
//! two heads, a merge over both collapses them to one, and every delta reaches the
//! applier exactly once.

use calimero_dag::CausalDelta;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::entities::Metadata;

/// Test that concurrent branches leave the DAG with two heads, and that both
/// deltas are applied.
///
/// Scenario:
///          → Delta A (id: [0x01...]) ↘
///   Root                                → 2 heads
///          → Delta B (id: [0x02...]) ↗
#[tokio::test]
async fn test_concurrent_branches_produce_two_heads() {
    use calimero_dag::{ApplyError, DagStore, DeltaApplier};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // Simple test applier that doesn't actually apply to storage
    struct TestApplier {
        #[allow(clippy::type_complexity, reason = "test fixture")]
        applied: Arc<Mutex<Vec<[u8; 32]>>>, // applied delta ids, in order
    }

    #[async_trait::async_trait]
    impl DeltaApplier<Vec<Action>> for TestApplier {
        async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
            self.applied.lock().await.push(delta.id);
            Ok(())
        }
    }

    let applier = TestApplier {
        applied: Arc::new(Mutex::new(Vec::new())),
    };

    let mut dag = DagStore::new([0; 32]);

    // Create two concurrent deltas with different expected_root_hashes
    let delta_a = create_delta([0x01; 32], vec![[0; 32]]);
    let delta_b = create_delta([0x02; 32], vec![[0; 32]]);

    // Apply both
    let _ = dag.add_delta(delta_a.clone(), &applier).await.unwrap();
    let _ = dag.add_delta(delta_b.clone(), &applier).await.unwrap();

    // Should have TWO heads
    let mut heads = dag.get_heads();
    heads.sort();
    assert_eq!(heads.len(), 2);
    assert_eq!(heads, vec![[0x01; 32], [0x02; 32]]);

    // Both branches reached the applier, exactly once each.
    let applied = applier.applied.lock().await;
    assert_eq!(applied.len(), 2);
    assert!(applied.contains(&[0x01; 32]), "Delta A must be applied");
    assert!(applied.contains(&[0x02; 32]), "Delta B must be applied");
}

/// Test that a merge over two concurrent branches collapses them to a single head.
#[tokio::test]
async fn test_merge_delta_collapses_to_single_head() {
    use calimero_dag::{ApplyError, DagStore, DeltaApplier};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct TestApplier {
        applied: Arc<Mutex<Vec<[u8; 32]>>>,
    }

    #[async_trait::async_trait]
    impl DeltaApplier<Vec<Action>> for TestApplier {
        async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
            self.applied.lock().await.push(delta.id);
            Ok(())
        }
    }

    let applier = TestApplier {
        applied: Arc::new(Mutex::new(Vec::new())),
    };

    let mut dag = DagStore::new([0; 32]);

    // Create concurrent branches
    let delta_a = create_delta([0x01; 32], vec![[0; 32]]);
    let delta_b = create_delta([0x02; 32], vec![[0; 32]]);

    let _ = dag.add_delta(delta_a, &applier).await.unwrap();
    let _ = dag.add_delta(delta_b, &applier).await.unwrap();

    let delta_merge = create_delta([0x03; 32], vec![[0x01; 32], [0x02; 32]]);

    let _ = dag.add_delta(delta_merge, &applier).await.unwrap();

    // Should have single head
    assert_eq!(dag.get_heads(), vec![[0x03; 32]]);

    // The merge itself reached the applier.
    let applied = applier.applied.lock().await;
    assert!(applied.contains(&[0x03; 32]), "merge delta must be applied");
}

// ═══════════════════════════════════════════════════════════════════════
// Test Helpers
// ═══════════════════════════════════════════════════════════════════════

fn create_delta(id: [u8; 32], parents: Vec<[u8; 32]>) -> CausalDelta<Vec<Action>> {
    // Create a simple action for testing
    let action = Action::Add {
        id: Id::from([id[0]; 32]),
        data: vec![1, 2, 3],
        ancestors: vec![],
        metadata: Metadata::default(),
    };

    CausalDelta {
        id,
        parents,
        payload: vec![action],
        hlc: calimero_storage::logical_clock::HybridTimestamp::default(),
        kind: calimero_dag::DeltaKind::Regular,
    }
}
