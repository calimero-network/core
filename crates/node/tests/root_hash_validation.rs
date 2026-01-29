//! Tests for conditional root hash validation in delta application.
//!
//! Verifies that root hash mismatches are only flagged when applying on a
//! deterministic base (linear, cascaded linear, or clean merge). For
//! concurrent-head cases, mismatches are expected and not validated.

use calimero_dag::{ApplyError, CausalDelta, DagStore, DeltaApplier};
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::entities::Metadata;
use calimero_storage::logical_clock::HybridTimestamp;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// Test applier that tracks whether each delta would be validated.
struct ValidationTrackingApplier {
    state: Arc<RwLock<ValidationState>>,
    applied: Arc<Mutex<Vec<AppliedRecord>>>,
}

#[derive(Default)]
struct ValidationState {
    pre_apply_heads: Vec<[u8; 32]>,
    last_applied_id: Option<[u8; 32]>,
    /// Whether the original base was deterministic (linear or clean merge).
    /// Cascaded validation only allowed when this is true.
    base_is_deterministic: bool,
}

impl ValidationState {
    fn should_validate(&self, parents: &[[u8; 32]]) -> bool {
        // Cascaded: single parent matches last applied, BUT only if base was deterministic
        if parents.len() == 1 {
            if let Some(last) = self.last_applied_id {
                if parents[0] == last && self.base_is_deterministic {
                    return true;
                }
            }
        }
        // Linear: single head matches single parent
        if let ([head], [parent]) = (self.pre_apply_heads.as_slice(), parents) {
            if head == parent {
                return true;
            }
        }
        // Clean merge: heads == parents (order-independent)
        if !self.pre_apply_heads.is_empty() && self.pre_apply_heads.len() == parents.len() {
            let mut heads = self.pre_apply_heads.clone();
            let mut parents = parents.to_vec();
            heads.sort();
            parents.sort();
            if heads == parents {
                return true;
            }
        }
        false
    }
}

#[derive(Debug, Clone)]
struct AppliedRecord {
    delta_id: [u8; 32],
    is_validatable: bool,
}

impl ValidationTrackingApplier {
    fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(ValidationState::default())),
            applied: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn prepare(&self, heads: Vec<[u8; 32]>, delta_parents: &[[u8; 32]]) {
        let mut state = self.state.write().await;

        // Compute whether this is a deterministic base
        let is_linear =
            heads.len() == 1 && delta_parents.len() == 1 && heads[0] == delta_parents[0];
        let is_clean_merge = if !heads.is_empty() && heads.len() == delta_parents.len() {
            let mut sorted_heads = heads.clone();
            let mut sorted_parents = delta_parents.to_vec();
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
}

#[async_trait::async_trait]
impl DeltaApplier<Vec<Action>> for ValidationTrackingApplier {
    async fn apply(&self, delta: &CausalDelta<Vec<Action>>) -> Result<(), ApplyError> {
        let mut state = self.state.write().await;
        let is_validatable = state.should_validate(&delta.parents);

        self.applied.lock().await.push(AppliedRecord {
            delta_id: delta.id,
            is_validatable,
        });

        state.last_applied_id = Some(delta.id);

        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Linear-Base Detection Tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Test genesis delta is recognized as linear base (no prior heads).
#[tokio::test]
async fn test_genesis_delta_is_linear_base() {
    let applier = ValidationTrackingApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Pre-apply heads: [0; 32] (genesis case)
    let delta = create_delta([0x01; 32], vec![[0; 32]], [0xAA; 32]);
    applier.prepare(dag.get_heads(), &delta.parents).await;
    dag.add_delta(delta, &applier).await.unwrap();

    let applied = applier.applied.lock().await;
    assert_eq!(applied.len(), 1);
    assert!(
        applied[0].is_validatable,
        "Genesis delta should be recognized as linear base"
    );
}

/// Test delta on single head matching parent is linear base.
#[tokio::test]
async fn test_single_head_matching_parent_is_linear_base() {
    let applier = ValidationTrackingApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Apply first delta
    let delta_a = create_delta([0x01; 32], vec![[0; 32]], [0xAA; 32]);
    applier.prepare(dag.get_heads(), &delta_a.parents).await;
    dag.add_delta(delta_a, &applier).await.unwrap();

    // Apply second delta on top of first
    let delta_b = create_delta([0x02; 32], vec![[0x01; 32]], [0xBB; 32]);
    applier.prepare(dag.get_heads(), &delta_b.parents).await;
    dag.add_delta(delta_b, &applier).await.unwrap();

    let applied = applier.applied.lock().await;
    assert_eq!(applied.len(), 2);

    // First delta: genesis case → linear base
    assert!(
        applied[0].is_validatable,
        "First delta (genesis) should be linear base"
    );

    // Second delta: single head [0x01] matches parent → linear base
    assert!(
        applied[1].is_validatable,
        "Second delta on single matching head should be linear base"
    );
}

/// Test delta creating a concurrent branch is NOT linear base.
#[tokio::test]
async fn test_concurrent_branch_is_not_linear_base() {
    let applier = ValidationTrackingApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Apply first delta
    let delta_a = create_delta([0x01; 32], vec![[0; 32]], [0xAA; 32]);
    applier.prepare(dag.get_heads(), &delta_a.parents).await;
    dag.add_delta(delta_a, &applier).await.unwrap();

    // Apply concurrent delta (same parent [0; 32], creates second branch)
    // At this point, heads = [[0x01; 32]], but delta_b's parent is [0; 32] (not matching)
    let delta_b = create_delta([0x02; 32], vec![[0; 32]], [0xBB; 32]);
    applier.prepare(dag.get_heads(), &delta_b.parents).await;
    dag.add_delta(delta_b, &applier).await.unwrap();

    let applied = applier.applied.lock().await;
    assert_eq!(applied.len(), 2);

    // First delta: linear base
    assert!(applied[0].is_validatable);

    // Second delta: parent [0; 32] doesn't match current head [0x01; 32]
    assert!(
        !applied[1].is_validatable,
        "Concurrent branch (parent doesn't match single head) should NOT be linear base"
    );
}

/// Test delta applied when multiple heads exist is NOT linear base.
#[tokio::test]
async fn test_multiple_heads_is_not_linear_base() {
    let applier = ValidationTrackingApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create two concurrent branches first
    let delta_a = create_delta([0x01; 32], vec![[0; 32]], [0xAA; 32]);
    applier.prepare(dag.get_heads(), &delta_a.parents).await;
    dag.add_delta(delta_a, &applier).await.unwrap();

    let delta_b = create_delta([0x02; 32], vec![[0; 32]], [0xBB; 32]);
    applier.prepare(dag.get_heads(), &delta_b.parents).await;
    dag.add_delta(delta_b, &applier).await.unwrap();

    // Now we have 2 heads: [0x01; 32], [0x02; 32]
    let heads = dag.get_heads();
    assert_eq!(heads.len(), 2, "Should have 2 concurrent heads");

    // Apply a delta extending one branch while 2 heads exist
    let delta_c = create_delta([0x03; 32], vec![[0x01; 32]], [0xCC; 32]);
    applier.prepare(dag.get_heads(), &delta_c.parents).await;
    dag.add_delta(delta_c, &applier).await.unwrap();

    let applied = applier.applied.lock().await;
    assert_eq!(applied.len(), 3);

    // Third delta: multiple heads exist → NOT linear base
    assert!(
        !applied[2].is_validatable,
        "Delta applied with multiple heads should NOT be linear base"
    );
}

/// Test clean merge delta (parents exactly match heads) IS validatable.
#[tokio::test]
async fn test_clean_merge_delta_is_validatable() {
    let applier = ValidationTrackingApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create two concurrent branches
    let delta_a = create_delta([0x01; 32], vec![[0; 32]], [0xAA; 32]);
    applier.prepare(dag.get_heads(), &delta_a.parents).await;
    dag.add_delta(delta_a, &applier).await.unwrap();

    let delta_b = create_delta([0x02; 32], vec![[0; 32]], [0xBB; 32]);
    applier.prepare(dag.get_heads(), &delta_b.parents).await;
    dag.add_delta(delta_b, &applier).await.unwrap();

    // Create merge delta with parents exactly matching the 2 heads
    // heads = [[0x01; 32], [0x02; 32]], parents = [[0x01; 32], [0x02; 32]]
    let delta_merge = create_delta([0x03; 32], vec![[0x01; 32], [0x02; 32]], [0xCC; 32]);
    applier.prepare(dag.get_heads(), &delta_merge.parents).await;
    dag.add_delta(delta_merge, &applier).await.unwrap();

    let applied = applier.applied.lock().await;

    // Clean merge: parents exactly equals heads → IS validatable
    let merge_record = applied.iter().find(|r| r.delta_id == [0x03; 32]).unwrap();
    assert!(
        merge_record.is_validatable,
        "Clean merge (parents == heads) should be validatable"
    );
}

/// Test partial merge (only some heads merged) is NOT validatable.
#[tokio::test]
async fn test_partial_merge_is_not_validatable() {
    let applier = ValidationTrackingApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Create three concurrent branches
    let delta_a = create_delta([0x01; 32], vec![[0; 32]], [0xAA; 32]);
    applier.prepare(dag.get_heads(), &delta_a.parents).await;
    dag.add_delta(delta_a, &applier).await.unwrap();

    let delta_b = create_delta([0x02; 32], vec![[0; 32]], [0xBB; 32]);
    applier.prepare(dag.get_heads(), &delta_b.parents).await;
    dag.add_delta(delta_b, &applier).await.unwrap();

    let delta_c = create_delta([0x03; 32], vec![[0; 32]], [0xCC; 32]);
    applier.prepare(dag.get_heads(), &delta_c.parents).await;
    dag.add_delta(delta_c, &applier).await.unwrap();

    // Now we have 3 heads: [0x01; 32], [0x02; 32], [0x03; 32]
    let heads = dag.get_heads();
    assert_eq!(heads.len(), 3, "Should have 3 concurrent heads");

    // Create partial merge (only merges 2 of 3 heads)
    let delta_partial_merge = create_delta([0x04; 32], vec![[0x01; 32], [0x02; 32]], [0xDD; 32]);
    applier
        .prepare(dag.get_heads(), &delta_partial_merge.parents)
        .await;
    dag.add_delta(delta_partial_merge, &applier).await.unwrap();

    let applied = applier.applied.lock().await;

    // Partial merge: parents != heads (only 2 of 3 merged) → NOT validatable
    let merge_record = applied.iter().find(|r| r.delta_id == [0x04; 32]).unwrap();
    assert!(
        !merge_record.is_validatable,
        "Partial merge (parents != all heads) should NOT be validatable"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Cascaded Delta Tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Test that cascaded deltas ARE validated when they extend the just-applied delta
/// AND the original base was deterministic.
///
/// When a delta triggers cascading application of pending deltas, the cascaded
/// delta's parent matches the just-applied delta, making it validatable via
/// the last_applied_delta_id tracking - but only if base_is_deterministic is true.
#[tokio::test]
async fn test_cascaded_deltas_are_validated_when_linear() {
    let applier = ValidationTrackingApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Apply delta A (genesis)
    let delta_a = create_delta([0x01; 32], vec![[0; 32]], [0xAA; 32]);
    applier.prepare(dag.get_heads(), &delta_a.parents).await;
    dag.add_delta(delta_a, &applier).await.unwrap();

    // Add delta C (depends on B which doesn't exist yet) - will be pending
    let delta_c = create_delta([0x03; 32], vec![[0x02; 32]], [0xCC; 32]);
    applier.prepare(dag.get_heads(), &delta_c.parents).await;
    let applied_c = dag.add_delta(delta_c, &applier).await.unwrap();
    assert!(!applied_c, "Delta C should be pending (parent B missing)");

    // Now add delta B - this should trigger cascade to apply C
    // Pre-apply heads: [0x01] (A is the only head)
    // This is a linear base (single head matches delta_b's parent)
    let delta_b = create_delta([0x02; 32], vec![[0x01; 32]], [0xBB; 32]);
    applier.prepare(dag.get_heads(), &delta_b.parents).await;
    dag.add_delta(delta_b, &applier).await.unwrap();

    let applied = applier.applied.lock().await;
    // Applied order: A, B, C (C cascades after B)
    assert_eq!(applied.len(), 3);

    // Delta B: linear base (single head [0x01] matches parent)
    let delta_b_record = applied.iter().find(|r| r.delta_id == [0x02; 32]).unwrap();
    assert!(
        delta_b_record.is_validatable,
        "Delta B should be validatable (linear base)"
    );

    // Delta C: IS validatable because its parent [0x02] matches last_applied (B)
    let delta_c_record = applied.iter().find(|r| r.delta_id == [0x03; 32]).unwrap();
    assert!(
        delta_c_record.is_validatable,
        "Cascaded delta C should be validatable (parent matches last applied)"
    );
}

/// Test that cascaded deltas are NOT validated when the original base had concurrent heads.
///
/// If the pre-apply base has multiple heads that aren't being merged (concurrent scenario),
/// cascaded deltas should NOT be validated even though their parent matches the just-applied
/// delta. This prevents false "non-determinism" warnings.
#[tokio::test]
async fn test_cascaded_deltas_not_validated_with_concurrent_base() {
    let applier = ValidationTrackingApplier::new();
    let mut dag = DagStore::new([0; 32]);

    // Apply delta A (genesis)
    let delta_a = create_delta([0x01; 32], vec![[0; 32]], [0xAA; 32]);
    applier.prepare(dag.get_heads(), &delta_a.parents).await;
    dag.add_delta(delta_a, &applier).await.unwrap();

    // Apply delta B (concurrent branch, same parent as A)
    let delta_b = create_delta([0x02; 32], vec![[0; 32]], [0xBB; 32]);
    applier.prepare(dag.get_heads(), &delta_b.parents).await;
    dag.add_delta(delta_b, &applier).await.unwrap();

    // Now we have 2 concurrent heads: [0x01; 32], [0x02; 32]
    let heads = dag.get_heads();
    assert_eq!(heads.len(), 2, "Should have 2 concurrent heads");

    // Add delta D (depends on C which doesn't exist yet) - will be pending
    let delta_d = create_delta([0x04; 32], vec![[0x03; 32]], [0xDD; 32]);
    applier.prepare(dag.get_heads(), &delta_d.parents).await;
    let applied_d = dag.add_delta(delta_d, &applier).await.unwrap();
    assert!(!applied_d, "Delta D should be pending (parent C missing)");

    // Now add delta C - extends one branch while concurrent heads exist
    // Pre-apply heads: [0x01, 0x02] (concurrent)
    // This is NOT a deterministic base (multiple heads, not all being merged)
    let delta_c = create_delta([0x03; 32], vec![[0x01; 32]], [0xCC; 32]);
    applier.prepare(dag.get_heads(), &delta_c.parents).await;
    dag.add_delta(delta_c, &applier).await.unwrap();

    let applied = applier.applied.lock().await;
    // Applied order: A, B, C, D (D cascades after C)
    assert_eq!(applied.len(), 4);

    // Delta C: NOT validatable (concurrent heads, not a merge)
    let delta_c_record = applied.iter().find(|r| r.delta_id == [0x03; 32]).unwrap();
    assert!(
        !delta_c_record.is_validatable,
        "Delta C should NOT be validatable (concurrent heads exist)"
    );

    // Delta D: cascaded, but base was NOT deterministic → NOT validatable
    // Even though D's parent [0x03] matches last_applied (C), the base had concurrent heads
    let delta_d_record = applied.iter().find(|r| r.delta_id == [0x04; 32]).unwrap();
    assert!(
        !delta_d_record.is_validatable,
        "Cascaded delta D should NOT be validatable (base had concurrent heads)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Edge Cases
// ═══════════════════════════════════════════════════════════════════════════════

/// Test that the DAG root head ([0; 32]) is properly handled.
#[tokio::test]
async fn test_genesis_root_head_is_handled() {
    let dag: DagStore<Vec<Action>> = DagStore::new([0; 32]);

    // DAG starts with root [0; 32] as the only head
    let heads = dag.get_heads();
    assert_eq!(heads, vec![[0; 32]], "Genesis DAG should have root as head");

    // Verify ValidationState correctly identifies this as linear
    let state = ValidationState {
        pre_apply_heads: heads,
        last_applied_id: None,
        base_is_deterministic: true, // Single head matches single parent
    };
    assert!(
        state.should_validate(&[[0; 32]]),
        "Delta with parent [0;32] on root head should be validatable"
    );
}

/// Test single head but parent doesn't match is NOT validatable.
#[tokio::test]
async fn test_single_head_non_matching_parent_not_validatable() {
    // State: single head [0x01], delta parent [0xFF]
    let state = ValidationState {
        pre_apply_heads: vec![[0x01; 32]],
        last_applied_id: None,
        base_is_deterministic: false, // Head doesn't match parent
    };
    assert!(
        !state.should_validate(&[[0xFF; 32]]),
        "Single head but non-matching parent should NOT be validatable"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test Helpers
// ═══════════════════════════════════════════════════════════════════════════════

fn create_delta(
    id: [u8; 32],
    parents: Vec<[u8; 32]>,
    expected_root_hash: [u8; 32],
) -> CausalDelta<Vec<Action>> {
    CausalDelta {
        id,
        parents,
        payload: vec![create_action(id)],
        hlc: HybridTimestamp::default(),
        expected_root_hash,
    }
}

fn create_action(id: [u8; 32]) -> Action {
    Action::Add {
        id: Id::from(id),
        data: vec![1, 2, 3],
        ancestors: vec![],
        metadata: Metadata::default(),
    }
}
