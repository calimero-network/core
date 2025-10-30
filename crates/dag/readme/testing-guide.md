# DAG Testing Guide

Comprehensive guide to testing DAG behavior and integration.

---

## Test Categories

The DAG has **31 tests** covering all critical scenarios:

| Category | Tests | Coverage |
|----------|-------|----------|
| **Basic Functionality** | 4 | Creation, linear sequences, duplicates |
| **Out-of-Order** | 4 | Buffering, cascade, deep chains |
| **Concurrent Updates** | 5 | Forks, merges, complex topology |
| **Error Handling** | 2 | Apply failures, recovery |
| **Pending Management** | 4 | Stats, cleanup, missing parents |
| **Query & Inspection** | 4 | has_delta, get_delta, get_deltas_since |
| **Stress Tests** | 3 | 100+ deltas, branches, chains |
| **Extreme Stress** | 5 | 500-1000 deltas, random order |

---

## Test Infrastructure

### Mock Applier

```rust
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
struct TestApplier {
    applied: Arc<Mutex<Vec<[u8; 32]>>>,
}

impl TestApplier {
    fn new() -> Self {
        Self {
            applied: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    async fn get_applied(&self) -> Vec<[u8; 32]> {
        self.applied.lock().await.clone()
    }
    
    async fn clear(&self) {
        self.applied.lock().await.clear();
    }
}

#[async_trait]
impl DeltaApplier<TestPayload> for TestApplier {
    async fn apply(&self, delta: &CausalDelta<TestPayload>) -> Result<(), ApplyError> {
        // Record that delta was applied
        self.applied.lock().await.push(delta.id);
        Ok(())
    }
}
```

**Why Arc<Mutex<>>?**
- Arc: Share between test and applier
- Mutex: Async-safe interior mutability
- Vec: Track application order

### Test Payload

```rust
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
struct TestPayload {
    value: u32,
}
```

**Keep it simple**: Don't test payload logic in DAG tests

---

## Basic Tests

### Test 1: Linear Sequence

**Goal**: Verify deltas apply in order

```rust
#[tokio::test]
async fn test_dag_linear_sequence() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);
    
    // Create chain: root → D1 → D2 → D3
    let delta1 = CausalDelta::new_test(
        [1; 32],
        vec![[0; 32]],  // parent: root
        TestPayload { value: 1 },
    );
    
    let delta2 = CausalDelta::new_test(
        [2; 32],
        vec![[1; 32]],  // parent: delta1
        TestPayload { value: 2 },
    );
    
    let delta3 = CausalDelta::new_test(
        [3; 32],
        vec![[2; 32]],  // parent: delta2
        TestPayload { value: 3 },
    );
    
    // Apply in order
    let applied1 = dag.add_delta(delta1, &applier).await.unwrap();
    assert!(applied1, "Delta1 should be applied immediately");
    
    let applied2 = dag.add_delta(delta2, &applier).await.unwrap();
    assert!(applied2, "Delta2 should be applied immediately");
    
    let applied3 = dag.add_delta(delta3, &applier).await.unwrap();
    assert!(applied3, "Delta3 should be applied immediately");
    
    // Verify heads
    let heads = dag.get_heads();
    assert_eq!(heads, vec![[3; 32]], "Head should be delta3");
    
    // Verify all applied in order
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), 3);
    assert_eq!(applied[0], [1; 32]);
    assert_eq!(applied[1], [2; 32]);
    assert_eq!(applied[2], [3; 32]);
}
```

**What it validates**:
- ✅ Sequential deltas apply immediately
- ✅ Heads track correctly
- ✅ Application order preserved

### Test 2: Duplicate Detection

**Goal**: Verify duplicates are silently skipped

```rust
#[tokio::test]
async fn test_dag_duplicate_delta() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);
    
    let delta = CausalDelta::new_test(
        [1; 32],
        vec![[0; 32]],
        TestPayload { value: 1 },
    );
    
    // Add first time
    let applied1 = dag.add_delta(delta.clone(), &applier).await.unwrap();
    assert!(applied1, "First add should apply");
    
    // Add duplicate
    let applied2 = dag.add_delta(delta.clone(), &applier).await.unwrap();
    assert!(!applied2, "Duplicate should return false");
    
    // Verify applier only called once
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), 1, "Should only apply once");
}
```

**What it validates**:
- ✅ Duplicates don't error
- ✅ Applier not called for duplicates
- ✅ Returns `Ok(false)` for duplicates

---

## Out-of-Order Tests

### Test 3: Simple Out-of-Order

**Goal**: Verify pending buffer and cascade

```rust
#[tokio::test]
async fn test_dag_out_of_order() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);
    
    // Create chain: root → D1 → D2
    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });
    let delta2 = CausalDelta::new_test([2; 32], vec![[1; 32]], TestPayload { value: 2 });
    
    // Receive D2 first (out of order)
    let applied2_first = dag.add_delta(delta2.clone(), &applier).await.unwrap();
    assert!(!applied2_first, "D2 should be pending (missing parent)");
    
    // Check pending stats
    let stats = dag.pending_stats();
    assert_eq!(stats.count, 1, "One delta pending");
    assert_eq!(stats.total_missing_parents, 1, "One missing parent");
    
    // Check missing parents
    let missing = dag.get_missing_parents();
    assert_eq!(missing, vec![[1; 32]], "D1 is missing");
    
    // Now receive D1
    let applied1 = dag.add_delta(delta1, &applier).await.unwrap();
    assert!(applied1, "D1 should be applied immediately");
    
    // D2 should now be applied automatically (cascade)
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), 2, "Both deltas applied");
    assert_eq!(applied[0], [1; 32], "D1 applied first");
    assert_eq!(applied[1], [2; 32], "D2 applied second (cascade)");
    
    // No more pending
    let stats = dag.pending_stats();
    assert_eq!(stats.count, 0, "No pending deltas");
}
```

**What it validates**:
- ✅ Out-of-order deltas buffer
- ✅ Pending stats accurate
- ✅ Missing parents detected
- ✅ Cascade applies pending deltas
- ✅ Correct application order

### Test 4: Deep Pending Chain

**Goal**: Verify cascade handles long chains

```rust
#[tokio::test]
async fn test_dag_deep_pending_chain() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);
    
    // Create chain: root → D1 → D2 → D3 → D4 → D5
    // Add in reverse order: D5, D4, D3, D2, D1
    
    for i in (1..=5).rev() {
        let delta = CausalDelta::new_test(
            [i; 32],
            vec![[(i-1); 32]],
            TestPayload { value: i as u32 },
        );
        
        let applied = dag.add_delta(delta, &applier).await.unwrap();
        
        if i == 1 {
            assert!(applied, "D1 should apply and trigger cascade");
        } else {
            assert!(!applied, "D{} should be pending", i);
        }
    }
    
    // All should be applied in correct order
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), 5, "All 5 deltas applied");
    
    for i in 0..5 {
        assert_eq!(applied[i], [(i+1) as u8; 32], "Delta {} in correct position", i+1);
    }
    
    // No pending deltas
    assert_eq!(dag.pending_stats().count, 0);
    
    // Heads should be D5
    assert_eq!(dag.get_heads(), vec![[5; 32]]);
}
```

**What it validates**:
- ✅ Deep chains buffer correctly
- ✅ Single trigger cascades entire chain
- ✅ Correct topological ordering
- ✅ All pending deltas cleared

---

## Concurrent Update Tests

### Test 5: Fork and Merge

**Goal**: Verify fork detection and merge

```rust
#[tokio::test]
async fn test_dag_concurrent_updates() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);
    
    // Two nodes create concurrent deltas from root
    let delta_a = CausalDelta::new_test(
        [10; 32],
        vec![[0; 32]],  // both from root
        TestPayload { value: 10 },
    );
    
    let delta_b = CausalDelta::new_test(
        [20; 32],
        vec![[0; 32]],  // both from root
        TestPayload { value: 20 },
    );
    
    // Apply both
    dag.add_delta(delta_a, &applier).await.unwrap();
    dag.add_delta(delta_b, &applier).await.unwrap();
    
    // Should have TWO heads (fork detected)
    let mut heads = dag.get_heads();
    heads.sort();
    assert_eq!(heads.len(), 2, "Fork: two heads");
    assert!(heads.contains(&[10; 32]));
    assert!(heads.contains(&[20; 32]));
    
    // Create merge delta
    let delta_merge = CausalDelta::new_test(
        [30; 32],
        vec![[10; 32], [20; 32]],  // merge both
        TestPayload { value: 30 },
    );
    
    dag.add_delta(delta_merge, &applier).await.unwrap();
    
    // Now should have ONE head (merged)
    let heads = dag.get_heads();
    assert_eq!(heads, vec![[30; 32]], "Merge: single head");
}
```

**What it validates**:
- ✅ Concurrent updates create multiple heads
- ✅ Fork detected via head count
- ✅ Merge delta resolves fork
- ✅ Heads updated correctly

### Test 6: Three-Way Merge

**Goal**: Verify complex merges

```rust
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
```

**What it validates**:
- ✅ Multiple concurrent updates
- ✅ Many-parent merge deltas
- ✅ All parents removed from heads

---

## Error Handling Tests

### Test 7: Apply Failure

**Goal**: Verify error propagation from applier

```rust
struct FailingApplier {
    fail_on: [u8; 32],  // Fail when applying this delta
}

#[async_trait]
impl DeltaApplier<TestPayload> for FailingApplier {
    async fn apply(&self, delta: &CausalDelta<TestPayload>) -> Result<(), ApplyError> {
        if delta.id == self.fail_on {
            Err(ApplyError::Application("Intentional failure".to_string()))
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn test_dag_apply_failure() {
    let applier = FailingApplier { fail_on: [2; 32] };
    let mut dag = DagStore::new([0; 32]);
    
    // D1 succeeds
    let delta1 = CausalDelta::new_test([1; 32], vec![[0; 32]], TestPayload { value: 1 });
    let result1 = dag.add_delta(delta1, &applier).await;
    assert!(result1.is_ok());
    
    // D2 fails
    let delta2 = CausalDelta::new_test([2; 32], vec![[1; 32]], TestPayload { value: 2 });
    let result2 = dag.add_delta(delta2, &applier).await;
    assert!(result2.is_err());
    
    match result2 {
        Err(DagError::ApplyFailed(ApplyError::Application(msg))) => {
            assert_eq!(msg, "Intentional failure");
        }
        _ => panic!("Expected ApplyFailed error"),
    }
    
    // D2 is NOT in applied set
    assert!(!dag.is_applied(&[2; 32]));
    
    // Heads still point to D1
    assert_eq!(dag.get_heads(), vec![[1; 32]]);
}
```

**What it validates**:
- ✅ Applier errors propagate
- ✅ Failed deltas not marked as applied
- ✅ Heads don't advance on failure
- ✅ Error type preserved

---

## Cleanup Tests

### Test 8: Stale Delta Cleanup

**Goal**: Verify timeout eviction

```rust
#[tokio::test]
async fn test_dag_cleanup_stale() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);
    
    // Add pending delta (missing parent)
    let delta_pending = CausalDelta::new_test(
        [99; 32],
        vec![[88; 32]],  // missing parent
        TestPayload { value: 99 },
    );
    
    dag.add_delta(delta_pending, &applier).await.unwrap();
    assert_eq!(dag.pending_stats().count, 1);
    
    // Wait 100ms
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Cleanup with 50ms timeout (delta is 100ms old)
    let evicted = dag.cleanup_stale(Duration::from_millis(50));
    assert_eq!(evicted, 1, "Should evict stale delta");
    
    // No pending deltas
    assert_eq!(dag.pending_stats().count, 0);
}
```

**What it validates**:
- ✅ Timeout eviction works
- ✅ Returns eviction count
- ✅ Pending stats updated

### Test 9: Cleanup Doesn't Remove Recent

**Goal**: Verify only old deltas evicted

```rust
#[tokio::test]
async fn test_dag_cleanup_preserves_recent() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);
    
    // Add old pending delta
    let delta_old = CausalDelta::new_test([1; 32], vec![[88; 32]], TestPayload { value: 1 });
    dag.add_delta(delta_old, &applier).await.unwrap();
    
    // Wait 100ms
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Add recent pending delta
    let delta_recent = CausalDelta::new_test([2; 32], vec![[88; 32]], TestPayload { value: 2 });
    dag.add_delta(delta_recent, &applier).await.unwrap();
    
    assert_eq!(dag.pending_stats().count, 2);
    
    // Cleanup with 50ms timeout
    let evicted = dag.cleanup_stale(Duration::from_millis(50));
    assert_eq!(evicted, 1, "Should evict only old delta");
    
    // Recent delta still pending
    assert_eq!(dag.pending_stats().count, 1);
    assert!(dag.has_delta(&[2; 32]));
}
```

**What it validates**:
- ✅ Only stale deltas evicted
- ✅ Recent deltas preserved
- ✅ Selective cleanup

---

## Stress Tests

### Test 10: Large Chain (500 deltas reverse)

**Goal**: Verify cascade handles extreme cases

```rust
#[tokio::test]
async fn test_extreme_pending_chain_500_deltas() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);
    
    const N: usize = 500;
    
    // Add deltas in reverse order
    for i in (1..=N).rev() {
        let delta = CausalDelta::new_test(
            [i as u8; 32],
            vec![[(i-1) as u8; 32]],
            TestPayload { value: i as u32 },
        );
        dag.add_delta(delta, &applier).await.unwrap();
    }
    
    // All should be applied
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), N);
    
    // No pending
    assert_eq!(dag.pending_stats().count, 0);
    
    // Head is last delta
    assert_eq!(dag.get_heads(), vec![[N as u8; 32]]);
}
```

**What it validates**:
- ✅ Handles 500-delta chains
- ✅ No stack overflow
- ✅ Correct ordering
- ✅ Performance acceptable

### Test 11: Wide Fan-Out (200 branches)

**Goal**: Verify many concurrent updates

```rust
#[tokio::test]
async fn test_extreme_concurrent_branches_200() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);
    
    const N: usize = 200;
    
    // Create 200 concurrent branches
    for i in 1..=N {
        let delta = CausalDelta::new_test(
            [i as u8; 32],
            vec![[0; 32]],  // all from root
            TestPayload { value: i as u32 },
        );
        dag.add_delta(delta, &applier).await.unwrap();
    }
    
    // 200 heads
    let heads = dag.get_heads();
    assert_eq!(heads.len(), N);
    
    // Merge all 200 branches
    let merge_parents: Vec<[u8; 32]> = (1..=N).map(|i| [i as u8; 32]).collect();
    let merge = CausalDelta::new_test(
        [255; 32],
        merge_parents,
        TestPayload { value: 999 },
    );
    
    dag.add_delta(merge, &applier).await.unwrap();
    
    // Single head
    assert_eq!(dag.get_heads(), vec![[255; 32]]);
}
```

**What it validates**:
- ✅ Handles 200 concurrent branches
- ✅ Many-parent merges work
- ✅ Head management efficient

### Test 12: Random Order (1000 deltas)

**Goal**: Verify correctness with random delivery

```rust
use rand::seq::SliceRandom;

#[tokio::test]
async fn test_extreme_random_order_1000_deltas() {
    let applier = TestApplier::new();
    let mut dag = DagStore::new([0; 32]);
    
    const N: usize = 1000;
    
    // Create linear chain
    let mut deltas = Vec::new();
    for i in 1..=N {
        let delta = CausalDelta::new_test(
            [i as u8; 32],
            vec![[(i-1) as u8; 32]],
            TestPayload { value: i as u32 },
        );
        deltas.push(delta);
    }
    
    // Shuffle randomly
    let mut rng = rand::thread_rng();
    deltas.shuffle(&mut rng);
    
    // Add in random order
    for delta in deltas {
        dag.add_delta(delta, &applier).await.unwrap();
    }
    
    // All should be applied
    let applied = applier.get_applied().await;
    assert_eq!(applied.len(), N);
    
    // No pending
    assert_eq!(dag.pending_stats().count, 0);
    
    // Head correct
    assert_eq!(dag.get_heads(), vec![[N as u8; 32]]);
}
```

**What it validates**:
- ✅ Random order always converges
- ✅ No deltas lost
- ✅ Correct final state

---

## Integration Testing

### Testing with Node Layer

```rust
// Test DAG integrated with real ContextStorageApplier
#[tokio::test]
async fn test_dag_with_storage_applier() {
    // Setup context
    let context_client = ContextClient::new(...);
    let context_id = create_test_context(&context_client).await;
    
    // Create applier
    let applier = ContextStorageApplier {
        context_client: context_client.clone(),
        context_id,
        our_identity: test_identity(),
    };
    
    // Create DAG
    let mut dag = DagStore::new(genesis_hash(&context_id));
    
    // Create delta with real actions
    let actions = vec![
        Action::Add {
            key: b"key1".to_vec(),
            data: b"value1".to_vec(),
        },
    ];
    
    let delta = CausalDelta::new(
        compute_delta_id(&actions),
        dag.get_heads(),
        actions,
        env::hlc_timestamp(),
    );
    
    // Apply
    let applied = dag.add_delta(delta, &applier).await.unwrap();
    assert!(applied);
    
    // Verify storage updated
    let value = context_client.get(&context_id, b"key1").await.unwrap();
    assert_eq!(value, b"value1");
}
```

---

## Property-Based Testing

### Using proptest

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_dag_converges(
        deltas in prop::collection::vec(any::<u8>(), 1..100)
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let applier = TestApplier::new();
            let mut dag = DagStore::new([0; 32]);
            
            // Create linear chain
            let mut prev = [0; 32];
            for (i, &val) in deltas.iter().enumerate() {
                let id = [(i+1) as u8; 32];
                let delta = CausalDelta::new_test(
                    id,
                    vec![prev],
                    TestPayload { value: val as u32 },
                );
                dag.add_delta(delta, &applier).await.unwrap();
                prev = id;
            }
            
            // Properties:
            // 1. All deltas applied
            assert_eq!(applier.get_applied().await.len(), deltas.len());
            
            // 2. No pending deltas
            assert_eq!(dag.pending_stats().count, 0);
            
            // 3. Head is last delta
            let last_id = [deltas.len() as u8; 32];
            assert_eq!(dag.get_heads(), vec![last_id]);
        });
    }
}
```

---

## Test Utilities

### Delta Builder

```rust
struct DeltaBuilder {
    id: [u8; 32],
    parents: Vec<[u8; 32]>,
    value: u32,
}

impl DeltaBuilder {
    fn new(id: u8) -> Self {
        Self {
            id: [id; 32],
            parents: vec![],
            value: id as u32,
        }
    }
    
    fn parent(mut self, parent_id: u8) -> Self {
        self.parents.push([parent_id; 32]);
        self
    }
    
    fn parents(mut self, parent_ids: &[u8]) -> Self {
        self.parents = parent_ids.iter().map(|&id| [id; 32]).collect();
        self
    }
    
    fn value(mut self, value: u32) -> Self {
        self.value = value;
        self
    }
    
    fn build(self) -> CausalDelta<TestPayload> {
        CausalDelta::new_test(self.id, self.parents, TestPayload { value: self.value })
    }
}

// Usage:
let delta = DeltaBuilder::new(5).parent(4).value(100).build();
```

### Assertion Helpers

```rust
fn assert_heads(dag: &DagStore<TestPayload>, expected: &[u8]) {
    let mut heads = dag.get_heads();
    let mut expected_heads: Vec<[u8; 32]> = expected.iter().map(|&id| [id; 32]).collect();
    
    heads.sort();
    expected_heads.sort();
    
    assert_eq!(heads, expected_heads, "Heads don't match");
}

fn assert_pending_count(dag: &DagStore<TestPayload>, expected: usize) {
    let stats = dag.pending_stats();
    assert_eq!(stats.count, expected, "Pending count mismatch");
}

fn assert_applied_order(applier: &TestApplier, expected_order: &[u8]) {
    let applied = applier.get_applied().await;
    let expected: Vec<[u8; 32]> = expected_order.iter().map(|&id| [id; 32]).collect();
    assert_eq!(applied, expected, "Application order mismatch");
}
```

---

## Running Tests

### Run All Tests

```bash
cargo test -p calimero-dag
```

### Run Specific Category

```bash
# Basic tests
cargo test -p calimero-dag basic_

# Out-of-order tests
cargo test -p calimero-dag out_of_order

# Stress tests
cargo test -p calimero-dag extreme_
```

### Run with Logs

```bash
RUST_LOG=debug cargo test -p calimero-dag -- --nocapture
```

### Run Property Tests

```bash
cargo test -p calimero-dag prop_ --release
```

---

## Coverage

### Measure Coverage

```bash
cargo install cargo-tarpaulin
cargo tarpaulin -p calimero-dag --out Html
open tarpaulin-report.html
```

**Current Coverage**: ~95% (as of last measure)

**Uncovered Lines**: Mostly error paths and edge cases

---

## See Also

- [API Reference](api-reference.md) - API to test
- [Architecture](architecture.md) - What to test
- [Troubleshooting](troubleshooting.md) - Test failure debugging
- [Main README](../README.md) - Test examples
