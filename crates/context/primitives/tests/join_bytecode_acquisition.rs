//! The join bootstrap binds a context to the application id governance named
//! and reaches the bytecode through the one resolver - never re-deriving the
//! id, and never fetching the source some other node happened to record.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use actix::{Actor, Addr, Context as ActorContext, Handler};
use calimero_app_downloader::registry::{RegistryConfig, RegistryMode, PENDING_BLOB_SHARE_SOURCE};
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_context_client::client::ContextClient;
use calimero_context_client::messages::ContextMessage;
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::messages::NetworkMessage;
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{ContextConfigParams, ContextId};
use calimero_store::db::InMemoryDB;
use calimero_store::{key, types, Store};
use calimero_utils_actix::LazyRecipient;
use libp2p::PeerId;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

/// Any syntactically valid peer id; nothing below ever dials it.
const PEER: &str = "12D3KooWR5V4zmisVtVdGE6i8jfFwtgRNq5t8eDGxfckKuhXu7Eh";

/// The bytes the admin published; the joiner only ever learns their blob id.
const WASM: &[u8] = b"join test wasm bytecode";

/// A peer that advertises the blob and serves it, so the resolver's final leg
/// completes in-process, and that counts how often it was asked at all.
struct BlobPeer {
    peer_id: PeerId,
    queries: Arc<AtomicUsize>,
}

impl Actor for BlobPeer {
    type Context = ActorContext<Self>;
}

impl Handler<NetworkMessage> for BlobPeer {
    type Result = ();

    fn handle(&mut self, msg: NetworkMessage, _ctx: &mut ActorContext<Self>) -> Self::Result {
        match msg {
            NetworkMessage::QueryBlob { outcome, .. } => {
                let _previous = self.queries.fetch_add(1, Ordering::SeqCst);
                let _ignored = outcome.send(Ok(vec![self.peer_id]));
            }
            NetworkMessage::RequestBlob { outcome, .. } => {
                let _ignored = outcome.send(Ok(Some(WASM.to_vec())));
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

async fn blob_manager(dir: &TempDir, store: &Store) -> BlobManager {
    // Nested one level down: the node derives its root as the blob root's
    // parent, so a bare TempDir would make every node share the OS temp dir.
    let root = dir.path().join("blobs");
    let filesystem = FileSystem::new(&BlobStoreConfig::new(root.try_into().expect("utf8 path")))
        .await
        .expect("blob filesystem");
    BlobManager::new(BlobStore::new(store.clone(), filesystem))
}

fn node_client(store: Store, blobs: BlobManager, network: NetworkClient) -> NodeClient {
    let (events, _) = broadcast::channel(16);
    let (ctx_sync_tx, _) = mpsc::channel(16);
    let (ns_sync_tx, _) = mpsc::channel(16);
    let (ns_join_tx, _) = mpsc::channel(16);
    let (open_subgroup_join_tx, _) = mpsc::channel(16);

    NodeClient::new(
        store,
        blobs,
        network,
        LazyRecipient::new(),
        events,
        SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx),
        None,
    )
}

/// The blob id `WASM` gets once stored - a hash over chunk ids, not content,
/// so it cannot be computed by hashing the bytes directly.
async fn published_blob_id() -> BlobId {
    let dir = TempDir::new().expect("temp dir");
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let blobs = blob_manager(&dir, &store).await;
    let (blob_id, _size) = node_client(store, blobs, NetworkClient::new(LazyRecipient::new()))
        .add_blob(WASM, Some(WASM.len() as u64), None)
        .await
        .expect("store bytes");
    blob_id
}

struct Joiner {
    client: ContextClient,
    node: NodeClient,
    queries: Arc<AtomicUsize>,
    _peer: Addr<BlobPeer>,
    _manager: Addr<SyncSink>,
    _dir: TempDir,
}

impl Joiner {
    /// A joiner whose one source is its peers.
    async fn dht() -> Self {
        Self::with_registry(RegistryConfig::new(RegistryMode::Dht, None)).await
    }

    /// A joiner whose one source is the registry at `base`.
    async fn http(base: url::Url) -> Self {
        Self::with_registry(RegistryConfig::new(RegistryMode::Http, Some(base))).await
    }

    async fn with_registry(registry: RegistryConfig) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let queries = Arc::new(AtomicUsize::new(0));

        let network = LazyRecipient::new();
        let peer = Actor::create({
            let (network, queries) = (network.clone(), Arc::clone(&queries));
            move |ctx| {
                assert!(network.init(ctx), "peer must own the network recipient");
                BlobPeer {
                    peer_id: PEER.parse().expect("peer id"),
                    queries,
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

        let blobs = blob_manager(&dir, &store).await;
        let node =
            node_client(store.clone(), blobs, NetworkClient::new(network)).with_registry(registry);

        Self {
            client: ContextClient::new(store, node.clone(), context_manager),
            node,
            queries,
            _peer: peer,
            _manager: manager,
            _dir: dir,
        }
    }

    /// Seed the row a joiner starts from: the blob id and source governance
    /// carried, with none of the admin's own per-node metadata.
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
                    WASM.len() as u64,
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

/// A raw-wasm id folds in the installing node's own source and metadata, so a
/// joiner that re-derives it can never reproduce the id governance named. The
/// bootstrap must therefore adopt that id, and must not turn a source it
/// cannot reach into a hard failure that strands the joiner forever.
#[actix::test]
async fn bootstrap_keeps_the_row_under_the_governance_named_id() {
    let joiner = Joiner::dht().await;
    let named_id = ApplicationId::from([0x5A; 32]);
    let blob_id = published_blob_id().await;
    joiner.seed_row(named_id, blob_id, "http://127.0.0.1:9/app.wasm", ("", ""));

    joiner
        .bootstrap(named_id)
        .await
        .expect("bootstrap must not fail on an id it cannot re-derive");

    let row = joiner
        .row(named_id)
        .expect("row must exist under the named id");
    assert_eq!(row.bytecode.blob_id(), blob_id);
    assert_eq!(
        joiner.rows(),
        1,
        "a re-derived id would have left a second row behind"
    );
    assert!(
        joiner.node.has_blob(&blob_id).expect("blob lookup"),
        "the resolver's peer leg must have delivered the bytecode"
    );
}

/// The recorded source belongs to whichever node installed the app. Only this
/// node's own configured source is ever used, so nothing dials the recorded one.
#[actix::test]
async fn bootstrap_never_fetches_the_recorded_source() {
    let joiner = Joiner::dht().await;
    let named_id = ApplicationId::from([0x5B; 32]);
    let blob_id = published_blob_id().await;
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
    let joiner = Joiner::dht().await;
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
    let joiner = Joiner::dht().await;
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

/// The join seam: an application whose coordinates governance carried must be
/// reached through this node's OWN registry, and through nothing else. The op
/// and the row each pass their own unit tests - only a bootstrap that runs both
/// catches the pair going missing between them.
#[actix::test]
async fn bootstrap_asks_the_registry_when_the_row_names_coordinates() {
    let (base, hits) = refusing_registry();
    let joiner = Joiner::http(base).await;
    let named_id = ApplicationId::from([0x5E; 32]);
    let blob_id = published_blob_id().await;
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
    let blob_id = published_blob_id().await;
    joiner.seed_row(named_id, blob_id, PENDING_BLOB_SHARE_SOURCE, ("", ""));

    joiner.bootstrap(named_id).await.expect("bootstrap");

    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(joiner.queries.load(Ordering::SeqCst), 0);
}
