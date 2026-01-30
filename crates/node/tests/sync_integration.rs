//! Sync Integration Tests
//!
//! Integration tests for the full sync flow using mocked network layer.
//! These tests verify that sync components work together correctly:
//! - Delta buffering during snapshot sync
//! - Post-snapshot DAG sync trigger
//! - Proactive sync from hints
//! - Protocol negotiation handshake

use calimero_node_primitives::sync_protocol::{
    BufferedDelta, DeltaBuffer, SyncCapabilities, SyncHandshake, SyncHandshakeResponse, SyncHints,
    SyncProtocolHint, SyncProtocolVersion, SyncSessionState,
};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ============================================================================
// Test Harness - Mock Infrastructure
// ============================================================================

/// Mock sync session tracker for testing.
///
/// Simulates the `NodeState.sync_sessions` behavior without requiring
/// the full node infrastructure.
#[derive(Debug, Default)]
struct MockSyncSessionTracker {
    sessions: Arc<Mutex<HashMap<ContextId, MockSyncSession>>>,
}

#[derive(Debug)]
struct MockSyncSession {
    state: SyncSessionState,
    delta_buffer: DeltaBuffer,
    buffered_delta_ids: Vec<[u8; 32]>,
}

impl MockSyncSessionTracker {
    fn new() -> Self {
        Self::default()
    }

    fn start_session(&self, context_id: ContextId, sync_start_hlc: u64) {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.insert(
            context_id,
            MockSyncSession {
                state: SyncSessionState::BufferingDeltas {
                    buffered_count: 0,
                    sync_start_hlc,
                },
                delta_buffer: DeltaBuffer::new(100, sync_start_hlc),
                buffered_delta_ids: Vec::new(),
            },
        );
    }

    fn should_buffer(&self, context_id: &ContextId) -> bool {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .get(context_id)
            .map_or(false, |s| s.state.should_buffer_deltas())
    }

    fn buffer_delta(&self, context_id: &ContextId, delta: BufferedDelta) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(session) = sessions.get_mut(context_id) {
            let delta_id = delta.id;
            if session.delta_buffer.push(delta).is_ok() {
                session.buffered_delta_ids.push(delta_id);
                if let SyncSessionState::BufferingDeltas {
                    ref mut buffered_count,
                    ..
                } = session.state
                {
                    *buffered_count += 1;
                }
                return true;
            }
        }
        false
    }

    fn end_session(&self, context_id: &ContextId) -> Option<Vec<BufferedDelta>> {
        let mut sessions = self.sessions.lock().unwrap();
        sessions
            .remove(context_id)
            .map(|mut s| s.delta_buffer.drain())
    }

    fn get_buffered_count(&self, context_id: &ContextId) -> usize {
        let sessions = self.sessions.lock().unwrap();
        sessions.get(context_id).map_or(0, |s| {
            if let SyncSessionState::BufferingDeltas { buffered_count, .. } = s.state {
                buffered_count
            } else {
                0
            }
        })
    }

    fn get_buffered_ids(&self, context_id: &ContextId) -> Vec<[u8; 32]> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .get(context_id)
            .map_or(Vec::new(), |s| s.buffered_delta_ids.clone())
    }
}

/// Mock peer state for testing sync scenarios.
#[derive(Debug, Clone)]
struct MockPeerState {
    root_hash: Hash,
    entity_count: u32,
    tree_depth: u8,
    dag_heads: Vec<[u8; 32]>,
}

impl MockPeerState {
    fn empty() -> Self {
        Self {
            root_hash: Hash::default(),
            entity_count: 0,
            tree_depth: 0,
            dag_heads: Vec::new(),
        }
    }

    fn with_state(root_hash: [u8; 32], entity_count: u32, dag_heads: Vec<[u8; 32]>) -> Self {
        Self {
            root_hash: Hash::from(root_hash),
            entity_count,
            tree_depth: (entity_count as f64).log2().ceil() as u8,
            dag_heads,
        }
    }

    fn to_sync_hints(&self) -> SyncHints {
        SyncHints::from_state(self.root_hash, self.entity_count, self.tree_depth)
    }

    fn to_capabilities(&self) -> SyncCapabilities {
        SyncCapabilities::full()
    }

    fn to_handshake(&self) -> SyncHandshake {
        SyncHandshake {
            capabilities: self.to_capabilities(),
            root_hash: self.root_hash,
            dag_heads: self.dag_heads.clone(),
            entity_count: self.entity_count as u64,
        }
    }
}

// ============================================================================
// Scenario 1: Delta Buffering During Snapshot Sync
// ============================================================================

#[test]
fn test_deltas_buffered_during_snapshot_sync() {
    let context_id = ContextId::from([1u8; 32]);
    let tracker = MockSyncSessionTracker::new();

    // Simulate snapshot sync starting
    let sync_start_hlc = 1000u64;
    tracker.start_session(context_id, sync_start_hlc);

    // Verify buffering is active
    assert!(tracker.should_buffer(&context_id));

    // Simulate incoming deltas during snapshot sync
    let delta1 = BufferedDelta {
        id: [1u8; 32],
        parents: vec![[0u8; 32]],
        hlc: 1001,
        payload: vec![1, 2, 3],
    };
    let delta2 = BufferedDelta {
        id: [2u8; 32],
        parents: vec![[1u8; 32]],
        hlc: 1002,
        payload: vec![4, 5, 6],
    };

    assert!(tracker.buffer_delta(&context_id, delta1));
    assert!(tracker.buffer_delta(&context_id, delta2));

    // Verify deltas are buffered
    assert_eq!(tracker.get_buffered_count(&context_id), 2);

    // Simulate snapshot sync completing
    let buffered = tracker.end_session(&context_id);
    assert!(buffered.is_some());

    let deltas = buffered.unwrap();
    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0].id, [1u8; 32]);
    assert_eq!(deltas[1].id, [2u8; 32]);

    // Buffering should no longer be active
    assert!(!tracker.should_buffer(&context_id));
}

#[test]
fn test_no_buffering_when_not_syncing() {
    let context_id = ContextId::from([2u8; 32]);
    let tracker = MockSyncSessionTracker::new();

    // No sync session started
    assert!(!tracker.should_buffer(&context_id));

    // Attempting to buffer should fail
    let delta = BufferedDelta {
        id: [1u8; 32],
        parents: vec![],
        hlc: 1000,
        payload: vec![],
    };
    assert!(!tracker.buffer_delta(&context_id, delta));
}

#[test]
fn test_buffer_overflow_handling() {
    let context_id = ContextId::from([3u8; 32]);
    let tracker = MockSyncSessionTracker::new();

    // Start with small buffer (capacity is set in MockSyncSession)
    tracker.start_session(context_id, 1000);

    // Buffer many deltas (100 is the limit in our mock)
    for i in 0..100u8 {
        let delta = BufferedDelta {
            id: [i; 32],
            parents: vec![],
            hlc: 1000 + i as u64,
            payload: vec![i],
        };
        assert!(tracker.buffer_delta(&context_id, delta));
    }

    // 101st should fail (buffer full)
    let overflow_delta = BufferedDelta {
        id: [101u8; 32],
        parents: vec![],
        hlc: 2000,
        payload: vec![],
    };
    assert!(!tracker.buffer_delta(&context_id, overflow_delta));
}

// ============================================================================
// Scenario 2: Post-Snapshot Delta IDs for DAG Sync
// ============================================================================

#[test]
fn test_buffered_delta_ids_available_for_dag_sync() {
    let context_id = ContextId::from([4u8; 32]);
    let tracker = MockSyncSessionTracker::new();

    tracker.start_session(context_id, 1000);

    // Buffer some deltas
    let ids: Vec<[u8; 32]> = (0..5u8).map(|i| [i; 32]).collect();
    for id in &ids {
        let delta = BufferedDelta {
            id: *id,
            parents: vec![],
            hlc: 1000,
            payload: vec![],
        };
        tracker.buffer_delta(&context_id, delta);
    }

    // Get buffered IDs (for requesting via DAG sync)
    let buffered_ids = tracker.get_buffered_ids(&context_id);
    assert_eq!(buffered_ids.len(), 5);
    for (i, id) in buffered_ids.iter().enumerate() {
        assert_eq!(*id, [i as u8; 32]);
    }
}

// ============================================================================
// Scenario 3: Proactive Sync From Hints
// ============================================================================

#[test]
fn test_hints_suggest_snapshot_for_large_divergence() {
    // Local node is empty
    let local = MockPeerState::empty();

    // Remote has significant state
    let remote = MockPeerState::with_state([1u8; 32], 50000, vec![[2u8; 32]]);

    let hints = remote.to_sync_hints();

    // Should suggest adaptive selection for large trees
    // (50000 entities > 10000 threshold)
    assert!(matches!(
        hints.suggested_protocol,
        SyncProtocolHint::AdaptiveSelection
    ));

    // Should detect divergence
    assert!(hints.suggests_divergence(&local.root_hash, local.entity_count));
}

#[test]
fn test_hints_suggest_delta_for_small_trees() {
    let _local = MockPeerState::with_state([1u8; 32], 50, vec![[2u8; 32]]);
    let remote = MockPeerState::with_state([3u8; 32], 60, vec![[4u8; 32]]);

    let hints = remote.to_sync_hints();

    // Small trees (<100 entities) should suggest delta sync
    assert!(matches!(
        hints.suggested_protocol,
        SyncProtocolHint::DeltaSync
    ));
}

#[test]
fn test_hints_suggest_hash_based_for_medium_trees() {
    let remote = MockPeerState::with_state([1u8; 32], 5000, vec![[2u8; 32]]);

    let hints = remote.to_sync_hints();

    // Medium trees (100-10000 entities) should suggest hash-based
    assert!(matches!(
        hints.suggested_protocol,
        SyncProtocolHint::HashBased
    ));
}

#[test]
fn test_no_divergence_when_hashes_match() {
    let root_hash = [42u8; 32];
    let local = MockPeerState::with_state(root_hash, 100, vec![[1u8; 32]]);
    let remote = MockPeerState::with_state(root_hash, 100, vec![[1u8; 32]]);

    let hints = remote.to_sync_hints();

    // Same root hash = no divergence
    assert!(!hints.suggests_divergence(&local.root_hash, local.entity_count));
}

// ============================================================================
// Scenario 4: Protocol Negotiation Flow
// ============================================================================

#[test]
fn test_handshake_negotiation_success() {
    let local = MockPeerState::with_state([1u8; 32], 1000, vec![[2u8; 32]]);
    let remote = MockPeerState::with_state([3u8; 32], 1200, vec![[4u8; 32]]);

    let local_handshake = local.to_handshake();
    let remote_handshake = remote.to_handshake();

    // Both support full capabilities
    let negotiated = local_handshake
        .capabilities
        .negotiate(&remote_handshake.capabilities);

    assert!(negotiated.is_some());
    assert!(matches!(
        negotiated.unwrap(),
        SyncProtocolVersion::HybridSync { .. }
    ));
}

#[test]
fn test_handshake_response_construction() {
    let local = MockPeerState::with_state([1u8; 32], 1000, vec![[2u8; 32]]);
    let remote = MockPeerState::with_state([3u8; 32], 1200, vec![[4u8; 32]]);

    let remote_handshake = remote.to_handshake();
    let negotiated = local
        .to_capabilities()
        .negotiate(&remote_handshake.capabilities);

    let response = SyncHandshakeResponse {
        negotiated_protocol: negotiated,
        capabilities: local.to_capabilities(),
        root_hash: local.root_hash,
        dag_heads: local.dag_heads.clone(),
        entity_count: local.entity_count as u64,
    };

    assert!(response.negotiated_protocol.is_some());
    assert_eq!(response.root_hash, local.root_hash);
    assert_eq!(response.dag_heads, local.dag_heads);
}

// ============================================================================
// Scenario 5: Full Sync Flow Simulation
// ============================================================================

/// Simulates a complete sync flow:
/// 1. Fresh node receives handshake
/// 2. Protocol negotiated
/// 3. Snapshot sync starts (buffering enabled)
/// 4. Deltas arrive during sync (buffered)
/// 5. Snapshot completes (buffering disabled)
/// 6. Buffered delta IDs available for DAG sync
#[test]
fn test_full_sync_flow_simulation() {
    let context_id = ContextId::from([10u8; 32]);
    let tracker = MockSyncSessionTracker::new();

    // Step 1: Fresh node (empty state)
    let local = MockPeerState::empty();
    let remote = MockPeerState::with_state([1u8; 32], 5000, vec![[2u8; 32], [3u8; 32]]);

    // Step 2: Protocol negotiation
    let remote_handshake = remote.to_handshake();
    let negotiated = local
        .to_capabilities()
        .negotiate(&remote_handshake.capabilities);
    assert!(negotiated.is_some());

    // Step 3: Snapshot sync starts
    let sync_start_hlc = 1000u64;
    tracker.start_session(context_id, sync_start_hlc);
    assert!(tracker.should_buffer(&context_id));

    // Step 4: Deltas arrive during sync
    let incoming_deltas: Vec<BufferedDelta> = (0..3u8)
        .map(|i| BufferedDelta {
            id: [100 + i; 32],
            parents: vec![[99 + i; 32]],
            hlc: sync_start_hlc + 10 + i as u64,
            payload: vec![i; 100],
        })
        .collect();

    for delta in incoming_deltas {
        assert!(tracker.buffer_delta(&context_id, delta));
    }
    assert_eq!(tracker.get_buffered_count(&context_id), 3);

    // Step 5: Snapshot completes
    let buffered = tracker.end_session(&context_id);
    assert!(!tracker.should_buffer(&context_id));

    // Step 6: Buffered deltas available for DAG sync
    let deltas = buffered.unwrap();
    assert_eq!(deltas.len(), 3);

    // Verify delta IDs for requesting via DAG sync
    let delta_ids: Vec<[u8; 32]> = deltas.iter().map(|d| d.id).collect();
    assert_eq!(delta_ids[0], [100u8; 32]);
    assert_eq!(delta_ids[1], [101u8; 32]);
    assert_eq!(delta_ids[2], [102u8; 32]);
}

// ============================================================================
// Edge Cases
// ============================================================================

#[test]
fn test_multiple_contexts_independent_sessions() {
    let tracker = MockSyncSessionTracker::new();
    let ctx1 = ContextId::from([1u8; 32]);
    let ctx2 = ContextId::from([2u8; 32]);

    // Start session for ctx1 only
    tracker.start_session(ctx1, 1000);

    assert!(tracker.should_buffer(&ctx1));
    assert!(!tracker.should_buffer(&ctx2));

    // Buffer delta for ctx1
    let delta = BufferedDelta {
        id: [1u8; 32],
        parents: vec![],
        hlc: 1001,
        payload: vec![],
    };
    assert!(tracker.buffer_delta(&ctx1, delta));

    // ctx2 should not buffer
    let delta2 = BufferedDelta {
        id: [2u8; 32],
        parents: vec![],
        hlc: 1002,
        payload: vec![],
    };
    assert!(!tracker.buffer_delta(&ctx2, delta2));

    // End ctx1 session
    let buffered = tracker.end_session(&ctx1);
    assert_eq!(buffered.unwrap().len(), 1);
}

#[test]
fn test_session_can_be_restarted() {
    let context_id = ContextId::from([5u8; 32]);
    let tracker = MockSyncSessionTracker::new();

    // First sync session
    tracker.start_session(context_id, 1000);
    tracker.buffer_delta(
        &context_id,
        BufferedDelta {
            id: [1u8; 32],
            parents: vec![],
            hlc: 1001,
            payload: vec![],
        },
    );
    let first_buffered = tracker.end_session(&context_id);
    assert_eq!(first_buffered.unwrap().len(), 1);

    // Second sync session (e.g., after failure/retry)
    tracker.start_session(context_id, 2000);
    assert!(tracker.should_buffer(&context_id));
    assert_eq!(tracker.get_buffered_count(&context_id), 0); // Fresh buffer

    tracker.buffer_delta(
        &context_id,
        BufferedDelta {
            id: [2u8; 32],
            parents: vec![],
            hlc: 2001,
            payload: vec![],
        },
    );
    let second_buffered = tracker.end_session(&context_id);
    assert_eq!(second_buffered.unwrap().len(), 1);
}

#[test]
fn test_hints_entity_count_difference_detection() {
    // Local has 100 entities
    let local_count = 100u32;
    let local_hash = Hash::from([1u8; 32]);

    // Remote has 120 entities (20 more)
    let hints = SyncHints::from_state(local_hash, 120, 7);

    // Same hash but large entity difference should suggest divergence
    // (threshold is 10 in suggests_divergence)
    assert!(hints.suggests_divergence(&local_hash, local_count));

    // Small difference should not
    let small_diff_hints = SyncHints::from_state(local_hash, 105, 7);
    assert!(!small_diff_hints.suggests_divergence(&local_hash, local_count));
}
