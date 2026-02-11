//! Protocol Negotiation Compliance Tests (CIP §2.3)
//!
//! These tests verify that protocol selection conforms to CIP §2.3 decision table.
//! Each test forces specific conditions to trigger a deterministic protocol selection.
//!
//! # Decision Table (CIP §2.3)
//!
//! | # | Condition | Selected Protocol |
//! |---|-----------|-------------------|
//! | 1 | `root_hash` match | `None` |
//! | 2 | `!has_state` (fresh node) | `Snapshot` |
//! | 3 | `has_state` AND divergence >50% | `HashComparison` |
//! | 4 | `max_depth` >3 AND divergence <20% | `SubtreePrefetch` |
//! | 5 | `entity_count` >50 AND divergence <10% | `BloomFilter` |
//! | 6 | `max_depth` 1-2 AND avg children/level >10 | `LevelWise` |
//! | 7 | (default) | `HashComparison` |

use calimero_node_primitives::sync::handshake::SyncHandshake;
use calimero_node_primitives::sync::protocol::{
    calculate_divergence, select_protocol, SyncProtocol,
};

use crate::sync_sim::prelude::*;

// =============================================================================
// Rule 1: Same root hash → None
// =============================================================================

/// CIP-2.3-R1: Same root hash results in protocol None.
#[test]
fn test_cip23_rule1_same_root_hash() {
    let (mut a, mut b) = Scenario::force_none();

    let hs_a = a.build_handshake();
    let hs_b = b.build_handshake();

    // Precondition: same root hash
    assert_eq!(
        hs_a.root_hash, hs_b.root_hash,
        "Precondition: same root hash"
    );

    let selection = select_protocol(&hs_a, &hs_b);

    assert!(
        matches!(selection.protocol, SyncProtocol::None),
        "CIP §2.3 R1 VIOLATION: Same root hash should select None, got {:?}",
        selection.protocol
    );
}

/// CIP-2.3-R1: Verify symmetry - both directions give None.
#[test]
fn test_cip23_rule1_symmetric() {
    let (mut a, mut b) = Scenario::force_none();

    let hs_a = a.build_handshake();
    let hs_b = b.build_handshake();

    let sel_ab = select_protocol(&hs_a, &hs_b);
    let sel_ba = select_protocol(&hs_b, &hs_a);

    assert!(matches!(sel_ab.protocol, SyncProtocol::None));
    assert!(matches!(sel_ba.protocol, SyncProtocol::None));
}

// =============================================================================
// Rule 2: Fresh node → Snapshot
// =============================================================================

/// CIP-2.3-R2: Fresh node bootstrap uses Snapshot.
#[test]
fn test_cip23_rule2_fresh_node_snapshot() {
    let (mut fresh, mut source) = Scenario::force_snapshot();

    let hs_fresh = fresh.build_handshake();
    let hs_source = source.build_handshake();

    // Preconditions
    assert!(!hs_fresh.has_state, "Precondition: fresh node has no state");
    assert!(hs_source.has_state, "Precondition: source has state");

    let selection = select_protocol(&hs_fresh, &hs_source);

    assert!(
        matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
        "CIP §2.3 R2 VIOLATION: Fresh node should use Snapshot, got {:?}",
        selection.protocol
    );
}

/// CIP-2.3-R2: Fresh node with empty source still returns None (both empty).
#[test]
fn test_cip23_rule2_both_fresh_nodes() {
    let fresh_a = SimNode::new("fresh_a");
    let fresh_b = SimNode::new("fresh_b");

    let mut a = fresh_a;
    let mut b = fresh_b;

    let hs_a = a.build_handshake();
    let hs_b = b.build_handshake();

    // Both empty - same root hash (zeros)
    assert_eq!(hs_a.root_hash, hs_b.root_hash);
    assert_eq!(hs_a.root_hash, [0; 32]);

    let selection = select_protocol(&hs_a, &hs_b);

    // Same hash = None (Rule 1 takes precedence)
    assert!(
        matches!(selection.protocol, SyncProtocol::None),
        "Both empty nodes should match and select None, got {:?}",
        selection.protocol
    );
}

// =============================================================================
// Rule 3: High divergence (>50%) → HashComparison
// =============================================================================

/// CIP-2.3-R3: High divergence triggers HashComparison.
#[test]
fn test_cip23_rule3_high_divergence_hash_comparison() {
    let (mut a, mut b) = Scenario::force_hash_high_divergence();

    let hs_a = a.build_handshake();
    let hs_b = b.build_handshake();

    // Preconditions
    assert!(hs_a.has_state);
    assert!(hs_b.has_state);
    // Calculate divergence using production formula
    let divergence = calculate_divergence(&hs_a, &hs_b);
    assert!(
        divergence > 0.5,
        "Precondition: divergence > 50%, got {:.2}%",
        divergence * 100.0
    );

    let selection = select_protocol(&hs_a, &hs_b);

    assert!(
        matches!(selection.protocol, SyncProtocol::HashComparison { .. }),
        "CIP §2.3 R3 VIOLATION: High divergence should use HashComparison, got {:?}",
        selection.protocol
    );
}

/// CIP-2.3-R3: Verify 51% divergence triggers HashComparison.
#[test]
fn test_cip23_rule3_boundary_51_percent() {
    // Create scenario with exactly ~51% divergence
    // Local: 49 entities, Remote: 100 entities → 51% divergence
    let hs_local = SyncHandshake::new([1; 32], 49, 3, vec![]);
    let hs_remote = SyncHandshake::new([2; 32], 100, 3, vec![]);

    let selection = select_protocol(&hs_local, &hs_remote);

    assert!(
        matches!(selection.protocol, SyncProtocol::HashComparison { .. }),
        "51% divergence should use HashComparison, got {:?}",
        selection.protocol
    );
}

// =============================================================================
// Rule 4: Deep tree + low divergence → SubtreePrefetch
// =============================================================================

/// CIP-2.3-R4: Deep tree with localized changes uses SubtreePrefetch.
#[test]
fn test_cip23_rule4_deep_tree_subtree_prefetch() {
    let (mut a, mut b) = Scenario::force_subtree_prefetch();

    let hs_a = a.build_handshake();
    let hs_b = b.build_handshake();

    // Preconditions
    assert!(hs_a.has_state);
    assert!(hs_b.has_state);
    // max_depth > 3
    assert!(
        hs_b.max_depth > 3,
        "Precondition: max_depth > 3, got {}",
        hs_b.max_depth
    );
    // divergence < 20% using production formula
    let divergence = calculate_divergence(&hs_a, &hs_b);
    assert!(
        divergence < 0.2,
        "Precondition: divergence < 20%, got {:.2}%",
        divergence * 100.0
    );

    let selection = select_protocol(&hs_a, &hs_b);

    assert!(
        matches!(selection.protocol, SyncProtocol::SubtreePrefetch { .. }),
        "CIP §2.3 R4 VIOLATION: Deep tree + low divergence should use SubtreePrefetch, got {:?}",
        selection.protocol
    );
}

// =============================================================================
// Rule 5: Large tree + small diff → BloomFilter
// =============================================================================

/// CIP-2.3-R5: Large tree with tiny diff uses BloomFilter.
///
/// Requirements: entity_count > 50 AND divergence < 10% AND NOT (max_depth > 3)
/// (R4 SubtreePrefetch takes precedence when depth > 3)
#[test]
fn test_cip23_rule5_large_tree_bloom_filter() {
    // Construct handshakes that precisely meet R5 conditions:
    // - entity_count > 50: yes (100)
    // - divergence < 10%: yes (~5%)
    // - max_depth <= 3: yes (3) - to avoid R4 SubtreePrefetch
    let hs_local = SyncHandshake::new([1; 32], 95, 3, vec![]); // 95 entities, depth 3
    let hs_remote = SyncHandshake::new([2; 32], 100, 3, vec![]); // 100 entities, depth 3

    // Verify preconditions
    assert!(
        hs_remote.entity_count > 50,
        "Precondition: entity_count > 50"
    );
    let divergence = 1.0 - (95.0 / 100.0);
    assert!(divergence < 0.1, "Precondition: divergence < 10%");
    assert!(
        hs_remote.max_depth <= 3,
        "Precondition: max_depth <= 3 to avoid R4"
    );

    let selection = select_protocol(&hs_local, &hs_remote);

    assert!(
        matches!(selection.protocol, SyncProtocol::BloomFilter { .. }),
        "CIP §2.3 R5 VIOLATION: Large tree + tiny diff should use BloomFilter, got {:?}",
        selection.protocol
    );
}

// =============================================================================
// Rule 6: Wide shallow tree → LevelWise
// =============================================================================

/// CIP-2.3-R6: Wide shallow tree uses LevelWise.
///
/// Requirements: max_depth 1-2 AND avg_children/level > 10 AND divergence <= 50%
#[test]
fn test_cip23_rule6_wide_shallow_levelwise() {
    // Construct handshakes that precisely meet R6 conditions:
    // - max_depth 1-2: yes (2)
    // - avg_children/level > 10: yes (50 entities / 2 depth = 25 avg)
    // - divergence <= 50%: yes (~20%)
    let hs_local = SyncHandshake::new([1; 32], 40, 2, vec![]); // 40 entities, depth 2
    let hs_remote = SyncHandshake::new([2; 32], 50, 2, vec![]); // 50 entities, depth 2

    // Verify preconditions
    assert!(hs_remote.has_state);
    assert!(
        hs_remote.max_depth >= 1 && hs_remote.max_depth <= 2,
        "Precondition: max_depth 1-2, got {}",
        hs_remote.max_depth
    );
    let avg_children = hs_remote.entity_count / u64::from(hs_remote.max_depth);
    assert!(
        avg_children > 10,
        "Precondition: avg_children > 10, got {}",
        avg_children
    );

    let selection = select_protocol(&hs_local, &hs_remote);

    assert!(
        matches!(selection.protocol, SyncProtocol::LevelWise { .. }),
        "CIP §2.3 R6 VIOLATION: Wide shallow tree should use LevelWise, got {:?}",
        selection.protocol
    );
}

// =============================================================================
// Rule 7: Default → HashComparison
// =============================================================================

/// CIP-2.3-R7: Default fallback is HashComparison.
#[test]
fn test_cip23_rule7_default_hash_comparison() {
    // Create a scenario that doesn't match any specific rule
    // Medium divergence, medium depth, medium entity count
    let hs_local = SyncHandshake::new([1; 32], 30, 3, vec![]); // 30 entities, depth 3
    let hs_remote = SyncHandshake::new([2; 32], 40, 3, vec![]); // 40 entities, depth 3

    // Divergence: ~25% (doesn't match R3 >50%, R4 <20%, R5 <10%)
    // Depth: 3 (doesn't match R6 depth 1-2)
    // Entity count: 40 (doesn't match R5 >50)

    let selection = select_protocol(&hs_local, &hs_remote);

    assert!(
        matches!(selection.protocol, SyncProtocol::HashComparison { .. }),
        "CIP §2.3 R7: Default should use HashComparison, got {:?}",
        selection.protocol
    );
}

// =============================================================================
// Version Compatibility Tests
// =============================================================================

/// CIP-2.3: Version mismatch falls back to HashComparison.
#[test]
fn test_cip23_version_mismatch_fallback() {
    let mut hs_local = SyncHandshake::new([1; 32], 50, 3, vec![]);
    let mut hs_remote = SyncHandshake::new([2; 32], 50, 3, vec![]);

    // Force version mismatch
    hs_local.version = 1;
    hs_remote.version = 999;

    let selection = select_protocol(&hs_local, &hs_remote);

    assert!(
        matches!(selection.protocol, SyncProtocol::HashComparison { .. }),
        "Version mismatch should fall back to HashComparison, got {:?}",
        selection.protocol
    );
}

// =============================================================================
// Edge Cases and Priority Tests
// =============================================================================

/// Rule priority: R1 (same hash) takes precedence over everything.
#[test]
fn test_cip23_rule1_priority() {
    // Even if other conditions would trigger different protocols,
    // same hash = None
    let hs = SyncHandshake::new([42; 32], 1000, 10, vec![]);

    let selection = select_protocol(&hs, &hs);

    assert!(matches!(selection.protocol, SyncProtocol::None));
}

/// Rule priority: R2 (fresh node) takes precedence over R3-R7.
#[test]
fn test_cip23_rule2_priority_over_divergence() {
    // Fresh node with 100% divergence should still use Snapshot
    let hs_fresh = SyncHandshake::new([0; 32], 0, 0, vec![]);
    let hs_full = SyncHandshake::new([1; 32], 10000, 20, vec![]);

    // This would be 100% divergence, but fresh node takes precedence
    let selection = select_protocol(&hs_fresh, &hs_full);

    assert!(
        matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
        "Fresh node should use Snapshot regardless of divergence, got {:?}",
        selection.protocol
    );
}

/// Rule priority: R3 (high divergence) takes precedence over R4-R6.
#[test]
fn test_cip23_rule3_priority_over_optimization() {
    // High divergence with deep tree should use HashComparison, not SubtreePrefetch
    let hs_local = SyncHandshake::new([1; 32], 20, 10, vec![]); // 20 entities, depth 10
    let hs_remote = SyncHandshake::new([2; 32], 100, 10, vec![]); // 100 entities, depth 10
                                                                  // Divergence: 80%

    let selection = select_protocol(&hs_local, &hs_remote);

    assert!(
        matches!(selection.protocol, SyncProtocol::HashComparison { .. }),
        "High divergence should use HashComparison even with deep tree, got {:?}",
        selection.protocol
    );
}

// =============================================================================
// Sweep Tests
// =============================================================================

/// Sweep test: verify all rules are reachable.
#[test]
fn test_cip23_all_rules_reachable() {
    let mut rules_hit = [false; 7]; // R1-R7

    // R1: Same hash
    {
        let (mut a, mut b) = Scenario::force_none();
        let hs_a = a.build_handshake();
        let hs_b = b.build_handshake();
        if matches!(select_protocol(&hs_a, &hs_b).protocol, SyncProtocol::None) {
            rules_hit[0] = true;
        }
    }

    // R2: Fresh node
    {
        let (mut fresh, mut source) = Scenario::force_snapshot();
        let hs_f = fresh.build_handshake();
        let hs_s = source.build_handshake();
        if matches!(
            select_protocol(&hs_f, &hs_s).protocol,
            SyncProtocol::Snapshot { .. }
        ) {
            rules_hit[1] = true;
        }
    }

    // R3: High divergence
    {
        let (mut a, mut b) = Scenario::force_hash_high_divergence();
        let hs_a = a.build_handshake();
        let hs_b = b.build_handshake();
        if matches!(
            select_protocol(&hs_a, &hs_b).protocol,
            SyncProtocol::HashComparison { .. }
        ) {
            // Only count as R3 if divergence > 50% using production formula
            let div = calculate_divergence(&hs_a, &hs_b);
            if div > 0.5 {
                rules_hit[2] = true;
            }
        }
    }

    // R4: SubtreePrefetch
    {
        let (mut a, mut b) = Scenario::force_subtree_prefetch();
        let hs_a = a.build_handshake();
        let hs_b = b.build_handshake();
        if matches!(
            select_protocol(&hs_a, &hs_b).protocol,
            SyncProtocol::SubtreePrefetch { .. }
        ) {
            rules_hit[3] = true;
        }
    }

    // R5: BloomFilter - use direct handshake construction for precise control
    // Requires: entity_count > 50, divergence < 10%, max_depth <= 3 (to avoid R4)
    {
        let hs_a = SyncHandshake::new([1; 32], 95, 3, vec![]);
        let hs_b = SyncHandshake::new([2; 32], 100, 3, vec![]);
        if matches!(
            select_protocol(&hs_a, &hs_b).protocol,
            SyncProtocol::BloomFilter { .. }
        ) {
            rules_hit[4] = true;
        }
    }

    // R6: LevelWise - use direct handshake construction for precise control
    // Requires: max_depth 1-2, avg_children/level > 10
    {
        let hs_a = SyncHandshake::new([1; 32], 40, 2, vec![]);
        let hs_b = SyncHandshake::new([2; 32], 50, 2, vec![]);
        if matches!(
            select_protocol(&hs_a, &hs_b).protocol,
            SyncProtocol::LevelWise { .. }
        ) {
            rules_hit[5] = true;
        }
    }

    // R7: Default HashComparison
    {
        let hs_a = SyncHandshake::new([1; 32], 30, 3, vec![]);
        let hs_b = SyncHandshake::new([2; 32], 40, 3, vec![]);
        if matches!(
            select_protocol(&hs_a, &hs_b).protocol,
            SyncProtocol::HashComparison { .. }
        ) {
            rules_hit[6] = true;
        }
    }

    // Verify all rules were hit
    for (i, hit) in rules_hit.iter().enumerate() {
        assert!(
            *hit,
            "CIP §2.3 Rule {} was never triggered by any scenario!",
            i + 1
        );
    }
}

/// Determinism test: same inputs = same outputs across 1000 runs.
#[test]
fn test_cip23_deterministic_selection() {
    let hs_a = SyncHandshake::new([1; 32], 100, 5, vec![[1; 32]]);
    let hs_b = SyncHandshake::new([2; 32], 80, 5, vec![[2; 32]]);

    let baseline = select_protocol(&hs_a, &hs_b);

    for i in 0..1000 {
        let result = select_protocol(&hs_a, &hs_b);
        assert_eq!(
            std::mem::discriminant(&result.protocol),
            std::mem::discriminant(&baseline.protocol),
            "Protocol selection changed on iteration {}!",
            i
        );
    }
}
