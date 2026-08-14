//! Shared scaffolding for the JSON-RPC handler tests.
//!
//! Builds a `ServiceState` over an in-memory store so a handler can be driven
//! directly (`Request::handle`) without standing up the HTTP stack, and seeds
//! the context-membership rows the auth gate reads.

use std::sync::Arc;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_context_client::client::ContextClient;
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_node_primitives::messages::NodeMessage;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::NodeEvent;
use calimero_primitives::identity::PublicKey;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

use super::ServiceState;

/// A `ServiceState` wired to an in-memory store, plus the store itself (so a
/// test can seed membership rows) and the `TempDir` backing the blob store
/// (dropping it would delete the directory out from under the state).
pub(crate) struct TestState {
    pub(crate) state: Arc<ServiceState>,
    pub(crate) store: Store,
    _blob_dir: TempDir,
}

/// Build a `ServiceState` over a fresh in-memory store.
///
/// `auth_enabled` mirrors the real flag: `false` is the intentional no-auth
/// deployment, `true` means the auth guard is expected to inject an identity
/// extension on every request.
///
/// `node_manager` is the recipient the `NodeClient` routes actor messages to.
/// Pass an uninitialized `LazyRecipient::new()` for tests that never reach the
/// node actor; pass one initialized by a stub actor when they do.
pub(crate) async fn state_with(
    auth_enabled: bool,
    node_manager: LazyRecipient<NodeMessage>,
) -> TestState {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let blob_dir = TempDir::new().expect("tempdir");
    let blob_store = BlobStore::new(
        store.clone(),
        FileSystem::new(&BlobStoreConfig::new(
            blob_dir.path().to_path_buf().try_into().expect("utf8 path"),
        ))
        .await
        .expect("blob fs"),
    );
    let blob_manager = BlobManager::new(blob_store);

    let (event_sender, _) = broadcast::channel::<NodeEvent>(16);
    let (ctx_sync_tx, _r0) = mpsc::channel(8);
    let (ns_sync_tx, _r1) = mpsc::channel(8);
    let (ns_join_tx, _r2) = mpsc::channel(8);
    let (open_subgroup_join_tx, _r3) = mpsc::channel(8);
    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

    let node_client = NodeClient::new(
        store.clone(),
        blob_manager,
        NetworkClient::new(LazyRecipient::new()),
        node_manager,
        event_sender,
        sync_client,
        None,
    );
    let ctx_client = ContextClient::new(store.clone(), node_client.clone(), LazyRecipient::new());

    TestState {
        state: Arc::new(ServiceState {
            ctx_client,
            node_client,
            auth_enabled,
        }),
        store,
        _blob_dir: blob_dir,
    }
}

/// Stand-in for `NodeManager`, answering only the messages the presence
/// handlers send. Anything else is dropped — its `oneshot` sender goes with
/// it, so a caller sees a closed channel rather than hanging.
///
/// Must be created inside an actix System (`#[actix::test]`).
pub(crate) struct StubNodeManager {
    /// The reply to every `GetEphemeralSnapshot`.
    pub(crate) snapshot: Vec<(PublicKey, Vec<u8>, u64)>,
}

impl actix::Actor for StubNodeManager {
    type Context = actix::Context<Self>;
}

impl actix::Handler<NodeMessage> for StubNodeManager {
    type Result = ();

    fn handle(&mut self, msg: NodeMessage, _ctx: &mut Self::Context) {
        if let NodeMessage::GetEphemeralSnapshot { outcome, .. } = msg {
            let _ignored = outcome.send(self.snapshot.clone());
        }
    }
}

/// Start a [`StubNodeManager`] and hand back the recipient wired to it.
pub(crate) fn stub_node_manager(
    snapshot: Vec<(PublicKey, Vec<u8>, u64)>,
) -> LazyRecipient<NodeMessage> {
    use actix::Actor;

    let recipient = LazyRecipient::<NodeMessage>::new();
    let handle = recipient.clone();
    let _addr = StubNodeManager::create(move |ctx| {
        assert!(handle.init(ctx), "node manager recipient init");
        StubNodeManager { snapshot }
    });
    recipient
}

/// Make `public_key` a member of `context_id` without an owned private key.
///
/// This is the row `ContextClient::has_member` reads on its fast path, so the
/// key passes the membership gate — while still resolving to *no owned
/// identity*, which keeps a handler from running off into the node actor.
pub(crate) fn seed_context_member(store: &Store, context_id: ContextId, public_key: PublicKey) {
    let key = calimero_store::key::ContextIdentity::new(context_id, public_key);
    let value = calimero_store::types::ContextIdentity { private_key: None };
    store.handle().put(&key, &value).expect("put identity");
}
