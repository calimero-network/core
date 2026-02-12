//! Protocol Dispatch Integration Tests
//!
//! These tests verify that both `SyncManager` (production) and `SimNode` (simulation):
//! 1. Use the shared `LocalSyncState` trait for handshake building
//! 2. Use the same `select_protocol()` function for negotiation
//! 3. Maintain critical invariants (I5, etc.) across both environments
//!
//! # Architecture
//!
//! Per the Simulation Framework Spec (§3 - Effects-Only Model):
//! > "Same code in simulation and production"
//!
//! Both `SimNode` and `SyncManager` now use:
//! - `LocalSyncState` trait for accessing local state
//! - `build_handshake()` from `calimero_node_primitives::sync::state_machine`
//! - `select_protocol()` from `calimero_node_primitives::sync::protocol`
//!
//! # Key Differences
//!
//! While using the same trait, implementations differ in:
//! - `SimNode`: Has exact `entity_count()` from storage
//! - `SyncManager`: Estimates `entity_count` from `dag_heads.len()`
//!
//! This is acceptable as the critical invariants (I5, None selection) still hold.

use calimero_node_primitives::sync::handshake::SyncHandshake;
use calimero_node_primitives::sync::protocol::{select_protocol, SyncProtocol};
use calimero_node_primitives::sync::state_machine::{
    build_handshake, build_handshake_from_raw, estimate_entity_count, estimate_max_depth,
    LocalSyncState,
};

use crate::sync_sim::prelude::*;
use crate::sync_sim::scenarios::deterministic::Scenario;

// =============================================================================
// LocalSyncState Trait Implementation Tests
// =============================================================================

/// Verify that `SimNode` implements `LocalSyncState` trait correctly.
///
/// This is the key integration test - it proves that `SimNode` uses the shared
/// trait infrastructure from `calimero_node_primitives::sync::state_machine`.
#[test]
fn test_simnode_implements_local_sync_state() {
    let sim_node = SimNode::new("test");

    // Verify we can use the trait methods
    let root_hash = LocalSyncState::root_hash(&sim_node);
    let entity_count = LocalSyncState::entity_count(&sim_node);
    let max_depth = LocalSyncState::max_depth(&sim_node);
    let dag_heads = LocalSyncState::dag_heads(&sim_node);
    let has_state = LocalSyncState::has_state(&sim_node);

    // Fresh node should have specific values
    // Note: SimNode initializes dag_heads with [DeltaId::ZERO] for simulation purposes,
    // but has_state is still false until entities are added.
    assert_eq!(root_hash, [0; 32], "Fresh node should have zero root hash");
    assert_eq!(entity_count, 0, "Fresh node should have zero entities");
    assert_eq!(max_depth, 0, "Fresh node should have zero depth");
    assert!(!has_state, "Fresh node should have has_state=false");
    // SimNode always has at least one DAG head (ZERO) - this is simulation behavior
    assert!(
        !dag_heads.is_empty(),
        "SimNode initializes with at least one DAG head"
    );

    // Verify build_handshake uses the trait
    let handshake = build_handshake(&sim_node);
    assert_eq!(handshake.root_hash, root_hash);
    assert_eq!(handshake.entity_count, entity_count);
    assert_eq!(handshake.max_depth, max_depth);
    assert!(!handshake.has_state);
}

/// Verify that `SimNode.build_handshake()` uses the shared `build_handshake()` function.
#[test]
fn test_simnode_build_handshake_uses_trait() {
    let mut sim_node = SimNode::new("test");

    // Both should produce identical results
    let via_method = sim_node.build_handshake();
    let via_trait = build_handshake(&sim_node);

    assert_eq!(via_method.root_hash, via_trait.root_hash);
    assert_eq!(via_method.entity_count, via_trait.entity_count);
    assert_eq!(via_method.max_depth, via_trait.max_depth);
    assert_eq!(via_method.dag_heads, via_trait.dag_heads);
    assert_eq!(via_method.has_state, via_trait.has_state);
}

// =============================================================================
// Handshake Building Consistency Tests
// =============================================================================

/// Verify handshake building algorithm produces same results as SimNode.
///
/// This tests that `SyncManager.build_local_handshake()` uses the same algorithm
/// as `SimNode.build_handshake()` for:
/// - entity_count estimation
/// - max_depth calculation
/// - has_state determination
#[test]
fn test_handshake_algorithm_consistency_fresh_node() {
    // Fresh node in simulation
    let sim_node = SimNode::new("fresh");

    // Use trait to build handshake
    let sim_hs = build_handshake(&sim_node);

    // Equivalent state using shared estimation functions (what SyncManager uses)
    let root_hash = [0u8; 32];
    let dag_heads: Vec<[u8; 32]> = vec![];
    let entity_count = estimate_entity_count(root_hash, dag_heads.len());
    let max_depth = estimate_max_depth(entity_count);
    let manager_hs = build_handshake_from_raw(root_hash, entity_count, max_depth, dag_heads);

    // Both should agree on fresh node state
    assert_eq!(
        sim_hs.has_state, manager_hs.has_state,
        "has_state mismatch for fresh node"
    );
    assert_eq!(
        sim_hs.entity_count, manager_hs.entity_count,
        "entity_count mismatch for fresh node"
    );
    assert_eq!(
        sim_hs.max_depth, manager_hs.max_depth,
        "max_depth mismatch for fresh node"
    );
}

/// Verify handshake algorithm for initialized nodes with entities.
#[test]
fn test_handshake_algorithm_consistency_initialized() {
    // Initialized node in simulation
    let (mut a, _) = Scenario::both_initialized();
    let sim_hs = a.build_handshake();

    // Verify SimNode has state
    assert!(sim_hs.has_state, "SimNode should have state");

    // Build using manager's algorithm with equivalent data
    let root_hash = sim_hs.root_hash;
    let dag_heads = sim_hs.dag_heads.clone();
    let manager_hs = build_manager_style_handshake(root_hash, &dag_heads);

    // Both should agree on initialized state
    assert_eq!(
        sim_hs.has_state, manager_hs.has_state,
        "has_state mismatch for initialized node"
    );
    assert_eq!(sim_hs.root_hash, manager_hs.root_hash, "root_hash mismatch");
    assert_eq!(sim_hs.dag_heads, manager_hs.dag_heads, "dag_heads mismatch");

    // Note: entity_count and max_depth may differ because SimNode counts actual
    // entities while manager estimates from dag_heads.len(). This is acceptable
    // as long as protocol selection still works correctly.
}

/// Verify protocol selection maintains critical invariants with manager-style handshakes.
///
/// Note: SimNode uses `log2` for max_depth, while SyncManager uses `log2/4` (log16).
/// This means the SPECIFIC protocol may differ (HashComparison vs SubtreePrefetch),
/// but the CRITICAL invariants must still hold:
/// - has_state consistency
/// - None selected when root hashes match
/// - Snapshot only for fresh nodes (I5)
#[test]
fn test_protocol_selection_critical_invariants_with_manager_handshakes() {
    // Test all major scenarios
    let scenarios: Vec<(&str, (SimNode, SimNode))> = vec![
        ("force_none", Scenario::force_none()),
        ("force_snapshot", Scenario::force_snapshot()),
        ("both_initialized", Scenario::both_initialized()),
        ("partial_overlap", Scenario::partial_overlap()),
    ];

    for (name, (mut a, mut b)) in scenarios {
        // Build handshakes using SimNode (existing test approach)
        let sim_hs_a = a.build_handshake();
        let sim_hs_b = b.build_handshake();

        // Build handshakes using manager-style algorithm
        let mgr_hs_a = build_manager_style_handshake(sim_hs_a.root_hash, &sim_hs_a.dag_heads);
        let mgr_hs_b = build_manager_style_handshake(sim_hs_b.root_hash, &sim_hs_b.dag_heads);

        // CRITICAL: has_state must match
        assert_eq!(
            sim_hs_a.has_state, mgr_hs_a.has_state,
            "has_state mismatch for {} (local)",
            name
        );
        assert_eq!(
            sim_hs_b.has_state, mgr_hs_b.has_state,
            "has_state mismatch for {} (remote)",
            name
        );

        let sim_selection = select_protocol(&sim_hs_a, &sim_hs_b);
        let mgr_selection = select_protocol(&mgr_hs_a, &mgr_hs_b);

        // CRITICAL: None must agree (same root hash case)
        if matches!(sim_selection.protocol, SyncProtocol::None) {
            assert!(
                matches!(mgr_selection.protocol, SyncProtocol::None),
                "None mismatch in scenario '{}': SimNode=None, Manager={:?}",
                name,
                mgr_selection.protocol
            );
        }

        // CRITICAL: Snapshot only when local has no state (Invariant I5)
        if mgr_hs_a.has_state {
            assert!(
                !matches!(mgr_selection.protocol, SyncProtocol::Snapshot { .. }),
                "I5 VIOLATION: Snapshot selected for initialized node in '{}'",
                name
            );
        }
    }
}

// =============================================================================
// Protocol Dispatch Tests
// =============================================================================

/// Verify that fresh node syncing from initialized gets Snapshot dispatch.
#[test]
fn test_dispatch_fresh_to_initialized_selects_snapshot() {
    let (mut fresh, mut source) = Scenario::force_snapshot();

    let local_hs = fresh.build_handshake();
    let remote_hs = source.build_handshake();

    assert!(!local_hs.has_state, "Precondition: fresh has no state");
    assert!(remote_hs.has_state, "Precondition: source has state");

    let selection = select_protocol(&local_hs, &remote_hs);

    // This would dispatch to fallback_to_snapshot_sync() in handle_dag_sync()
    assert!(
        matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
        "Expected Snapshot dispatch, got {:?}",
        selection.protocol
    );
    assert!(
        selection.reason.contains("fresh"),
        "Reason should mention fresh node: {}",
        selection.reason
    );
}

/// Verify that same root hash gets None dispatch (no sync needed).
#[test]
fn test_dispatch_same_hash_selects_none() {
    let (mut a, mut b) = Scenario::force_none();

    let local_hs = a.build_handshake();
    let remote_hs = b.build_handshake();

    assert_eq!(
        local_hs.root_hash, remote_hs.root_hash,
        "Precondition: same root hash"
    );

    let selection = select_protocol(&local_hs, &remote_hs);

    // This would return Ok(None) in handle_dag_sync()
    assert!(
        matches!(selection.protocol, SyncProtocol::None),
        "Expected None dispatch (already synced), got {:?}",
        selection.protocol
    );
    assert!(
        selection.reason.contains("already in sync") || selection.reason.contains("match"),
        "Reason should mention already synced: {}",
        selection.reason
    );
}

/// Verify that diverged initialized nodes get state-based protocol (not Snapshot).
#[test]
fn test_dispatch_diverged_initialized_avoids_snapshot() {
    let (mut a, mut b) = Scenario::both_initialized();

    let local_hs = a.build_handshake();
    let remote_hs = b.build_handshake();

    assert!(local_hs.has_state);
    assert!(remote_hs.has_state);
    assert_ne!(local_hs.root_hash, remote_hs.root_hash);

    let selection = select_protocol(&local_hs, &remote_hs);

    // Should NOT be Snapshot (Invariant I5)
    assert!(
        !matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
        "VIOLATION: Snapshot selected for initialized nodes!\n\
         Should use HashComparison/DeltaSync/etc., got {:?}",
        selection.protocol
    );
}

/// Verify unimplemented protocols are identified for fallback.
#[test]
fn test_dispatch_identifies_unimplemented_protocols() {
    // Force deep tree scenario (would select SubtreePrefetch)
    let (mut a, mut b) = Scenario::force_subtree_prefetch();

    let local_hs = a.build_handshake();
    let remote_hs = b.build_handshake();

    let selection = select_protocol(&local_hs, &remote_hs);

    // SubtreePrefetch is not yet implemented, so SyncManager would log warning
    // and fall back to snapshot
    if matches!(selection.protocol, SyncProtocol::SubtreePrefetch { .. }) {
        // This is expected - handle_dag_sync() would warn and fallback
        assert!(
            selection.reason.contains("subtree") || selection.reason.contains("deep"),
            "Reason should explain subtree selection: {}",
            selection.reason
        );
    }

    // Force wide shallow scenario (would select LevelWise)
    let (mut c, mut d) = Scenario::force_levelwise();
    let local_hs = c.build_handshake();
    let remote_hs = d.build_handshake();

    let selection = select_protocol(&local_hs, &remote_hs);

    if matches!(selection.protocol, SyncProtocol::LevelWise { .. }) {
        assert!(
            selection.reason.contains("level") || selection.reason.contains("wide"),
            "Reason should explain levelwise selection: {}",
            selection.reason
        );
    }
}

// =============================================================================
// Reason Logging Tests
// =============================================================================

/// Verify all protocol selections have meaningful reasons.
#[test]
fn test_all_selections_have_reasons() {
    let test_cases: Vec<(&str, SyncHandshake, SyncHandshake)> = vec![
        (
            "same_hash",
            SyncHandshake::new([42; 32], 100, 5, vec![]),
            SyncHandshake::new([42; 32], 100, 5, vec![]),
        ),
        (
            "fresh_to_init",
            SyncHandshake::new([0; 32], 0, 0, vec![]),
            SyncHandshake::new([42; 32], 100, 5, vec![]),
        ),
        (
            "high_divergence",
            SyncHandshake::new([1; 32], 10, 2, vec![]),
            SyncHandshake::new([2; 32], 100, 5, vec![]),
        ),
        (
            "low_divergence_deep",
            SyncHandshake::new([1; 32], 90, 5, vec![]),
            SyncHandshake::new([2; 32], 100, 5, vec![]),
        ),
    ];

    for (name, local, remote) in test_cases {
        let selection = select_protocol(&local, &remote);

        assert!(
            !selection.reason.is_empty(),
            "Selection for '{}' has empty reason",
            name
        );
        assert!(
            selection.reason.len() > 5,
            "Selection reason for '{}' is too short: '{}'",
            name,
            selection.reason
        );
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Build a SyncHandshake using the shared functions from `calimero_node_primitives::sync`.
///
/// This uses the same `estimate_entity_count()` and `estimate_max_depth()` functions
/// that `SyncManager.build_local_handshake()` uses, ensuring consistency.
///
/// **Note**: This matches `SyncManager`'s estimation-based approach.
/// `SimNode` may have different values because it uses actual storage counts.
fn build_manager_style_handshake(root_hash: [u8; 32], dag_heads: &[[u8; 32]]) -> SyncHandshake {
    let entity_count = estimate_entity_count(root_hash, dag_heads.len());
    let max_depth = estimate_max_depth(entity_count);
    build_handshake_from_raw(root_hash, entity_count, max_depth, dag_heads.to_vec())
}
