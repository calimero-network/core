//! What this node hands a peer, and where it goes for an application itself.
//! An http node resolves applications from its registry, so it neither serves
//! nor announces application bytecode and never asks a peer for it - while
//! user-data blob sharing, a different subsystem, stays open in both modes.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use actix::{Actor, Context, Handler};
use calimero_app_downloader::registry::{RegistryConfig, RegistryMode};
use calimero_app_downloader::{AppRequest, Outcome};
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::messages::NetworkMessage;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_utils_actix::LazyRecipient;

mod common;

const USER_DATA: &[u8] = b"a user's blob, nothing to do with any application";

const WASM: &[u8] = b"raw wasm, not a bundle"; // raw install adopts the governance-named id rather than deriving one

/// A network that accepts every announce and counts them.
struct AnnounceCounter {
    announces: Arc<AtomicUsize>,
}

impl Actor for AnnounceCounter {
    type Context = Context<Self>;
}

impl Handler<NetworkMessage> for AnnounceCounter {
    type Result = ();

    fn handle(&mut self, msg: NetworkMessage, _ctx: &mut Context<Self>) -> Self::Result {
        if let NetworkMessage::AnnounceBlob { outcome, .. } = msg {
            let _previous = self.announces.fetch_add(1, Ordering::SeqCst);
            let _ignored = outcome.send(Ok(()));
        }
    }
}

fn counting_network() -> (
    NetworkClient,
    Arc<AtomicUsize>,
    actix::Addr<AnnounceCounter>,
) {
    let announces = Arc::new(AtomicUsize::new(0));
    let recipient = LazyRecipient::new();
    let addr = Actor::create({
        let (recipient, announces) = (recipient.clone(), Arc::clone(&announces));
        move |ctx| {
            assert!(recipient.init(ctx));
            AnnounceCounter { announces }
        }
    });
    (NetworkClient::new(recipient), announces, addr)
}

fn http(node_client: &NodeClient) -> NodeClient {
    node_client.clone().with_registry(RegistryConfig::new(
        RegistryMode::Http,
        Some("https://reg.example".parse().expect("base")),
    ))
}

/// An installed application plus one unrelated user blob, on the same node.
async fn node_with_an_installed_app(
    network: NetworkClient,
) -> (
    NodeClient,
    BlobId,
    BlobId,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let (node_client, data, blobs) = common::create_test_node_client_with(None, network).await;
    let (bundle, named_id) = common::minimal_signed_bundle_bytes("com.example.app", "1.0.0");
    let (bytecode, size) = node_client
        .add_blob(bundle.as_slice(), Some(bundle.len() as u64), None)
        .await
        .expect("store bytecode");
    let source: ApplicationSource = "https://reg.example/app.mpk".parse().expect("source");
    node_client
        .write_application_row(&named_id, &bytecode, size, &source, None)
        .expect("install the application");

    let (user_blob, _size) = node_client
        .add_blob(USER_DATA, Some(USER_DATA.len() as u64), None)
        .await
        .expect("store user data");

    (node_client, bytecode, user_blob, data, blobs)
}

#[actix::test]
async fn http_mode_refuses_to_serve_app_bytecode() {
    let (network, _announces, _peer) = counting_network();
    let (node_client, bytecode, user_blob, _data, _blobs) =
        node_with_an_installed_app(network).await;

    let http = http(&node_client);
    assert!(
        !http.may_share_blob(&bytecode).expect("gate read"),
        "an http node must not serve application bytecode to a peer"
    );
    assert!(
        http.may_share_blob(&user_blob).expect("gate read"),
        "user-data blob sharing is a different subsystem and stays open"
    );

    let dht = node_client.with_registry(RegistryConfig::new(RegistryMode::Dht, None));
    assert!(
        dht.may_share_blob(&bytecode).expect("gate read"),
        "peers are where a dht node's applications come from"
    );
}

#[actix::test]
async fn http_mode_announces_no_app_bytecode() {
    let (network, announces, _peer) = counting_network();
    let (node_client, bytecode, user_blob, _data, _blobs) =
        node_with_an_installed_app(network).await;
    let context_id = ContextId::from([0x11; 32]);

    let http = http(&node_client);
    http.announce_blob_to_network(&bytecode, &context_id, 1)
        .await
        .expect("a dropped announce is not a fault");
    assert_eq!(
        announces.load(Ordering::SeqCst),
        0,
        "an http node must not advertise itself as a source of application bytecode"
    );

    http.announce_blob_to_network(&user_blob, &context_id, 1)
        .await
        .expect("announce");
    assert_eq!(
        announces.load(Ordering::SeqCst),
        1,
        "user-data blobs are announced in either mode"
    );

    let dht = node_client.with_registry(RegistryConfig::new(RegistryMode::Dht, None));
    dht.announce_blob_to_network(&bytecode, &context_id, 1)
        .await
        .expect("announce");
    assert_eq!(
        announces.load(Ordering::SeqCst),
        2,
        "a dht node must advertise the bytecode its peers fetch from it"
    );
}

/// A node holding no application at all must not have its user blobs mistaken
/// for bytecode by the scan the gate runs.
#[actix::test]
async fn a_node_with_no_applications_shares_everything() {
    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let (user_blob, _size) = node_client
        .add_blob(USER_DATA, Some(USER_DATA.len() as u64), None)
        .await
        .expect("store user data");

    assert!(http(&node_client)
        .may_share_blob(&user_blob)
        .expect("gate read"));
    assert!(http(&node_client)
        .may_share_blob(&BlobId::from([0x99; 32]))
        .expect("gate read"));
}

/// The shape sync phase 2 finds: a row naming bytecode this node does not hold.
/// The app was built locally, so it carries no coordinates - the case where a
/// mode-blind fetch would quietly fall through to whatever peer answered.
async fn node_missing_its_bytecode(
    network: NetworkClient,
) -> (
    NodeClient,
    ApplicationId,
    BlobId,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let (node_client, data, blobs) = common::create_test_node_client_with(None, network).await;
    let named_id = ApplicationId::from([0x7A; 32]);
    let bytecode = common::blob_id_of(WASM).await;
    let source: ApplicationSource = "file:///home/dev/app.wasm".parse().expect("source");
    node_client
        .write_application_row(&named_id, &bytecode, WASM.len() as u64, &source, None)
        .expect("seed the row governance named");
    assert!(!node_client.has_blob(&bytecode).expect("lookup"));

    (node_client, named_id, bytecode, data, blobs)
}

fn acquire(application_id: ApplicationId, bytecode: BlobId, context: &ContextId) -> AppRequest<'_> {
    AppRequest {
        bytecode_id: Some(bytecode),
        application_id: Some(application_id),
        package: "",
        version: "",
        context_id: Some(context),
    }
}

// An http node must leave the serving peer untouched and a dht node must still
// reach it - a mode-blind fetch cannot satisfy both.
#[actix::test]
async fn context_bytecode_is_acquired_only_from_the_configured_source() {
    let context = ContextId::from([0x11; 32]);

    let (network, _peer, queries) =
        common::counting_peer_network(common::PeerBehavior::Serves(WASM.to_vec()));
    let (node_client, named_id, bytecode, _data, _blobs) = node_missing_its_bytecode(network).await;

    let http = http(&node_client);
    assert_eq!(
        http.acquire_bytecode(&acquire(named_id, bytecode, &context))
            .await,
        Outcome::Unavailable,
        "an uncoordinated app is not addressable in the registry, and there is \
         no second route behind it"
    );
    assert_eq!(
        queries.load(Ordering::SeqCst),
        0,
        "an http node must not ask a peer for application bytecode, even one \
         standing by to serve it"
    );
    assert!(!http.has_blob(&bytecode).expect("lookup"));

    let (network, _peer, queries) =
        common::counting_peer_network(common::PeerBehavior::Serves(WASM.to_vec()));
    let (node_client, named_id, bytecode, _data, _blobs) = node_missing_its_bytecode(network).await;

    let dht = node_client.with_registry(RegistryConfig::new(RegistryMode::Dht, None));
    assert_eq!(
        dht.acquire_bytecode(&acquire(named_id, bytecode, &context))
            .await,
        Outcome::Installed,
        "peers are where a dht node's applications come from"
    );
    assert_eq!(queries.load(Ordering::SeqCst), 1);
    assert!(dht.has_blob(&bytecode).expect("lookup"));
}
