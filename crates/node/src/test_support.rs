//! Shared `#[cfg(test)]` scaffolding for the node crate's `DeltaStore` tests.
//!
//! `delta_store_over` / `build_delta_store` construct a real `DeltaStore` over an
//! in-memory store with fully-wired (but inert) node/sync/context clients.
//! Several test modules (`crash_recovery_test`, `delta_store_batch_test`, and the
//! in-file `apply_lock_poison_recovery_tests`) previously each carried a
//! near-identical ~35-line copy of this boilerplate; this is the single source of
//! truth so those copies can't silently diverge.

use std::sync::Arc;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_context_client::client::ContextClient;
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use tokio::sync::{broadcast, mpsc};

use crate::delta_store::DeltaStore;

/// Root / genesis hash every test `DeltaStore` is seeded with.
pub(crate) const GENESIS: [u8; 32] = [0u8; 32];

/// The single context id every `DeltaStore` test operates in.
pub(crate) fn context() -> ContextId {
    ContextId::from([0xAA; 32])
}

/// Build a `DeltaStore` over the supplied `store`, so a caller can pre-seed the
/// store and then "restart" by building a fresh `DeltaStore` over the same rows.
/// The returned `TempDir` guard must be kept alive for the blob filesystem.
pub(crate) async fn delta_store_over(store: Store) -> (DeltaStore, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");

    let blob_config =
        BlobStoreConfig::new(tmp.path().to_path_buf().try_into().expect("utf8 blob path"));
    let file_system = FileSystem::new(&blob_config).await.expect("blob fs");
    let blob_store = BlobStore::new(store.clone(), file_system);
    let blob_manager = BlobManager::new(blob_store);

    let network_client = NetworkClient::new(LazyRecipient::new());

    // Keep every channel receiver bound for the life of this helper. Binding to a
    // discard pattern (`_`) would drop the receiver at the `let`, leaving the
    // paired sender with no receivers — so an emit/send would observe a closed
    // channel and the constructed clients would exercise a degraded path. These
    // receivers can't be threaded back through the `(DeltaStore, TempDir)` return
    // shape, so keeping them alive to the end of construction is the documented
    // minimum.
    let (event_sender, _event_rx) = broadcast::channel(16);
    let (ctx_sync_tx, _ctx_sync_rx) = mpsc::channel(1);
    let (ns_sync_tx, _ns_sync_rx) = mpsc::channel(1);
    let (ns_join_tx, _ns_join_rx) = mpsc::channel(1);
    let (open_subgroup_join_tx, _open_subgroup_join_rx) = mpsc::channel(1);
    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

    let node_client = NodeClient::new(
        store.clone(),
        blob_manager,
        network_client,
        LazyRecipient::new(),
        event_sender,
        sync_client,
        String::new(),
        None,
    );

    let context_client = ContextClient::new(store, node_client, LazyRecipient::new());
    let our_identity = PublicKey::from([0xBB; 32]);

    (
        DeltaStore::new(GENESIS, context_client, context(), our_identity),
        tmp,
    )
}

/// Build a standalone `DeltaStore` over a fresh in-memory store.
pub(crate) async fn build_delta_store() -> (DeltaStore, tempfile::TempDir) {
    delta_store_over(Store::new(Arc::new(InMemoryDB::owned()))).await
}
