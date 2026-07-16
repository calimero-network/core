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

/// `merge_members_first` puts cached members ahead of discovered peers,
/// preserves discovery order for the rest, and dedups the overlap.
#[test]
fn merge_members_first_orders_and_dedups() {
    use libp2p::PeerId;

    let m1 = PeerId::random();
    let m2 = PeerId::random();
    let d1 = PeerId::random();

    // m2 also appears among discovered → must not be duplicated.
    let merged = SyncManager::merge_members_first(vec![m1, m2], vec![d1, m2]);
    assert_eq!(
        merged,
        vec![m1, m2, d1],
        "members first, discovered appended once"
    );

    // No cached members → discovery order is preserved verbatim.
    assert_eq!(
        SyncManager::merge_members_first(vec![], vec![d1, m1]),
        vec![d1, m1]
    );
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
            (log2_approx / 4).clamp(1, 32)
        };

        assert!(
            max_depth >= expected_min_depth,
            "entity_count={entity_count} should have max_depth >= {expected_min_depth}, got {max_depth}"
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

// =========================================================================
// Tests for the #2613 group-key recovery trigger
//
// Direct (pull-based) key delivery folded the pull into
// `sync_namespace_from_peer`, which only runs on an edge trigger (join /
// startup / readiness) or when `should_backfill_governance` fires
// (governance-pending > 0). A member that is caught up on governance but
// missing only a group key has an EMPTY pending buffer and no pending edge
// — so the pull never re-fired and it stayed permanently locked out of
// group decryption (the exact #2613 failure mode, relocated to the pull
// side). The fix drives key recovery from the interval tick too, gated on
// "do I lack a key for a group I hold buffered ops for" — INDEPENDENT of
// the governance-pending buffer. These tests pin that decoupling so a
// future refactor can't silently re-gate key recovery behind
// governance-pending.
// =========================================================================

mod key_recovery_trigger {
    use std::sync::Arc;

    use calimero_context::group_store::{
        namespace_groups_awaiting_key, GroupKeyring, NamespaceOpLogService,
    };
    use calimero_context_client::local_governance::{GroupOp, NamespaceOp, SignedNamespaceOp};
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::super::should_backfill_governance;

    fn fresh_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    #[test]
    fn keyless_member_awaits_key_even_with_empty_governance_pending() {
        let store = fresh_store();
        let mut rng = rand::rngs::OsRng;
        let signer_sk = PrivateKey::random(&mut rng);

        let namespace_id = [0xE7u8; 32];
        let group_id = [0xE8u8; 32];
        let group_gid = ContextGroupId::from(group_id);

        // Buffer an encrypted group op we can't decrypt (no key held) — the
        // steady state of a member that joined but never got its key.
        let op = SignedNamespaceOp::sign(
            &signer_sk,
            namespace_id.into(),
            vec![],
            1,
            NamespaceOp::Group {
                group_id: group_id.into(),
                key_id: GroupKeyring::key_id_for(&[0xAA; 32]).into(),
                encrypted: GroupKeyring::encrypt_op(&[0xAA; 32], &GroupOp::Noop).unwrap(),
                key_rotation: None,
            },
        )
        .unwrap();
        NamespaceOpLogService::new(&store, namespace_id.into())
            .store_signed_operation(&op)
            .unwrap();

        // The governance-pending buffer is empty, so the #2625 backfill gate
        // would NOT fire. This is exactly the lockout state — and the reason
        // key recovery must NOT depend on this gate.
        assert!(!should_backfill_governance(0));

        // Key recovery still has work to do: the group is awaiting a key.
        // The interval tick drives `recover_missing_group_keys` on this
        // signal, decoupled from the gate above.
        let awaiting = namespace_groups_awaiting_key(&store, namespace_id.into()).unwrap();
        assert_eq!(
            awaiting,
            vec![group_id],
            "a keyless member must surface an awaiting group regardless of the empty governance-pending buffer"
        );

        // Once the key arrives, the recovery trigger condition clears.
        GroupKeyring::new(&store, group_gid)
            .store_key(&[0xAA; 32])
            .unwrap();
        assert!(namespace_groups_awaiting_key(&store, namespace_id.into())
            .unwrap()
            .is_empty());
    }
}

// `should_stop_peer_retry` stops `perform_interval_sync` only for the
// unsatisfiable PendingParentsUnresolved, not peer-specific failures.
mod pending_parents_short_circuit {
    use calimero_primitives::context::ContextId;

    use super::super::{should_stop_peer_retry, NoPeersAvailable, PendingParentsUnresolved};

    fn ctx() -> ContextId {
        ContextId::from([7u8; 32])
    }

    #[test]
    fn mesh_swept_parent_pull_stops_the_peer_loop() {
        // Emitted only when the whole mesh was swept — another peer is pointless.
        let err = eyre::Error::new(PendingParentsUnresolved {
            context_id: ctx(),
            remaining: 3,
            attempts: 4,
        });
        assert!(should_stop_peer_retry(&err));
    }

    #[test]
    fn detection_survives_nested_wrap_err() {
        // Production wraps twice (handle_dag_sync + initiate_sync_inner) before
        // the retry loop; if downcast didn't traverse the whole eyre chain the
        // fix would be a silent no-op.
        use eyre::WrapErr as _;
        let wrapped = Err::<(), _>(eyre::Error::new(PendingParentsUnresolved {
            context_id: ctx(),
            remaining: 1,
            attempts: 2,
        }))
        .wrap_err("request DAG heads and sync")
        .wrap_err("DAG sync")
        .unwrap_err();
        assert!(should_stop_peer_retry(&wrapped));
    }

    #[test]
    fn peer_specific_errors_keep_iterating() {
        // A different peer can still succeed, so these must not short-circuit.
        assert!(!should_stop_peer_retry(&eyre::eyre!(
            "connection reset by peer"
        )));
        assert!(!should_stop_peer_retry(&eyre::Error::new(
            NoPeersAvailable { context_id: ctx() }
        )));
    }

    #[test]
    fn non_swept_path_message_alone_does_not_short_circuit() {
        // The cap/time-budget path bail!s the same wording untyped; only the
        // type stops the loop, so a message-identical plain error must not.
        let plain = eyre::eyre!(
            "pending parents unresolved for context {}: 3 remaining after 4 peer attempt(s)",
            ctx()
        );
        assert!(!should_stop_peer_retry(&plain));
    }

    #[test]
    fn display_pins_operator_log_format() {
        // The non-swept path bail!s via this Display, so keep the operator-facing
        // line stable regardless of which path produced it.
        let shown = format!(
            "{}",
            PendingParentsUnresolved {
                context_id: ctx(),
                remaining: 3,
                attempts: 4,
            }
        );
        assert_eq!(
            shown,
            format!(
                "pending parents unresolved for context {}: 3 remaining after 4 peer attempt(s)",
                ctx()
            )
        );
    }
}

// The inbound materialisation wait must abandon the moment the dialing peer
// disconnects, instead of polling for the full window.
mod materialization_wait {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use calimero_network_primitives::stream::Stream;
    use tokio::time::{Duration, Instant};

    use super::super::{
        await_materialization_or_close, MaterializationOutcome, MaterializationProbe,
    };

    /// A probe that always reports "verified, but not materialised yet".
    fn never_ready() -> eyre::Result<MaterializationProbe<()>> {
        Ok(MaterializationProbe::Waiting {
            dialer_verified: true,
        })
    }

    #[tokio::test]
    async fn cancels_immediately_when_dialer_closes() {
        let (mut responder, dialer) = Stream::test_pair();
        // Dialer gives up: dropping its end yields EOF on the responder's read.
        drop(dialer);

        // Deadline an hour out: cancellation is the ONLY path that returns
        // promptly, so a prompt return is itself the proof. Bounding the call
        // with a generous `timeout` (rather than asserting a tight elapsed)
        // means a slow CI scheduler can't make this flaky — the 1h deadline is
        // never the thing we race against.
        let outcome = tokio::time::timeout(
            Duration::from_secs(30),
            await_materialization_or_close::<(), _>(
                &mut responder,
                Instant::now() + Duration::from_secs(3600),
                Duration::from_millis(200),
                never_ready,
            ),
        )
        .await
        .expect("must abort on disconnect, not block until the deadline")
        .unwrap();

        assert!(matches!(outcome, MaterializationOutcome::PeerGone));
    }

    // `start_paused` drives the poll/deadline on tokio's virtual clock, so the
    // loop advances deterministically regardless of real CI scheduler latency.
    #[tokio::test(start_paused = true)]
    async fn runs_to_deadline_while_dialer_stays_connected() {
        // Keep both ends alive so the read stays pending and only the poll
        // timer drives the loop to its deadline.
        let (mut responder, _dialer) = Stream::test_pair();

        let outcome = await_materialization_or_close::<(), _>(
            &mut responder,
            Instant::now() + Duration::from_millis(120),
            Duration::from_millis(20),
            never_ready,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            MaterializationOutcome::Elapsed {
                dialer_verified: true
            }
        ));
    }

    // Virtual clock (see `runs_to_deadline_*`): deterministic, no real-time race.
    #[tokio::test(start_paused = true)]
    async fn returns_ready_when_probe_resolves() {
        let (mut responder, _dialer) = Stream::test_pair();
        let calls = AtomicUsize::new(0);
        // Ready on the 2nd poll; Waiting on the 1st.
        let probe = || {
            if calls.fetch_add(1, Ordering::SeqCst) >= 1 {
                Ok(MaterializationProbe::Ready(42u32))
            } else {
                Ok(MaterializationProbe::Waiting {
                    dialer_verified: true,
                })
            }
        };

        let outcome = await_materialization_or_close::<u32, _>(
            &mut responder,
            Instant::now() + Duration::from_secs(10),
            Duration::from_millis(10),
            probe,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, MaterializationOutcome::Ready(42)));
    }

    // Virtual clock (see `runs_to_deadline_*`): deterministic, no real-time race.
    #[tokio::test(start_paused = true)]
    async fn times_out_unverified_dialer() {
        // Dialer stays connected but is never verified as a member: the wait
        // runs to the deadline and reports the unverified outcome (which the
        // caller turns into an `OpaqueError` close).
        let (mut responder, _dialer) = Stream::test_pair();

        let outcome = await_materialization_or_close::<(), _>(
            &mut responder,
            Instant::now() + Duration::from_millis(120),
            Duration::from_millis(20),
            || {
                Ok(MaterializationProbe::Waiting {
                    dialer_verified: false,
                })
            },
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            MaterializationOutcome::Elapsed {
                dialer_verified: false
            }
        ));
    }
}
