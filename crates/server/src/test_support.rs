//! Test scaffolding shared across the server's transports.
//!
//! Lives at the crate root rather than under one transport because the same
//! node-actor stub backs the WS, SSE, and JSON-RPC tests: all three read the
//! node's ephemeral-presence snapshot, and none of them should stand up a real
//! `NodeManager` to do it.

use actix::{Actor, Context as ActixContext, Handler};
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_node_primitives::messages::NodeMessage;
use calimero_primitives::events::NodeEvent;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

/// One live presence entry as the node's awareness store reports it:
/// `(author, slice, age_ms)`.
pub(crate) type SnapshotEntry = (PublicKey, Vec<u8>, u64);

/// Stand-in for `NodeManager`, answering only the messages the presence paths
/// send. Anything else is dropped — its `oneshot` sender goes with it, so a
/// caller sees a closed channel rather than hanging.
///
/// Must be created inside an actix System (`#[actix::test]`).
pub(crate) struct StubNodeManager {
    /// The reply to every `GetEphemeralSnapshot`.
    snapshot: Vec<SnapshotEntry>,
    /// An event to broadcast *while* a snapshot request is being served, and
    /// before it is answered.
    ///
    /// This is how a test pins the subscribe/replay interleaving: the server
    /// must have the subscription live before it reads the snapshot, so an
    /// event emitted at exactly this moment has to reach the subscriber. If
    /// the order were ever flipped to snapshot-then-subscribe, this event would
    /// fall in the gap and the test that waits for it would fail.
    interleaved: Option<(broadcast::Sender<NodeEvent>, NodeEvent)>,
}

impl Actor for StubNodeManager {
    type Context = ActixContext<Self>;
}

impl Handler<NodeMessage> for StubNodeManager {
    type Result = ();

    fn handle(&mut self, msg: NodeMessage, _ctx: &mut Self::Context) {
        if let NodeMessage::GetEphemeralSnapshot { outcome, .. } = msg {
            if let Some((sender, event)) = &self.interleaved {
                // A send with no receivers is not an error the test cares
                // about — the assertion is on what the subscriber sees.
                let _ignored = sender.send(event.clone());
            }
            let _ignored = outcome.send(self.snapshot.clone());
        }
    }
}

/// Start a [`StubNodeManager`] answering snapshot reads with `snapshot`, and
/// hand back the recipient wired to it.
pub(crate) fn stub_node_manager(snapshot: Vec<SnapshotEntry>) -> LazyRecipient<NodeMessage> {
    stub_node_manager_full(snapshot, None)
}

/// As [`stub_node_manager`], but broadcasts `event` on `sender` while serving
/// each snapshot read — see [`StubNodeManager::interleaved`].
pub(crate) fn stub_node_manager_interleaving(
    snapshot: Vec<SnapshotEntry>,
    sender: broadcast::Sender<NodeEvent>,
    event: NodeEvent,
) -> LazyRecipient<NodeMessage> {
    stub_node_manager_full(snapshot, Some((sender, event)))
}

fn stub_node_manager_full(
    snapshot: Vec<SnapshotEntry>,
    interleaved: Option<(broadcast::Sender<NodeEvent>, NodeEvent)>,
) -> LazyRecipient<NodeMessage> {
    let recipient = LazyRecipient::<NodeMessage>::new();
    let handle = recipient.clone();
    let _addr = StubNodeManager::create(move |ctx| {
        assert!(handle.init(ctx), "node manager recipient init");
        StubNodeManager {
            snapshot,
            interleaved,
        }
    });
    recipient
}

/// Make `public_key` a member of `context_id` without an owned private key.
///
/// This is the row `ContextClient::has_member` reads on its fast path, so the
/// key passes the membership gate — while still resolving to *no owned
/// identity*, which keeps a handler from running off into the node actor.
pub(crate) fn seed_context_member(
    store: &calimero_store::Store,
    context_id: calimero_primitives::context::ContextId,
    public_key: PublicKey,
) {
    let key = calimero_store::key::ContextIdentity::new(context_id, public_key);
    let value = calimero_store::types::ContextIdentity { private_key: None };
    store.handle().put(&key, &value).expect("put identity");
}

/// A `NodeClient` over `store`, routing actor messages to `node_manager` and
/// publishing node events on `event_sender`.
///
/// The returned `TempDir` backs the blob store and must outlive the client —
/// dropping it deletes the directory out from under it.
pub(crate) async fn test_node_client(
    store: &Store,
    node_manager: LazyRecipient<NodeMessage>,
    event_sender: broadcast::Sender<NodeEvent>,
) -> (NodeClient, TempDir) {
    let blob_dir = TempDir::new().expect("tempdir");
    let blob_store = BlobStore::new(
        store.clone(),
        FileSystem::new(&BlobStoreConfig::new(
            blob_dir.path().to_path_buf().try_into().expect("utf8 path"),
        ))
        .await
        .expect("blob fs"),
    );

    let (ctx_sync_tx, _r0) = mpsc::channel(8);
    let (ns_sync_tx, _r1) = mpsc::channel(8);
    let (ns_join_tx, _r2) = mpsc::channel(8);
    let (open_subgroup_join_tx, _r3) = mpsc::channel(8);
    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

    let node_client = NodeClient::new(
        store.clone(),
        BlobManager::new(blob_store),
        NetworkClient::new(LazyRecipient::new()),
        node_manager,
        event_sender,
        sync_client,
        None,
    );

    (node_client, blob_dir)
}
