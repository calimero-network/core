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

/// Keeps a test `DeltaStore`'s client channel receivers alive.
///
/// The node/sync clients hold the *senders*; if the paired receivers were
/// dropped at the end of `delta_store_over` (which is what a `_`-binding does —
/// the `_` prefix silences the lint but does NOT extend the binding's lifetime),
/// every sender would see a closed channel and an emit/send would exercise a
/// degraded path. Returning them to the caller keeps them alive for the whole
/// test, not just for construction. Opaque so the (nameable-only-via-inference)
/// receiver types don't leak into the return signature; bind it (e.g. `_rx`).
pub(crate) struct KeepAlive(#[allow(dead_code)] Box<dyn std::any::Any>);

/// Build a `DeltaStore` over the supplied `store`, so a caller can pre-seed the
/// store and then "restart" by building a fresh `DeltaStore` over the same rows.
/// The returned `TempDir` guard must be kept alive for the blob filesystem, and
/// the [`KeepAlive`] for the client channel receivers.
pub(crate) async fn delta_store_over(store: Store) -> (DeltaStore, tempfile::TempDir, KeepAlive) {
    let tmp = tempfile::tempdir().expect("tempdir");

    let blob_config =
        BlobStoreConfig::new(tmp.path().to_path_buf().try_into().expect("utf8 blob path"));
    let file_system = FileSystem::new(&blob_config).await.expect("blob fs");
    let blob_store = BlobStore::new(store.clone(), file_system);
    let blob_manager = BlobManager::new(blob_store);

    let network_client = NetworkClient::new(LazyRecipient::new());

    let (event_sender, event_rx) = broadcast::channel(16);
    let (ctx_sync_tx, ctx_sync_rx) = mpsc::channel(1);
    let (ns_sync_tx, ns_sync_rx) = mpsc::channel(1);
    let (ns_join_tx, ns_join_rx) = mpsc::channel(1);
    let (open_subgroup_join_tx, open_subgroup_join_rx) = mpsc::channel(1);
    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

    // Handed back to the caller so the senders inside the clients keep live
    // receivers for the duration of the test (see `KeepAlive`).
    let keep_alive = KeepAlive(Box::new((
        event_rx,
        ctx_sync_rx,
        ns_sync_rx,
        ns_join_rx,
        open_subgroup_join_rx,
    )));

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
        keep_alive,
    )
}

/// Build a standalone `DeltaStore` over a fresh in-memory store.
pub(crate) async fn build_delta_store() -> (DeltaStore, tempfile::TempDir, KeepAlive) {
    delta_store_over(Store::new(Arc::new(InMemoryDB::owned()))).await
}
