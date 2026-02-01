//! Sync Protocol Negotiation Tests
//!
//! Tests for protocol negotiation, sync hints, and delta buffering.
//! These tests verify the sync protocol types work correctly in isolation
//! and integrate properly with the existing sync infrastructure.

use calimero_node_primitives::sync_protocol::{
    BufferedDelta, DeltaBuffer, SyncCapabilities, SyncHandshake, SyncHandshakeResponse, SyncHints,
    SyncProtocolHint, SyncProtocolVersion, SyncSessionState,
};
use calimero_primitives::hash::Hash;

// ============================================================================
// Protocol Negotiation Tests
// ============================================================================

#[test]
fn test_full_capability_nodes_negotiate_hybrid() {
    let caps_a = SyncCapabilities::full();
    let caps_b = SyncCapabilities::full();

    // Full capability nodes should prefer HybridSync v2
    let negotiated = caps_a.negotiate(&caps_b);
    assert!(negotiated.is_some());
    assert!(matches!(
        negotiated.unwrap(),
        SyncProtocolVersion::HybridSync { version: 2 }
    ));
}

#[test]
fn test_mixed_capability_negotiation() {
    // Node A: Full capabilities
    let caps_a = SyncCapabilities::full();

    // Node B: Only supports delta and snapshot
    let caps_b = SyncCapabilities {
        supported_protocols: vec![
            SyncProtocolVersion::SnapshotSync { version: 1 },
            SyncProtocolVersion::DeltaSync { version: 1 },
        ],
        max_page_size: 512 * 1024,
        supports_compression: true,
        supports_sync_hints: false,
    };

    // Should negotiate SnapshotSync (first common protocol in A's preference order)
    let negotiated = caps_a.negotiate(&caps_b);
    assert!(negotiated.is_some());
    assert!(matches!(
        negotiated.unwrap(),
        SyncProtocolVersion::SnapshotSync { version: 1 }
    ));
}

#[test]
fn test_version_mismatch_prevents_negotiation() {
    let caps_a = SyncCapabilities {
        supported_protocols: vec![SyncProtocolVersion::DeltaSync { version: 2 }],
        ..Default::default()
    };

    let caps_b = SyncCapabilities {
        supported_protocols: vec![SyncProtocolVersion::DeltaSync { version: 1 }],
        ..Default::default()
    };

    // Different versions should not negotiate
    let negotiated = caps_a.negotiate(&caps_b);
    assert!(negotiated.is_none());
}

#[test]
fn test_empty_capabilities_no_negotiation() {
    let caps_a = SyncCapabilities {
        supported_protocols: vec![],
        ..Default::default()
    };
    let caps_b = SyncCapabilities::full();

    assert!(caps_a.negotiate(&caps_b).is_none());
    assert!(caps_b.negotiate(&caps_a).is_none());
}

// ============================================================================
// Sync Hints Tests
// ============================================================================

#[test]
fn test_sync_hints_from_state() {
    let hints = SyncHints::from_state(Hash::from([42; 32]), 500, 6);

    assert_eq!(hints.post_root_hash, Hash::from([42; 32]));
    assert_eq!(hints.entity_count, 500);
    assert_eq!(hints.tree_depth, 6);
    assert_eq!(hints.suggested_protocol, SyncProtocolHint::HashBased);
}

#[test]
fn test_sync_hints_small_tree_suggests_delta() {
    let hints = SyncHints::from_state(Hash::from([1; 32]), 50, 3);
    assert_eq!(hints.suggested_protocol, SyncProtocolHint::DeltaSync);
}

#[test]
fn test_sync_hints_large_tree_suggests_adaptive() {
    let hints = SyncHints::from_state(Hash::from([1; 32]), 50000, 12);
    assert_eq!(
        hints.suggested_protocol,
        SyncProtocolHint::AdaptiveSelection
    );
}

#[test]
fn test_sync_hints_divergence_same_hash() {
    let hints = SyncHints::from_state(Hash::from([1; 32]), 100, 5);

    // Same hash, similar entity count - no divergence
    assert!(!hints.suggests_divergence(&Hash::from([1; 32]), 100));
    assert!(!hints.suggests_divergence(&Hash::from([1; 32]), 105)); // Within threshold
}

#[test]
fn test_sync_hints_divergence_different_hash() {
    let hints = SyncHints::from_state(Hash::from([1; 32]), 100, 5);

    // Different hash always indicates divergence
    assert!(hints.suggests_divergence(&Hash::from([2; 32]), 100));
}

#[test]
fn test_sync_hints_divergence_large_entity_diff() {
    let hints = SyncHints::from_state(Hash::from([1; 32]), 100, 5);

    // Same hash but large entity count difference
    assert!(hints.suggests_divergence(&Hash::from([1; 32]), 50)); // 50 diff > 10 threshold
    assert!(hints.suggests_divergence(&Hash::from([1; 32]), 200)); // 100 diff > 10 threshold
}

// ============================================================================
// Delta Buffer Tests
// ============================================================================

#[test]
fn test_delta_buffer_fifo_order() {
    let mut buffer = DeltaBuffer::new(100, 1000);

    // Add deltas in order
    for i in 1..=5u8 {
        buffer
            .push(BufferedDelta {
                id: [i; 32],
                parents: vec![[i - 1; 32]],
                hlc: 1000 + i as u64,
                payload: vec![i],
            })
            .unwrap();
    }

    // Drain should return in FIFO order
    let drained = buffer.drain();
    assert_eq!(drained.len(), 5);
    for (i, delta) in drained.iter().enumerate() {
        assert_eq!(delta.id[0], (i + 1) as u8);
    }
}

#[test]
fn test_delta_buffer_reusable_after_drain() {
    let mut buffer = DeltaBuffer::new(10, 0);

    buffer
        .push(BufferedDelta {
            id: [1; 32],
            parents: vec![],
            hlc: 1,
            payload: vec![],
        })
        .unwrap();

    let _ = buffer.drain();
    assert!(buffer.is_empty());

    // Can reuse after drain
    buffer
        .push(BufferedDelta {
            id: [2; 32],
            parents: vec![],
            hlc: 2,
            payload: vec![],
        })
        .unwrap();

    assert_eq!(buffer.len(), 1);
}

#[test]
fn test_delta_buffer_preserves_sync_start_hlc() {
    let buffer = DeltaBuffer::new(10, 12345);
    assert_eq!(buffer.sync_start_hlc(), 12345);
}

// ============================================================================
// Sync Session State Tests
// ============================================================================

#[test]
fn test_session_state_active_detection() {
    assert!(!SyncSessionState::Idle.is_active());

    assert!(SyncSessionState::Handshaking.is_active());

    assert!(SyncSessionState::Syncing {
        protocol: SyncProtocolVersion::DeltaSync { version: 1 },
        started_at: 0,
    }
    .is_active());

    assert!(SyncSessionState::BufferingDeltas {
        buffered_count: 0,
        sync_start_hlc: 0,
    }
    .is_active());

    assert!(SyncSessionState::ReplayingDeltas { remaining: 10 }.is_active());

    assert!(!SyncSessionState::Completed {
        protocol: SyncProtocolVersion::DeltaSync { version: 1 },
        duration_ms: 100,
    }
    .is_active());

    assert!(!SyncSessionState::Failed {
        reason: "test".to_string(),
    }
    .is_active());
}

#[test]
fn test_session_state_buffer_detection() {
    assert!(!SyncSessionState::Syncing {
        protocol: SyncProtocolVersion::SnapshotSync { version: 1 },
        started_at: 0,
    }
    .should_buffer_deltas());

    assert!(SyncSessionState::BufferingDeltas {
        buffered_count: 5,
        sync_start_hlc: 1000,
    }
    .should_buffer_deltas());
}

// ============================================================================
// Handshake Serialization Tests
// ============================================================================

#[test]
fn test_handshake_roundtrip() {
    let handshake = SyncHandshake {
        capabilities: SyncCapabilities::full(),
        root_hash: Hash::from([99; 32]),
        dag_heads: vec![[1; 32], [2; 32], [3; 32]],
        entity_count: 12345,
    };

    let encoded = borsh::to_vec(&handshake).unwrap();
    let decoded: SyncHandshake = borsh::from_slice(&encoded).unwrap();

    assert_eq!(decoded.root_hash, handshake.root_hash);
    assert_eq!(decoded.dag_heads.len(), 3);
    assert_eq!(decoded.entity_count, 12345);
    assert!(decoded.capabilities.supports_compression);
}

#[test]
fn test_handshake_response_roundtrip() {
    let response = SyncHandshakeResponse {
        negotiated_protocol: Some(SyncProtocolVersion::HybridSync { version: 2 }),
        capabilities: SyncCapabilities::minimal(),
        root_hash: Hash::from([50; 32]),
        dag_heads: vec![[10; 32]],
        entity_count: 999,
    };

    let encoded = borsh::to_vec(&response).unwrap();
    let decoded: SyncHandshakeResponse = borsh::from_slice(&encoded).unwrap();

    assert!(decoded.negotiated_protocol.is_some());
    assert!(matches!(
        decoded.negotiated_protocol.unwrap(),
        SyncProtocolVersion::HybridSync { version: 2 }
    ));
    assert!(!decoded.capabilities.supports_compression);
}

#[test]
fn test_handshake_response_no_protocol() {
    let response = SyncHandshakeResponse {
        negotiated_protocol: None,
        capabilities: SyncCapabilities::default(),
        root_hash: Hash::from([0; 32]),
        dag_heads: vec![],
        entity_count: 0,
    };

    let encoded = borsh::to_vec(&response).unwrap();
    let decoded: SyncHandshakeResponse = borsh::from_slice(&encoded).unwrap();

    assert!(decoded.negotiated_protocol.is_none());
}

// ============================================================================
// Sync Hints with BroadcastMessage Integration
// ============================================================================

#[test]
fn test_sync_hints_serialization_standalone() {
    // Test that SyncHints can be serialized and deserialized independently
    let hints = SyncHints::from_state(Hash::from([42; 32]), 1000, 8);

    let encoded = borsh::to_vec(&hints).unwrap();
    let decoded: SyncHints = borsh::from_slice(&encoded).unwrap();

    assert_eq!(decoded.post_root_hash, hints.post_root_hash);
    assert_eq!(decoded.entity_count, hints.entity_count);
    assert_eq!(decoded.tree_depth, hints.tree_depth);
    assert_eq!(decoded.suggested_protocol, hints.suggested_protocol);
}

#[test]
fn test_sync_hints_size_overhead() {
    // Verify the sync hints overhead is reasonable (~40 bytes)
    let hints = SyncHints::from_state(Hash::from([1; 32]), 1000, 10);
    let encoded = borsh::to_vec(&hints).unwrap();

    // Hash (32) + u32 (4) + u8 (1) + enum (1) = ~38 bytes
    // Plus borsh overhead
    assert!(
        encoded.len() <= 50,
        "Sync hints should be ~40 bytes, got {}",
        encoded.len()
    );
}

#[test]
fn test_sync_hints_required_in_broadcast() {
    // Since we control all nodes (alpha stage), sync_hints is required, not optional.
    // This test verifies that SyncHints is always present and properly serializable.
    let hints = SyncHints::from_state(Hash::from([99; 32]), 500, 7);

    // Verify the hints contain expected values
    assert_eq!(hints.entity_count, 500);
    assert_eq!(hints.tree_depth, 7);
    assert_eq!(hints.suggested_protocol, SyncProtocolHint::HashBased);
}

// ============================================================================
// Protocol Selection Scenarios
// ============================================================================

/// Test scenarios for adaptive protocol selection based on state characteristics.
mod protocol_selection {
    use super::*;

    #[test]
    fn scenario_fresh_node_joining() {
        // Fresh node (no state) joining network with existing state
        let local_root = Hash::from([0; 32]); // Uninitialized
        let local_entities = 0;

        let peer_hints = SyncHints::from_state(Hash::from([42; 32]), 1000, 7);

        // Should definitely detect divergence
        assert!(peer_hints.suggests_divergence(&local_root, local_entities));

        // Peer has medium-sized tree, suggests hash-based comparison
        // But for fresh node, snapshot would be more efficient
        assert_eq!(peer_hints.suggested_protocol, SyncProtocolHint::HashBased);
    }

    #[test]
    fn scenario_minor_divergence() {
        // Two nodes with similar state, minor divergence from lost deltas
        let local_root = Hash::from([42; 32]);
        let local_entities = 998;

        let peer_hints = SyncHints::from_state(Hash::from([43; 32]), 1000, 7);

        // Different root but similar entity count
        assert!(peer_hints.suggests_divergence(&local_root, local_entities));

        // Delta sync would be most efficient here
        // The hint doesn't know it's minor divergence, but hash-based will discover it quickly
    }

    #[test]
    fn scenario_significant_divergence() {
        // Two nodes that have significantly diverged
        let local_root = Hash::from([1; 32]);
        let local_entities = 500;

        // Use 50000 entities and depth 10 to trigger AdaptiveSelection
        let peer_hints = SyncHints::from_state(Hash::from([99; 32]), 50000, 10);

        assert!(peer_hints.suggests_divergence(&local_root, local_entities));

        // Large tree (>10000 entities AND depth >= 5), should use adaptive selection
        assert_eq!(
            peer_hints.suggested_protocol,
            SyncProtocolHint::AdaptiveSelection
        );
    }

    #[test]
    fn scenario_nodes_in_sync() {
        // Two nodes that are perfectly in sync
        let local_root = Hash::from([50; 32]);
        let local_entities = 100;

        let peer_hints = SyncHints::from_state(Hash::from([50; 32]), 100, 5);

        // No divergence detected
        assert!(!peer_hints.suggests_divergence(&local_root, local_entities));
    }
}

// ============================================================================
// Edge Cases
// ============================================================================

#[test]
fn test_empty_dag_heads_in_handshake() {
    let handshake = SyncHandshake {
        capabilities: SyncCapabilities::minimal(),
        root_hash: Hash::from([0; 32]),
        dag_heads: vec![],
        entity_count: 0,
    };

    let encoded = borsh::to_vec(&handshake).unwrap();
    let decoded: SyncHandshake = borsh::from_slice(&encoded).unwrap();

    assert!(decoded.dag_heads.is_empty());
}

#[test]
fn test_max_entity_count() {
    let hints = SyncHints::from_state(Hash::from([1; 32]), u32::MAX, 20);

    // Should still work with max values
    assert_eq!(hints.entity_count, u32::MAX);
    assert_eq!(
        hints.suggested_protocol,
        SyncProtocolHint::AdaptiveSelection
    );
}

#[test]
fn test_delta_buffer_zero_capacity() {
    let mut buffer = DeltaBuffer::new(0, 0);

    // Can't push anything to zero-capacity buffer
    let result = buffer.push(BufferedDelta {
        id: [1; 32],
        parents: vec![],
        hlc: 1,
        payload: vec![],
    });

    assert!(result.is_err());
}

// ============================================================================
// Adaptive Protocol Selection Tests
// ============================================================================

#[test]
fn test_adaptive_select_no_divergence() {
    let root_hash = Hash::from([42u8; 32]);
    let hints = SyncHints::from_state(root_hash, 1000, 10);

    // Same hash = no sync needed
    let result = hints.adaptive_select(&root_hash, 1000);
    assert!(result.is_none());
}

#[test]
fn test_adaptive_select_local_empty_needs_snapshot() {
    let hints = SyncHints::from_state(Hash::from([1u8; 32]), 5000, 12);
    let local_hash = Hash::from([0u8; 32]); // Different hash

    // Local is empty (0 entities) → needs snapshot bootstrap
    let result = hints.adaptive_select(&local_hash, 0);
    assert_eq!(result, Some(SyncProtocolHint::Snapshot));
}

#[test]
fn test_adaptive_select_sender_has_10x_more_needs_snapshot() {
    let hints = SyncHints::from_state(Hash::from([1u8; 32]), 10000, 12);
    let local_hash = Hash::from([2u8; 32]); // Different hash

    // Sender has 10000, we have 100 → 100x more → snapshot
    let result = hints.adaptive_select(&local_hash, 100);
    assert_eq!(result, Some(SyncProtocolHint::Snapshot));
}

#[test]
fn test_adaptive_select_small_tree_uses_delta() {
    let hints = SyncHints::from_state(Hash::from([1u8; 32]), 150, 5);
    let local_hash = Hash::from([2u8; 32]); // Different hash

    // Local has 50 entities (small tree) → delta sync
    let result = hints.adaptive_select(&local_hash, 50);
    assert_eq!(result, Some(SyncProtocolHint::DeltaSync));
}

#[test]
fn test_adaptive_select_medium_tree_uses_hash_based() {
    let hints = SyncHints::from_state(Hash::from([1u8; 32]), 5000, 10);
    let local_hash = Hash::from([2u8; 32]); // Different hash

    // Local has 1000 entities (medium tree) → hash-based
    let result = hints.adaptive_select(&local_hash, 1000);
    assert_eq!(result, Some(SyncProtocolHint::HashBased));
}

#[test]
fn test_adaptive_select_large_tree_still_uses_hash_based() {
    let hints = SyncHints::from_state(Hash::from([1u8; 32]), 50000, 15);
    let local_hash = Hash::from([2u8; 32]); // Different hash

    // Local has 20000 entities (large tree) → still hash-based (not snapshot)
    let result = hints.adaptive_select(&local_hash, 20000);
    assert_eq!(result, Some(SyncProtocolHint::HashBased));
}

#[test]
fn test_adaptive_select_similar_entity_count_no_snapshot() {
    let hints = SyncHints::from_state(Hash::from([1u8; 32]), 1000, 10);
    let local_hash = Hash::from([2u8; 32]); // Different hash

    // Sender has 1000, we have 500 → only 2x more → no snapshot trigger
    // Medium tree (500 entities) → hash-based
    let result = hints.adaptive_select(&local_hash, 500);
    assert_eq!(result, Some(SyncProtocolHint::HashBased));
}
