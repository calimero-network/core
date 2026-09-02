//! Fixtures shared by the bundle integration tests. Each `tests/*.rs` is its
//! own crate, so a helper only some of them call reads as dead code elsewhere.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use actix::{Actor, Context, Handler};
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::messages::NetworkMessage;
use calimero_node_primitives::bundle::{
    derive_signer_id_did_key, sign_manifest_json, BundleArtifact, BundleManifest, BundleService,
};
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::test_fixtures::node_client_over;
pub use calimero_node_primitives::test_fixtures::{hex_lower, pack_entries};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use camino::Utf8PathBuf;
use ed25519_dalek::SigningKey;
use libp2p::PeerId;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;
use url::Url;

/// Any syntactically valid peer id; the fakes below are never dialled.
pub const FAKE_PEER: &str = "12D3KooWR5V4zmisVtVdGE6i8jfFwtgRNq5t8eDGxfckKuhXu7Eh";

/// How a fake peer answers the two blob messages, one variant per arm of the
/// peer leg so a test can drive each without a network.
pub enum PeerBehavior {
    Serves(Vec<u8>),
    ServesUnannounced(Vec<u8>), // holds it, never announced - every bytecode blob
    NoProviders,
    QueryFails,
}

pub struct FakePeer {
    pub peer_id: PeerId,
    pub behavior: PeerBehavior,
    pub queries: Arc<AtomicUsize>, // pinned at zero when a route must not run
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
                    PeerBehavior::ServesUnannounced(_) | PeerBehavior::NoProviders => Ok(vec![]),
                    PeerBehavior::QueryFails => Err(eyre::eyre!("dht query failed")),
                };
                let _ignored = outcome.send(answer);
            }
            NetworkMessage::SubscribedPeers { outcome, .. } => {
                let answer = match &self.behavior {
                    PeerBehavior::Serves(_) | PeerBehavior::ServesUnannounced(_) => {
                        vec![self.peer_id]
                    }
                    PeerBehavior::NoProviders | PeerBehavior::QueryFails => vec![],
                };
                let _ignored = outcome.send(answer);
            }
            NetworkMessage::RequestBlob { outcome, .. } => {
                let answer = match &self.behavior {
                    PeerBehavior::Serves(bytes) | PeerBehavior::ServesUnannounced(bytes) => {
                        Some(bytes.clone())
                    }
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

/// A test NodeClient with temporary directories. `datastore` injects a custom
/// Store; `None` defaults to `InMemoryDB`.
pub async fn create_test_node_client(datastore: Option<Store>) -> (NodeClient, TempDir, TempDir) {
    create_test_node_client_with(datastore, NetworkClient::new(LazyRecipient::new())).await
}

/// As [`create_test_node_client`], but with a caller-supplied `NetworkClient`
/// so a test can stand in a fake peer for the DHT fetch path.
pub async fn create_test_node_client_with(
    datastore: Option<Store>,
    network_client: NetworkClient,
) -> (NodeClient, TempDir, TempDir) {
    let datastore = datastore.unwrap_or_else(|| Store::new(Arc::new(InMemoryDB::owned())));
    node_client_over(datastore, network_client).await
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
    let signing_key = SigningKey::generate(&mut OsRng);

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
            hash: hex_lower(&Sha256::digest(wasm_content)),
            size: wasm_content.len() as u64,
        }),
        abi: abi_content.map(|content| BundleArtifact {
            path: "abi.json".to_owned(),
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

    let mut entries: Vec<(&str, &[u8])> = vec![
        ("manifest.json", manifest_bytes.as_slice()),
        ("app.wasm", wasm_content),
    ];
    entries.extend(abi_content.map(|content| ("abi.json", content)));
    entries.extend(migrations);

    let name = format!("{package}-{version}.mpk");
    let _bytes = pack_entries(temp_dir, &name, &entries);
    temp_dir.path().join(name).try_into().unwrap()
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
