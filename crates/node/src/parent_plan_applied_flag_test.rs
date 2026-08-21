//! What `ParentPlan::Add` may and may not persist when `add_delta` returns
//! `Ok(false)`.
//!
//! `DagStore::add_delta` collapses two outcomes into `Ok(false)`: the delta went
//! **pending** because its own ancestors are missing, or it was a **duplicate**
//! already in the DAG. The `ParentPlan::Add` arm cannot tell them apart, and the
//! comment there explains why it therefore persists nothing — writing
//! `applied: true` for a pending delta would send `load_persisted_deltas` down
//! the `restore_applied_delta` path on restart, which skips WASM, so the delta's
//! actions would never run on this node. That is a silent state divergence.
//!
//! Nothing tested that. These pin both halves of the reasoning:
//!
//! 1. a delta that goes pending keeps `applied: false` — the direction the
//!    comment calls catastrophic to get wrong; and
//! 2. it is nonetheless still reported as missing, so sync fetches its ancestor
//!    rather than the delta being silently dropped on the floor.
//!
//! Why the duplicate half is not here: reaching it requires the cascade to apply
//! a delta, which runs WASM through `ContextStorageApplier` and so needs a real
//! installed application — out of reach in-crate, and covered by the e2e
//! sync-catchup suites. That half is safe for a different reason, one worth
//! naming because it lives ~60 lines from the code relying on it: a delta the
//! cascade applies *leaves the pending map*, so phase 3's
//! `pending_before.difference(&pending_after)` picks it up and persists
//! `applied: true` there. The `Ok(false)` arm does not need to.

use calimero_dag::{CausalDelta, DeltaKind};
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::entities::Metadata;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use std::sync::Arc;

use crate::test_support::{context, delta_store_over};

/// An ancestor id never supplied to the store, so anything naming it stays
/// pending and the applier — i.e. WASM — is never invoked.
const ABSENT_ANCESTOR: [u8; 32] = [0x99; 32];

fn one_action(delta_id: [u8; 32]) -> Vec<Action> {
    vec![Action::Add {
        id: Id::new(delta_id),
        data: delta_id[..3].to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    }]
}

/// Persist a `ContextDagDelta` row the way the pre-apply persistence path does:
/// present, not yet applied.
fn persist_unapplied_row(store: &Store, delta_id: [u8; 32], parents: Vec<[u8; 32]>) {
    let mut handle = store.handle();
    let actions = borsh::to_vec(&one_action(delta_id)).expect("serialize actions");
    handle
        .put(
            &calimero_store::key::ContextDagDelta::new(context(), delta_id),
            &calimero_store::types::ContextDagDelta {
                delta_id,
                parents,
                actions,
                hlc: HybridTimestamp::default(),
                applied: false,
                checkpoint_root_hash: None,
                events: None,
                author_id: Some(PublicKey::from([0xBB; 32])),
                governance_position_blob: None,
                delta_signature: None,
            },
        )
        .expect("persist ContextDagDelta row");
}

fn row_is_applied(store: &Store, delta_id: [u8; 32]) -> bool {
    store
        .handle()
        .get(&calimero_store::key::ContextDagDelta::new(
            context(),
            delta_id,
        ))
        .expect("read persisted row")
        .is_some_and(|row| row.applied)
}

fn delta(id: [u8; 32], parents: Vec<[u8; 32]>) -> CausalDelta<Vec<Action>> {
    CausalDelta {
        id,
        parents,
        payload: one_action(id),
        hlc: HybridTimestamp::default(),
        kind: DeltaKind::Regular,
    }
}

/// **A parent that goes pending must NOT be persisted as applied.**
///
/// Shape: the DAG holds `child`, pending on `parent`. `parent` is persisted with
/// `applied: false` and itself names `ABSENT_ANCESTOR`, which nothing supplies.
/// So `get_missing_parents` classifies `parent` as `ParentPlan::Add`, `add_delta`
/// stores it pending (no WASM), and the arm sees `Ok(false)`.
///
/// If that arm ever persisted `applied: true` here, restart would take
/// `restore_applied_delta` for `parent` and its actions would never run.
#[tokio::test]
async fn a_parent_that_goes_pending_keeps_its_unapplied_row() {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (delta_store, _tmp, _keep) = delta_store_over(store.clone()).await;

    let parent_id = [0x11; 32];
    let child_id = [0x22; 32];

    // `parent` is on disk but unapplied, and is itself blocked on an ancestor
    // this node has never seen.
    persist_unapplied_row(&store, parent_id, vec![ABSENT_ANCESTOR]);

    // The DAG wants `parent`, so it shows up as potentially-missing.
    let applied = delta_store
        .add_delta(delta(child_id, vec![parent_id]), None, None, None)
        .await
        .expect("child is accepted");
    assert!(
        !applied,
        "the child must be pending on the parent, or this test proves nothing"
    );

    assert!(
        !row_is_applied(&store, parent_id),
        "precondition: the parent row starts unapplied"
    );

    let result = delta_store.get_missing_parents().await;

    assert!(
        !row_is_applied(&store, parent_id),
        "a parent that went PENDING must keep `applied: false` — persisting true \
         here sends restart down `restore_applied_delta`, which skips WASM, and \
         the delta's actions never run on this node"
    );
    assert!(
        !result.cascaded_ids.contains(&parent_id),
        "a pending parent is not a newly-applied delta and must not be reported \
         as cascaded — `cascaded_events` is documented as a subset of this"
    );
}

/// The other half: refusing to persist it must not mean losing it. The parent's
/// own absent ancestor has to be reported so sync fetches it, otherwise the
/// delta sits pending forever with nothing driving it.
#[tokio::test]
async fn the_absent_ancestor_behind_a_pending_parent_is_still_reported_missing() {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (delta_store, _tmp, _keep) = delta_store_over(store.clone()).await;

    let parent_id = [0x33; 32];
    let child_id = [0x44; 32];

    persist_unapplied_row(&store, parent_id, vec![ABSENT_ANCESTOR]);
    let _ = delta_store
        .add_delta(delta(child_id, vec![parent_id]), None, None, None)
        .await
        .expect("child is accepted");

    // First pass loads `parent` from the DB into the DAG as pending.
    let _ = delta_store.get_missing_parents().await;
    // Second pass sees the ancestor `parent` now names.
    let again = delta_store.get_missing_parents().await;

    assert!(
        again.missing_ids.contains(&ABSENT_ANCESTOR),
        "the ancestor behind the pending parent must be requested from the \
         network; missing_ids was {:?}",
        again.missing_ids
    );
}
