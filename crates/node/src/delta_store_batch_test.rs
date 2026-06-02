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

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_context_client::client::ContextClient;
use calimero_dag::{CausalDelta, DeltaKind};
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use tokio::sync::{broadcast, mpsc};

use crate::delta_store::{BatchDeltaInput, DeltaStore};

/// A parent id that is never supplied, so every delta referencing it stays
/// pending (and the applier — i.e. WASM — is never invoked).
const MISSING_PARENT: [u8; 32] = [0x99; 32];

fn make_delta(
    id: [u8; 32],
    parents: Vec<[u8; 32]>,
    expected_root_hash: [u8; 32],
) -> CausalDelta<Vec<Action>> {
    CausalDelta {
        id,
        parents,
        payload: Vec::new(),
        hlc: HybridTimestamp::default(),
        expected_root_hash,
        kind: DeltaKind::Regular,
    }
}

fn pending_input(id: [u8; 32], hash: [u8; 32]) -> BatchDeltaInput {
    BatchDeltaInput {
        delta: make_delta(id, vec![MISSING_PARENT], hash),
        events: None,
        author_id: Some(PublicKey::from([0xBB; 32])),
        governance_position_blob: None,
        delta_signature: None,
    }
}

/// Build a standalone `DeltaStore` backed by an in-memory store. Returns the
/// store plus the `TempDir` guard, which the caller must keep alive for the
/// blob filesystem to stay valid.
async fn build_delta_store() -> (DeltaStore, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = Store::new(Arc::new(InMemoryDB::owned()));

    let blob_config =
        BlobStoreConfig::new(tmp.path().to_path_buf().try_into().expect("utf8 blob path"));
    let file_system = FileSystem::new(&blob_config).await.expect("blob fs");
    let blob_store = BlobStore::new(store.clone(), file_system);
    let blob_manager = BlobManager::new(blob_store);

    let node_recipient = LazyRecipient::new();
    let context_recipient = LazyRecipient::new();
    let network_recipient = LazyRecipient::new();

    let network_client = NetworkClient::new(network_recipient);
    let (event_sender, _) = broadcast::channel(16);
    let (ctx_sync_tx, _ctx_sync_rx) = mpsc::channel(1);
    let (ns_sync_tx, _ns_sync_rx) = mpsc::channel(1);
    let (ns_join_tx, _ns_join_rx) = mpsc::channel(1);
    let (open_subgroup_join_tx, _open_subgroup_join_rx) = mpsc::channel(1);
    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

    let node_client = NodeClient::new(
        store.clone(),
        blob_manager,
        network_client,
        node_recipient,
        event_sender,
        sync_client,
        String::new(),
        None,
    );

    let context_client = ContextClient::new(store, node_client, context_recipient);

    let context_id = ContextId::from([0xAA; 32]);
    let our_identity = PublicKey::from([0xBB; 32]);
    let root = [0u8; 32];

    (
        DeltaStore::new(root, context_client, context_id, our_identity),
        tmp,
    )
}

#[tokio::test]
async fn add_deltas_batch_empty_is_noop() {
    let (delta_store, _tmp) = build_delta_store().await;

    let result = delta_store
        .add_deltas_batch(Vec::new())
        .await
        .expect("empty batch succeeds");

    assert!(result.applied.is_empty());
    assert!(result.pending.is_empty());
    assert!(result.forwarded_events.is_empty());
    assert!(
        delta_store.head_root_hash_ids().await.is_empty(),
        "empty batch must not touch head-root-hash tracking"
    );
}

#[tokio::test]
async fn add_deltas_batch_classifies_all_pending() {
    let (delta_store, _tmp) = build_delta_store().await;

    let ids = [[0x01u8; 32], [0x02u8; 32], [0x03u8; 32]];
    let inputs: Vec<BatchDeltaInput> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| pending_input(*id, [i as u8 + 1; 32]))
        .collect();

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
    assert!(
        delta_store.head_root_hash_ids().await.is_empty(),
        "no heads exist until something applies, so head-root-hashes prune to empty"
    );
}

/// One `add_deltas_batch` over all-pending inputs must leave the same observable
/// state as feeding the same deltas one-by-one through `add_delta`.
#[tokio::test]
async fn add_deltas_batch_matches_single_path_for_pending() {
    let ids = [[0x0Au8; 32], [0x0Bu8; 32], [0x0Cu8; 32], [0x0Du8; 32]];
    let hashes = [[0x10u8; 32], [0x20u8; 32], [0x30u8; 32], [0x40u8; 32]];

    // Store A: single-delta path, one call per delta.
    let (store_a, _tmp_a) = build_delta_store().await;
    for (id, hash) in ids.iter().zip(hashes.iter()) {
        let applied = store_a
            .add_delta(
                make_delta(*id, vec![MISSING_PARENT], *hash),
                Some(PublicKey::from([0xBB; 32])),
                None,
                None,
            )
            .await
            .expect("single add succeeds");
        assert!(!applied, "missing parent → pending");
    }

    // Store B: batch path, one call for all deltas.
    let (store_b, _tmp_b) = build_delta_store().await;
    let inputs: Vec<BatchDeltaInput> = ids
        .iter()
        .zip(hashes.iter())
        .map(|(id, hash)| pending_input(*id, *hash))
        .collect();
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

    let mut heads_a = store_a.head_root_hash_ids().await;
    let mut heads_b = store_b.head_root_hash_ids().await;
    heads_a.sort_unstable();
    heads_b.sort_unstable();
    assert_eq!(
        heads_a, heads_b,
        "head-root-hash tracking must match between paths"
    );
}
