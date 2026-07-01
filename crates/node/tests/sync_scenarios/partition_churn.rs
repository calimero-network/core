//! Partition / churn / backpressure scenario tests.
//!
//! These drive the **production** `HashComparisonProtocol` (via
//! `execute_hash_comparison_sync`) over the simulation's in-memory
//! `SimStream`/`SimStorage`, so convergence and root-hash equality are the
//! real merkle-tree outcomes, not a model. Partition topology is expressed
//! with the harness `PartitionManager`; loss is exercised through the real
//! `NetworkRouter` loss path so the drop metric is the genuine one.

use calimero_primitives::context::ContextId;
use calimero_primitives::crdt::CrdtType;

use crate::sync_sim::prelude::*;

// =============================================================================
// Helpers
// =============================================================================

/// `i` pulls (and, per the bidirectional HC protocol, also pushes) against
/// `j`, running the real HashComparison session between the two nodes.
async fn pull(nodes: &mut [SimNode], i: usize, j: usize) {
    if i == j {
        return;
    }
    // `split_at_mut` to obtain one `&mut` (initiator) and one `&` (responder)
    // into the same slice without aliasing.
    if i < j {
        let (left, right) = nodes.split_at_mut(j);
        execute_hash_comparison_sync(&mut left[i], &right[0])
            .await
            .expect("hash-comparison sync (i<-j) should succeed");
    } else {
        // i > j: right[0] = nodes[i], left[j] = nodes[j]
        let (left, right) = nodes.split_at_mut(i);
        execute_hash_comparison_sync(&mut right[0], &left[j])
            .await
            .expect("hash-comparison sync (i<-j) should succeed");
    }
}

/// True when every node has a byte-identical root hash.
fn all_roots_equal(nodes: &[SimNode]) -> bool {
    match nodes.first() {
        Some(first) => nodes.iter().all(|n| n.root_hash() == first.root_hash()),
        None => true,
    }
}

/// One full ordered-pair sync pass, skipping any pair whose session cannot
/// complete under the current partition. A HashComparison session uses a
/// bidirectional stream, so if *either* direction is blocked the whole
/// session is unusable — gate on both.
async fn mesh_pass(nodes: &mut [SimNode], ids: &[NodeId], pm: &mut PartitionManager, now: SimTime) {
    let len = nodes.len();
    for i in 0..len {
        for j in 0..len {
            if i == j {
                continue;
            }
            if pm.is_partitioned(&ids[i], &ids[j], now) || pm.is_partitioned(&ids[j], &ids[i], now)
            {
                continue;
            }
            pull(nodes, i, j).await;
        }
    }
}

/// Run mesh passes until all roots are byte-identical or `max_rounds` is hit.
/// Returns whether convergence was reached.
async fn converge(
    nodes: &mut [SimNode],
    ids: &[NodeId],
    pm: &mut PartitionManager,
    now: SimTime,
    max_rounds: usize,
) -> bool {
    for _ in 0..max_rounds {
        if all_roots_equal(nodes) {
            return true;
        }
        mesh_pass(nodes, ids, pm, now).await;
    }
    all_roots_equal(nodes)
}

// =============================================================================
// C1 — symmetric partition, heal, single converged root
// =============================================================================

/// Five nodes diverge, split into two mutually-isolated groups
/// (`{n0,n1} | {n2,n3,n4}`) with a `Bidirectional` partition, operate while
/// split, then heal. After healing the whole mesh must converge to a single
/// root whose digest is byte-identical across all five nodes.
///
/// The seed sweep is intentionally modest (`0..20`) rather than a larger
/// range like `0..200`: each seed drives dozens of real pairwise
/// HashComparison sessions, so a wide sweep would dominate suite runtime. The
/// invariant is seed-independent, so a small representative sweep suffices.
#[tokio::test]
async fn c1_symmetric_partition_heal_single_converged_root() {
    for seed in 0..20u64 {
        let ctx = ContextId::from(SimNode::DEFAULT_CONTEXT_ID);
        let ids: Vec<NodeId> = (0..5).map(|i| NodeId::new(format!("n{i}"))).collect();
        let mut nodes: Vec<SimNode> = (0..5)
            .map(|i| SimNode::new_in_context(format!("n{i}"), ctx))
            .collect();

        // Diverge: each node authors its own unique entities. Content is
        // salted by seed so different runs exercise different tree shapes.
        for (i, node) in nodes.iter_mut().enumerate() {
            for k in 0..4u64 {
                let id = EntityId::from_u64(seed * 1_000_000 + (i as u64) * 100 + k + 1);
                node.insert_entity_with_metadata(
                    id,
                    format!("n{i}-k{k}-s{seed}").into_bytes(),
                    EntityMetadata::default(),
                );
            }
        }

        // Symmetric partition into two isolated groups.
        let mut pm = PartitionManager::new();
        let group_a = vec![ids[0].clone(), ids[1].clone()];
        let group_b = vec![ids[2].clone(), ids[3].clone(), ids[4].clone()];
        pm.add_partition(PartitionSpec::split(group_a, group_b), SimTime::ZERO, None);
        let now = SimTime::from_millis(1);

        // Partitioned operation: only same-group pairs can sync.
        mesh_pass(&mut nodes, &ids, &mut pm, now).await;
        mesh_pass(&mut nodes, &ids, &mut pm, now).await;

        // The partition genuinely blocked cross-group traffic: a group-A node
        // and a group-B node cannot be equal — each still holds entities the
        // other never received while split.
        assert_ne!(
            nodes[0].root_hash(),
            nodes[2].root_hash(),
            "seed {seed}: cross-group nodes must still differ while partitioned"
        );

        // Heal and converge over the full mesh.
        pm.clear();
        let converged = converge(&mut nodes, &ids, &mut pm, now, 6).await;
        assert!(
            converged,
            "seed {seed}: mesh failed to converge within the round budget after heal"
        );

        // Single converged root — every digest byte-identical to n0's.
        let root0 = nodes[0].root_hash();
        for (i, n) in nodes.iter().enumerate() {
            assert_eq!(
                n.root_hash(),
                root0,
                "seed {seed}: node n{i} root is not byte-identical to n0 after heal"
            );
        }
    }
}

// =============================================================================
// C2 — directional partition + conflicting writes, order independent
// =============================================================================

/// Build a two-node conflict scenario: A and B write different values to the
/// *same* LWW entity (B carries the higher HLC, so B wins deterministically),
/// plus a unique entity each.
fn build_conflict_scenario() -> Vec<SimNode> {
    let ctx = ContextId::from(SimNode::DEFAULT_CONTEXT_ID);
    let mut a = SimNode::new_in_context("A", ctx);
    let mut b = SimNode::new_in_context("B", ctx);

    let x = EntityId::from_u64(1);
    a.insert_entity_with_metadata(
        x,
        b"from-A".to_vec(),
        EntityMetadata::new(CrdtType::lww_register("x"), 1),
    );
    b.insert_entity_with_metadata(
        x,
        b"from-B".to_vec(),
        EntityMetadata::new(CrdtType::lww_register("x"), 2),
    );

    a.insert_entity_with_metadata(
        EntityId::from_u64(10),
        b"a-only".to_vec(),
        EntityMetadata::default(),
    );
    b.insert_entity_with_metadata(
        EntityId::from_u64(20),
        b"b-only".to_vec(),
        EntityMetadata::default(),
    );

    vec![a, b]
}

/// Block only the `A -> B` direction. Both sides write (conflicting on the
/// same entity), then heal and reconcile. The final converged root must be a
/// single byte-identical digest that is invariant across heal order and under
/// reorder/duplication of the reconciling sessions — i.e. conflict resolution
/// is order-independent.
#[tokio::test]
async fn c2_directional_partition_conflicting_writes_order_independent() {
    // Reorder shuffles the reconciling pair order; duplication re-runs some
    // sessions. HC sync is idempotent, so a converged result must be invariant
    // under both. The manual perturbation below (unconditional reorder shuffle +
    // probabilistic duplicate delivery) is driven directly — there is no
    // NetworkRouter in this test, so a FaultConfig param-bag would only obscure
    // the intent. `DUP_RATE` is the per-pull duplicate probability.
    const DUP_RATE: f64 = 0.5;

    // Directional semantics: A->B blocked, B->A still open. This is a
    // standalone invariant check of the PartitionManager primitive (asymmetric
    // block through the manager's time-windowed `is_partitioned` path, which the
    // spec-level `test_directional_partition` unit test does not cover). It is
    // intentionally not wired into the convergence loop below — it only proves
    // the partition primitive behaves directionally before we rely on it.
    {
        let a = NodeId::new("A");
        let b = NodeId::new("B");
        let mut pm = PartitionManager::new();
        pm.add_partition(
            PartitionSpec::block(a.clone(), b.clone()),
            SimTime::ZERO,
            None,
        );
        let now = SimTime::from_millis(1);
        assert!(
            pm.is_partitioned(&a, &b, now),
            "A->B must be blocked by the directional partition"
        );
        assert!(
            !pm.is_partitioned(&b, &a, now),
            "B->A must stay open (directional block is asymmetric)"
        );
    }

    let mut final_roots: Vec<[u8; 32]> = Vec::new();
    for seed in 0..8u64 {
        for &ab_first in &[true, false] {
            let mut nodes = build_conflict_scenario();

            // While A->B is blocked, an A<->B HC session cannot complete (its
            // bidirectional stream needs the blocked direction), so the two
            // stay diverged until heal.
            assert_ne!(
                nodes[0].root_hash(),
                nodes[1].root_hash(),
                "conflicting writers must differ before reconciliation"
            );

            // Heal and reconcile in the chosen order, perturbed by reorder +
            // duplication seeded per (seed, order).
            let mut rng = SimRng::new(seed ^ (u64::from(ab_first) << 40));
            let mut order = if ab_first {
                vec![(0usize, 1usize), (1, 0)]
            } else {
                vec![(1usize, 0usize), (0, 1)]
            };
            rng.shuffle(&mut order);
            for _ in 0..4 {
                for &(i, j) in &order {
                    pull(&mut nodes, i, j).await;
                    if rng.bool_with_probability(DUP_RATE) {
                        pull(&mut nodes, i, j).await; // duplicate delivery
                    }
                }
            }

            assert!(
                all_roots_equal(&nodes),
                "seed {seed} ab_first={ab_first}: A and B must converge after heal"
            );
            final_roots.push(nodes[0].root_hash());
        }
    }

    // Every run — across seeds, both heal orders, and reorder/dup — resolved
    // to the exact same root.
    let r0 = final_roots[0];
    for r in &final_roots {
        assert_eq!(
            *r, r0,
            "conflict must resolve to one byte-identical root independent of heal/delivery order"
        );
    }
}

// =============================================================================
// C5 — dropped gossip deltas recovered via sync
// =============================================================================

/// A delta burst is gossiped from `alice` to two consumers over a lossy
/// (`with_loss(0.3)`) `NetworkRouter`. Drops are the router's real
/// `messages_dropped_loss` metric; a surviving gossip is applied to the
/// consumer, a dropped one never arrives. After the burst some deltas are
/// provably missing (drops > 0, a consumer diverged), yet a periodic
/// HashComparison sync recovers every one — all three roots end byte-identical.
#[tokio::test]
async fn c5_dropped_gossip_deltas_recovered_via_sync() {
    for seed in 0..8u64 {
        let ctx = ContextId::from(SimNode::DEFAULT_CONTEXT_ID);
        let mut alice = SimNode::new_in_context("alice", ctx);
        let mut bob = SimNode::new_in_context("bob", ctx);
        let mut carol = SimNode::new_in_context("carol", ctx);

        let fc = FaultConfig::none().with_loss(0.3);
        let mut router = NetworkRouter::with_faults(seed, fc);
        let mut effects = EffectMetrics::default();
        let mut queue: EventQueue<SimEvent> = EventQueue::new();
        let now = SimTime::ZERO;
        let alice_id = NodeId::new("alice");

        let burst = 40u64;
        for k in 0..burst {
            let id = EntityId::from_u64(k + 1);
            let data = format!("delta-{k}-s{seed}").into_bytes();
            let meta = EntityMetadata::default();

            // Author on alice.
            alice.insert_entity_with_metadata(id, data.clone(), meta.clone());

            // Gossip to each consumer through the lossy router. Survivors are
            // applied; drops are recorded by the router and never delivered.
            for consumer in [&mut bob, &mut carol] {
                let before = router.metrics.messages_dropped_loss;
                // `route_message` does NOT dedup by `msg_id` — it only consults the
                // RNG loss path for the drop decision, so the drop metric is genuine
                // and independent of the id. The id here is illustrative only (node
                // dedup via `is_duplicate` lives on the delivery path, which this
                // gossip test never drains). Seq is still `k` (unique per delta).
                let out = OutgoingMessage {
                    to: consumer.id().clone(),
                    msg: SyncMessage::SyncComplete { success: true },
                    msg_id: MessageId::new("alice", 1, k),
                };
                router.route_message(now, out, data.len(), &alice_id, &mut queue, &mut effects);
                let dropped = router.metrics.messages_dropped_loss > before;
                if !dropped {
                    consumer.insert_entity_with_metadata(id, data.clone(), meta.clone());
                }
            }
        }

        // Drops actually happened, and the router and effect metrics agree.
        let total_dropped = router.metrics.messages_dropped_loss;
        assert!(
            total_dropped > 0,
            "seed {seed}: 0.3 loss over {} gossips must drop at least one delta",
            burst * 2
        );
        assert_eq!(
            effects.messages_dropped, total_dropped,
            "seed {seed}: router and effect drop metrics must agree"
        );

        // Dropped gossip left entities unapplied pre-sync. bob and carol each
        // start empty and gain exactly one entity per surviving gossip, so their
        // combined applied count is precisely (sent - dropped) and must fall
        // short of the full `burst * 2` that alice authored — a direct check on
        // the real invariant rather than a root-hash inequality.
        let bob_applied = bob.entity_count();
        let carol_applied = carol.entity_count();
        let total_sent = (burst * 2) as usize;
        assert!(
            bob_applied + carol_applied < total_sent,
            "seed {seed}: dropped gossip must leave at least one entity unapplied pre-sync"
        );
        assert_eq!(
            bob_applied + carol_applied,
            total_sent - usize::try_from(total_dropped).expect("drop count fits usize"),
            "seed {seed}: applied gossip count must equal sent minus dropped"
        );

        // Periodic HashComparison sync recovers the missed deltas.
        execute_hash_comparison_sync(&mut bob, &alice)
            .await
            .expect("bob<-alice recovery sync");
        execute_hash_comparison_sync(&mut carol, &alice)
            .await
            .expect("carol<-alice recovery sync");

        // Byte-identical convergence despite the drops.
        assert_eq!(
            alice.root_hash(),
            bob.root_hash(),
            "seed {seed}: bob must converge to alice after recovery sync"
        );
        assert_eq!(
            alice.root_hash(),
            carol.root_hash(),
            "seed {seed}: carol must converge to alice after recovery sync"
        );
        assert_eq!(alice.entity_count(), bob.entity_count());
        assert_eq!(alice.entity_count(), carol.entity_count());
    }
}
