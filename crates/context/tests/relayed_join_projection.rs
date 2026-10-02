//! A join an admitter relayed must fold as the joiner's membership.
//!
//! An account with no node joins by handing its signed join to an admitter,
//! which publishes it sealed inside an envelope of its own
//! (`NamespaceOp::RootRelaySealed`). The live apply opens the envelope and
//! admits the joiner, and binds the joiner's device to its account. Every peer
//! then authorizes that account's state deltas against the projection at the
//! delta's governance cut, so the projection has to see the same join.
//!
//! When the projection reads the envelope instead, it folds a `Noop` signed by
//! the admitter: the cut is complete, the device names no account in it, and
//! the peer refuses every delta the account writes as "not a member at the
//! governance cut" while its own rows list the account as a member.

use std::sync::Arc;

use calimero_context::scope_projection::ScopeProjections;
use calimero_op::OpPayload;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;

fn store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

/// The projection a peer builds by walking the governance DAG must admit the
/// relayed joiner's device at the cut the join produced.
#[test]
fn a_walked_projection_admits_a_relayed_joiner() {
    let store = store();
    let join = calimero_context::test_support::relayed_join(&store);
    let namespace = join.namespace.to_bytes();

    let ops = ScopeProjections::collect_namespace_ops(&store, namespace)
        .expect("the namespace head is readable");
    let folded = ops
        .iter()
        .find(|op| op.id() == join.envelope_id)
        .expect("the walk reaches the relayed join");
    assert!(
        matches!(folded.payload, OpPayload::MemberJoinedWithDevice { member, .. } if member == join.account),
        "a relayed join must fold as the joiner's membership, not as the admitter's \
         envelope; folded {:?}",
        folded.payload,
    );

    let mut projection = ScopeProjections::new();
    projection.apply_backfill(namespace, ops);
    assert_eq!(
        projection.device_account_at_cut(
            &store,
            join.namespace,
            &join.device_key,
            &[join.envelope_id]
        ),
        Some(join.account),
        "the joiner's device must speak for its account at the join's cut",
    );
    assert_eq!(
        projection.member_at_cut(
            &store,
            join.namespace,
            &join.device_key,
            &[join.envelope_id]
        ),
        Some(true),
        "a delta the joiner's device signs after the join must be authorized",
    );
}

/// Re-persisting the op-store after a key arrives must not overwrite a relayed
/// join's membership with the envelope's `Noop`.
///
/// The op-store row the apply writes is the one a cold backfill folds, and
/// `repersist_namespace_ops` rewrites every row from a fresh walk — so a walk
/// that reads the envelope would turn a correct row into a wrong one the next
/// time any key is delivered.
#[test]
fn a_repersisted_op_store_still_admits_a_relayed_joiner() {
    let store = store();
    let join = calimero_context::test_support::relayed_join(&store);
    let namespace = join.namespace.to_bytes();
    let member_from_op_store = || {
        let ops = ScopeProjections::ops_for_namespace(&store, namespace)
            .expect("the op-store holds the namespace");
        let mut projection = ScopeProjections::new();
        projection.apply_backfill(namespace, ops);
        projection.member_at_cut(
            &store,
            join.namespace,
            &join.device_key,
            &[join.envelope_id],
        )
    };

    assert_eq!(
        member_from_op_store(),
        Some(true),
        "the row the apply wrote admits the relayed joiner",
    );

    ScopeProjections::repersist_namespace_ops(&store, namespace);

    assert_eq!(
        member_from_op_store(),
        Some(true),
        "the op-store must keep the relayed joiner a member after a re-persist",
    );
}
