//! Snapshot Merge Protection Tests
//!
//! # Invariant I5: No Silent Data Loss
//!
//! > "State-based sync on initialized nodes MUST use CRDT merge.
//! >  LWW overwrite is ONLY permitted when local value is absent (fresh node bootstrap)."
//!
//! This is one of the most critical safety invariants in the Calimero Sync Protocol.
//!
//! # Why This Matters
//!
//! When syncing, **Snapshot** means "copy all state, replacing local state entirely".
//! If a node already has data and receives a Snapshot, all its local data would be
//! **silently deleted** and replaced. This violates CRDT semantics where concurrent
//! changes should be merged, not overwritten.
//!
//! ```text
//! Example of what I5 prevents:
//!
//!   Node A: has entities [1, 2, 3]
//!   Node B: has entities [4, 5, 6]
//!
//!   WITHOUT I5 protection:
//!     B syncs from A using Snapshot
//!     B now has: [1, 2, 3]  ← entities 4, 5, 6 LOST FOREVER!
//!
//!   WITH I5 protection:
//!     Protocol selector sees B.has_state = true
//!     Forces HashComparison instead of Snapshot
//!     B now has: [1, 2, 3, 4, 5, 6]  ← All data preserved via CRDT merge
//! ```
//!
//! **Rule**: Snapshot is ONLY allowed for fresh (empty) nodes. Initialized nodes
//! MUST use CRDT-merge protocols (HashComparison, BloomFilter, SubtreePrefetch, etc.).
//!
//! # CIP Reference
//! - CIP §6.3 - Snapshot Usage Constraints
//! - CIP §2.3 - Protocol Selection Rules (Rule 2 is the ONLY case for Snapshot)
//!
//! # Test Categories
//! - Protocol selection layer (automatic protection)
//! - Simulation-based tests (runtime verification)
//! - Edge cases (single entity, version mismatch, etc.)

use calimero_node_primitives::sync::handshake::SyncHandshake;
use calimero_node_primitives::sync::protocol::{select_protocol, SyncProtocol};

use crate::sync_sim::prelude::*;
use crate::sync_sim::scenarios::deterministic::Scenario;

// =============================================================================
// Protocol Selection Layer Tests (CIP §2.3)
// =============================================================================

/// Protocol selection NEVER returns Snapshot when local node has state.
///
/// This is the primary protection layer. The `select_protocol` function
/// must return a state-based protocol (HashComparison, etc.) when `has_state = true`.
#[test]
fn test_initialized_node_never_gets_snapshot() {
    // Test with both_initialized scenario
    let (mut a, mut b) = Scenario::both_initialized();

    let local_hs = build_handshake(&mut a);
    let remote_hs = build_handshake(&mut b);

    // Verify preconditions
    assert!(local_hs.has_state, "Local must have state for this test");
    assert!(remote_hs.has_state, "Remote must have state for this test");

    let selection = select_protocol(&local_hs, &remote_hs);

    assert!(
        !matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
        "VIOLATION: Snapshot selected for initialized node!\n\
         Local has_state: {}\n\
         Remote has_state: {}\n\
         Selected: {:?}\n\
         Reason: {}",
        local_hs.has_state,
        remote_hs.has_state,
        selection.protocol,
        selection.reason
    );
}

/// Even with extreme divergence (60%+), Snapshot must NOT be selected
/// for initialized nodes - uses HashComparison instead.
#[test]
fn test_high_divergence_uses_hash_comparison_not_snapshot() {
    let (mut a, mut b) = Scenario::force_hash_high_divergence();

    let local_hs = build_handshake(&mut a);
    let remote_hs = build_handshake(&mut b);

    // This scenario has ~60% divergence
    assert!(local_hs.has_state);
    assert!(remote_hs.has_state);

    let selection = select_protocol(&local_hs, &remote_hs);

    assert!(
        !matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
        "VIOLATION: Snapshot selected despite high divergence!\n\
         Expected: HashComparison (or similar state-based protocol)\n\
         Got: {:?}",
        selection.protocol
    );

    // Should use HashComparison for high divergence
    assert!(
        matches!(selection.protocol, SyncProtocol::HashComparison { .. }),
        "Expected HashComparison for high divergence, got {:?}",
        selection.protocol
    );
}

/// Verify Snapshot IS selected for fresh (empty) nodes - the valid case.
///
/// This confirms that Snapshot works correctly when it SHOULD be used.
#[test]
fn test_fresh_node_allowed_to_use_snapshot() {
    let (mut fresh, mut source) = Scenario::force_snapshot();

    let local_hs = build_handshake(&mut fresh);
    let remote_hs = build_handshake(&mut source);

    // Verify preconditions
    assert!(!local_hs.has_state, "Fresh node must have has_state=false");
    assert!(remote_hs.has_state, "Source must have state");

    let selection = select_protocol(&local_hs, &remote_hs);

    assert!(
        matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
        "Snapshot should be selected for fresh node, got {:?}",
        selection.protocol
    );
}

/// Property test - protection holds regardless of entity counts or divergence levels.
#[test]
fn test_protection_holds_across_entity_count_combinations() {
    // Test various entity count combinations where both have state
    let test_cases = [
        (1, 1),       // Minimal
        (1, 100),     // Asymmetric
        (100, 1),     // Asymmetric reversed
        (50, 50),     // Balanced
        (1000, 1000), // Large
        (1, 10000),   // Extreme asymmetric
    ];

    for (local_count, remote_count) in test_cases {
        let local_hs = SyncHandshake::new(
            [1; 32], // Non-zero hash (has state)
            local_count,
            3,
            vec![],
        );

        let remote_hs = SyncHandshake::new(
            [2; 32], // Different hash
            remote_count,
            3,
            vec![],
        );

        assert!(local_hs.has_state);
        assert!(remote_hs.has_state);

        let selection = select_protocol(&local_hs, &remote_hs);

        assert!(
            !matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
            "VIOLATION at counts ({}, {}): {:?}",
            local_count,
            remote_count,
            selection.protocol
        );
    }
}

/// All deterministic scenarios with initialized nodes must not use Snapshot.
#[test]
fn test_all_initialized_scenarios_avoid_snapshot() {
    let scenarios: Vec<(&str, (SimNode, SimNode))> = vec![
        ("both_initialized", Scenario::both_initialized()),
        ("partial_overlap", Scenario::partial_overlap()),
        (
            "force_hash_high_div",
            Scenario::force_hash_high_divergence(),
        ),
        ("force_subtree_prefetch", Scenario::force_subtree_prefetch()),
        ("force_bloom_filter", Scenario::force_bloom_filter()),
        ("force_levelwise", Scenario::force_levelwise()),
        ("force_delta_sync", Scenario::force_delta_sync()),
    ];

    for (name, (mut a, mut b)) in scenarios {
        let local_hs = build_handshake(&mut a);
        let remote_hs = build_handshake(&mut b);

        // Skip if local doesn't have state (e.g., force_snapshot)
        if !local_hs.has_state {
            continue;
        }

        let selection = select_protocol(&local_hs, &remote_hs);

        assert!(
            !matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
            "VIOLATION in scenario '{}': Snapshot selected!\n\
             Local entities: {}, Remote entities: {}\n\
             Protocol: {:?}",
            name,
            local_hs.entity_count,
            remote_hs.entity_count,
            selection.protocol
        );
    }
}

// =============================================================================
// Simulation-Based Tests
// =============================================================================

/// Verify protocol negotiation in simulation runtime.
#[test]
fn test_simulation_runtime_negotiation() {
    let mut rt = SimRuntime::new(42);

    // Add two initialized nodes
    let (node_a, node_b) = Scenario::both_initialized();
    let a = rt.add_existing_node(node_a);
    let b = rt.add_existing_node(node_b);

    // Build handshakes
    let hs_a = rt.node_mut(&a).unwrap().build_handshake();
    let hs_b = rt.node_mut(&b).unwrap().build_handshake();

    // Negotiate
    let selection = select_protocol(&hs_a, &hs_b);

    assert!(
        !matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
        "VIOLATION in simulation: {:?}",
        selection.protocol
    );
}

/// Protection holds across 100 random seeds.
#[test]
fn test_protection_holds_across_random_seeds() {
    for seed in 0..100 {
        let mut rt = SimRuntime::new(seed);

        // Create two diverged nodes (both have state)
        let nodes = Scenario::n_nodes_diverged(2);
        let a = rt.add_existing_node(nodes.into_iter().next().unwrap());
        let b_node = Scenario::n_nodes_diverged(2).into_iter().nth(1).unwrap();
        let b = rt.add_existing_node(b_node);

        let hs_a = rt.node_mut(&a).unwrap().build_handshake();
        let hs_b = rt.node_mut(&b).unwrap().build_handshake();

        // Both should have state
        if !hs_a.has_state || !hs_b.has_state {
            continue;
        }

        let selection = select_protocol(&hs_a, &hs_b);

        assert!(
            !matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
            "VIOLATION at seed {}: {:?}",
            seed,
            selection.protocol
        );
    }
}

/// Three-node topology - all pairwise negotiations avoid Snapshot.
#[test]
fn test_three_node_pairwise_negotiations() {
    let (mut a, mut b, mut c) = Scenario::three_nodes_one_diverged();

    let hs_a = build_handshake(&mut a);
    let hs_b = build_handshake(&mut b);
    let hs_c = build_handshake(&mut c);

    // All nodes have state
    assert!(hs_a.has_state);
    assert!(hs_b.has_state);
    assert!(hs_c.has_state);

    // Test all pairs
    let pairs = [
        ("a→b", &hs_a, &hs_b),
        ("a→c", &hs_a, &hs_c),
        ("b→a", &hs_b, &hs_a),
        ("b→c", &hs_b, &hs_c),
        ("c→a", &hs_c, &hs_a),
        ("c→b", &hs_c, &hs_b),
    ];

    for (label, local, remote) in pairs {
        let selection = select_protocol(local, remote);

        assert!(
            !matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
            "VIOLATION in pair {}: {:?}",
            label,
            selection.protocol
        );
    }
}

// =============================================================================
// Edge Case Tests
// =============================================================================

/// Even a single entity should prevent Snapshot (no data loss tolerance).
#[test]
fn test_single_entity_still_prevents_snapshot() {
    // Local has just 1 entity, remote has 1000
    let local_hs = SyncHandshake::new([1; 32], 1, 1, vec![]);
    let remote_hs = SyncHandshake::new([2; 32], 1000, 10, vec![]);

    assert!(local_hs.has_state);

    let selection = select_protocol(&local_hs, &remote_hs);

    assert!(
        !matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
        "VIOLATION: Even 1 entity should prevent Snapshot!\n\
         Got: {:?}",
        selection.protocol
    );
}

/// Version mismatch falls back to HashComparison, not Snapshot.
#[test]
fn test_version_mismatch_uses_safe_fallback() {
    let mut local_hs = SyncHandshake::new([1; 32], 50, 3, vec![]);
    let mut remote_hs = SyncHandshake::new([2; 32], 50, 3, vec![]);

    // Force version mismatch
    local_hs.version = 1;
    remote_hs.version = 2;

    let selection = select_protocol(&local_hs, &remote_hs);

    // Should fall back to HashComparison
    assert!(
        matches!(selection.protocol, SyncProtocol::HashComparison { .. }),
        "Version mismatch should fall back to HashComparison, got {:?}",
        selection.protocol
    );
}

/// Same root hash means no sync needed - nodes already in sync.
#[test]
fn test_same_hash_means_no_sync_needed() {
    let (mut a, mut b) = Scenario::force_none();

    let hs_a = build_handshake(&mut a);
    let hs_b = build_handshake(&mut b);

    assert_eq!(hs_a.root_hash, hs_b.root_hash);

    let selection = select_protocol(&hs_a, &hs_b);

    assert!(
        matches!(selection.protocol, SyncProtocol::None),
        "Same root hash should select None, got {:?}",
        selection.protocol
    );
}

// =============================================================================
// Helpers
// =============================================================================

/// Build a SyncHandshake from a SimNode.
fn build_handshake(node: &mut SimNode) -> SyncHandshake {
    node.build_handshake()
}

#[cfg(test)]
mod compile_check {
    //! Ensure all tests compile and types are correct.
    use super::*;

    #[test]
    fn types_compile() {
        let _ = Scenario::both_initialized();
        let _ = Scenario::force_snapshot();
    }
}
