//! `DeltaStore::refuse_delta` (core#4070): a refused delta is never applied
//! here, and the deltas built on it stop waiting for it.
//!
//! The release itself applies the waiting delta, which runs WASM through
//! `ContextStorageApplier` and so needs a real installed application — out of
//! reach in-crate (`calimero-dag`'s own tests cover the cascade over a test
//! applier). What this pins is the half a sync loop feels: a refused parent is
//! no longer requested from peers.

use calimero_dag::{CausalDelta, DeltaKind};
use calimero_storage::action::Action;
use calimero_storage::logical_clock::HybridTimestamp;

use crate::test_support::build_delta_store;

const REFUSED: [u8; 32] = [0x71; 32];
/// A second parent never supplied, so the child stays pending after the
/// refusal and the applier is never reached.
const OTHER_MISSING: [u8; 32] = [0x72; 32];
const CHILD: [u8; 32] = [0x73; 32];

fn delta(id: [u8; 32], parents: Vec<[u8; 32]>) -> CausalDelta<Vec<Action>> {
    CausalDelta {
        id,
        parents,
        payload: Vec::new(),
        hlc: HybridTimestamp::default(),
        kind: DeltaKind::Regular,
    }
}

#[tokio::test]
async fn a_refused_parent_is_no_longer_requested() {
    let (delta_store, _tmp, _rx) = build_delta_store().await;

    let applied = delta_store
        .add_delta(
            delta(CHILD, vec![REFUSED, OTHER_MISSING]),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("pending add succeeds");
    assert!(!applied, "precondition: both parents are missing");
    let mut missing = delta_store.get_missing_parents().await.missing_ids;
    missing.sort();
    assert_eq!(missing, vec![REFUSED, OTHER_MISSING]);

    let released = delta_store.refuse_delta(REFUSED).await;

    assert!(
        released.is_empty(),
        "the child still waits on its other parent"
    );
    assert_eq!(
        delta_store.get_missing_parents().await.missing_ids,
        vec![OTHER_MISSING],
        "a refused parent must not be requested again: this node would only refuse it"
    );
}
