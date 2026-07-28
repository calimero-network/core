//! Fixtures shared by the bundle integration tests.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use flate2::write::GzEncoder;
use flate2::Compression;
use tar::Builder;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

pub fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub async fn node_client() -> (NodeClient, TempDir, TempDir) {
    let data_dir = TempDir::new().unwrap();
    let blob_dir = TempDir::new().unwrap();
    let datastore = Store::new(Arc::new(InMemoryDB::owned()));

    // Nested one level down: the node derives its root as the blob root's
    // parent, so a bare TempDir would make every node share the OS temp dir.
    let blob_root = blob_dir.path().join("blobs");
    let blob_store = BlobStore::new(
        datastore.clone(),
        FileSystem::new(&BlobStoreConfig::new(blob_root.try_into().unwrap()))
            .await
            .unwrap(),
    );
    let blob_manager = BlobManager::new(blob_store);

    let (event_sender, _) = broadcast::channel(256);
    let (ctx_sync_tx, _) = mpsc::channel(64);
    let (ns_sync_tx, _) = mpsc::channel(64);
    let (ns_join_tx, _) = mpsc::channel(16);
    let (open_subgroup_join_tx, _) = mpsc::channel(16);
    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

    let node_client = NodeClient::new(
        datastore,
        blob_manager,
        NetworkClient::new(LazyRecipient::new()),
        LazyRecipient::new(),
        event_sender,
        sync_client,
        None,
    );
    (node_client, data_dir, blob_dir)
}

/// Pack a `.mpk` from an explicit list of tar entries, so a test can ship a
/// decoy or duplicate entry the way a hostile mirror would. Generic over the
/// path so a test can also ship one no `str` can hold.
pub fn pack_entries<P: AsRef<Path>>(dir: &TempDir, name: &str, entries: &[(P, &[u8])]) -> Vec<u8> {
    let path = dir.path().join(name);
    let encoder = GzEncoder::new(fs::File::create(&path).unwrap(), Compression::default());
    let mut tar = Builder::new(encoder);
    for (entry_path, content) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_path(entry_path).unwrap();
        header.set_size(content.len() as u64);
        header.set_cksum();
        tar.append(&header, *content).unwrap();
    }
    tar.finish().unwrap();
    drop(tar);
    fs::read(&path).unwrap()
}
