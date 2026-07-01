//! The C2 unified-store applier: folds each unified [`Op`] into a
//! [`ScopeProjections`] as the per-context `DagStore<Op>` releases it in causal
//! order.
//!
//! This is the **projection-backing half** of the unified store (cutover plan
//! C2.0). A single op-log — one [`calimero_dag::DagStore<Op>`] — drives the
//! projection (the auth + data planes that fold into [`ScopeProjections`] and
//! produce the `scope_root` convergence signal). The DAG owns causal ordering and
//! parent-buffering; this applier just folds whatever the DAG hands it, in the
//! order the DAG decides is causally valid.
//!
//! Deliberately NOT covered here (lands in the per-plane C2 slices, C2.2+):
//! - the data-plane **storage** apply (`__calimero_sync_next` → the Merkle tree);
//! - the legacy **materialized** governance/rotation writes, which stay on the
//!   existing typed receive path (dual-write) until C5.
//!
//! Additive: nothing in production drives this yet. It is the foundation the
//! dual-write (C2.1) and per-plane flips build on, exercised by the
//! out-of-order convergence test below.

use std::sync::{Arc, PoisonError, RwLock};

use async_trait::async_trait;
use calimero_dag::{ApplyError, CausalDelta, DeltaApplier};
use calimero_op::Op;

use crate::scope_projection::ScopeProjections;

/// Folds unified [`Op`]s into a [`ScopeProjections`] in the order a
/// `DagStore<Op>` releases them.
///
/// Holds the projection behind the **same** `Arc<std::sync::RwLock<…>>` that
/// `ContextManager` / `NodeState` already use for `scope_projections`, so the
/// dual-write (C2.1) shares the node's live projection via [`with_projection`]
/// rather than folding into a private copy. The lock is held only across the
/// synchronous [`ScopeProjections::ingest_op`] — never across an `.await` — so the
/// `std` (non-async) lock is correct here, matching the node's own usage.
///
/// [`with_projection`]: Self::with_projection
pub struct UnifiedApplier {
    projection: Arc<RwLock<ScopeProjections>>,
}

impl UnifiedApplier {
    /// A fresh, private projection — for standalone replay and tests.
    #[must_use]
    pub fn new() -> Self {
        Self {
            projection: Arc::new(RwLock::new(ScopeProjections::new())),
        }
    }

    /// Fold into an **existing** shared projection — the C2.1 wiring seam, where
    /// the applier writes into `ContextManager`'s
    /// `Arc<std::sync::RwLock<ScopeProjections>>` so the unified op-log and the
    /// node's maintained projection are one and the same.
    #[must_use]
    pub fn with_projection(projection: Arc<RwLock<ScopeProjections>>) -> Self {
        Self { projection }
    }

    /// The shared projection handle (read via `.read()`), for inspection.
    #[must_use]
    pub fn projection(&self) -> Arc<RwLock<ScopeProjections>> {
        Arc::clone(&self.projection)
    }
}

impl Default for UnifiedApplier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeltaApplier<Op> for UnifiedApplier {
    async fn apply(&self, delta: &CausalDelta<Op>) -> Result<(), ApplyError> {
        // The DAG has already established that `delta`'s parents are applied, so
        // folding its op now respects causal order. `ingest_op` is infallible and
        // idempotent by op id (a re-delivered delta is a safe no-op), so there is
        // no failure to surface as `ApplyError`. The lock is held only across this
        // synchronous fold (no `.await`). On poison we recover the guard rather
        // than skip — the same deliberate stance `NodeState::read_scope_projections`
        // documents: a panicking writer elsewhere must not silently blind the
        // projection.
        self.projection
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .ingest_op(&delta.payload);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU128;

    use calimero_context_config::types::ContextGroupId;
    use calimero_dag::DagStore;
    use calimero_op::{Op, OpPayload, ScopeId};
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PublicKey;
    use calimero_storage::address::Id;
    use calimero_storage::entities::OpMask;
    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};

    use super::*;

    const GENESIS: [u8; 32] = [0u8; 32];

    fn hlc(ns: u64) -> HybridTimestamp {
        HybridTimestamp::new(Timestamp::new(NTP64(ns), ID::from(NonZeroU128::MIN)))
    }

    /// Build a fully-formed `Op` (id derived from content) under one scope, with a
    /// fixed author. The signature is unused here — the projection folds verified
    /// ops on content alone (the DAG/applier under test does no auth).
    fn op(scope: ScopeId, ns: u64, parents: Vec<[u8; 32]>, payload: OpPayload) -> Op {
        let author = PublicKey::from([7u8; 32]);
        let h = hlc(ns);
        Op::new(scope, parents, author, h, payload, [0u8; 32], [0u8; 64])
    }

    fn delta(op: &Op) -> CausalDelta<Op> {
        // The unified delta IS the op: mirror id/parents so the DAG's causal model
        // matches the op's own parent set.
        CausalDelta::new(op.id(), op.parents.clone(), op.clone(), op.hlc, [0u8; 32])
    }

    /// Fold a causal chain of mixed-plane ops (admin → member → writers → data)
    /// through `DagStore<Op>` + `UnifiedApplier` in several delivery orders, and
    /// assert the projection converges to the same `scope_root` every time — i.e.
    /// the DAG's parent-buffering releases out-of-order deltas in a causal order
    /// the applier folds consistently.
    #[tokio::test]
    async fn dag_releases_out_of_order_ops_and_the_projection_converges() {
        let scope = ScopeId::from([0xA1; 32]);
        let group = ContextGroupId::from([3u8; 32]);
        let member = PublicKey::from([0x55; 32]);
        let admin = PublicKey::from([1u8; 32]);

        // A causal chain spanning the admin, membership, ACL, and data planes.
        let op_admin = op(
            scope,
            10,
            vec![],
            OpPayload::AdminChanged { new_admin: admin },
        );
        let op_member = op(
            scope,
            20,
            vec![op_admin.id()],
            OpPayload::MemberAdded {
                group,
                member,
                role: GroupMemberRole::Member,
            },
        );
        let op_writers = op(
            scope,
            30,
            vec![op_member.id()],
            OpPayload::SetWriters {
                object: Id::new([9u8; 32]),
                writers: [(member, OpMask::FULL)].into_iter().collect(),
            },
        );
        let op_put = op(
            scope,
            40,
            vec![op_writers.id()],
            OpPayload::Put {
                entity: Id::new([9u8; 32]),
                value: b"v1".to_vec(),
            },
        );
        let ops = [&op_admin, &op_member, &op_writers, &op_put];

        // Reference: fold in causal order via the projection directly.
        let mut reference = ScopeProjections::new();
        for op in ops {
            reference.ingest_op(op);
        }
        let want = reference
            .scope_root_for(&scope, [0u8; 32])
            .expect("scope fed");

        // Index permutations of the 4 ops, including the worst case (full reverse,
        // every child before its parent). The DAG must buffer + cascade each into
        // the same applied order.
        let orders: [[usize; 4]; 4] = [
            [0, 1, 2, 3], // causal
            [3, 2, 1, 0], // reverse — every op arrives before its parent
            [3, 0, 2, 1], // interleaved
            [1, 3, 0, 2], // interleaved
        ];

        for order in orders {
            let mut dag = DagStore::<Op>::new(GENESIS);
            let applier = UnifiedApplier::new();
            for &i in &order {
                dag.add_delta(delta(ops[i]), &applier)
                    .await
                    .expect("add_delta");
            }
            // Every op must have applied (none stuck pending) regardless of order.
            for op in ops {
                assert!(dag.is_applied(&op.id()), "op {order:?} left unapplied");
            }
            let got = applier
                .projection()
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .scope_root_for(&scope, [0u8; 32])
                .expect("scope fed");
            assert_eq!(
                got, want,
                "delivery order {order:?} converged to a different scope_root"
            );
        }
    }
}
