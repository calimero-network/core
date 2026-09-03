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
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
/// (`calimero_governance_store::sign_apply_and_publish`) samples mesh peer count and best-
/// effort-publishes before/after the local store apply; both go through the
/// `LazyRecipient<NetworkMessage>`. Left uninitialised, a `send().await` on
/// that recipient queues and never resolves, deadlocking the admission task.
///
/// This stub answers every `NetworkMessage` variant with a benign default
/// (no mesh peers, no connected peers, publish "succeeds" with a dummy id) so
/// the publish path returns promptly and the local apply — the part this test
/// asserts on — actually runs. It sends nothing on the wire: there is no peer.
struct StubNetworkActor {
    /// Every peer an outbound sync tried to open a stream to, in order. Lets a
    /// test assert *which* peer a pull targeted, not merely that one happened.
    stream_opens: Arc<Mutex<Vec<libp2p::PeerId>>>,
    /// Raw payload of every gossipsub publish, in order, so a test can decode
    /// what this node actually put on the wire.
    publishes: Arc<Mutex<Vec<Vec<u8>>>>,
}

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
        use calimero_network_primitives::network_status::{
            AutonatEntry, NetworkStatusSnapshot, ReachabilityKind,
        };
        // EVERY variant is answered, and the match is deliberately exhaustive.
        //
        // `NetworkClient` resolves most of these with
        // `.expect("Mailbox not to be dropped")`, so dropping a sender does not
        // surface as a tidy `MailboxError` — it PANICS the caller. An unhandled
        // variant therefore does not make a code path merely untested, it makes
        // it untestable: the handler dies before reaching its own logic.
        //
        // That is not hypothetical. This arm list previously stopped at the
        // paths one set of tests happened to touch, with a comment asserting
        // none of the others were reached. `delete_context` calls `unsubscribe`
        // as its first act, so it could not be driven by any test at all — and a
        // real bug shipped on that handler, found by review rather than by a
        // test, because no test could reach it.
        //
        // Exhaustive, so adding a `NetworkMessage` variant fails to compile here
        // instead of quietly re-arming the trap. `let _ =` tolerates a caller
        // that already stopped awaiting.
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
            NetworkMessage::Publish { request, outcome } => {
                self.publishes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(request.data);
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
            // Record the target, then fail the open: there is no transport, so
            // the caller's best-effort sync ends promptly. `NetworkClient::
            // open_stream` *expects* on the oneshot, so the error must be sent
            // rather than the sender dropped. The record is the assertion.
            NetworkMessage::OpenStream { request, outcome } => {
                self.stream_opens
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(request.0);
                let _ = outcome.send(Err(eyre::eyre!("stub network: no transport")));
            }
            // Echo the topic back, mirroring `Subscribe`. `delete_context` and
            // the self-purge cascade both unsubscribe before doing anything
            // else, so this is what makes those handlers reachable at all.
            NetworkMessage::Unsubscribe { request, outcome } => {
                let _ = outcome.send(Ok(request.0));
            }
            // No transport, so these are no-ops that succeed. Reporting failure
            // would be a different lie from reporting success, and success keeps
            // best-effort callers on their normal path rather than their error
            // path — which is the one the tests here mean to exercise.
            NetworkMessage::Dial { outcome, .. }
            | NetworkMessage::ListenOn { outcome, .. }
            | NetworkMessage::Bootstrap { outcome, .. } => {
                let _ = outcome.send(Ok(()));
            }
            // Nobody is subscribed and no blob is anywhere: an empty answer, not
            // an error. A caller that treats "no peers" as a failure is exercising
            // its real degraded path.
            NetworkMessage::SubscribedPeers { outcome, .. } => {
                let _ = outcome.send(Vec::new());
            }
            NetworkMessage::QueryBlob { outcome, .. } => {
                let _ = outcome.send(Ok(Vec::new()));
            }
            NetworkMessage::RequestBlob { outcome, .. } => {
                let _ = outcome.send(Ok(None));
            }
            // No transport, so no peer holds anything. `false` (not an error)
            // is the honest answer here: the probe protocol reports a peer that
            // cannot be reached as a non-holder, and a caller searching for a
            // holder should see this stub's neighbours as simply not having it.
            NetworkMessage::ProbeBlob { outcome, .. } => {
                let _ = outcome.send(Ok(false));
            }
            // No transport, so nothing to announce to. Announcing is
            // best-effort at the call site, so the error is the honest answer
            // and is swallowed there.
            NetworkMessage::SendBlobAnnouncement { outcome, .. } => {
                let _ = outcome.send(Err(eyre::eyre!("no transport in the test harness")));
            }
            // Best-effort and already drop-tolerant on the client side, but
            // answered anyway so the exhaustive match stays honest.
            NetworkMessage::SetPeerScore { outcome, .. } => {
                let _ = outcome.send(());
            }
            // A snapshot of a node with no transport: itself, reachable from
            // nowhere.
            NetworkMessage::NetworkStatus { outcome, .. } => {
                let _ = outcome.send(NetworkStatusSnapshot {
                    local_peer_id: libp2p::PeerId::random(),
                    listen_addrs: Vec::new(),
                    external_addrs: Vec::new(),
                    relays: Vec::new(),
                    rendezvous: Vec::new(),
                    direct_upgrades: Vec::new(),
                    autonat: AutonatEntry {
                        reachability: ReachabilityKind::Unknown,
                        last_test: None,
                    },
                });
            }
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
    /// Gossipsub payloads this node published. See [`StubNetworkActor`].
    pub(crate) publishes: Arc<Mutex<Vec<Vec<u8>>>>,
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

    let node_state = NodeState::new();

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

    let publishes: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));

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
    // The stub needs somewhere to record stream opens; no test reads them back
    // today, so this sink is deliberately not surfaced on `TestNode`.
    let stub_opens: Arc<Mutex<Vec<libp2p::PeerId>>> = Arc::new(Mutex::new(Vec::new()));
    let stub_publishes = publishes.clone();
    let _network_addr = Actor::start_in_arbiter(&arb3, move |ctx| {
        assert!(network_recipient.init(ctx), "network recipient");
        StubNetworkActor {
            stream_opens: stub_opens,
            publishes: stub_publishes,
        }
    });

    sleep(Duration::from_millis(50)).await;

    TestNode {
        _pool: pool,
        _tmp: tmp,
        store,
        context_client,
        node_client,
        node_addr,
        publishes,
    }
}

/// Every `boot_test_node()` call site must carry `#[serial(boot_test_node)]`.
///
/// A boot rebinds process-global singletons (the `op_events` bridges, the
/// TEE-admit subscriber), so an unannotated one does not fail on itself: it
/// silently steals a concurrently running module's event stream mid-assertion.
/// Scanning source text rather than the compiled crate is deliberate, so the
/// `mock-attestation`-gated modules are covered by an ungated `cargo test` too.
/// The lint above fails CI, so a signature shape it cannot parse reports a
/// correctly-annotated test as an offender. Every shape that reaches the
/// enclosing `async fn` must find the same attribute block.
#[test]
fn attribute_block_survives_visibility_modifiers_and_indentation() {
    for signature in [
        "async fn t()",
        "pub async fn t()",
        "pub(crate) async fn t()",
        "    async fn t()",
        "    pub(super) async fn t()",
    ] {
        let head = format!(
            "mod outer {{\n\n#[serial(boot_test_node)]\n{signature} {{\n    boot_test_node().await"
        );
        assert!(
            enclosing_attribute_block(&head).contains("#[serial(boot_test_node)]"),
            "signature {signature:?} hid its attribute block"
        );
    }
}

#[test]
fn every_boot_test_node_call_site_is_serialized() {
    let mut sources = Vec::new();
    collect_rs_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    assert!(!sources.is_empty(), "found no sources to scan");

    let mut offenders = Vec::new();
    for path in sources {
        // This file carries the pattern as a search needle, not as a call.
        if path.ends_with(file!()) {
            continue;
        }
        let text = fs::read_to_string(&path).expect("read source");
        for (idx, _) in text.match_indices("boot_test_node().await") {
            let head = &text[..idx];
            if !enclosing_attribute_block(head).contains("#[serial(boot_test_node)]") {
                offenders.push(format!(
                    "{}:{}",
                    path.display(),
                    head.matches('\n').count() + 1
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "boot_test_node() without #[serial(boot_test_node)]: {offenders:#?}"
    );
}

/// The doc comment and attributes above the `async fn` that encloses the end of
/// `head` - the span between the blank line above the signature and the
/// signature itself.
///
/// Anchored on the signature's own LINE, not on a newline immediately before
/// `async fn`: a visibility modifier or an indented (nested-module) signature
/// puts other bytes there, and a needle carrying its own `\n` silently matched
/// nothing and reported the call site as an offender.
fn enclosing_attribute_block(head: &str) -> &str {
    let Some(keyword) = head.rfind("async fn ") else {
        return "";
    };
    let sig = head[..keyword].rfind('\n').map_or(0, |i| i + 1);
    let block_start = head[..sig].rfind("\n\n").map_or(0, |i| i + 1);
    &head[block_start..sig]
}

fn collect_rs_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rs_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}
