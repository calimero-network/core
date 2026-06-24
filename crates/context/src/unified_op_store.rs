//! Persistence for the unified causal-log op-store (cutover plan C2.1).
//!
//! Writes each unified [`Op`] to its own keyspace — one borsh row per op, keyed
//! `[op.scope ‖ op.id]` in [`Column::UnifiedOp`](calimero_store::db::Column) — so
//! the op-stream that backs the projection is durable. During the dual-write
//! transition this lives **alongside** the legacy stores and is **observe-only**:
//! nothing reads it for a sync/auth decision yet. The per-plane flips (C2.2+)
//! switch reads onto it; this slice validates the op-log can be written, reloaded,
//! and replayed to reconstruct the projection.
//!
//! Heads are intentionally NOT persisted here: the op-log is self-describing
//! (every op carries its `parents`), so [`load_scope_ops`] returns the rows in any
//! order and the caller replays them through a `DagStore<Op>`, which re-establishes
//! causal order and derives the frontier (proven by the convergence test in
//! [`crate::unified_applier`]).

use borsh::BorshDeserialize;
use calimero_op::{Op, ScopeId};
use calimero_store::key::ScopeUnifiedOp as ScopeUnifiedOpKey;
use calimero_store::types::ScopeUnifiedOp as ScopeUnifiedOpValue;
use calimero_store::Store;
use eyre::Result as EyreResult;

/// Persist one unified [`Op`] under `[op.scope ‖ op.id]` — keyed by the op's
/// **scope**, so a scope's whole op-log is one contiguous key range (governance
/// ops are namespace-scoped and shared across that namespace's contexts, so a
/// single context id is the wrong partition). Idempotent — re-persisting the same
/// op id overwrites with identical bytes (the id is a content address).
pub fn persist_op(store: &Store, op: &Op) -> EyreResult<()> {
    let bytes = borsh::to_vec(op)?;
    let mut handle = store.handle();
    handle.put(
        &ScopeUnifiedOpKey::new(scope_bytes(&op.scope), op.id),
        &ScopeUnifiedOpValue::from(calimero_store::slice::Slice::from(bytes)),
    )?;
    Ok(())
}

/// Load every persisted unified [`Op`] for `scope`, in key (not causal) order. The
/// caller replays them through a `DagStore<Op>` to recover causal order; see the
/// module docs.
pub fn load_scope_ops(store: &Store, scope: &ScopeId) -> EyreResult<Vec<Op>> {
    let scope = scope_bytes(scope);
    let handle = store.handle();
    let mut iter = handle.iter::<ScopeUnifiedOpKey>()?;
    // Seek to the first row of this scope's `[scope ‖ *]` range.
    let start = ScopeUnifiedOpKey::new(scope, [0u8; 32]);
    let mut ops = Vec::new();

    // `Iter::entries()` yields rows STRICTLY AFTER the cursor (the contract the
    // whole codebase's seek-scan code relies on, e.g. the data-plane
    // `load_persisted_deltas`), so the seek's own row is read once via `handle.get`
    // and the loop below covers the rest — each row read exactly once, never
    // duplicated. The `get`-after-`seek` is not a torn read: unified-op rows are
    // content-addressed by op id and never rewritten or deleted in this store, so a
    // row's bytes can't change between the seek and the get.
    let first_key = iter.seek(start)?;
    if let Some(key) = first_key {
        if key.scope() != scope {
            return Ok(ops);
        }
        // `seek` just reported this key exists, so `get` returning `None` is a store
        // inconsistency — surface it as a clear error rather than a silent skip that
        // would later look like a confusing "parent not found" during DAG replay.
        let value = handle.get(&key)?.ok_or_else(|| {
            eyre::eyre!("unified op-store: seek found key {key:?} but its value is missing")
        })?;
        ops.push(Op::try_from_slice(value.as_ref())?);
    } else {
        return Ok(ops);
    }

    for (key, value) in iter.entries() {
        let key = key?;
        if key.scope() != scope {
            break;
        }
        let value = value?;
        ops.push(Op::try_from_slice(value.as_ref())?);
    }

    Ok(ops)
}

/// `ScopeId`'s 32-byte representation (the store key is byte-addressed and can't
/// depend on `calimero_op`).
fn scope_bytes(scope: &ScopeId) -> [u8; 32] {
    *scope.as_bytes()
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU128;

    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_dag::DagStore;
    use calimero_op::{Op, OpPayload, ScopeId};
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PublicKey;
    use calimero_storage::address::Id;
    use calimero_storage::entities::OpMask;
    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
    use calimero_store::db::InMemoryDB;

    use super::*;
    use crate::scope_projection::ScopeProjections;
    use crate::unified_applier::UnifiedApplier;

    const GENESIS: [u8; 32] = [0u8; 32];

    fn hlc(ns: u64) -> HybridTimestamp {
        HybridTimestamp::new(Timestamp::new(NTP64(ns), ID::from(NonZeroU128::MIN)))
    }

    fn op(scope: ScopeId, ns: u64, parents: Vec<[u8; 32]>, payload: OpPayload) -> Op {
        let author = PublicKey::from([7u8; 32]);
        let h = hlc(ns);
        let id = Op::compute_id(scope, &parents, &author, &h, &payload);
        // `expected_scope_root` and `signature` are zeroed: this is a
        // persistence/replay round-trip test, and neither the op-store nor the
        // projection fold verifies them (the projection folds already-verified ops
        // on content alone). If a signature or root check is ever added to that
        // path, this helper must produce valid values rather than zeros.
        Op {
            id,
            scope,
            parents,
            author,
            hlc: h,
            payload,
            expected_scope_root: [0u8; 32],
            signature: [0u8; 64],
        }
    }

    fn delta(op: &Op) -> calimero_dag::CausalDelta<Op> {
        calimero_dag::CausalDelta::new(op.id, op.parents.clone(), op.clone(), op.hlc, [0u8; 32])
    }

    /// Persist a causal chain of mixed-plane ops, reload it from a fresh handle,
    /// replay the loaded rows through `DagStore<Op>` + `UnifiedApplier`, and assert
    /// the reconstructed projection's `scope_root` matches an in-memory fold of the
    /// same ops — i.e. the durable op-log faithfully backs the projection.
    #[tokio::test]
    async fn persisted_op_log_reloads_and_replays_to_the_same_projection() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));

        let scope = ScopeId::from([0xA1; 32]);
        let group = ContextGroupId::from([3u8; 32]);
        let member = PublicKey::from([0x55; 32]);
        let admin = PublicKey::from([1u8; 32]);

        let op_admin = op(
            scope,
            10,
            vec![],
            OpPayload::AdminChanged { new_admin: admin },
        );
        let op_member = op(
            scope,
            20,
            vec![op_admin.id],
            OpPayload::MemberAdded {
                group,
                member,
                role: GroupMemberRole::Member,
            },
        );
        let op_writers = op(
            scope,
            30,
            vec![op_member.id],
            OpPayload::SetWriters {
                object: Id::new([9u8; 32]),
                writers: [(member, OpMask::FULL)].into_iter().collect(),
            },
        );
        let ops = [&op_admin, &op_member, &op_writers];

        // Persist (in causal order; key order on read is by op id, unrelated).
        for op in ops {
            persist_op(&store, op).expect("persist");
        }

        // Reference fold, in memory.
        let mut reference = ScopeProjections::new();
        for op in ops {
            reference.ingest_op(op);
        }
        let want = reference
            .scope_root_for(&scope, [0u8; 32])
            .expect("scope fed");

        // A scope with no rows loads empty.
        let other = ScopeId::from([0x22; 32]);
        assert!(load_scope_ops(&store, &other)
            .expect("load other")
            .is_empty());

        // Reload + replay through the DAG (which re-establishes causal order).
        let loaded = load_scope_ops(&store, &scope).expect("load");
        assert_eq!(loaded.len(), ops.len(), "every persisted op reloaded");

        let mut dag = DagStore::<Op>::new(GENESIS);
        let applier = UnifiedApplier::new();
        for op in &loaded {
            dag.add_delta(delta(op), &applier).await.expect("add_delta");
        }
        for op in ops {
            assert!(dag.is_applied(&op.id), "reloaded op left unapplied");
        }

        let got = applier
            .projection()
            .read()
            .expect("projection lock not poisoned in this single-threaded test")
            .scope_root_for(&scope, [0u8; 32])
            .expect("scope fed");
        assert_eq!(
            got, want,
            "reloaded+replayed op-log diverged from the in-memory fold"
        );
    }
}
