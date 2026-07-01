//! Byzantine handshake tests (lying root_hash / entity_count / heads).
//!
//! # What this covers
//!
//! A malicious peer controls exactly what it puts in its `SyncHandshake`:
//! `root_hash`, `entity_count`, `max_depth`, `dag_heads`, `has_state`. A
//! Byzantine peer can therefore advertise a fabricated root hash and an
//! inflated entity count to try to manipulate the victim's protocol
//! selection or to get the victim to adopt a state summary it never
//! independently verified.
//!
//! Two invariants must hold against such a peer:
//!
//! 1. **No Snapshot escalation (Invariant I5).** Protocol selection must
//!    NEVER pick `Snapshot` for an initialized (has-state) node, no matter
//!    what the remote advertises. Snapshot overwrites local state wholesale;
//!    allowing a lie to trigger it would be silent data loss. The guard lives
//!    in `select_protocol` (Rule 2a is the only Snapshot case, and it keys off
//!    the LOCAL node being fresh) and again at apply time in
//!    `request_snapshot_sync` / `check_snapshot_safety`.
//!
//! 2. **Root only advances to an independently recomputed value.** Even
//!    though the selected `HashComparison` protocol object carries the
//!    remote-advertised root, the victim's STORED root after syncing is a
//!    pure function of the entities actually written to its own Merkle tree —
//!    never the fabricated value the peer advertised. The snapshot apply path
//!    enforces the same rule by recomputing the root from storage and trusting
//!    the local computation over the peer's claim.
//!
//! These tests use the real `select_protocol` and drive a real
//! `HashComparison` session between two `SimNode`s (which use the production
//! `calimero-storage` Merkle tree), so the recomputed-root assertion runs
//! against genuine hash propagation.

use calimero_node_primitives::sync::handshake::SyncHandshake;
use calimero_node_primitives::sync::protocol::{select_protocol, SyncProtocol};
use calimero_primitives::context::ContextId;
use calimero_primitives::crdt::CrdtType;

use crate::sync_sim::prelude::*;

/// A fabricated root hash no honest node in these fixtures will ever compute.
const FABRICATED_ROOT: [u8; 32] = [0xAB; 32];

/// An initialized victim node seeded with a few entities, so it genuinely has
/// state and a well-defined, non-zero root hash.
fn victim_node(id: &str) -> SimNode {
    let ctx = ContextId::from(SimNode::DEFAULT_CONTEXT_ID);
    let mut node = SimNode::new_in_context(id, ctx);
    for i in 1..=3u64 {
        node.insert_entity(
            EntityId::from_u64(i),
            format!("victim-{i}").into_bytes(),
            CrdtType::lww_register("seed"),
        );
    }
    node
}

/// A1/A4: an initialized node is never escalated to Snapshot by a Byzantine
/// handshake, across a spread of fabricated `root_hash` / `entity_count` /
/// `max_depth` lies.
#[test]
fn byzantine_handshake_never_escalates_initialized_node_to_snapshot() {
    let mut victim = victim_node("victim");
    let local_hs = victim.build_handshake();
    assert!(
        local_hs.has_state,
        "precondition: victim must be an initialized node"
    );

    // A menu of Byzantine handshakes. Every one sets `root_hash` to a
    // non-zero fabricated value (so `has_state` is true — the peer claims to
    // have state to sync from) and lies about the size/shape of its tree to
    // try to trip a snapshot-favoring branch.
    let malicious_handshakes = [
        // Wildly inflated entity_count + deep tree: tempt a "just copy it all".
        SyncHandshake::new(FABRICATED_ROOT, u64::MAX, u32::MAX, vec![[0xCD; 32]]),
        // Huge count, shallow tree.
        SyncHandshake::new(FABRICATED_ROOT, 10_000_000, 1, vec![]),
        // Tiny remote claiming a single entity but a bogus root.
        SyncHandshake::new(FABRICATED_ROOT, 1, 1, vec![]),
        // Fabricated root with zero entity_count but has_state true — an
        // internally inconsistent lie (root implies state, count says empty).
        SyncHandshake::new(FABRICATED_ROOT, 0, 0, vec![[0x01; 32], [0x02; 32]]),
    ];

    for (i, remote_hs) in malicious_handshakes.iter().enumerate() {
        assert!(
            remote_hs.has_state,
            "fixture {i}: malicious remote advertises state"
        );
        let selection = select_protocol(&local_hs, remote_hs);
        assert!(
            !matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
            "VIOLATION (fixture {i}): a Byzantine handshake escalated an \
             initialized node to Snapshot!\n\
             remote entity_count: {}, max_depth: {}\n\
             selected: {:?} ({})",
            remote_hs.entity_count,
            remote_hs.max_depth,
            selection.protocol,
            selection.reason,
        );
    }
}

/// A4: after syncing against a peer that LIED in its handshake about its root
/// hash, the victim's stored root advances only to a value it independently
/// recomputed from its own Merkle tree — never to the fabricated value.
///
/// We craft the peer's *advertised* handshake to claim `FABRICATED_ROOT` and a
/// huge entity_count, confirm selection stays on a CRDT-merge protocol (not
/// Snapshot), then run the real `HashComparison` session against the peer's
/// genuine node. The victim's post-sync root is checked against an independent
/// oracle recomputation of the converged entity set.
#[tokio::test]
async fn byzantine_root_hash_not_adopted_only_recomputed_root_stored() {
    let ctx = ContextId::from(SimNode::DEFAULT_CONTEXT_ID);

    // Victim: initialized with entities {1,2,3}.
    let mut victim = victim_node("victim");

    // Honest data source (bob): a superset {1,2,3,4,5}. This is the node whose
    // REAL state the victim will actually merge.
    let mut source = SimNode::new_in_context("source", ctx);
    for i in 1..=5u64 {
        source.insert_entity(
            EntityId::from_u64(i),
            format!("victim-{i}").into_bytes(),
            CrdtType::lww_register("seed"),
        );
    }
    // Entities 1..=3 must be byte-identical to the victim's so the merge
    // converges to a single deterministic tree.

    let source_real_root = source.root_hash();
    assert_ne!(
        source_real_root, FABRICATED_ROOT,
        "sanity: the honest source's real root is not the fabricated value"
    );

    // The Byzantine handshake the peer PUTS ON THE WIRE: it lies about its root
    // (claims FABRICATED_ROOT) and inflates entity_count. `select_protocol`
    // sees this fabricated summary.
    let victim_hs = victim.build_handshake();
    let lying_remote_hs = SyncHandshake::new(FABRICATED_ROOT, u64::MAX, 8, vec![[0xEE; 32]]);
    let selection = select_protocol(&victim_hs, &lying_remote_hs);

    // Invariant 1: no Snapshot escalation even though the peer lied big.
    assert!(
        !matches!(selection.protocol, SyncProtocol::Snapshot { .. }),
        "lying handshake must not escalate to Snapshot; got {:?}",
        selection.protocol
    );
    // The selected HashComparison object literally carries the fabricated root
    // as the comparison target — proving the advertised value flows into
    // protocol selection, which makes the recompute assertion below meaningful.
    if let SyncProtocol::HashComparison { root_hash } = selection.protocol {
        assert_eq!(
            root_hash, FABRICATED_ROOT,
            "selection carries the remote-advertised (fabricated) root as its target"
        );
    }

    let victim_root_before = victim.root_hash();

    // Now run the REAL sync against the peer's GENUINE node. The data path
    // ignores the advertised handshake root entirely and merges actual
    // entities, recomputing the victim's root from its own storage.
    execute_hash_comparison_sync(&mut victim, &source)
        .await
        .expect("hash-comparison sync should succeed");

    let victim_root_after = victim.root_hash();

    // Invariant 2a: the victim NEVER adopted the fabricated root.
    assert_ne!(
        victim_root_after, FABRICATED_ROOT,
        "victim must never store the peer's fabricated root_hash"
    );

    // Invariant 2b: the victim's root advanced (it learned entities 4,5) and
    // now equals the honest source's REAL, independently-computed root.
    assert_ne!(
        victim_root_after, victim_root_before,
        "victim should have merged new state and advanced its root"
    );
    assert_eq!(
        victim_root_after, source_real_root,
        "victim converged to the source's genuine (recomputed) root"
    );

    // Invariant 2c (independent oracle): build a fresh node from scratch with
    // exactly the converged entity set and confirm it computes the same root.
    // This proves the victim's stored root is a pure function of the entities
    // actually written to its Merkle tree — a value anyone can recompute — and
    // was not lifted from any advertised handshake field.
    let mut oracle = SimNode::new_in_context("oracle", ctx);
    for i in 1..=5u64 {
        oracle.insert_entity(
            EntityId::from_u64(i),
            format!("victim-{i}").into_bytes(),
            CrdtType::lww_register("seed"),
        );
    }
    assert_eq!(
        victim_root_after,
        oracle.root_hash(),
        "victim's stored root must equal an independent recomputation of the \
         converged state, never a peer-advertised value"
    );
}
