//! Fixtures for the bundle and acquisition tests, here rather than in `tests/`
//! so a dependent crate can share the one copy.
//!
//! Behind the `testing` feature outside this crate. A node over an in-memory
//! store and a signed `.mpk` on disk are what every one of those tests starts
//! from, and mirroring them per crate is how two copies drift apart.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_network_primitives::client::NetworkClient;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use camino::Utf8PathBuf;
use ed25519_dalek::SigningKey;
use flate2::write::GzEncoder;
use flate2::Compression;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use sha2::{Digest, Sha256};
use tar::Builder;
use tempfile::TempDir;

use crate::join_bundle::JoinBundle;
use tokio::sync::{broadcast, mpsc};

use crate::bundle::{derive_signer_id_did_key, sign_manifest_json, BundleArtifact, BundleManifest};
use crate::client::{BlobManager, NodeClient, SyncClient};

pub fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A node with its own store and blobstore. No peers: the network recipient is
/// unbound, so every peer fetch declines rather than hangs.
///
/// The store is handed back because the tests that drive a node from the outside
/// have to write the rows a replicated op would otherwise write.
pub async fn node_client() -> (NodeClient, Store, TempDir, TempDir) {
    let datastore = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, data_dir, blob_dir) =
        node_client_over(datastore.clone(), NetworkClient::new(LazyRecipient::new())).await;
    (node_client, datastore, data_dir, blob_dir)
}

/// [`node_client`], over a store and a network the caller supplies.
///
/// An unbound network recipient QUEUES rather than declines, so a test that
/// drives a path which subscribes or publishes has to stand an actor in front of
/// it or the call never returns.
pub async fn node_client_over(
    datastore: Store,
    network_client: NetworkClient,
) -> (NodeClient, TempDir, TempDir) {
    let data_dir = TempDir::new().unwrap();
    let blob_dir = TempDir::new().unwrap();

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
        network_client,
        LazyRecipient::new(),
        event_sender,
        sync_client,
        None,
    );
    (node_client, data_dir, blob_dir)
}

/// [`node_client_over`], with a responder that answers every namespace-join
/// request with `bundle`.
///
/// The plain fixture drops the join receiver, so `request_namespace_join`
/// fails and the handler takes its no-peer fallback. That fallback no longer
/// records a membership — a join has to carry an admitter's endorsement, and
/// only a peer can produce one — so a test that means to exercise what happens
/// *after* a join succeeds has to stand a responder up rather than relying on
/// the fallback to get there.
pub async fn node_client_over_answering_joins(
    datastore: Store,
    network_client: NetworkClient,
    bundle: JoinBundle,
) -> (NodeClient, TempDir, TempDir) {
    let data_dir = TempDir::new().unwrap();
    let blob_dir = TempDir::new().unwrap();

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
    let (ns_join_tx, mut ns_join_rx) = mpsc::channel(16);
    let (open_subgroup_join_tx, _) = mpsc::channel(16);
    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

    // Detached: the client awaits the oneshot, so the answer has to come from
    // somewhere other than the task making the request.
    let _responder = tokio::spawn(async move {
        while let Some((_params, reply)) = ns_join_rx.recv().await {
            let _ignored = reply.send(Ok(bundle.clone()));
        }
    });

    let node_client = NodeClient::new(
        datastore,
        blob_manager,
        network_client,
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

/// A signed single-wasm bundle on disk, the shape `cargo mero bundle` produces.
pub fn bundle(dir: &TempDir, package: &str, version: &str, wasm: &[u8]) -> Utf8PathBuf {
    let path = dir.path().join(format!("{package}-{version}.mpk"));
    let mut tar = Builder::new(GzEncoder::new(
        fs::File::create(&path).unwrap(),
        Compression::default(),
    ));

    let signing_key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let manifest = BundleManifest {
        version: "1.0".to_owned(),
        package: package.to_owned(),
        app_version: version.to_owned(),
        signer_id: Some(derive_signer_id_did_key(
            signing_key.verifying_key().as_bytes(),
        )),
        min_runtime_version: "0.1.0".to_owned(),
        metadata: None,
        handlers: None,
        interfaces: None,
        wasm: Some(BundleArtifact {
            path: "app.wasm".to_owned(),
            hash: hex_lower(&Sha256::digest(wasm)),
            size: wasm.len() as u64,
        }),
        abi: None,
        links: None,
        services: None,
        signature: None,
    };
    let mut manifest_json = serde_json::to_value(&manifest).unwrap();
    sign_manifest_json(&mut manifest_json, &signing_key).unwrap();

    for (name, bytes) in [
        ("manifest.json", serde_json::to_vec(&manifest_json).unwrap()),
        ("app.wasm", wasm.to_vec()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        tar.append(&header, bytes.as_slice()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();

    path.try_into().unwrap()
}
