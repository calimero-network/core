//! Fixtures shared by the bundle integration tests.
//!
//! Each `tests/*.rs` file is its own crate, so a helper only some of them
//! call via `mod common;` still reads as dead code to the others.
#![allow(dead_code)]

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use actix::{Actor, Context, Handler};
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::messages::NetworkMessage;
use calimero_node_primitives::bundle::{
    derive_signer_id_did_key, sign_manifest_json, BundleArtifact, BundleManifest, BundleService,
};
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use camino::Utf8PathBuf;
use ed25519_dalek::SigningKey;
use flate2::write::GzEncoder;
use flate2::Compression;
use libp2p::PeerId;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use tar::Builder;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use url::Url;

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

/// Any syntactically valid peer id; the fakes below are never dialled.
pub const FAKE_PEER: &str = "12D3KooWR5V4zmisVtVdGE6i8jfFwtgRNq5t8eDGxfckKuhXu7Eh";

/// How a fake peer answers the two blob messages, one variant per arm of the
/// peer leg so a test can drive each without a network.
pub enum PeerBehavior {
    Serves(Vec<u8>),
    NoProviders,
    QueryFails,
}

pub struct FakePeer {
    pub peer_id: PeerId,
    pub behavior: PeerBehavior,
    /// Every blob query this peer was asked, so a route that must not run can
    /// be pinned at zero rather than inferred from its result.
    pub queries: Arc<AtomicUsize>,
}

impl Actor for FakePeer {
    type Context = Context<Self>;
}

impl Handler<NetworkMessage> for FakePeer {
    type Result = ();

    fn handle(&mut self, msg: NetworkMessage, _ctx: &mut Context<Self>) -> Self::Result {
        match msg {
            NetworkMessage::QueryBlob { outcome, .. } => {
                let _previous = self.queries.fetch_add(1, Ordering::SeqCst);
                let answer = match &self.behavior {
                    PeerBehavior::Serves(_) => Ok(vec![self.peer_id]),
                    PeerBehavior::NoProviders => Ok(vec![]),
                    PeerBehavior::QueryFails => Err(eyre::eyre!("dht query failed")),
                };
                let _ignored = outcome.send(answer);
            }
            NetworkMessage::RequestBlob { outcome, .. } => {
                let answer = match &self.behavior {
                    PeerBehavior::Serves(bytes) => Some(bytes.clone()),
                    PeerBehavior::NoProviders | PeerBehavior::QueryFails => None,
                };
                let _ignored = outcome.send(Ok(answer));
            }
            _ => {}
        }
    }
}

/// A `NetworkClient` backed by one fake peer. Keep the returned address alive
/// for the client to stay answerable.
pub fn fake_peer_network(behavior: PeerBehavior) -> (NetworkClient, actix::Addr<FakePeer>) {
    let (network, addr, _queries) = counting_peer_network(behavior);
    (network, addr)
}

/// As [`fake_peer_network`], plus the counter of blob queries the peer saw.
pub fn counting_peer_network(
    behavior: PeerBehavior,
) -> (NetworkClient, actix::Addr<FakePeer>, Arc<AtomicUsize>) {
    let peer_id = FAKE_PEER.parse::<PeerId>().expect("peer id");
    let queries = Arc::new(AtomicUsize::new(0));
    let recipient = LazyRecipient::new();
    let addr = Actor::create({
        let (recipient, queries) = (recipient.clone(), Arc::clone(&queries));
        move |ctx| {
            assert!(recipient.init(ctx));
            FakePeer {
                peer_id,
                behavior,
                queries,
            }
        }
    });
    (NetworkClient::new(recipient), addr, queries)
}

/// Answer exactly one request on a loopback port with `response` verbatim.
async fn respond_once(response: Vec<u8>) -> (Url, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let handle = tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut scratch = [0_u8; 1024];
            let _ignored = sock.read(&mut scratch).await;
            let _ignored = sock.write_all(&response).await;
            let _ignored = sock.flush().await;
        }
    });
    let url = format!("http://{addr}/com.example.app/1.0.0.mpk")
        .parse()
        .expect("valid url");
    (url, handle)
}

/// Serve `body` on a loopback port for one request; loopback proves no host
/// guard applies to the operator's own configured base.
pub async fn serve_once(body: Vec<u8>) -> (Url, JoinHandle<()>) {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(&body);
    respond_once(response).await
}

/// Answer one request with `status` and no body, so a test can drive the
/// registry leg's failure arm.
pub async fn serve_status_once(status: &str) -> (Url, JoinHandle<()>) {
    respond_once(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n").into_bytes()).await
}

/// Redirect one request to `target`, the way a registry hands an artifact off
/// to its object store.
pub async fn redirect_once(target: &Url) -> (Url, JoinHandle<()>) {
    respond_once(
        format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\n\r\n")
            .into_bytes(),
    )
    .await
}

/// The blob id `bytes` gets once stored - a hash over chunk ids, not content,
/// so it cannot be computed by hashing `bytes` directly.
pub async fn blob_id_of(bytes: &[u8]) -> BlobId {
    let (node_client, _data, _blobs) = create_test_node_client(None).await;
    let (blob_id, _size) = node_client
        .add_blob(bytes, Some(bytes.len() as u64), None)
        .await
        .expect("store bytes");
    blob_id
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

/// Create a test NodeClient with temporary directories.
///
/// `datastore` lets a caller inject a custom Store; `None` defaults to
/// `InMemoryDB` (no file I/O, faster tests).
pub async fn create_test_node_client(datastore: Option<Store>) -> (NodeClient, TempDir, TempDir) {
    create_test_node_client_with(datastore, NetworkClient::new(LazyRecipient::new())).await
}

/// As [`create_test_node_client`], but with a caller-supplied `NetworkClient`
/// so a test can stand in a fake peer for the DHT fetch path.
pub async fn create_test_node_client_with(
    datastore: Option<Store>,
    network_client: NetworkClient,
) -> (NodeClient, TempDir, TempDir) {
    let data_dir = TempDir::new().unwrap();
    let blob_dir = TempDir::new().unwrap();

    let datastore = datastore.unwrap_or_else(|| Store::new(Arc::new(InMemoryDB::owned())));

    // Nest the blobstore one level down: the node derives its root as the blob
    // root's parent, so a bare TempDir would make every node share the OS temp
    // dir as its root.
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

/// Create a test bundle archive with manifest.json, app.wasm, abi.json, and migrations.
pub fn create_test_bundle(
    temp_dir: &TempDir,
    package: &str,
    version: &str,
    wasm_content: &[u8],
    abi_content: Option<&[u8]>,
    migrations: Vec<(&str, &[u8])>,
) -> Utf8PathBuf {
    let bundle_path = temp_dir.path().join(format!("{package}-{version}.mpk"));
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    let signing_key = SigningKey::generate(&mut OsRng);
    let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

    let manifest = BundleManifest {
        version: "1.0".to_string(),
        package: package.to_string(),
        app_version: version.to_string(),
        signer_id: Some(signer_id),
        min_runtime_version: "0.1.0".to_string(),
        metadata: None,
        handlers: None,
        interfaces: None,
        wasm: Some(BundleArtifact {
            path: "app.wasm".to_string(),
            hash: hex_lower(&Sha256::digest(wasm_content)),
            size: wasm_content.len() as u64,
        }),
        abi: abi_content.map(|content| BundleArtifact {
            path: "abi.json".to_string(),
            hash: hex_lower(&Sha256::digest(content)),
            size: content.len() as u64,
        }),
        links: None,
        services: None,
        signature: None,
    };

    let mut manifest_json: serde_json::Value = serde_json::to_value(&manifest).unwrap();
    sign_manifest_json(&mut manifest_json, &signing_key).unwrap();

    let manifest_bytes = serde_json::to_vec(&manifest_json).unwrap();
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_path("manifest.json").unwrap();
    manifest_header.set_size(manifest_bytes.len() as u64);
    manifest_header.set_cksum();
    tar.append(&manifest_header, manifest_bytes.as_slice())
        .unwrap();

    let mut wasm_header = tar::Header::new_gnu();
    wasm_header.set_path("app.wasm").unwrap();
    wasm_header.set_size(wasm_content.len() as u64);
    wasm_header.set_cksum();
    tar.append(&wasm_header, wasm_content).unwrap();

    if let Some(abi_content) = abi_content {
        let mut abi_header = tar::Header::new_gnu();
        abi_header.set_path("abi.json").unwrap();
        abi_header.set_size(abi_content.len() as u64);
        abi_header.set_cksum();
        tar.append(&abi_header, abi_content).unwrap();
    }

    for (path, content) in migrations {
        let mut migration_header = tar::Header::new_gnu();
        migration_header.set_path(path).unwrap();
        migration_header.set_size(content.len() as u64);
        migration_header.set_cksum();
        tar.append(&migration_header, content).unwrap();
    }

    tar.finish().unwrap();
    bundle_path.try_into().unwrap()
}

/// A minimal signed `.mpk` plus the `ApplicationId` it derives - built
/// directly so the signing key stays in scope to compute the id.
pub fn minimal_signed_bundle_bytes(package: &str, version: &str) -> (Vec<u8>, ApplicationId) {
    signed_bundle_bytes(package, version, &[])
}

/// One named service per entry in `services`, each getting a blob of its own at
/// install - which the unnamed single-artifact shape (empty `services`) never does.
pub fn signed_bundle_bytes(
    package: &str,
    version: &str,
    services: &[&str],
) -> (Vec<u8>, ApplicationId) {
    let dir = TempDir::new().expect("temp dir");

    let signing_key = SigningKey::generate(&mut OsRng);
    let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());
    let application_id =
        ApplicationId::for_bundle(package, &signer_id).expect("derive application id");

    // Distinct bytes per service, so no two services share one blob id.
    let wasm: Vec<(String, Vec<u8>)> = if services.is_empty() {
        vec![(
            "app.wasm".to_owned(),
            b"registry test wasm bytecode".to_vec(),
        )]
    } else {
        services
            .iter()
            .map(|name| {
                (
                    format!("{name}.wasm"),
                    format!("registry test wasm bytecode for {name}").into_bytes(),
                )
            })
            .collect()
    };
    let artifact = |(path, content): &(String, Vec<u8>)| BundleArtifact {
        path: path.clone(),
        hash: hex_lower(&Sha256::digest(content)),
        size: content.len() as u64,
    };

    let manifest = BundleManifest {
        version: "1.0".to_owned(),
        package: package.to_owned(),
        app_version: version.to_owned(),
        signer_id: Some(signer_id),
        min_runtime_version: "0.1.0".to_owned(),
        metadata: None,
        handlers: None,
        interfaces: None,
        wasm: services.is_empty().then(|| artifact(&wasm[0])),
        abi: None,
        links: None,
        services: (!services.is_empty()).then(|| {
            services
                .iter()
                .zip(&wasm)
                .map(|(name, entry)| BundleService {
                    name: (*name).to_owned(),
                    wasm: artifact(entry),
                    abi: None,
                })
                .collect()
        }),
        signature: None,
    };
    let mut manifest_json: serde_json::Value =
        serde_json::to_value(&manifest).expect("serialize manifest");
    sign_manifest_json(&mut manifest_json, &signing_key).expect("sign manifest");
    let manifest_bytes = serde_json::to_vec(&manifest_json).expect("serialize signed manifest");

    let mut entries: Vec<(&str, &[u8])> = vec![("manifest.json", manifest_bytes.as_slice())];
    entries.extend(
        wasm.iter()
            .map(|(path, content)| (path.as_str(), content.as_slice())),
    );
    let bytes = pack_entries(&dir, &format!("{package}-{version}.mpk"), &entries);
    (bytes, application_id)
}
