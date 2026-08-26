//! What `acquire_bytecode` leaves behind, one arm per outcome. The node has
//! exactly one source; this is the function that walks it and must never error.

use std::sync::Arc;

use calimero_app_downloader::registry::{RegistryConfig, RegistryMode};
use calimero_app_downloader::{AppRequest, Outcome};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_store::config::StoreConfig;
use calimero_store::db::{Column, Database, InMemoryDB};
use calimero_store::iter::Iter;
use calimero_store::slice::Slice;
use calimero_store::tx::Transaction;
use calimero_store::Store;
use url::Url;

mod common;

/// Raw wasm rather than a bundle: a raw install adopts the named id instead of
/// re-deriving one, which keeps these tests about acquisition.
const WASM: &[u8] = b"raw wasm, not a bundle";

/// Any context id; the peer source only needs one to authorize against.
fn context() -> ContextId {
    ContextId::from([0x11; 32])
}

fn req<'a>(
    bytecode_id: BlobId,
    package: Option<&'a str>,
    context_id: Option<&'a ContextId>,
) -> AppRequest<'a> {
    AppRequest {
        bytecode_id: Some(bytecode_id),
        application_id: Some(ApplicationId::from([0x22; 32])),
        package: package.unwrap_or_default(),
        version: package.map_or("", |_| "1.0.0"),
        context_id,
    }
}

/// The scheme+authority of a served URL, which is what an operator puts in
/// `[registry].base_url`.
fn base_of(url: &Url) -> Url {
    url.join("/").expect("base")
}

/// A node whose one source is its peers.
fn dht(node_client: &NodeClient) -> NodeClient {
    node_client
        .clone()
        .with_registry(RegistryConfig::new(RegistryMode::Dht, None))
}

/// A node whose one source is the registry at `base`.
fn http(node_client: &NodeClient, base: &Url) -> NodeClient {
    node_client
        .clone()
        .with_registry(RegistryConfig::new(RegistryMode::Http, Some(base.clone())))
}

#[tokio::test]
async fn a_zero_bytecode_id_is_never_fetched() {
    let context = context();
    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    assert_eq!(
        dht(&node_client)
            .acquire_bytecode(&req(BlobId::from([0; 32]), None, Some(&context)))
            .await,
        Outcome::Unavailable
    );
}

// Holding the bytes is not holding an application: without a row bound to
// them there is nothing to execute, so the blob alone is not "already installed".
#[actix::test]
async fn a_blob_held_without_a_row_is_still_installed() {
    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let node_client = dht(&node_client);
    let (stored, _size) = node_client
        .add_blob(WASM, Some(WASM.len() as u64), None)
        .await
        .expect("store");

    let context = context();
    let request = req(stored, None, Some(&context));
    assert_eq!(
        node_client.acquire_bytecode(&request).await,
        Outcome::Installed
    );

    let installed = node_client
        .get_application(&request.application_id.expect("a named id"))
        .expect("row read")
        .expect("a local blob with no row must still be installed");
    assert_eq!(installed.blob.bytecode, stored);
}

// Once the row names the bytes, a second acquisition is a no-op rather than
// another install.
#[actix::test]
async fn an_installed_application_is_not_acquired_again() {
    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let node_client = dht(&node_client);
    let (stored, _size) = node_client
        .add_blob(WASM, Some(WASM.len() as u64), None)
        .await
        .expect("store");

    let context = context();
    let request = req(stored, None, Some(&context));
    assert_eq!(
        node_client.acquire_bytecode(&request).await,
        Outcome::Installed
    );
    assert_eq!(
        node_client.acquire_bytecode(&request).await,
        Outcome::AlreadyInstalled
    );
}

/// A datastore that refuses every blob lookup, so the `has_blob` error arm is
/// reachable without a corrupt on-disk store.
#[derive(Debug)]
struct BlobLookupFails(Arc<dyn for<'x> Database<'x>>);

impl<'a> Database<'a> for BlobLookupFails {
    fn open(_config: &StoreConfig) -> eyre::Result<Self> {
        eyre::bail!("test double: not openable")
    }

    fn has(&self, col: Column, key: Slice<'_>) -> eyre::Result<bool> {
        if matches!(col, Column::Blobs) {
            eyre::bail!("blobstore lookup failed");
        }
        self.0.has(col, key)
    }

    fn get(&self, col: Column, key: Slice<'_>) -> eyre::Result<Option<Slice<'_>>> {
        self.0.get(col, key)
    }

    fn put(&self, col: Column, key: Slice<'a>, value: Slice<'a>) -> eyre::Result<()> {
        self.0.put(col, key, value)
    }

    fn delete(&self, col: Column, key: Slice<'_>) -> eyre::Result<()> {
        self.0.delete(col, key)
    }

    fn iter(&self, col: Column) -> eyre::Result<Iter<'_>> {
        self.0.iter(col)
    }

    fn apply(&self, tx: &Transaction<'a>) -> eyre::Result<()> {
        self.0.apply(tx)
    }
}

// A failed lookup must not be read as "absent" and send the node fetching: it
// would re-download bytes it may already hold, once per retry. The peer below
// would serve them, so reaching it at all is the failure this pins.
#[actix::test]
async fn a_failed_blobstore_lookup_stops_the_walk() {
    let expected = common::blob_id_of(WASM).await;
    let datastore = Store::new(Arc::new(BlobLookupFails(Arc::new(InMemoryDB::owned()))));
    let (network, _peer) = common::fake_peer_network(common::PeerBehavior::Serves(WASM.to_vec()));
    let (node_client, _data, _blobs) =
        common::create_test_node_client_with(Some(datastore), network).await;

    let context = context();
    assert_eq!(
        dht(&node_client)
            .acquire_bytecode(&req(expected, None, Some(&context)))
            .await,
        Outcome::Unavailable
    );
}

#[tokio::test]
async fn the_registry_source_installs_the_bytecode() {
    let context = context();
    let expected = common::blob_id_of(WASM).await;
    let (url, server) = common::serve_once(WASM.to_vec()).await;
    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let node_client = http(&node_client, &base_of(&url));

    assert_eq!(
        node_client
            .acquire_bytecode(&req(expected, Some("com.example.app"), Some(&context)))
            .await,
        Outcome::Installed
    );
    let _ignored = server.await;
    assert!(node_client.has_blob(&expected).expect("lookup"));
}

// There is no route behind the registry. A registry that has not published the
// version yet leaves the node without it, however willing a peer would have been.
#[actix::test]
async fn an_unpublished_registry_version_leaves_the_bytecode_unavailable() {
    let context = context();
    let expected = common::blob_id_of(WASM).await;
    let (url, server) = common::serve_status_once("404 Not Found").await;
    let (network, _peer) = common::fake_peer_network(common::PeerBehavior::Serves(WASM.to_vec()));

    let (node_client, _data, _blobs) = common::create_test_node_client_with(None, network).await;
    let node_client = http(&node_client, &base_of(&url));

    assert_eq!(
        node_client
            .acquire_bytecode(&req(expected, Some("com.example.app"), Some(&context)))
            .await,
        Outcome::Unavailable
    );
    let _ignored = server.await;
    assert!(!node_client.has_blob(&expected).expect("lookup"));
}

// An http node with nowhere to fetch from cannot resolve anything, and reports
// that as "retry later" rather than erroring out of the caller's hands.
#[tokio::test]
async fn http_mode_without_a_base_url_leaves_the_bytecode_unavailable() {
    let context = context();
    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let node_client = node_client.with_registry(RegistryConfig::new(RegistryMode::Http, None));

    assert_eq!(
        node_client
            .acquire_bytecode(&req(
                BlobId::from([0x88; 32]),
                Some("com.example.app"),
                Some(&context)
            ))
            .await,
        Outcome::Unavailable
    );
}

// The peer source authorizes by context membership, so without one there is
// nothing to ask and the walk ends rather than falling off the end silently.
#[tokio::test]
async fn no_context_ends_the_walk_before_the_peer_source() {
    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    assert_eq!(
        dht(&node_client)
            .acquire_bytecode(&req(BlobId::from([0x55; 32]), None, None))
            .await,
        Outcome::Unavailable
    );
}

// The outcome the peer source owes its caller, not the route it took: downloading
// the bytes and stopping leaves the joiner unable to execute, which is exactly
// how a joined context sits at the zero root until sync times out. Raw wasm,
// so the row must be bound under the id governance named, never a re-derived
// one - that id folds in per-node source and metadata.
#[actix::test]
async fn the_peer_source_leaves_the_application_installed() {
    let expected = common::blob_id_of(WASM).await;
    let (network, _peer) = common::fake_peer_network(common::PeerBehavior::Serves(WASM.to_vec()));
    let (node_client, _data, _blobs) = common::create_test_node_client_with(None, network).await;
    let node_client = dht(&node_client);

    let context = context();
    let request = req(expected, None, Some(&context));
    assert_eq!(
        node_client.acquire_bytecode(&request).await,
        Outcome::Installed
    );

    let installed = node_client
        .get_application(&request.application_id.expect("a named id"))
        .expect("row read")
        .expect("the peer source must install what it acquired");
    assert_eq!(
        installed.blob.bytecode, expected,
        "the row must name the acquired bytecode"
    );
    assert_eq!(Some(installed.id), request.application_id);
    assert!(node_client.has_blob(&expected).expect("lookup"));
}

#[actix::test]
async fn no_provider_leaves_the_bytecode_unavailable() {
    let context = context();
    let (network, _peer) = common::fake_peer_network(common::PeerBehavior::NoProviders);
    let (node_client, _data, _blobs) = common::create_test_node_client_with(None, network).await;

    assert_eq!(
        dht(&node_client)
            .acquire_bytecode(&req(BlobId::from([0x66; 32]), None, Some(&context)))
            .await,
        Outcome::Unavailable
    );
}

// A source that had nothing is never an error: the caller keeps the version it
// runs and retries, so a failing DHT must land on the same outcome as an empty one.
#[actix::test]
async fn a_failing_peer_query_leaves_the_bytecode_unavailable() {
    let context = context();
    let (network, _peer) = common::fake_peer_network(common::PeerBehavior::QueryFails);
    let (node_client, _data, _blobs) = common::create_test_node_client_with(None, network).await;

    assert_eq!(
        dht(&node_client)
            .acquire_bytecode(&req(BlobId::from([0x77; 32]), None, Some(&context)))
            .await,
        Outcome::Unavailable
    );
}
