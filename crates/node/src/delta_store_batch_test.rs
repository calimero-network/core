//! Unit coverage for `DeltaStore::add_deltas_batch`.
//!
//! The *applied* path runs WASM through `ContextStorageApplier`, which needs a
//! real installed application/context — out of reach for an in-crate unit test
//! (and exercised by the e2e sync-catchup suites instead). The *pending* path,
//! however, never invokes the applier (a delta with a missing parent is stored
//! pending without applying), so it is fully unit-testable here. These tests
//! pin the batch orchestration that is reachable without WASM: empty handling,
//! pending classification, the no-apply persist gate, and behavioural
//! equivalence to a loop of single `add_delta` calls.

use std::sync::Arc;

use calimero_dag::{CausalDelta, DeltaKind};
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::db::InMemoryDB;
use calimero_store::key::ContextDagDelta;
use calimero_store::Store;

use crate::delta_store::BatchDeltaInput;
use crate::test_support::{build_delta_store, context, delta_store_over};

/// A parent id that is never supplied, so every delta referencing it stays
/// pending (and the applier — i.e. WASM — is never invoked).
const MISSING_PARENT: [u8; 32] = [0x99; 32];

fn make_delta(id: [u8; 32], parents: Vec<[u8; 32]>) -> CausalDelta<Vec<Action>> {
    CausalDelta {
        id,
        parents,
        payload: Vec::new(),
        hlc: HybridTimestamp::default(),
        kind: DeltaKind::Regular,
    }
}

fn pending_input(id: [u8; 32]) -> BatchDeltaInput {
    BatchDeltaInput {
        delta: make_delta(id, vec![MISSING_PARENT]),
        events: None,
        author_id: Some(PublicKey::from([0xBB; 32])),
        governance_position_blob: None,
        delta_signature: None,
        delegation: None,
    }
}

#[tokio::test]
async fn add_deltas_batch_empty_is_noop() {
    let (delta_store, _tmp, _rx) = build_delta_store().await;

    let result = delta_store
        .add_deltas_batch(Vec::new())
        .await
        .expect("empty batch succeeds");

    assert!(result.applied.is_empty());
    assert!(result.pending.is_empty());
    assert!(result.forwarded_events.is_empty());
}

#[tokio::test]
async fn add_deltas_batch_classifies_all_pending() {
    let (delta_store, _tmp, _rx) = build_delta_store().await;

    let ids = [[0x01u8; 32], [0x02u8; 32], [0x03u8; 32]];
    let inputs: Vec<BatchDeltaInput> = ids.iter().map(|id| pending_input(*id)).collect();

    let result = delta_store
        .add_deltas_batch(inputs)
        .await
        .expect("pending batch succeeds");

    assert!(
        result.applied.is_empty(),
        "nothing applies while the shared parent is missing"
    );
    assert_eq!(result.pending.len(), ids.len(), "every input is pending");
    assert!(result.forwarded_events.is_empty());

    for id in &ids {
        assert!(delta_store.has_delta(id).await, "pending delta is tracked");
    }
    let stats = delta_store.pending_stats().await;
    assert_eq!(stats.count, ids.len());
}

/// One `add_deltas_batch` over all-pending inputs must leave the same observable
/// state as feeding the same deltas one-by-one through `add_delta`.
#[tokio::test]
async fn add_deltas_batch_matches_single_path_for_pending() {
    let ids = [[0x0Au8; 32], [0x0Bu8; 32], [0x0Cu8; 32], [0x0Du8; 32]];
    // Store A: single-delta path, one call per delta.
    let (store_a, _tmp_a, _rx_a) = build_delta_store().await;
    for id in ids.iter() {
        let applied = store_a
            .add_delta(
                make_delta(*id, vec![MISSING_PARENT]),
                Some(PublicKey::from([0xBB; 32])),
                None,
                None,
                None,
            )
            .await
            .expect("single add succeeds");
        assert!(!applied, "missing parent → pending");
    }

    // Store B: batch path, one call for all deltas.
    let (store_b, _tmp_b, _rx_b) = build_delta_store().await;
    let inputs: Vec<BatchDeltaInput> = ids.iter().map(|id| pending_input(*id)).collect();
    let result = store_b
        .add_deltas_batch(inputs)
        .await
        .expect("batch add succeeds");
    assert_eq!(result.pending.len(), ids.len());
    assert!(result.applied.is_empty());

    // Both stores converge on identical observable state.
    for id in &ids {
        assert_eq!(
            store_a.has_delta(id).await,
            store_b.has_delta(id).await,
            "delta presence must match between paths"
        );
        assert!(store_b.has_delta(id).await);
    }
    let stats_a = store_a.pending_stats().await;
    let stats_b = store_b.pending_stats().await;
    assert_eq!(stats_a.count, stats_b.count, "pending counts must match");
    assert_eq!(
        stats_a.total_missing_parents, stats_b.total_missing_parents,
        "missing-parent accounting must match"
    );
}

/// A delta naming more parents than a delta may have is refused on the head-pull,
/// parent-pull and gossip paths, leaves no row behind, and an ordinary pending
/// delta stays.
#[tokio::test]
async fn every_ingest_path_refuses_a_delta_naming_too_many_parents() {
    let flood_parents: Vec<[u8; 32]> = (0..=calimero_dag::MAX_DELTA_PARENTS)
        .map(|i| {
            let mut id = [0xAA; 32];
            id[..8].copy_from_slice(&(i as u64).to_be_bytes());
            id
        })
        .collect();
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (delta_store, _tmp, _rx) = delta_store_over(store.clone()).await;
    let author = Some(PublicKey::from([0xBB; 32]));

    let ordinary = [0x01u8; 32];
    let _ = delta_store
        .add_delta(
            make_delta(ordinary, vec![MISSING_PARENT]),
            author,
            None,
            None,
            None,
        )
        .await
        .expect("ordinary pending delta is accepted");

    let head_pull = [0x02u8; 32];
    let refused = delta_store
        .add_delta(
            make_delta(head_pull, flood_parents.clone()),
            author,
            None,
            None,
            None,
        )
        .await;
    assert!(refused.is_err(), "single path reports the refusal");

    let parent_pull = [0x03u8; 32];
    let _ = delta_store
        .add_deltas_batch(vec![BatchDeltaInput {
            delta: make_delta(parent_pull, flood_parents.clone()),
            ..pending_input(parent_pull)
        }])
        .await
        .expect("the rest of a batch is unaffected");

    let gossip = [0x04u8; 32];
    let refused = delta_store
        .add_delta_with_events(
            make_delta(gossip, flood_parents),
            Some(b"events".to_vec()),
            author,
            None,
            None,
            None,
        )
        .await;
    assert!(refused.is_err(), "events path reports the refusal");

    assert!(
        delta_store.has_delta(&ordinary).await,
        "honest pending delta kept"
    );
    for id in [head_pull, parent_pull, gossip] {
        assert!(
            !delta_store.has_delta(&id).await,
            "refused delta is not held"
        );
    }
    let row = store
        .handle()
        .get(&ContextDagDelta::new(context(), gossip))
        .expect("read row");
    assert!(row.is_none(), "a refused delta leaves no row on disk");
}
