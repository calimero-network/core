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
// Tests for the DAG-head divergence rule (`SyncManager::peer_heads_diverge`)
// backing the divergence catch-up in `handle_dag_sync`.
//
// `peer_heads_diverge` is the pure kernel of `local_dag_behind_peer_heads`:
// the async wrapper resolves each non-genesis peer head against the local
// DAG's *applied* set (a present-but-pending delta is NOT applied, so it is
// absent from `applied_heads` and counts as divergent) and a missing delta
// store short-circuits to "behind" before this rule runs.
// =========================================================================

#[cfg(test)]
mod peer_heads_diverge_tests {
    use std::collections::HashSet;

    use super::SyncManager;

    /// No heads advertised — nothing to catch up on.
    #[test]
    fn empty_heads_do_not_diverge() {
        assert!(!SyncManager::peer_heads_diverge(&[], &HashSet::new()));
    }

    /// A genesis sentinel head is skipped — it is never a real delta.
    #[test]
    fn genesis_only_heads_do_not_diverge() {
        assert!(!SyncManager::peer_heads_diverge(
            &[[0u8; 32]],
            &HashSet::new()
        ));
    }

    /// Peer advertises a real head we have not applied — diverge.
    #[test]
    fn unapplied_head_diverges() {
        assert!(SyncManager::peer_heads_diverge(
            &[[1u8; 32]],
            &HashSet::new()
        ));
    }

    /// Peer's only head is already applied locally — no divergence.
    #[test]
    fn applied_head_does_not_diverge() {
        assert!(!SyncManager::peer_heads_diverge(
            &[[1u8; 32]],
            &HashSet::from([[1u8; 32]])
        ));
    }

    /// One unapplied head among applied ones still diverges.
    #[test]
    fn one_unapplied_among_applied_diverges() {
        assert!(SyncManager::peer_heads_diverge(
            &[[1u8; 32], [2u8; 32]],
            &HashSet::from([[1u8; 32]])
        ));
    }

    /// Every advertised head is applied locally — no divergence.
    #[test]
    fn all_heads_applied_do_not_diverge() {
        assert!(!SyncManager::peer_heads_diverge(
            &[[1u8; 32], [2u8; 32]],
            &HashSet::from([[1u8; 32], [2u8; 32]])
        ));
    }

    /// A genesis sentinel alongside an applied real head — genesis is
    /// skipped, the real head is applied, so no divergence.
    #[test]
    fn genesis_alongside_applied_head_does_not_diverge() {
        assert!(!SyncManager::peer_heads_diverge(
            &[[0u8; 32], [1u8; 32]],
            &HashSet::from([[1u8; 32]])
        ));
    }

    /// A genesis sentinel alongside an unapplied real head — genesis is
    /// skipped, the real head is missing, so it diverges.
    #[test]
    fn genesis_alongside_unapplied_head_diverges() {
        assert!(SyncManager::peer_heads_diverge(
            &[[0u8; 32], [1u8; 32]],
            &HashSet::new()
        ));
    }
}

// =========================================================================
// Tests for the #2319 dispatch-attempt backoff helper
// =========================================================================

#[cfg(test)]
mod dispatch_backoff_tests {
    use super::*;

    fn ctx(byte: u8) -> ContextId {
        ContextId::from([byte; 32])
    }

    #[test]
    fn no_entry_means_not_recently_attempted() {
        let map: HashMap<ContextId, time::Instant> = HashMap::new();
        assert!(!dispatch_recently_attempted(
            &map,
            &ctx(1),
            time::Duration::from_secs(5)
        ));
    }

    #[test]
    fn fresh_attempt_within_interval_is_recent() {
        let mut map = HashMap::new();
        let _ = map.insert(ctx(2), time::Instant::now());
        assert!(dispatch_recently_attempted(
            &map,
            &ctx(2),
            time::Duration::from_secs(5)
        ));
    }

    #[test]
    fn old_attempt_beyond_interval_is_not_recent() {
        let mut map = HashMap::new();
        let _ = map.insert(ctx(3), time::Instant::now() - time::Duration::from_secs(10));
        assert!(!dispatch_recently_attempted(
            &map,
            &ctx(3),
            time::Duration::from_secs(5)
        ));
    }

    #[test]
    fn other_contexts_are_unaffected() {
        let mut map = HashMap::new();
        let _ = map.insert(ctx(4), time::Instant::now());
        assert!(!dispatch_recently_attempted(
            &map,
            &ctx(5),
            time::Duration::from_secs(5)
        ));
    }
}

// =========================================================================
// Tests for the #2319 wedged-session watchdog helper
// =========================================================================

#[cfg(test)]
mod session_watchdog_tests {
    use super::*;

    fn ctx(byte: u8) -> ContextId {
        ContextId::from([byte; 32])
    }

    /// `SyncState` with `last_sync == None` (sync dispatched, no result yet).
    fn in_progress_state() -> SyncState {
        let mut s = SyncState::new();
        s.start();
        s
    }

    /// `SyncState` with `last_sync == Some(_)` (a result has cleared it).
    fn settled_state() -> SyncState {
        let mut s = SyncState::new();
        s.on_failure("prior failure".to_owned());
        s
    }

    const GRACE: time::Duration = time::Duration::from_secs(60);

    #[test]
    fn fresh_dispatch_in_progress_is_not_wedged() {
        let mut dispatched = HashMap::new();
        let _ = dispatched.insert(ctx(1), time::Instant::now());
        let mut state = HashMap::new();
        let _ = state.insert(ctx(1), in_progress_state());
        assert!(!session_dispatch_wedged(
            &dispatched,
            &state,
            &ctx(1),
            GRACE
        ));
    }

    #[test]
    fn stale_dispatch_still_in_progress_is_wedged() {
        let mut dispatched = HashMap::new();
        let _ = dispatched.insert(
            ctx(2),
            time::Instant::now() - time::Duration::from_secs(120),
        );
        let mut state = HashMap::new();
        let _ = state.insert(ctx(2), in_progress_state());
        assert!(session_dispatch_wedged(&dispatched, &state, &ctx(2), GRACE));
    }

    #[test]
    fn stale_dispatch_but_settled_is_not_wedged() {
        let mut dispatched = HashMap::new();
        let _ = dispatched.insert(
            ctx(3),
            time::Instant::now() - time::Duration::from_secs(120),
        );
        let mut state = HashMap::new();
        let _ = state.insert(ctx(3), settled_state());
        assert!(!session_dispatch_wedged(
            &dispatched,
            &state,
            &ctx(3),
            GRACE
        ));
    }

    #[test]
    fn no_dispatch_record_is_not_wedged() {
        let dispatched: HashMap<ContextId, time::Instant> = HashMap::new();
        let mut state = HashMap::new();
        let _ = state.insert(ctx(4), in_progress_state());
        assert!(!session_dispatch_wedged(
            &dispatched,
            &state,
            &ctx(4),
            GRACE
        ));
    }

    #[test]
    fn other_contexts_are_unaffected() {
        let mut dispatched = HashMap::new();
        let _ = dispatched.insert(
            ctx(5),
            time::Instant::now() - time::Duration::from_secs(120),
        );
        let mut state = HashMap::new();
        let _ = state.insert(ctx(5), in_progress_state());
        assert!(!session_dispatch_wedged(
            &dispatched,
            &state,
            &ctx(6),
            GRACE
        ));
    }
}

// =========================================================================
// `partition_peers_anchor_first` — anchor-preferred peer ordering
// =========================================================================
//
// Contract: stable order within each (anchor / non-anchor) partition.
// The caller pre-shuffles for randomness; this helper just hoists the
// anchor partition to the front without reordering within it.
//
// These exercise the pure-function shape. The integration (looking up
// the anchor identity set from the store and threading it through
// `perform_interval_sync`) is covered by the `trusted_anchors_*`
// helper-level tests in `calimero-context` and end-to-end by multi-node
// e2e runs.

use std::collections::BTreeSet;

use calimero_primitives::identity::PublicKey;
use dashmap::DashMap;
use libp2p::PeerId;

use super::partition_peers_anchor_first;

fn dummy_peer(n: u8) -> PeerId {
    // Deterministic peer-id keyed by a single byte — only equality
    // matters here, not the byte structure.
    let seed = [n; 32];
    let kp = libp2p::identity::Keypair::ed25519_from_bytes(seed).expect("valid seed");
    PeerId::from_public_key(&kp.public())
}

fn dummy_pk(n: u8) -> PublicKey {
    PublicKey::from([n; 32])
}

#[test]
fn partition_empty_anchors_set_returns_zero() {
    // No anchor set defined → no preference; every peer is non-anchor.
    let mut peers = vec![dummy_peer(1), dummy_peer(2), dummy_peer(3)];
    let cache: DashMap<PeerId, BTreeSet<PublicKey>> = DashMap::new();
    let anchors: BTreeSet<PublicKey> = BTreeSet::new();

    let count = partition_peers_anchor_first(&mut peers, &cache, &anchors);
    assert_eq!(count, 0);
}

#[test]
fn partition_empty_cache_no_anchors_found() {
    // Anchor set non-empty but we've observed nothing → fall back to
    // non-anchor; relative order preserved.
    let mut peers = vec![dummy_peer(1), dummy_peer(2)];
    let original = peers.clone();
    let cache: DashMap<PeerId, BTreeSet<PublicKey>> = DashMap::new();
    let anchors: BTreeSet<PublicKey> = [dummy_pk(0xAA)].into_iter().collect();

    let count = partition_peers_anchor_first(&mut peers, &cache, &anchors);
    assert_eq!(count, 0);
    assert_eq!(peers, original);
}

#[test]
fn partition_all_peers_are_anchors() {
    let peer1 = dummy_peer(1);
    let peer2 = dummy_peer(2);
    let pk_admin = dummy_pk(0xAA);
    let cache: DashMap<PeerId, BTreeSet<PublicKey>> = DashMap::new();
    let _replaced = cache.insert(peer1, [pk_admin].into_iter().collect());
    let _replaced = cache.insert(peer2, [pk_admin].into_iter().collect());
    let anchors: BTreeSet<PublicKey> = [pk_admin].into_iter().collect();

    let mut peers = vec![peer1, peer2];
    let count = partition_peers_anchor_first(&mut peers, &cache, &anchors);
    assert_eq!(count, 2);
    assert_eq!(peers, vec![peer1, peer2]);
}

#[test]
fn partition_mixed_anchor_and_non_anchor_preserves_relative_order() {
    let anchor_a = dummy_peer(1);
    let anchor_b = dummy_peer(2);
    let plain_a = dummy_peer(3);
    let plain_b = dummy_peer(4);
    let pk_admin = dummy_pk(0xAA);

    let cache: DashMap<PeerId, BTreeSet<PublicKey>> = DashMap::new();
    let _replaced = cache.insert(anchor_a, [pk_admin].into_iter().collect());
    let _replaced = cache.insert(anchor_b, [pk_admin].into_iter().collect());

    let anchors: BTreeSet<PublicKey> = [pk_admin].into_iter().collect();

    // Pre-shuffled order interleaves the two partitions.
    let mut peers = vec![plain_a, anchor_a, plain_b, anchor_b];
    let count = partition_peers_anchor_first(&mut peers, &cache, &anchors);
    assert_eq!(count, 2);
    // Anchors first (anchor_a before anchor_b, preserved from input),
    // then non-anchors (plain_a before plain_b, preserved).
    assert_eq!(peers, vec![anchor_a, anchor_b, plain_a, plain_b]);
}

#[test]
fn partition_peer_with_one_anchor_identity_among_many_qualifies() {
    // A peer can host multiple context identities — partition matches
    // if ANY identity intersects the anchor set.
    let peer = dummy_peer(1);
    let pk_admin = dummy_pk(0xAA);
    let pk_other_context = dummy_pk(0xBB);

    let cache: DashMap<PeerId, BTreeSet<PublicKey>> = DashMap::new();
    let _replaced = cache.insert(peer, [pk_admin, pk_other_context].into_iter().collect());

    let anchors: BTreeSet<PublicKey> = [pk_admin].into_iter().collect();

    let mut peers = vec![peer];
    let count = partition_peers_anchor_first(&mut peers, &cache, &anchors);
    assert_eq!(count, 1);
}

#[test]
fn partition_peer_with_only_non_anchor_identities_does_not_qualify() {
    let peer = dummy_peer(1);
    let pk_member = dummy_pk(0xCC);
    let pk_admin = dummy_pk(0xAA);

    let cache: DashMap<PeerId, BTreeSet<PublicKey>> = DashMap::new();
    let _replaced = cache.insert(peer, [pk_member].into_iter().collect());

    let anchors: BTreeSet<PublicKey> = [pk_admin].into_iter().collect();

    let mut peers = vec![peer];
    let count = partition_peers_anchor_first(&mut peers, &cache, &anchors);
    assert_eq!(count, 0);
}

// =========================================================================
// `reconcile_cooldown` / `record_reconcile_*` — backoff for the
// reconcile-after-divergence path
// =========================================================================
//
// Contract:
// - `reconcile_cooldown(n)` doubles from a 30 s base, caps at 30 min.
// - A failure record bumps the counter and refreshes the timestamp.
// - A success record clears the entry entirely (no inherited cooldown).
// - `reconcile_remaining_cooldown` returns `None` outside the window.

use std::time::Duration;

use calimero_primitives::context::ContextId;

use super::{
    reconcile_cooldown, reconcile_remaining_cooldown, record_reconcile_failure,
    record_reconcile_success,
};
use crate::state::ReconcileAttempt;

fn dummy_context(n: u8) -> ContextId {
    ContextId::from([n; 32])
}

#[test]
fn reconcile_cooldown_schedule_doubles_then_caps() {
    assert_eq!(reconcile_cooldown(1), Duration::from_secs(30));
    assert_eq!(reconcile_cooldown(2), Duration::from_secs(60));
    assert_eq!(reconcile_cooldown(3), Duration::from_secs(120));
    assert_eq!(reconcile_cooldown(4), Duration::from_secs(240));
    assert_eq!(reconcile_cooldown(5), Duration::from_secs(480));
    assert_eq!(reconcile_cooldown(6), Duration::from_secs(960));
    assert_eq!(reconcile_cooldown(7), Duration::from_secs(30 * 60));
    // Cap holds for arbitrarily large counters.
    assert_eq!(reconcile_cooldown(50), Duration::from_secs(30 * 60));
    assert_eq!(reconcile_cooldown(u32::MAX), Duration::from_secs(30 * 60));
}

#[test]
fn reconcile_cooldown_zero_failures_treated_as_one() {
    // The function is only meant to be called when at least one
    // failure has been recorded; we still want a defined value at 0
    // rather than a panic or underflow.
    assert_eq!(reconcile_cooldown(0), Duration::from_secs(30));
}

#[test]
fn record_reconcile_failure_increments_counter_and_stamps_time() {
    let attempts: DashMap<ContextId, ReconcileAttempt> = DashMap::new();
    let ctx = dummy_context(1);

    assert_eq!(record_reconcile_failure(&attempts, ctx), 1);
    assert_eq!(record_reconcile_failure(&attempts, ctx), 2);
    assert_eq!(record_reconcile_failure(&attempts, ctx), 3);

    let entry = attempts.get(&ctx).expect("entry was inserted");
    assert_eq!(entry.consecutive_failures, 3);
    // Stamp should be very recent (within the last few seconds).
    assert!(entry.last_attempt_at.elapsed() < Duration::from_secs(5));
}

#[test]
fn record_reconcile_success_clears_entry() {
    let attempts: DashMap<ContextId, ReconcileAttempt> = DashMap::new();
    let ctx = dummy_context(1);

    let _ = record_reconcile_failure(&attempts, ctx);
    let _ = record_reconcile_failure(&attempts, ctx);
    assert!(attempts.contains_key(&ctx));

    record_reconcile_success(&attempts, &ctx);
    assert!(
        !attempts.contains_key(&ctx),
        "success should clear all backoff state for the context"
    );
}

#[test]
fn reconcile_remaining_cooldown_none_when_no_entry() {
    let attempts: DashMap<ContextId, ReconcileAttempt> = DashMap::new();
    let ctx = dummy_context(1);
    assert!(reconcile_remaining_cooldown(&attempts, &ctx).is_none());
}

#[test]
fn reconcile_remaining_cooldown_some_after_recent_failure() {
    let attempts: DashMap<ContextId, ReconcileAttempt> = DashMap::new();
    let ctx = dummy_context(1);
    let _ = record_reconcile_failure(&attempts, ctx);

    let (remaining, failures) =
        reconcile_remaining_cooldown(&attempts, &ctx).expect("within cooldown");
    assert_eq!(failures, 1);
    // The first cooldown is 30 s; the test runs in <1 s.
    assert!(remaining > Duration::from_secs(25));
    assert!(remaining <= Duration::from_secs(30));
}

#[test]
fn reconcile_remaining_cooldown_none_after_cooldown_lapsed() {
    let attempts: DashMap<ContextId, ReconcileAttempt> = DashMap::new();
    let ctx = dummy_context(1);
    // Synthesize an entry whose timestamp is far enough in the past
    // that even the maximum cooldown has lapsed.
    let _replaced = attempts.insert(
        ctx,
        ReconcileAttempt {
            last_attempt_at: std::time::Instant::now() - Duration::from_secs(60 * 60),
            consecutive_failures: 7,
        },
    );
    assert!(reconcile_remaining_cooldown(&attempts, &ctx).is_none());
}
