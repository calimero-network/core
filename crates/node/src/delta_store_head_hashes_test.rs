//! Regression: `add_local_applied_delta` must prune non-head ancestors
//! from `head_root_hashes`, matching `add_delta_internal`'s `retain(...)`
//! trimming at the end of its own flow.

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

use crate::delta_store::DeltaStore;

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

#[tokio::test]
async fn add_local_applied_delta_prunes_non_head_ancestors() {
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
    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx);

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

    let delta_store = DeltaStore::new(root, context_client, context_id, our_identity);

    // Register a "parent" delta (genesis as its parent).
    let parent_id = [0x11u8; 32];
    let parent_hash = [0xA1; 32];
    delta_store
        .add_local_applied_delta(make_delta(parent_id, vec![[0u8; 32]], parent_hash))
        .await
        .expect("add parent");

    let ids = delta_store.head_root_hash_ids().await;
    assert_eq!(
        ids,
        vec![parent_id],
        "parent is the sole head after first add"
    );

    // Register a child of the parent. Parent is no longer a head and
    // should be pruned from `head_root_hashes` so stale lookups don't
    // return its root hash.
    let child_id = [0x22u8; 32];
    let child_hash = [0xC1; 32];
    delta_store
        .add_local_applied_delta(make_delta(child_id, vec![parent_id], child_hash))
        .await
        .expect("add child");

    let ids = delta_store.head_root_hash_ids().await;
    assert_eq!(
        ids,
        vec![child_id],
        "parent entry must be pruned once it is no longer a head"
    );
}
