use super::*;

use calimero_node_primitives::sync::{
    build_handshake_from_raw, estimate_entity_count, estimate_max_depth, SyncHandshake,
};
use calimero_primitives::hash::Hash;

use super::SyncManager;

/// Build a handshake using the estimation fallback path (no store available).
///
/// This mirrors the fallback in `SyncManager::build_local_handshake` when
/// `query_tree_stats` returns `None`.
fn build_estimated_handshake(root_hash: [u8; 32], dag_heads: Vec<[u8; 32]>) -> SyncHandshake {
    let entity_count = estimate_entity_count(root_hash, dag_heads.len());
    let max_depth = estimate_max_depth(entity_count);
    build_handshake_from_raw(root_hash, entity_count, max_depth, dag_heads)
}

/// `dedup_peers_by_strongest_role` collapses a peer's multiple role
/// observations to its strongest, regardless of input order.
#[test]
fn dedup_peers_keeps_strongest_role() {
    use calimero_primitives::context::GroupMemberRole;
    use libp2p::PeerId;

    let p = PeerId::random();
    let q = PeerId::random();
    let out = SyncManager::dedup_peers_by_strongest_role(vec![
        (p, GroupMemberRole::Member),
        (p, GroupMemberRole::Admin), // strongest for p
        (q, GroupMemberRole::ReadOnlyTee),
    ]);
    let map: std::collections::BTreeMap<_, _> = out.into_iter().collect();
    assert_eq!(map.get(&p), Some(&GroupMemberRole::Admin));
    assert_eq!(map.get(&q), Some(&GroupMemberRole::ReadOnlyTee));
}

// =========================================================================
// Tests for handshake estimation fallback
// =========================================================================

/// Fresh node (zero root_hash) should have has_state=false and entity_count=0
#[test]
fn test_build_local_handshake_fresh_node() {
    let handshake = build_estimated_handshake([0; 32], vec![]);

    assert!(
        !handshake.has_state,
        "Fresh node should have has_state=false"
    );
    assert_eq!(
        handshake.entity_count, 0,
        "Fresh node should have entity_count=0"
    );
    assert_eq!(handshake.max_depth, 0, "Fresh node should have max_depth=0");
    assert_eq!(handshake.root_hash, [0; 32]);
}

/// Initialized node should have has_state=true and entity_count >= 1
#[test]
fn test_build_local_handshake_initialized_node() {
    let handshake = build_estimated_handshake([42; 32], vec![[1; 32], [2; 32]]);

    assert!(
        handshake.has_state,
        "Initialized node should have has_state=true"
    );
    assert_eq!(
        handshake.entity_count, 2,
        "Entity count should match dag_heads length in fallback"
    );
    assert!(
        handshake.max_depth >= 1,
        "Initialized node should have max_depth >= 1"
    );
    assert_eq!(handshake.root_hash, [42; 32]);
    assert_eq!(handshake.dag_heads.len(), 2);
}

/// Initialized node with empty dag_heads should still have entity_count >= 1
#[test]
fn test_build_local_handshake_initialized_no_heads() {
    let handshake = build_estimated_handshake([42; 32], vec![]);

    assert!(handshake.has_state);
    assert_eq!(
        handshake.entity_count, 1,
        "Initialized node with no heads should have entity_count=1 (minimum)"
    );
}

// =========================================================================
// Tests for build_remote_handshake()
// =========================================================================

/// Test building remote handshake from peer state
#[test]
fn test_build_remote_handshake_with_state() {
    let peer_root_hash = Hash::from([99; 32]);
    let peer_dag_heads: Vec<[u8; 32]> = vec![[10; 32], [20; 32], [30; 32]];

    let handshake = SyncManager::build_remote_handshake(peer_root_hash, &peer_dag_heads);

    assert!(handshake.has_state);
    assert_eq!(handshake.root_hash, [99; 32]);
    assert_eq!(handshake.entity_count, 3);
    assert_eq!(handshake.dag_heads.len(), 3);
}

/// Test building remote handshake from fresh peer
#[test]
fn test_build_remote_handshake_fresh_peer() {
    let peer_root_hash = Hash::from([0; 32]);
    let peer_dag_heads: Vec<[u8; 32]> = vec![];

    let handshake = SyncManager::build_remote_handshake(peer_root_hash, &peer_dag_heads);

    assert!(!handshake.has_state);
    assert_eq!(handshake.root_hash, [0; 32]);
    assert_eq!(handshake.entity_count, 0);
    assert_eq!(handshake.max_depth, 0);
}

// =========================================================================
// Tests for protocol selection integration
// =========================================================================

/// Test that select_protocol is called correctly with built handshakes
#[test]
fn test_protocol_selection_fresh_to_initialized() {
    use calimero_node_primitives::sync::{select_protocol, SyncProtocol};

    // Fresh local node
    let local_hs = SyncHandshake::new([0; 32], 0, 0, vec![]);

    // Initialized remote node
    let remote_hs = SyncHandshake::new([42; 32], 100, 4, vec![[1; 32]]);

    let selection = select_protocol(&local_hs, &remote_hs);

    assert!(
        matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
        "Fresh node syncing from initialized should use Snapshot, got {:?}",
        selection.protocol
    );
    assert!(
        selection.reason.contains("fresh node"),
        "Reason should mention fresh node"
    );
}

/// Test that same root hash results in None protocol
#[test]
fn test_protocol_selection_already_synced() {
    use calimero_node_primitives::sync::{select_protocol, SyncProtocol};

    let local_hs = SyncHandshake::new([42; 32], 50, 3, vec![[1; 32]]);
    let remote_hs = SyncHandshake::new([42; 32], 100, 4, vec![[2; 32]]);

    let selection = select_protocol(&local_hs, &remote_hs);

    assert!(
        matches!(selection.protocol, SyncProtocol::None),
        "Same root hash should result in None, got {:?}",
        selection.protocol
    );
}

/// Test max_depth calculation for various entity counts
#[test]
fn test_max_depth_calculation() {
    // Test the log16 approximation: log16(n) ≈ log2(n) / 4
    let test_cases: Vec<(u64, u32)> = vec![
        (0, 0),   // No entities
        (1, 1),   // Single entity -> depth 1
        (16, 1),  // 16 entities -> log2(16)/4 = 4/4 = 1
        (256, 2), // 256 entities -> log2(256)/4 = 8/4 = 2
    ];

    for (entity_count, expected_min_depth) in test_cases {
        let max_depth = if entity_count == 0 {
            0
        } else {
            let log2_approx = 64u32.saturating_sub(entity_count.leading_zeros());
            (log2_approx / 4).max(1).min(32)
        };

        assert!(
            max_depth >= expected_min_depth,
            "entity_count={} should have max_depth >= {}, got {}",
            entity_count,
            expected_min_depth,
            max_depth
        );
    }
}

// =========================================================================
// Tests for the #2625 governance-pending backfill trigger
//
// Regression guard for the cross-DAG governance gate buffering a root-context
// delta that never drains → permanent split-brain (group-subgroup e2e flake).
// `perform_interval_sync` now calls `backfill_governance_for_pending_deltas`,
// which (a) fires only when the governance-pending buffer is non-empty
// (`should_backfill_governance`) and (b) pulls the *correct* namespace
// governance DAG (`resolve_namespace_id`). Both pieces are unit-tested here so
// a future refactor that inverts the gate or mis-resolves the namespace gets
// caught without needing the full e2e.
// =========================================================================

mod governance_backfill_trigger {
    use std::sync::Arc;

    use calimero_context::group_store::{register_context_in_group, NamespaceRepository};
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::context::ContextId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::super::{resolve_namespace_id, should_backfill_governance};

    fn fresh_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn ctx(byte: u8) -> ContextId {
        ContextId::from([byte; 32])
    }

    fn gid(byte: u8) -> ContextGroupId {
        ContextGroupId::from([byte; 32])
    }

    #[test]
    fn empty_buffer_does_not_trigger_backfill() {
        // Steady state: no deltas parked → no namespace pull. Inverting this
        // gate would pull the governance DAG on every interval tick for every
        // context.
        assert!(!should_backfill_governance(0));
    }

    #[test]
    fn non_empty_buffer_triggers_backfill() {
        // The bug: a delta sat buffered forever because nothing pulled the
        // governance op it waited on. Any pending delta must arm the pull.
        assert!(should_backfill_governance(1));
        assert!(should_backfill_governance(42));
    }

    #[test]
    fn resolve_namespace_id_root_group_resolves_to_itself() {
        // The flake hit the ROOT context: its owning group IS the namespace
        // root (no parent), so resolution returns that group's bytes.
        let store = fresh_store();
        let context_id = ctx(0x11);
        let root_group = gid(0x22);

        register_context_in_group(&store, &root_group, &context_id)
            .expect("register_context_in_group");

        let resolved = resolve_namespace_id(&store, &context_id);
        assert_eq!(resolved, Some(root_group.to_bytes()));
    }

    #[test]
    fn resolve_namespace_id_subgroup_context_resolves_to_root() {
        // A subgroup-owned context must resolve to the namespace ROOT, not the
        // immediate subgroup — pulling the subgroup's DAG would miss the
        // root-level governance op and never converge.
        let store = fresh_store();
        let context_id = ctx(0x31);
        let root_group = gid(0x32);
        let subgroup = gid(0x33);

        NamespaceRepository::new(&store)
            .nest(&root_group, &subgroup)
            .expect("nest subgroup under root");
        register_context_in_group(&store, &subgroup, &context_id)
            .expect("register_context_in_group");

        let resolved = resolve_namespace_id(&store, &context_id);
        assert_eq!(
            resolved,
            Some(root_group.to_bytes()),
            "subgroup-owned context should resolve to the namespace root"
        );
    }

    #[test]
    fn resolve_namespace_id_unregistered_context_returns_none() {
        // Legacy non-group context (no `ContextGroupRef`): nothing to pull, so
        // resolution returns None and the backfill is skipped rather than
        // pulling a bogus namespace.
        let store = fresh_store();
        let resolved = resolve_namespace_id(&store, &ctx(0x99));
        assert_eq!(resolved, None);
    }
}
