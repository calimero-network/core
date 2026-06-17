//! Substrate for the unified causal log: convert the live apply stream into
//! [`Op`]s and maintain one [`ScopeState`] projection per context.
//!
//! This is **additive** — nothing routes a decision or a convergence check
//! through it yet. It is the building block the apply path feeds once the
//! unified op-log replaces the separate data / governance / rotation stores:
//! maintain one projection per context, and derive its convergence root by
//! folding the projection's ACL + governance hashes onto the storage layer's
//! existing Merkle entities root (so a hash-neutral writer/membership rotation
//! moves the root). Wiring it into the live appliers, persistence, and bounding
//! the registry come in later increments.

use std::collections::HashMap;

use calimero_op::{Op, OpPayload, ScopeId};
use calimero_op_adapter::payload_from_action;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_projection::ScopeState;
use calimero_storage::action::Action;
use calimero_storage::logical_clock::HybridTimestamp;

/// Convert one data delta's worth of storage [`Action`]s into the unified
/// [`Op`]s representing the same writes, all sharing the delta's causal
/// coordinates (`scope` / `parents` / `author` / `hlc`).
///
/// One op per state-changing action (`Action::Compare` is a sync hint, not a
/// change, so it is dropped). Each op is content-addressed via
/// [`Op::compute_id`]; distinct entities give distinct ids even under the
/// shared coordinates because the payload differs. `signature` is left zero —
/// signing / verification is a wire-boundary concern handled by the caller.
///
/// Granularity note: this emits one op **per action**. Whether the live cutover
/// keeps that or batches a multi-action delta into a single op is a modeling
/// choice for the wiring increment; the projection result is the same either
/// way (per-entity LWW), so the substrate maps the simplest faithful shape.
#[must_use]
pub fn actions_to_ops(
    scope: ScopeId,
    author: PublicKey,
    hlc: HybridTimestamp,
    parents: &[[u8; 32]],
    actions: &[Action],
) -> Vec<Op> {
    actions
        .iter()
        .filter_map(|action| {
            payload_from_action(action)
                .map(|payload| build_op(scope, author, hlc, parents, payload))
        })
        .collect()
}

fn build_op(
    scope: ScopeId,
    author: PublicKey,
    hlc: HybridTimestamp,
    parents: &[[u8; 32]],
    payload: OpPayload,
) -> Op {
    let id = Op::compute_id(scope, parents, &author, &hlc, &payload);
    Op {
        id,
        scope,
        parents: parents.to_vec(),
        author,
        hlc,
        payload,
        expected_scope_root: [0u8; 32],
        signature: [0u8; 64],
    }
}

/// In-memory per-context registry of unified-op [`ScopeState`] projections.
///
/// Additive and unbounded for now: nothing populates it in production yet, so
/// growth is a non-issue until the apply path feeds it — at which point the
/// wiring increment adds eviction (gated like the other per-context caches) and
/// persistence. Kept deliberately small so the wiring is the only moving part.
#[derive(Default)]
pub struct ContextProjections {
    states: HashMap<ContextId, ScopeState>,
}

impl ContextProjections {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold `ops` into `context`'s projection (creating it if absent). Apply is
    /// per-slot last-writer-wins, so the order ops are ingested doesn't matter.
    pub fn ingest<'a>(&mut self, context: ContextId, ops: impl IntoIterator<Item = &'a Op>) {
        let state = self.states.entry(context).or_default();
        for op in ops {
            state.apply(op);
        }
    }

    /// `context`'s convergence root: the projection's ACL + governance folded
    /// onto the supplied storage Merkle `entities_root`. `None` if `context`
    /// has no projection yet.
    ///
    /// `entities_root` MUST be the storage layer's Merkle root, not the
    /// projection's own entity hash (see
    /// [`ScopeState::scope_root_with_entities`]).
    #[must_use]
    pub fn scope_root(&self, context: &ContextId, entities_root: [u8; 32]) -> Option<[u8; 32]> {
        self.states
            .get(context)
            .map(|state| state.scope_root_with_entities(entities_root))
    }

    /// Read-only access to a context's projection (for shadow comparison /
    /// authorization once the apply path feeds this).
    #[must_use]
    pub fn get(&self, context: &ContextId) -> Option<&ScopeState> {
        self.states.get(context)
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU128;

    use calimero_primitives::identity::PublicKey;
    use calimero_storage::address::Id;
    use calimero_storage::entities::{Metadata, OpMask};
    use calimero_storage::logical_clock::{Timestamp, ID, NTP64};

    use super::*;

    fn hlc(ns: u64) -> HybridTimestamp {
        HybridTimestamp::new(Timestamp::new(
            NTP64(ns),
            ID::from(NonZeroU128::new(1).unwrap()),
        ))
    }

    #[test]
    fn actions_convert_to_matching_ops() {
        let scope = ScopeId::from([0u8; 32]);
        let author = PublicKey::from([1u8; 32]);
        let e1 = Id::new([0xA1; 32]);
        let e2 = Id::new([0xA2; 32]);

        let actions = vec![
            Action::Add {
                id: e1,
                data: vec![1, 2, 3],
                ancestors: Vec::new(),
                metadata: Metadata::default(),
            },
            Action::DeleteRef {
                id: e2,
                deleted_at: 0,
                metadata: Metadata::default(),
            },
            // A Compare hint produces no op.
            Action::Compare { id: e1 },
        ];

        let ops = actions_to_ops(scope, author, hlc(10), &[], &actions);
        assert_eq!(ops.len(), 2, "Compare is dropped; Add + DeleteRef map");
        assert_eq!(
            ops[0].payload,
            OpPayload::Put {
                entity: e1,
                value: vec![1, 2, 3]
            }
        );
        assert_eq!(ops[1].payload, OpPayload::Delete { entity: e2 });
        // Distinct entities ⇒ distinct content-addressed ids under shared coords.
        assert_ne!(ops[0].id, ops[1].id);
        // ids are the canonical content address.
        assert_eq!(
            ops[0].id,
            Op::compute_id(scope, &[], &author, &hlc(10), &ops[0].payload)
        );
    }

    #[test]
    fn projection_registry_is_per_context_and_folds_acl_into_root() {
        let scope = ScopeId::from([0u8; 32]);
        let author = PublicKey::from([1u8; 32]);
        let ctx_a = ContextId::from([0xAA; 32]);
        let ctx_b = ContextId::from([0xBB; 32]);
        let storage_root = [0x42u8; 32];

        let put = actions_to_ops(
            scope,
            author,
            hlc(10),
            &[],
            &[Action::Add {
                id: Id::new([1u8; 32]),
                data: vec![9],
                ancestors: Vec::new(),
                metadata: Metadata::default(),
            }],
        );

        let mut reg = ContextProjections::new();
        reg.ingest(ctx_a, &put);

        // ctx_b has no projection yet.
        assert!(reg.scope_root(&ctx_b, storage_root).is_none());
        let root_a_before = reg.scope_root(&ctx_a, storage_root).expect("ctx_a present");

        // Ingest a writer-set rotation (ACL plane) — hash-neutral on entities,
        // but it must move ctx_a's scope_root.
        let rotation = Op {
            id: [0u8; 32],
            scope,
            parents: vec![],
            author,
            hlc: hlc(20),
            payload: OpPayload::SetWriters {
                object: Id::new([2u8; 32]),
                writers: [(author, OpMask::FULL)].into_iter().collect(),
            },
            expected_scope_root: [0u8; 32],
            signature: [0u8; 64],
        };
        reg.ingest(ctx_a, [&rotation]);
        let root_a_after = reg.scope_root(&ctx_a, storage_root).expect("ctx_a present");

        assert_ne!(
            root_a_before, root_a_after,
            "an ACL change moves scope_root at a fixed storage entities root"
        );
    }
}
