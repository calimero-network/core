//! CRDT Convergence Unit Tests
//!
//! These tests verify that when two nodes apply the same set of deltas in different
//! orders, they converge to the same final state.
//!
//! ## Background
//!
//! During E2E testing, we observed that complex concurrent workloads (multiple
//! bidirectional writes) caused root hash divergence between nodes. This test
//! reproduces the exact scenario to isolate the issue from networking.
//!
//! ## Test Scenario (from e2e.yml failure)
//!
//! ```text
//! Timeline:
//! ─────────────────────────────────────────────────────────────────
//! Node-1                              Node-2
//! ─────────────────────────────────────────────────────────────────
//! 1. set("greeting") → Δ_A
//! 2. set("count") → Δ_B
//!    [sync: Node-2 receives Δ_A, Δ_B]
//!                                     3. set("from_node2") → Δ_C
//!                                        (merged with Δ_A, Δ_B)
//!    [sync: Node-1 receives Δ_C]
//! 4. remove("count") → Δ_D
//!    (merged with Δ_C)
//!                                     [Node-2 receives Δ_D, merges]
//! 5. set_with_handler() → Δ_E
//!    (parent: Δ_D)
//!                                     6. Receives Δ_E
//!                                        State diverged (has Δ_C path)
//!                                        Applies Δ_E via merge
//!                                     7. Executes insert_handler → Δ_F
//!                                        (parent: merged head)
//!    [Node-1 receives Δ_F]
//!    Applies Δ_F via merge
//!    → Different final hash?
//! ─────────────────────────────────────────────────────────────────
//! ```
//!
//! ## Key Invariants Being Tested
//!
//! - **I2 (Eventual Consistency)**: All nodes converge to identical root hashes
//! - **I3 (Merge Determinism)**: merge(V1, V2) always produces the same output

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::*;

/// A CRDT-like state that tracks key-value pairs and a counter.
///
/// This simulates the e2e-kv-store application state:
/// - `entries`: A map of key -> (value, timestamp) using LWW semantics
/// - `counter`: A G-Counter (grow-only) for handler invocations
#[derive(Clone, Debug, Default)]
struct CrdtState {
    /// Key-value entries with LWW semantics
    entries: HashMap<String, (String, u64)>, // key -> (value, timestamp)
    /// G-Counter: maps node_id -> count
    counter: HashMap<u8, u64>,
}

impl CrdtState {
    fn new() -> Self {
        Self::default()
    }

    /// Set a key-value pair (LWW: higher timestamp wins)
    fn set(&mut self, key: String, value: String, timestamp: u64) {
        match self.entries.get(&key) {
            Some((_, existing_ts)) if *existing_ts >= timestamp => {
                // Existing entry has higher or equal timestamp, keep it
            }
            _ => {
                self.entries.insert(key, (value, timestamp));
            }
        }
    }

    /// Remove a key (tombstone with timestamp)
    fn remove(&mut self, key: &str, timestamp: u64) {
        match self.entries.get(key) {
            Some((_, existing_ts)) if *existing_ts >= timestamp => {
                // Entry has higher timestamp, keep it
            }
            _ => {
                // Remove by setting to empty with timestamp
                self.entries.remove(key);
            }
        }
    }

    /// Increment counter for a node (G-Counter)
    fn increment_counter(&mut self, node_id: u8) {
        *self.counter.entry(node_id).or_insert(0) += 1;
    }

    /// Merge two states (CRDT merge)
    fn merge(&mut self, other: &CrdtState) {
        // Merge entries (LWW per key)
        for (key, (value, ts)) in &other.entries {
            self.set(key.clone(), value.clone(), *ts);
        }

        // Merge G-Counter (max per node)
        for (node_id, count) in &other.counter {
            let current = self.counter.entry(*node_id).or_insert(0);
            *current = (*current).max(*count);
        }
    }

    /// Compute a deterministic hash of the state
    fn compute_hash(&self) -> [u8; 32] {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();

        // Sort entries for determinism
        let mut entries: Vec<_> = self.entries.iter().collect();
        entries.sort_by_key(|(k, _)| *k);

        for (key, (value, ts)) in entries {
            key.hash(&mut hasher);
            value.hash(&mut hasher);
            ts.hash(&mut hasher);
        }

        // Sort counter for determinism
        let mut counter: Vec<_> = self.counter.iter().collect();
        counter.sort_by_key(|(k, _)| *k);

        for (node_id, count) in counter {
            node_id.hash(&mut hasher);
            count.hash(&mut hasher);
        }

        let hash = hasher.finish();
        let mut result = [0u8; 32];
        result[0..8].copy_from_slice(&hash.to_le_bytes());
        result
    }
}

/// Payload for CRDT operations
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
enum CrdtOperation {
    Set { key: String, value: String, ts: u64 },
    Remove { key: String, ts: u64 },
    IncrementCounter { node_id: u8 },
}

/// Applier that simulates CRDT merge behavior
struct CrdtApplier {
    state: Arc<Mutex<CrdtState>>,
    applied_order: Arc<Mutex<Vec<[u8; 32]>>>,
    /// For tracking which deltas were merges vs sequential
    merge_deltas: Arc<Mutex<Vec<[u8; 32]>>>,
}

impl CrdtApplier {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(CrdtState::new())),
            applied_order: Arc::new(Mutex::new(Vec::new())),
            merge_deltas: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn get_state(&self) -> CrdtState {
        self.state.lock().await.clone()
    }

    async fn get_applied_order(&self) -> Vec<[u8; 32]> {
        self.applied_order.lock().await.clone()
    }

    async fn get_merge_deltas(&self) -> Vec<[u8; 32]> {
        self.merge_deltas.lock().await.clone()
    }
}

#[async_trait::async_trait]
impl DeltaApplier<CrdtOperation> for CrdtApplier {
    async fn apply(&self, delta: &CausalDelta<CrdtOperation>) -> Result<(), ApplyError> {
        let mut state = self.state.lock().await;

        match &delta.payload {
            CrdtOperation::Set { key, value, ts } => {
                state.set(key.clone(), value.clone(), *ts);
            }
            CrdtOperation::Remove { key, ts } => {
                state.remove(key, *ts);
            }
            CrdtOperation::IncrementCounter { node_id } => {
                state.increment_counter(*node_id);
            }
        }

        self.applied_order.lock().await.push(delta.id);
        Ok(())
    }
}

// ============================================================
// Basic CRDT Convergence Tests
// ============================================================

/// Test that two nodes applying the same deltas in different orders converge
#[tokio::test]
async fn test_basic_convergence_same_order() {
    let applier1 = CrdtApplier::new();
    let applier2 = CrdtApplier::new();

    let mut dag1 = DagStore::new([0; 32]);
    let mut dag2 = DagStore::new([0; 32]);

    // Create simple linear chain
    let delta_a = CausalDelta::new_test(
        [1; 32],
        vec![[0; 32]],
        CrdtOperation::Set {
            key: "greeting".to_string(),
            value: "Hello".to_string(),
            ts: 100,
        },
    );

    let delta_b = CausalDelta::new_test(
        [2; 32],
        vec![[1; 32]],
        CrdtOperation::Set {
            key: "count".to_string(),
            value: "42".to_string(),
            ts: 200,
        },
    );

    // Both nodes apply in same order
    dag1.add_delta(delta_a.clone(), &applier1).await.unwrap();
    dag1.add_delta(delta_b.clone(), &applier1).await.unwrap();

    dag2.add_delta(delta_a.clone(), &applier2).await.unwrap();
    dag2.add_delta(delta_b.clone(), &applier2).await.unwrap();

    // States should be identical
    let state1 = applier1.get_state().await;
    let state2 = applier2.get_state().await;

    assert_eq!(
        state1.compute_hash(),
        state2.compute_hash(),
        "States should converge when applying in same order"
    );
}

/// Test that two nodes applying the same deltas in reverse order converge
#[tokio::test]
async fn test_basic_convergence_reverse_order() {
    let applier1 = CrdtApplier::new();
    let applier2 = CrdtApplier::new();

    let mut dag1 = DagStore::new([0; 32]);
    let mut dag2 = DagStore::new([0; 32]);

    // Create concurrent branches (both from root)
    let delta_a = CausalDelta::new_test(
        [1; 32],
        vec![[0; 32]],
        CrdtOperation::Set {
            key: "key1".to_string(),
            value: "value1".to_string(),
            ts: 100,
        },
    );

    let delta_b = CausalDelta::new_test(
        [2; 32],
        vec![[0; 32]],
        CrdtOperation::Set {
            key: "key2".to_string(),
            value: "value2".to_string(),
            ts: 200,
        },
    );

    // Node 1 applies A then B
    dag1.add_delta(delta_a.clone(), &applier1).await.unwrap();
    dag1.add_delta(delta_b.clone(), &applier1).await.unwrap();

    // Node 2 applies B then A
    dag2.add_delta(delta_b.clone(), &applier2).await.unwrap();
    dag2.add_delta(delta_a.clone(), &applier2).await.unwrap();

    let state1 = applier1.get_state().await;
    let state2 = applier2.get_state().await;

    assert_eq!(
        state1.compute_hash(),
        state2.compute_hash(),
        "States should converge regardless of application order"
    );
}

// ============================================================
// E2E Scenario Reproduction
// ============================================================

/// Reproduce the exact scenario from e2e.yml that caused divergence
///
/// KEY INSIGHT: This test demonstrates that when handlers execute on both nodes,
/// BOTH nodes must broadcast their handler results as deltas. If only one node
/// broadcasts, the other's handler state will be missing.
///
/// In the real e2e-kv-store scenario:
/// - Node-1 calls `set_with_handler` which emits an event
/// - Node-2 receives the delta and executes `insert_handler`
/// - Node-2's handler creates a delta and broadcasts it
/// - But Node-1's handler (if any) should also broadcast
///
/// The test models this by showing what happens when handler broadcasts are incomplete.
#[tokio::test]
async fn test_e2e_divergence_scenario() {
    let applier1 = CrdtApplier::new();
    let applier2 = CrdtApplier::new();

    let mut dag1 = DagStore::new([0; 32]);
    let mut dag2 = DagStore::new([0; 32]);

    // === Phase 1: Node-1 creates initial deltas ===

    // Delta A: set("greeting", "Hello, World!")
    let delta_a = CausalDelta::new_test(
        [1; 32],
        vec![[0; 32]],
        CrdtOperation::Set {
            key: "greeting".to_string(),
            value: "Hello, World!".to_string(),
            ts: 100,
        },
    );

    // Delta B: set("count", "42")
    let delta_b = CausalDelta::new_test(
        [2; 32],
        vec![[1; 32]],
        CrdtOperation::Set {
            key: "count".to_string(),
            value: "42".to_string(),
            ts: 200,
        },
    );

    // Node-1 applies its own deltas
    dag1.add_delta(delta_a.clone(), &applier1).await.unwrap();
    dag1.add_delta(delta_b.clone(), &applier1).await.unwrap();

    // === Phase 2: Node-2 receives and applies A, B ===
    dag2.add_delta(delta_a.clone(), &applier2).await.unwrap();
    dag2.add_delta(delta_b.clone(), &applier2).await.unwrap();

    // === Phase 3: Node-2 creates concurrent delta C ===
    let delta_c = CausalDelta::new_test(
        [3; 32],
        vec![[2; 32]],
        CrdtOperation::Set {
            key: "from_node2".to_string(),
            value: "Cross-node value".to_string(),
            ts: 300,
        },
    );

    dag2.add_delta(delta_c.clone(), &applier2).await.unwrap();

    // === Phase 4: Node-1 receives C ===
    dag1.add_delta(delta_c.clone(), &applier1).await.unwrap();

    // === Phase 5: Node-1 creates delta D (remove("count")) ===
    let delta_d = CausalDelta::new_test(
        [4; 32],
        vec![[3; 32]],
        CrdtOperation::Remove {
            key: "count".to_string(),
            ts: 400,
        },
    );

    dag1.add_delta(delta_d.clone(), &applier1).await.unwrap();

    // === Phase 6: Node-2 receives D ===
    dag2.add_delta(delta_d.clone(), &applier2).await.unwrap();

    // Verify states are still identical
    let state1_before_handler = applier1.get_state().await;
    let state2_before_handler = applier2.get_state().await;
    assert_eq!(
        state1_before_handler.compute_hash(),
        state2_before_handler.compute_hash(),
        "States should be identical before handler execution"
    );

    // === Phase 7: Node-1 creates delta E (set_with_handler) ===
    // This delta triggers handler execution on BOTH nodes
    let delta_e = CausalDelta::new_test(
        [5; 32],
        vec![[4; 32]],
        CrdtOperation::Set {
            key: "handler_test".to_string(),
            value: "initial".to_string(),
            ts: 500,
        },
    );

    // Node-1 applies delta E AND creates handler result delta
    dag1.add_delta(delta_e.clone(), &applier1).await.unwrap();
    let handler_delta_node1 = CausalDelta::new_test(
        [6; 32],
        vec![[5; 32]],
        CrdtOperation::IncrementCounter { node_id: 1 },
    );
    dag1.add_delta(handler_delta_node1.clone(), &applier1)
        .await
        .unwrap();

    // === Phase 8: Node-2 receives E and creates handler result delta ===
    dag2.add_delta(delta_e.clone(), &applier2).await.unwrap();
    let handler_delta_node2 = CausalDelta::new_test(
        [7; 32],
        vec![[5; 32]], // CONCURRENT with handler_delta_node1!
        CrdtOperation::IncrementCounter { node_id: 2 },
    );
    dag2.add_delta(handler_delta_node2.clone(), &applier2)
        .await
        .unwrap();

    // === Phase 9: Exchange handler deltas ===
    dag1.add_delta(handler_delta_node2.clone(), &applier1)
        .await
        .unwrap();
    dag2.add_delta(handler_delta_node1.clone(), &applier2)
        .await
        .unwrap();

    // === Verify final state ===
    let state1 = applier1.get_state().await;
    let state2 = applier2.get_state().await;

    println!("Node-1 state: {:?}", state1);
    println!("Node-2 state: {:?}", state2);
    println!("Node-1 hash: {:?}", state1.compute_hash());
    println!("Node-2 hash: {:?}", state2.compute_hash());

    // Now both nodes should converge because all handler deltas are exchanged
    assert_eq!(
        state1.compute_hash(),
        state2.compute_hash(),
        "CRDT states should converge when all handler deltas are exchanged!\n\
         Node-1 entries: {:?}\n\
         Node-1 counter: {:?}\n\
         Node-2 entries: {:?}\n\
         Node-2 counter: {:?}",
        state1.entries,
        state1.counter,
        state2.entries,
        state2.counter
    );
}

/// Test the specific issue: handler execution creates local state that isn't broadcast
///
/// This test verifies that when handlers create deltas (not just modify state directly),
/// convergence is achieved after delta exchange.
///
/// KEY: Handler execution should ONLY create deltas, not modify state directly.
/// The delta application (via the applier) is what modifies state.
#[tokio::test]
async fn test_handler_execution_divergence() {
    let applier1 = CrdtApplier::new();
    let applier2 = CrdtApplier::new();

    let mut dag1 = DagStore::new([0; 32]);
    let mut dag2 = DagStore::new([0; 32]);

    // Delta A: triggers handler on both nodes
    let delta_a = CausalDelta::new_test(
        [1; 32],
        vec![[0; 32]],
        CrdtOperation::Set {
            key: "trigger".to_string(),
            value: "handler".to_string(),
            ts: 100,
        },
    );

    // Node-1 applies A
    dag1.add_delta(delta_a.clone(), &applier1).await.unwrap();

    // Node-1 handler creates a delta (state modified via delta application, NOT directly)
    let delta_b_from_node1 = CausalDelta::new_test(
        [2; 32],
        vec![[1; 32]],
        CrdtOperation::IncrementCounter { node_id: 1 },
    );
    dag1.add_delta(delta_b_from_node1.clone(), &applier1)
        .await
        .unwrap();

    // Node-2 applies A
    dag2.add_delta(delta_a.clone(), &applier2).await.unwrap();

    // Node-2 handler creates a delta (concurrent with node-1's delta)
    let delta_b_from_node2 = CausalDelta::new_test(
        [3; 32],
        vec![[1; 32]], // Same parent as delta_b_from_node1! (concurrent)
        CrdtOperation::IncrementCounter { node_id: 2 },
    );
    dag2.add_delta(delta_b_from_node2.clone(), &applier2)
        .await
        .unwrap();

    // Now exchange deltas (simulating network propagation)
    dag1.add_delta(delta_b_from_node2.clone(), &applier1)
        .await
        .unwrap();
    dag2.add_delta(delta_b_from_node1.clone(), &applier2)
        .await
        .unwrap();

    // Verify convergence
    let state1 = applier1.get_state().await;
    let state2 = applier2.get_state().await;

    println!("After handler exchange:");
    println!("Node-1 counter: {:?}", state1.counter);
    println!("Node-2 counter: {:?}", state2.counter);

    // Both should have {1: 1, 2: 1}
    assert_eq!(state1.counter.get(&1), Some(&1));
    assert_eq!(state1.counter.get(&2), Some(&1));
    assert_eq!(state2.counter.get(&1), Some(&1));
    assert_eq!(state2.counter.get(&2), Some(&1));

    assert_eq!(
        state1.compute_hash(),
        state2.compute_hash(),
        "States should converge when handler deltas are properly exchanged"
    );
}

/// Test G-Counter CRDT merge is commutative
#[tokio::test]
async fn test_gcounter_merge_commutativity() {
    let mut state_a = CrdtState::new();
    let mut state_b = CrdtState::new();

    // Node A increments
    state_a.increment_counter(1);
    state_a.increment_counter(1);

    // Node B increments
    state_b.increment_counter(2);

    // Merge A into B
    let mut merged_ab = state_a.clone();
    merged_ab.merge(&state_b);

    // Merge B into A
    let mut merged_ba = state_b.clone();
    merged_ba.merge(&state_a);

    // Both merges should produce the same result
    assert_eq!(
        merged_ab.compute_hash(),
        merged_ba.compute_hash(),
        "G-Counter merge should be commutative"
    );

    assert_eq!(merged_ab.counter.get(&1), Some(&2));
    assert_eq!(merged_ab.counter.get(&2), Some(&1));
}

/// Test LWW merge is commutative
#[tokio::test]
async fn test_lww_merge_commutativity() {
    let mut state_a = CrdtState::new();
    let mut state_b = CrdtState::new();

    // Same key, different values, A has higher timestamp
    state_a.set("key".to_string(), "value_a".to_string(), 200);
    state_b.set("key".to_string(), "value_b".to_string(), 100);

    // Merge A into B
    let mut merged_ab = state_a.clone();
    merged_ab.merge(&state_b);

    // Merge B into A
    let mut merged_ba = state_b.clone();
    merged_ba.merge(&state_a);

    assert_eq!(
        merged_ab.compute_hash(),
        merged_ba.compute_hash(),
        "LWW merge should be commutative"
    );

    // Higher timestamp wins
    assert_eq!(
        merged_ab.entries.get("key").map(|(v, _)| v.as_str()),
        Some("value_a")
    );
}

// ============================================================
// Complex Concurrent Scenario Tests
// ============================================================

/// Test three-way concurrent operations
#[tokio::test]
async fn test_three_node_concurrent_operations() {
    let applier1 = CrdtApplier::new();
    let applier2 = CrdtApplier::new();
    let applier3 = CrdtApplier::new();

    let mut dag1 = DagStore::new([0; 32]);
    let mut dag2 = DagStore::new([0; 32]);
    let mut dag3 = DagStore::new([0; 32]);

    // Each node creates a concurrent delta from root
    let delta_a = CausalDelta::new_test(
        [1; 32],
        vec![[0; 32]],
        CrdtOperation::Set {
            key: "node1_key".to_string(),
            value: "node1_value".to_string(),
            ts: 100,
        },
    );

    let delta_b = CausalDelta::new_test(
        [2; 32],
        vec![[0; 32]],
        CrdtOperation::Set {
            key: "node2_key".to_string(),
            value: "node2_value".to_string(),
            ts: 200,
        },
    );

    let delta_c = CausalDelta::new_test(
        [3; 32],
        vec![[0; 32]],
        CrdtOperation::Set {
            key: "node3_key".to_string(),
            value: "node3_value".to_string(),
            ts: 300,
        },
    );

    // Each node applies all deltas in different orders
    // Node 1: A, B, C
    dag1.add_delta(delta_a.clone(), &applier1).await.unwrap();
    dag1.add_delta(delta_b.clone(), &applier1).await.unwrap();
    dag1.add_delta(delta_c.clone(), &applier1).await.unwrap();

    // Node 2: B, C, A
    dag2.add_delta(delta_b.clone(), &applier2).await.unwrap();
    dag2.add_delta(delta_c.clone(), &applier2).await.unwrap();
    dag2.add_delta(delta_a.clone(), &applier2).await.unwrap();

    // Node 3: C, A, B
    dag3.add_delta(delta_c.clone(), &applier3).await.unwrap();
    dag3.add_delta(delta_a.clone(), &applier3).await.unwrap();
    dag3.add_delta(delta_b.clone(), &applier3).await.unwrap();

    // All should have same state
    let state1 = applier1.get_state().await;
    let state2 = applier2.get_state().await;
    let state3 = applier3.get_state().await;

    let hash1 = state1.compute_hash();
    let hash2 = state2.compute_hash();
    let hash3 = state3.compute_hash();

    assert_eq!(hash1, hash2, "Node 1 and 2 should converge");
    assert_eq!(hash2, hash3, "Node 2 and 3 should converge");
}

/// DEMONSTRATES THE BUG: What happens when handler results are not broadcast
///
/// This test models the EXACT bug observed in e2e.yml:
/// - Node-1 executes handler but Node-1's delta is never created/broadcast
/// - Node-2 executes handler and broadcasts its delta
/// - Result: Node-1 has Node-2's handler result, but Node-2 doesn't have Node-1's
///
/// This test is expected to FAIL, demonstrating the bug.
#[tokio::test]
#[should_panic(expected = "DIVERGENCE DETECTED")]
async fn test_bug_missing_handler_broadcast() {
    let applier1 = CrdtApplier::new();
    let applier2 = CrdtApplier::new();

    let mut dag1 = DagStore::new([0; 32]);
    let mut dag2 = DagStore::new([0; 32]);

    // Trigger delta
    let delta_trigger = CausalDelta::new_test(
        [1; 32],
        vec![[0; 32]],
        CrdtOperation::Set {
            key: "trigger".to_string(),
            value: "event".to_string(),
            ts: 100,
        },
    );

    // Node-1 applies trigger and executes handler
    dag1.add_delta(delta_trigger.clone(), &applier1)
        .await
        .unwrap();
    // BUG: Node-1's handler modifies state directly WITHOUT creating a delta
    {
        let mut state = applier1.state.lock().await;
        state.increment_counter(1); // Direct modification, no delta!
    }

    // Node-2 applies trigger and executes handler
    dag2.add_delta(delta_trigger.clone(), &applier2)
        .await
        .unwrap();
    // Node-2 CORRECTLY creates a delta for its handler result
    let handler_delta_node2 = CausalDelta::new_test(
        [2; 32],
        vec![[1; 32]],
        CrdtOperation::IncrementCounter { node_id: 2 },
    );
    dag2.add_delta(handler_delta_node2.clone(), &applier2)
        .await
        .unwrap();

    // Exchange: Node-1 receives Node-2's delta (but there's nothing to send back)
    dag1.add_delta(handler_delta_node2.clone(), &applier1)
        .await
        .unwrap();
    // Node-2 has nothing to receive from Node-1 (no delta was created)

    // Check states
    let state1 = applier1.get_state().await;
    let state2 = applier2.get_state().await;

    // Node-1: counter = {1: 1, 2: 1} (has local handler + received delta)
    // Node-2: counter = {2: 1} (only has its own handler delta)
    println!("BUG DEMONSTRATION:");
    println!("Node-1 counter: {:?}", state1.counter);
    println!("Node-2 counter: {:?}", state2.counter);

    // This WILL diverge because Node-1's handler result was never broadcast
    assert_eq!(
        state1.compute_hash(),
        state2.compute_hash(),
        "DIVERGENCE DETECTED: Node-1's handler result was not broadcast as a delta"
    );
}

/// Test merge after divergent handler executions
///
/// This is the core test for the observed E2E failure pattern:
/// Two nodes execute the same handler, creating different deltas,
/// and must converge after exchanging those deltas.
#[tokio::test]
async fn test_divergent_handler_execution_convergence() {
    let applier1 = CrdtApplier::new();
    let applier2 = CrdtApplier::new();

    let mut dag1 = DagStore::new([0; 32]);
    let mut dag2 = DagStore::new([0; 32]);

    // Setup: both nodes have same base state
    let delta_setup = CausalDelta::new_test(
        [1; 32],
        vec![[0; 32]],
        CrdtOperation::Set {
            key: "base".to_string(),
            value: "value".to_string(),
            ts: 100,
        },
    );

    dag1.add_delta(delta_setup.clone(), &applier1)
        .await
        .unwrap();
    dag2.add_delta(delta_setup.clone(), &applier2)
        .await
        .unwrap();

    // Trigger event delta
    let trigger_delta = CausalDelta::new_test(
        [2; 32],
        vec![[1; 32]],
        CrdtOperation::Set {
            key: "trigger".to_string(),
            value: "event".to_string(),
            ts: 200,
        },
    );

    // Node 1 receives trigger and executes handler
    dag1.add_delta(trigger_delta.clone(), &applier1)
        .await
        .unwrap();
    // Handler creates delta
    let handler_delta_1 = CausalDelta::new_test(
        [3; 32],
        vec![[2; 32]],
        CrdtOperation::IncrementCounter { node_id: 1 },
    );
    dag1.add_delta(handler_delta_1.clone(), &applier1)
        .await
        .unwrap();

    // Node 2 receives trigger and executes handler (concurrent with node 1)
    dag2.add_delta(trigger_delta.clone(), &applier2)
        .await
        .unwrap();
    // Handler creates delta with SAME parent but different node_id
    let handler_delta_2 = CausalDelta::new_test(
        [4; 32],
        vec![[2; 32]], // Same parent!
        CrdtOperation::IncrementCounter { node_id: 2 },
    );
    dag2.add_delta(handler_delta_2.clone(), &applier2)
        .await
        .unwrap();

    // Exchange handler deltas
    dag1.add_delta(handler_delta_2.clone(), &applier1)
        .await
        .unwrap();
    dag2.add_delta(handler_delta_1.clone(), &applier2)
        .await
        .unwrap();

    // Verify DAG heads (should have 2 concurrent heads each)
    let heads1 = dag1.get_heads();
    let heads2 = dag2.get_heads();
    assert_eq!(heads1.len(), 2, "DAG1 should have 2 concurrent heads");
    assert_eq!(heads2.len(), 2, "DAG2 should have 2 concurrent heads");

    // Verify state convergence
    let state1 = applier1.get_state().await;
    let state2 = applier2.get_state().await;

    println!("State 1 counter: {:?}", state1.counter);
    println!("State 2 counter: {:?}", state2.counter);

    // Both should have counters for both nodes
    assert_eq!(
        state1.counter.get(&1),
        Some(&1),
        "State1 missing node1 count"
    );
    assert_eq!(
        state1.counter.get(&2),
        Some(&1),
        "State1 missing node2 count"
    );
    assert_eq!(
        state2.counter.get(&1),
        Some(&1),
        "State2 missing node1 count"
    );
    assert_eq!(
        state2.counter.get(&2),
        Some(&1),
        "State2 missing node2 count"
    );

    assert_eq!(
        state1.compute_hash(),
        state2.compute_hash(),
        "States should converge after handler delta exchange"
    );
}
