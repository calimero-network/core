//! Shared in-process node harness for the crate's node-level e2e test modules.
//!
//! Boots a real `ContextManager` + `NodeManager` (plus `SyncManager` and a
//! stub network actor) over an in-memory store and a tempdir blobstore, with
//! no libp2p transport wired up.
//!
//! This module is deliberately **feature-ungated** (`#[cfg(test)]` only): it
//! contains no mock-attestation code and must stay that way. It is shared by
//! `local_governance_node_e2e` (which *is* gated behind `mock-attestation`,
//! for its mock-quote admission tests) and `cascade_dispatch_e2e` (which is
//! not a mock test and runs in the default `cargo test`). Keep mock-quote
//! minting and any `calimero_tee_attestation` mock symbols out of here —
//! adding one would silently drag `cascade_dispatch_e2e` back behind the
//! feature gate.
use std::sync::Arc;
use std::time::Duration;

use actix::Actor;
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_context::ContextManager;
use calimero_context_client::client::ContextClient;
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::messages::MessageId;
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_node_primitives::messages::NodeMessage;
use calimero_node_primitives::NodeMode;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use prometheus_client::registry::Registry;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};
use tokio::time::sleep;

use crate::arbiter_pool::ArbiterPool;
use crate::sync::{SyncConfig, SyncManager};
use crate::{NodeManager, NodeState};

/// Minimal stand-in for the real network actor. The governance publish path
/// (`group_store::sign_apply_and_publish`) samples mesh peer count and best-
/// effort-publishes before/after the local store apply; both go through the
/// `LazyRecipient<NetworkMessage>`. Left uninitialised, a `send().await` on
/// that recipient queues and never resolves, deadlocking the admission task.
///
/// This stub answers every `NetworkMessage` variant with a benign default
/// (no mesh peers, no connected peers, publish "succeeds" with a dummy id) so
/// the publish path returns promptly and the local apply — the part this test
/// asserts on — actually runs. It sends nothing on the wire: there is no peer.
struct StubNetworkActor;

impl actix::Actor for StubNetworkActor {
    type Context = actix::Context<Self>;
}

impl actix::Handler<calimero_network_primitives::messages::NetworkMessage> for StubNetworkActor {
    type Result = ();

    fn handle(
        &mut self,
        msg: calimero_network_primitives::messages::NetworkMessage,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // `MessageId` is already in scope from the module-level import; only
        // `NetworkMessage` needs bringing in here for the match arms below.
        use calimero_network_primitives::messages::NetworkMessage;
        // The admission publish path only samples mesh/peer state and
        // best-effort-publishes. Resolve those `outcome` oneshots with a
        // benign default so the awaiting client future completes; drop every
        // other variant (none are reached by the paths under test, and a
        // dropped receiver simply surfaces `MailboxError::Closed`). `let _ =`
        // tolerates a caller that already stopped awaiting.
        match msg {
            NetworkMessage::MeshPeerCount { outcome, .. } => {
                let _ = outcome.send(0);
            }
            NetworkMessage::MeshPeers { outcome, .. } => {
                let _ = outcome.send(Vec::new());
            }
            NetworkMessage::MeshStats { outcome, .. } => {
                let _ = outcome.send(Vec::new());
            }
            NetworkMessage::PeerCount { outcome, .. } => {
                let _ = outcome.send(0);
            }
            NetworkMessage::Publish { outcome, .. } => {
                let _ = outcome.send(Ok(MessageId(b"stub".to_vec())));
            }
            // The create-group path subscribes to the namespace governance
            // topic before publishing GroupCreated; echo the requested topic
            // back so `NetworkClient::subscribe` resolves instead of panicking
            // on a dropped mailbox.
            NetworkMessage::Subscribe { request, outcome } => {
                let _ = outcome.send(Ok(request.0));
            }
            // Lazy upgrades announce each rung blob on the DHT; the stub
            // acknowledges so the awaiting client future completes.
            NetworkMessage::AnnounceBlob { outcome, .. } => {
                let _ = outcome.send(Ok(()));
            }
            _ => {}
        }
    }
}

/// Bundle of resources kept alive for the duration of a test — dropping
/// `_tmp` or `_pool` would tear down the blobstore / arbiters underneath
/// the running actors.
// Visibility note: this struct (and `boot_test_node` below) are
// `pub(crate)` so the sibling `cascade_dispatch_e2e` test module can
// share the same actor harness without duplicating ~120 LOC of
// `ContextManager` + `NodeManager` boot machinery. The fields it
// reads (`store`, `context_client`) are likewise `pub(crate)`.
pub(crate) struct TestNode {
    _pool: ArbiterPool,
    _tmp: TempDir,
    pub(crate) store: Store,
    pub(crate) context_client: ContextClient,
    /// Blob/network client for tests that need to seed real blob bytes
    /// (e.g. the cascade tests' ABI-bearing bytecode fixtures).
    pub(crate) node_client: NodeClient,
    /// Address of the running `NodeManager` actor. Lets a test deliver a
    /// synthesized `NetworkEvent` straight to the production
    /// `Handler<NetworkEvent>` dispatch (the same entrypoint a real
    /// gossipsub message takes), exercising the network-event → admission
    /// path without standing up a libp2p transport.
    /// Justification for the `dead_code` allow: this field is *read* only by
    /// `local_governance_node_e2e` (gated behind `mock-attestation`), so the
    /// default build sees no reader. It must still be held regardless of
    /// feature: dropping the last `Addr<NodeManager>` stops the actor, which
    /// would tear the node down under the ungated `cascade_dispatch_e2e`
    /// tests. Keeping it is load-bearing, not vestigial.
    #[cfg_attr(not(feature = "mock-attestation"), allow(dead_code))]
    pub(crate) node_addr: actix::Addr<NodeManager>,
}

/// Boots a `ContextManager` + `NodeManager` against an in-memory store and
/// a tempdir-backed blobstore, with no peer wired up (the network client's
/// recipient is a never-initialised `LazyRecipient`, so any outbound op
/// publish becomes a local-only apply). Sufficient for governance handlers
/// that just need the actor mailbox and the datastore.
pub(crate) async fn boot_test_node() -> TestNode {
    let mut pool = ArbiterPool::new().await.expect("arbiter pool");
    let tmp = tempfile::tempdir().expect("tempdir");

    let db = InMemoryDB::owned();
    let store = Store::new(Arc::new(db));

    let blob_store_config =
        BlobStoreConfig::new(tmp.path().to_path_buf().try_into().expect("utf8 blob path"));
    let file_system = FileSystem::new(&blob_store_config).await.expect("blob fs");
    let blob_store = BlobStore::new(store.clone(), file_system);
    let blob_manager = BlobManager::new(blob_store.clone());

    let node_recipient = LazyRecipient::<NodeMessage>::new();
    let context_recipient = LazyRecipient::new();
    let network_recipient = LazyRecipient::new();

    let network_client = NetworkClient::new(network_recipient.clone());
    let (event_sender, _) = broadcast::channel(16);
    let (ctx_sync_tx, ctx_sync_rx) = mpsc::channel(64);
    let (ns_sync_tx, ns_sync_rx) = mpsc::channel(16);
    let (ns_join_tx, ns_join_rx) = mpsc::channel(16);
    let (open_subgroup_join_tx, open_subgroup_join_rx) = mpsc::channel(16);

    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

    let node_client = NodeClient::new(
        store.clone(),
        blob_manager.clone(),
        network_client.clone(),
        node_recipient.clone(),
        event_sender,
        sync_client,
        String::new(),
        None,
    );

    let context_client = ContextClient::new(
        store.clone(),
        node_client.clone(),
        context_recipient.clone(),
    );

    let mut registry = Registry::default();
    // These node-e2e fixtures assert the *legacy* cascade write-gate behaviour
    // (an InProgress upgrade freezes state-op writes). PR-6b flipped the
    // `migration_v2` default ON (no freeze + absorb-don't-drop), so pin the
    // flag OFF here to keep exercising the legacy gate; the new default is
    // covered by the absorb tests and the migration e2e scenarios.
    let context_manager = ContextManager::new(
        store.clone(),
        node_client.clone(),
        context_client.clone(),
        Some(&mut registry),
    )
    .with_migration_v2(false);

    let node_state = NodeState::new(false, NodeMode::Standard);

    let mut sync_manager = SyncManager::new(
        SyncConfig::default(),
        node_client.clone(),
        context_client.clone(),
        network_client.clone(),
        node_state.clone(),
        ctx_sync_rx,
        ns_sync_rx,
        ns_join_rx,
        open_subgroup_join_rx,
    );

    let state_delta_arbiter = pool.get().await.expect("state-delta arbiter");
    let state_delta_tx = crate::state_delta_bridge::start_state_delta_actor(
        &state_delta_arbiter,
        crate::state_delta_bridge::STATE_DELTA_CHANNEL_CAPACITY,
    );

    let sync_session_arbiter = pool.get().await.expect("sync-session arbiter");
    let (session_result_tx, session_result_rx) = tokio::sync::mpsc::unbounded_channel();
    let sync_session_tx = crate::sync_session_bridge::start_sync_session_actor(
        &sync_session_arbiter,
        crate::sync_session_bridge::SYNC_SESSION_CHANNEL_CAPACITY,
        SyncConfig::default().max_concurrent,
        sync_manager.clone(),
        SyncConfig::default().session_deadline,
        Some(session_result_tx),
        &mut registry,
    );
    sync_manager.set_session_handles(sync_session_tx.clone(), session_result_rx);

    let node_manager = NodeManager::new(
        blob_store,
        sync_manager,
        context_client.clone(),
        node_client.clone(),
        store.clone(),
        node_state,
        state_delta_tx,
        sync_session_tx,
        prometheus_client::metrics::counter::Counter::default(),
    );

    let arb = pool.get().await.expect("arbiter");
    let _context_addr = Actor::start_in_arbiter(&arb, move |ctx| {
        assert!(context_recipient.init(ctx), "context recipient");
        context_manager
    });

    let arb2 = pool.get().await.expect("arbiter 2");
    let node_addr = Actor::start_in_arbiter(&arb2, move |ctx| {
        assert!(node_recipient.init(ctx), "node recipient");
        node_manager
    });

    // Wire the network recipient to a stub so the governance publish path
    // (mesh sampling + best-effort publish) resolves instead of deadlocking
    // on an uninitialised `LazyRecipient`. See `StubNetworkActor`.
    let arb3 = pool.get().await.expect("arbiter 3");
    let _network_addr = Actor::start_in_arbiter(&arb3, move |ctx| {
        assert!(network_recipient.init(ctx), "network recipient");
        StubNetworkActor
    });

    sleep(Duration::from_millis(50)).await;

    TestNode {
        _pool: pool,
        _tmp: tmp,
        store,
        context_client,
        node_client,
        node_addr,
    }
}
