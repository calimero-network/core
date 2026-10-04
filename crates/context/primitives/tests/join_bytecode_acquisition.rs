//! The join bootstrap binds to the id governance named and reaches bytecode
//! through the one resolver - never re-deriving, never another node's source.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use actix::{Actor, Addr, Context as ActorContext, Handler};
use calimero_app_downloader::registry::{RegistryConfig, RegistryMode, PENDING_BLOB_SHARE_SOURCE};
use calimero_context_client::client::ContextClient;
use calimero_context_client::messages::ContextMessage;
use calimero_network_primitives::blob_types::BlobProbe;
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::messages::NetworkMessage;
use calimero_node_primitives::client::application::lock_application_rows;
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::test_fixtures::{bundle, node_client, node_client_over};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{ContextConfigParams, ContextId};
use calimero_store::db::InMemoryDB;
use calimero_store::{key, types, Store};
use calimero_utils_actix::LazyRecipient;
use libp2p::PeerId;
use tempfile::TempDir;

/// Any syntactically valid peer id; nothing below ever dials it.
const PEER: &str = "12D3KooWR5V4zmisVtVdGE6i8jfFwtgRNq5t8eDGxfckKuhXu7Eh";

/// The wasm the admin built; the joiner only ever learns its blob's id.
const WASM: &[u8] = b"join test wasm bytecode";

/// How long an install holds the row lock: long enough for an unlocked stub write.
const STUB_WAIT: Duration = Duration::from_millis(500);

/// A peer that advertises the blob and serves it, so the resolver's final leg
/// completes in-process, and that counts how often it was asked at all.
struct BlobPeer {
    peer_id: PeerId,
    queries: Arc<AtomicUsize>,
    served: Arc<[u8]>,
}

impl Actor for BlobPeer {
    type Context = ActorContext<Self>;
}

impl Handler<NetworkMessage> for BlobPeer {
    type Result = ();

    fn handle(&mut self, msg: NetworkMessage, _ctx: &mut ActorContext<Self>) -> Self::Result {
        match msg {
            // Discovery no longer reads a provider record: the joiner resolves
            // the context's subscribers and asks each one directly. `queries`
            // therefore counts probes — still "how often it asked a peer",
            // which is what these tests assert on.
            NetworkMessage::SubscribedPeers { outcome, .. } => {
                let _ignored = outcome.send(vec![self.peer_id]);
            }
            NetworkMessage::ProbeBlob { outcome, .. } => {
                let _previous = self.queries.fetch_add(1, Ordering::SeqCst);
                let _ignored = outcome.send(Ok(BlobProbe::Held {
                    size: Some(self.served.len() as u64),
                }));
            }
            NetworkMessage::QueryBlob { outcome, .. } => {
                let _ignored = outcome.send(Ok(vec![self.peer_id]));
            }
            NetworkMessage::RequestBlob { outcome, .. } => {
                let _ignored = outcome.send(Ok(Some(self.served.to_vec())));
            }
            _ => {}
        }
    }
}

/// The context manager the bootstrap hands off to once metadata is written.
struct SyncSink;

impl Actor for SyncSink {
    type Context = ActorContext<Self>;
}

impl Handler<ContextMessage> for SyncSink {
    type Result = ();

    fn handle(&mut self, msg: ContextMessage, _ctx: &mut ActorContext<Self>) -> Self::Result {
        if let ContextMessage::Sync { outcome, .. } = msg {
            let _ignored = outcome.send(());
        }
    }
}

/// The blob id `bytes` get once stored - a hash over chunk ids, not content,
/// so it cannot be computed by hashing the bytes directly.
async fn published_blob_id(bytes: &[u8]) -> BlobId {
    let (node, _store, _data, _blobs) = node_client().await;
    let (blob_id, _size) = node
        .add_blob(bytes, Some(bytes.len() as u64), None)
        .await
        .expect("store bytes");
    blob_id
}

/// What the admin published: a signed bundle of `WASM`, and the id it derives.
async fn published_bundle() -> (Vec<u8>, ApplicationId) {
    let dir = TempDir::new().expect("temp dir");
    let path = bundle(&dir, "com.example.joined", "1.0.0", WASM);
    let (node, _store, _data, _blobs) = node_client().await;
    let application_id = node
        .install_application_from_path(path.clone())
        .await
        .expect("derive the bundle's id");
    (std::fs::read(path).expect("bundle bytes"), application_id)
}

struct Joiner {
    client: ContextClient,
    store: Store,
    node: NodeClient,
    queries: Arc<AtomicUsize>,
    _peer: Addr<BlobPeer>,
    _manager: Addr<SyncSink>,
    _dirs: (TempDir, TempDir),
}

impl Joiner {
    /// A joiner whose one source is its peers, which serve `served`.
    async fn dht(served: &[u8]) -> Self {
        Self::with_registry(RegistryConfig::new(RegistryMode::Dht, None), served).await
    }

    /// A joiner whose one source is the registry at `base`.
    async fn http(base: url::Url) -> Self {
        Self::with_registry(RegistryConfig::new(RegistryMode::Http, Some(base)), WASM).await
    }

    async fn with_registry(registry: RegistryConfig, served: &[u8]) -> Self {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let queries = Arc::new(AtomicUsize::new(0));

        let network = LazyRecipient::new();
        let peer = Actor::create({
            let (network, queries) = (network.clone(), Arc::clone(&queries));
            let served = Arc::from(served);
            move |ctx| {
                assert!(network.init(ctx), "peer must own the network recipient");
                BlobPeer {
                    peer_id: PEER.parse().expect("peer id"),
                    queries,
                    served,
                }
            }
        });

        let context_manager = LazyRecipient::new();
        let manager = Actor::create({
            let context_manager = context_manager.clone();
            move |ctx| {
                assert!(
                    context_manager.init(ctx),
                    "sink must own the context recipient"
                );
                SyncSink
            }
        });

        let (node, data_dir, blob_dir) =
            node_client_over(store.clone(), NetworkClient::new(network)).await;
        let node = node.with_registry(registry);

        Self {
            client: ContextClient::new(store.clone(), node.clone(), context_manager),
            store,
            node,
            queries,
            _peer: peer,
            _manager: manager,
            _dirs: (data_dir, blob_dir),
        }
    }

    /// Seed the stub `ContextRegistered` writes: the blob id and source governance
    /// carried, with no bytes and none of the admin's own per-node metadata.
    fn seed_row(
        &self,
        application_id: ApplicationId,
        blob_id: BlobId,
        source: &str,
        coords: (&str, &str),
    ) {
        let mut handle = self.client.datastore_handle();
        handle
            .put(
                &key::ApplicationMeta::new(application_id),
                &types::ApplicationMeta::new(
                    key::BlobMeta::new(blob_id),
                    0,
                    source.to_owned().into_boxed_str(),
                    Box::default(),
                    key::BlobMeta::new(BlobId::from([0_u8; 32])),
                    types::PackageInfo {
                        package: coords.0.into(),
                        version: coords.1.into(),
                        signer_id: String::new().into_boxed_str(),
                        state_version: 0,
                    },
                ),
            )
            .expect("seed row");
    }

    fn row(&self, application_id: ApplicationId) -> Option<types::ApplicationMeta> {
        let handle = self.client.datastore_handle();
        handle
            .get(&key::ApplicationMeta::new(application_id))
            .expect("row read")
    }

    fn rows(&self) -> usize {
        let handle = self.client.datastore_handle();
        let mut iter = handle.iter::<key::ApplicationMeta>().expect("iter rows");
        iter.keys().count()
    }

    async fn bootstrap(&self, application_id: ApplicationId) -> eyre::Result<()> {
        let _context = self
            .client
            .sync_context_config(
                ContextId::from([0x11; 32]),
                Some(ContextConfigParams {
                    application_id: Some(application_id),
                    application_revision: 0,
                    members_revision: 0,
                    service_name: None,
                }),
            )
            .await?;
        Ok(())
    }
}

/// The joiner binds the bundle under the id governance named, and an
/// unreachable recorded source must not strand it.
#[actix::test]
async fn bootstrap_keeps_the_row_under_the_governance_named_id() {
    let (bundle, named_id) = published_bundle().await;
    let joiner = Joiner::dht(&bundle).await;
    let blob_id = published_blob_id(&bundle).await;
    joiner.seed_row(named_id, blob_id, "http://127.0.0.1:9/app.mpk", ("", ""));

    joiner.bootstrap(named_id).await.expect("bootstrap");

    let row = joiner
        .row(named_id)
        .expect("row must exist under the named id");
    assert_eq!(row.bytecode.blob_id(), blob_id);
    assert_eq!(
        joiner.rows(),
        1,
        "the bundle must land under the named id, not beside it"
    );
    assert!(
        joiner.node.has_blob(&blob_id).expect("blob lookup"),
        "the resolver's peer leg must have delivered the bytecode"
    );
}

/// A raw-wasm id cannot be derived from its bytes, so raw wasm a peer serves
/// for a registered context would land under whatever id governance named.
#[actix::test]
async fn bootstrap_installs_no_raw_wasm_a_peer_serves() {
    let joiner = Joiner::dht(WASM).await;
    let named_id = ApplicationId::from([0x59; 32]);
    let blob_id = published_blob_id(WASM).await;
    joiner.seed_row(named_id, blob_id, PENDING_BLOB_SHARE_SOURCE, ("", ""));

    joiner
        .bootstrap(named_id)
        .await
        .expect("a refused acquisition does not fail the bootstrap");

    assert_eq!(joiner.queries.load(Ordering::SeqCst), 1);
    assert_eq!(
        joiner.row(named_id).expect("the stub stays").size,
        0,
        "raw wasm must not fill the stub governance seeded"
    );
}

/// The recorded source belongs to whichever node installed the app. Only this
/// node's own configured source is ever used, so nothing dials the recorded one.
#[actix::test]
async fn bootstrap_never_fetches_the_recorded_source() {
    let (bundle, named_id) = published_bundle().await;
    let joiner = Joiner::dht(&bundle).await;
    let blob_id = published_blob_id(&bundle).await;
    joiner.seed_row(named_id, blob_id, "http://127.0.0.1:9/app.wasm", ("", ""));

    joiner.bootstrap(named_id).await.expect("bootstrap");

    assert_eq!(
        joiner.queries.load(Ordering::SeqCst),
        1,
        "a dht joiner asks its peers, never the row's recorded source"
    );
}

/// A stub row carries no blob id yet: there is nothing to ask a peer for, so
/// the bootstrap must not spend the DHT retry window before letting sync run.
#[actix::test]
async fn bootstrap_asks_no_peer_for_a_stub_row() {
    let joiner = Joiner::dht(WASM).await;
    let named_id = ApplicationId::from([0x5C; 32]);
    joiner.seed_row(
        named_id,
        BlobId::from([0_u8; 32]),
        PENDING_BLOB_SHARE_SOURCE,
        ("", ""),
    );

    joiner.bootstrap(named_id).await.expect("bootstrap");

    assert_eq!(
        joiner.queries.load(Ordering::SeqCst),
        0,
        "a row with no blob id has nothing to fetch"
    );
}

/// Nothing has told this node what the application is yet. The bootstrap
/// writes the marker row blob sharing fills in, and asks no peer for it.
#[actix::test]
async fn bootstrap_writes_a_stub_when_no_row_exists() {
    let joiner = Joiner::dht(WASM).await;
    let named_id = ApplicationId::from([0x5D; 32]);

    joiner.bootstrap(named_id).await.expect("bootstrap");

    let row = joiner.row(named_id).expect("stub row must be written");
    assert_eq!(&*row.source, PENDING_BLOB_SHARE_SOURCE);
    assert_eq!(*row.bytecode.blob_id(), [0_u8; 32]);
    assert!(
        row.package.is_empty() && row.version.is_empty(),
        "absent coordinates must stay absent, never a placeholder"
    );
    assert_eq!(joiner.queries.load(Ordering::SeqCst), 0);
}

/// An install holding the row lock writes the full row; the bootstrap's stub
/// must wait for it, then leave that row alone.
#[actix::test]
async fn bootstrap_stub_waits_for_an_install_of_the_same_application() {
    let joiner = Joiner::dht(WASM).await;
    let named_id = ApplicationId::from([0x5E; 32]);
    let row_key = key::ApplicationMeta::new(named_id);

    let (held_tx, held_rx) = mpsc::channel();
    let store = joiner.store.clone();
    let installer = std::thread::spawn(move || {
        let _install = lock_application_rows();
        held_tx.send(()).expect("signal the lock is held");
        std::thread::sleep(STUB_WAIT);
        let mut handle = store.handle();
        let stub_ran_inside = handle.has(&row_key).expect("row lookup");
        handle
            .put(
                &row_key,
                &types::ApplicationMeta::new(
                    key::BlobMeta::new(BlobId::from([0xDB; 32])),
                    42,
                    "https://reg.example/app-1.0.0.mpk".into(),
                    Box::default(),
                    key::BlobMeta::new(BlobId::from([0_u8; 32])),
                    types::PackageInfo {
                        package: "com.acme.app".into(),
                        version: "1.0.0".into(),
                        signer_id: "did:key:installer".into(),
                        state_version: 0,
                    },
                ),
            )
            .expect("install the row");
        stub_ran_inside
    });
    held_rx.recv().expect("the installer holds the lock");

    joiner.bootstrap(named_id).await.expect("bootstrap");

    assert!(
        !installer.join().expect("installer thread"),
        "the bootstrap stub must not run inside an install's check-then-write"
    );
    let row = joiner.row(named_id).expect("the installed row");
    assert_eq!(
        (row.size, &*row.signer_id),
        (42, "did:key:installer"),
        "a stub must never replace the installed row"
    );
}

/// A stand-in registry that refuses every request and counts what it saw.
/// Refusing is enough: what is under test is whether it was dialled at all.
fn refusing_registry() -> (url::Url, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("local addr"))
        .parse()
        .expect("registry base");
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&hits);
    let _serving = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let _previous = seen.fetch_add(1, Ordering::SeqCst);
            let _request = stream.read(&mut [0_u8; 1024]);
            let _served = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            );
        }
    });
    (base, hits)
}

/// The join seam: coordinates governance carried must resolve through this
/// node's own registry. Only a test spanning op and row catches them going missing.
#[actix::test]
async fn bootstrap_asks_the_registry_when_the_row_names_coordinates() {
    let (base, hits) = refusing_registry();
    let joiner = Joiner::http(base).await;
    let named_id = ApplicationId::from([0x5E; 32]);
    let blob_id = published_blob_id(WASM).await;
    joiner.seed_row(
        named_id,
        blob_id,
        "https://apps.example/artifacts/com.acme.app/1.2.3/com.acme.app-1.2.3.mpk",
        ("com.acme.app", "1.2.3"),
    );

    joiner.bootstrap(named_id).await.expect("bootstrap");

    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the row's coordinates must reach this node's own registry"
    );
    assert_eq!(
        joiner.queries.load(Ordering::SeqCst),
        0,
        "an http joiner has no peer source to fall back to"
    );
    assert!(
        !joiner.node.has_blob(&blob_id).expect("blob lookup"),
        "a registry that has nothing published leaves the joiner without the bytes"
    );
}

/// A locally built application is published nowhere, so a configured registry
/// must not be dialled on its behalf - a coordinate is never guessed.
#[actix::test]
async fn bootstrap_leaves_the_registry_alone_for_an_uncoordinated_row() {
    let (base, hits) = refusing_registry();
    let joiner = Joiner::http(base).await;
    let named_id = ApplicationId::from([0x5F; 32]);
    let blob_id = published_blob_id(WASM).await;
    joiner.seed_row(named_id, blob_id, PENDING_BLOB_SHARE_SOURCE, ("", ""));

    joiner.bootstrap(named_id).await.expect("bootstrap");

    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(joiner.queries.load(Ordering::SeqCst), 0);
}
