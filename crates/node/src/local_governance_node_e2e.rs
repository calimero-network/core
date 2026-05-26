//! `ContextClient::apply_signed_group_op` → `group_store`.
//!
//! Complements `calimero-context` store-only tests and `calimero-network` gossipsub tests.
use std::sync::Arc;
use std::time::Duration;

use actix::Actor;
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_context::group_store::{apply_local_signed_group_op, get_local_gov_nonce};
use calimero_context::ContextManager;
use calimero_context_client::client::ContextClient;
use calimero_context_client::group::SetMemberAutoFollowRequest;
use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::messages::{IdentTopic, Message, MessageId, NetworkEvent};
use calimero_network_primitives::specialized_node_invite::SpecializedNodeType;
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_node_primitives::messages::NodeMessage;
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_node_primitives::NodeMode;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::db::InMemoryDB;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use prometheus_client::registry::Registry;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
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
            _ => {}
        }
    }
}

fn sample_meta(admin: PublicKey) -> GroupMetaValue {
    GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
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
    /// Address of the running `NodeManager` actor. Lets a test deliver a
    /// synthesized `NetworkEvent` straight to the production
    /// `Handler<NetworkEvent>` dispatch (the same entrypoint a real
    /// gossipsub message takes), exercising the network-event → admission
    /// path without standing up a libp2p transport.
    node_addr: actix::Addr<NodeManager>,
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
    let context_manager = ContextManager::new(
        store.clone(),
        node_client.clone(),
        context_client.clone(),
        Some(&mut registry),
    );

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
        node_client,
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
        node_addr,
    }
}

#[tokio::test]
async fn apply_signed_group_op_via_context_client() {
    let node = boot_test_node().await;

    let mut rng = OsRng;
    let gid = ContextGroupId::from([0x77u8; 32]);
    let gid_bytes = gid.to_bytes();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    calimero_context::group_store::MetaRepository::new(&node.store)
        .save(&gid, &sample_meta(admin_pk))
        .expect("save_group_meta");
    calimero_context::group_store::MembershipRepository::new(&node.store)
        .add_member(&gid, &admin_pk, GroupMemberRole::Admin)
        .expect("add admin");

    let new_member = PrivateKey::random(&mut rng).public_key();

    let op = SignedGroupOp::sign(
        &admin_sk,
        gid_bytes,
        vec![],
        [0u8; 32],
        1,
        GroupOp::MemberAdded {
            member: new_member,
            role: GroupMemberRole::Member,
        },
    )
    .expect("sign op");

    match node
        .context_client
        .apply_signed_group_op(op)
        .await
        .expect("apply")
    {
        true => {}
        false => panic!("expected op applied immediately (no pending parents)"),
    }

    assert!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .is_member(&gid, &new_member)
            .expect("check_group_membership"),
        "member should be present after apply_signed_group_op"
    );
    assert_eq!(
        get_local_gov_nonce(&node.store, &gid, &admin_pk)
            .expect("get_local_gov_nonce")
            .expect("nonce row"),
        1
    );
}

/// Plumbing test for the synchronous-error paths in the
/// `set_member_auto_follow` actor handler. Both cases short-circuit
/// before the async `sign_apply_and_publish` block, so they don't
/// require a wired-up network actor — `ActorResponse::reply(Err(...))`
/// returns immediately:
///
/// 1. Unknown group — preflight bails with `"not found"` before any
///    signing or apply. Exercises that the request actually reaches the
///    handler and that preflight runs against the requester's view.
/// 2. Non-member target — surfaced by the handler's up-front
///    `get_group_member_role` check (`"not a member"`), giving a
///    clearer error than the apply-path bail.
///
/// The non-admin-non-self and happy-path cases are intentionally not
/// tested here: both reach the async block which awaits the network
/// actor (`mesh_peer_count_for_namespace`) — the apply-path admin-or-self
/// bail and the apply itself live inside that block. Happy-path apply
/// semantics and the admin-or-self rule are exhaustively covered by
/// `group_store::tests::auto_follow_tests` (12 cases including admin-set,
/// self-set, non-admin-rejected, and non-member-target variants).
#[tokio::test]
async fn set_member_auto_follow_handler_error_paths() {
    let node = boot_test_node().await;

    let mut rng = OsRng;
    let gid = ContextGroupId::from([0x55u8; 32]);

    let admin_sk = PrivateKey::random(&mut rng);
    let alice_sk = PrivateKey::random(&mut rng);
    let stranger = PrivateKey::random(&mut rng).public_key();

    calimero_context::group_store::MetaRepository::new(&node.store)
        .save(&gid, &sample_meta(admin_sk.public_key()))
        .unwrap();
    calimero_context::group_store::MembershipRepository::new(&node.store)
        .add_member(&gid, &admin_sk.public_key(), GroupMemberRole::Admin)
        .unwrap();
    calimero_context::group_store::MembershipRepository::new(&node.store)
        .add_member(&gid, &alice_sk.public_key(), GroupMemberRole::Member)
        .unwrap();

    // Admin needs a signing key registered so preflight can resolve one
    // when admin acts as requester.
    calimero_context::group_store::SigningKeysRepository::new(&node.store)
        .store_key(&gid, &admin_sk.public_key(), &admin_sk)
        .unwrap();

    // Case 1: unknown group — preflight bails before the membership
    // check, before signing, before any async work.
    let unknown_gid = ContextGroupId::from([0xEE; 32]);
    let err = node
        .context_client
        .set_member_auto_follow(SetMemberAutoFollowRequest {
            group_id: unknown_gid,
            target: alice_sk.public_key(),
            auto_follow_contexts: true,
            auto_follow_subgroups: false,
            requester: Some(admin_sk.public_key()),
        })
        .await
        .expect_err("unknown group should fail preflight");
    assert!(
        err.to_string().contains("not found"),
        "unexpected error: {err}"
    );

    // Case 2: non-member target — handler's up-front check rejects after
    // preflight but before signing. The clearer error is the whole reason
    // the handler does this ahead of the apply path's bail.
    let err = node
        .context_client
        .set_member_auto_follow(SetMemberAutoFollowRequest {
            group_id: gid,
            target: stranger,
            auto_follow_contexts: true,
            auto_follow_subgroups: false,
            requester: Some(admin_sk.public_key()),
        })
        .await
        .expect_err("stranger is not a member");
    assert!(
        err.to_string().contains("not a member"),
        "unexpected error: {err}"
    );

    // Alice's flags remain at the default produced by `add_group_member`
    // — neither failed call mutated her row. The default is
    // `{ contexts: true, subgroups: false }` per #2422.
    let alice_row = calimero_context::group_store::MembershipRepository::new(&node.store)
        .member_value(&gid, &alice_sk.public_key())
        .unwrap()
        .expect("alice row");
    assert!(alice_row.auto_follow.contexts);
    assert!(!alice_row.auto_follow.subgroups);
}

// ---------------------------------------------------------------------------
// TEE attestation announce → admission, end-to-end regression for #2441.
//
// PR #2096 published fleet `TeeAttestationAnnounce` messages on the namespace
// governance topic `ns/<hex(namespace_id)>`, but the network-event dispatcher
// stripped `group/` and so dropped every announce — `admit_tee_node` never ran
// and fleet TEE nodes were never admitted. #2441 fixed the dispatcher to
// resolve `ns/` topics. The unit tests in
// `handlers/network_event/specialized.rs` cover topic *parsing*; the tests
// below close the loop: a real `NetworkEvent::Message` carrying a borsh-encoded
// mock-attestation `TeeAttestationAnnounce` is delivered to the production
// `Handler<NetworkEvent>` (the exact entrypoint a gossipsub message takes), and
// we assert the owner admits the announcer as a `ReadOnlyTee` group member.
//
// The libp2p transport itself is the only thing not exercised here; real
// two-swarm gossipsub delivery over a live connection is covered by
// `calimero-network/tests/gossipsub_group_topic.rs`. Stubbing the wire keeps
// this test deterministic in CI while still driving announce → verify → admit
// through real actors and the real store.

/// `MOCK_TDX_QUOTE_V1` — the marker prefix that
/// `calimero_tee_attestation::is_mock_quote` matches on. Kept in lock-step
/// with `crates/tee-attestation/src/generate.rs::MOCK_QUOTE_HEADER`, which is
/// `pub` at the item level but deliberately NOT re-exported from the crate
/// root, so it is not reachable from here. Exposing it would widen a published
/// crate's public surface (consumed by mero-tee at a pinned rev), so this test
/// keeps a local copy instead. The report-data half of the quote IS built via
/// the crate's public `build_report_data`, so only the header marker is
/// duplicated. Building the mock quote bytes by hand (rather than via
/// `generate_attestation`) keeps the test platform-independent: on Linux,
/// `generate_attestation` would attempt a real TDX quote.
const MOCK_QUOTE_HEADER: &[u8] = b"MOCK_TDX_QUOTE_V1";

/// The all-zero 48-byte measurement (96 hex chars) that `create_mock_quote`
/// reports for `mrtd`/`rtmr*`. The owner's `TeeAdmissionPolicy` must allow this
/// MRTD for the mock announcer to be admitted.
const MOCK_MEASUREMENT_48_HEX: &str =
    "000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

/// Build mock TDX quote bytes that bind `nonce` and `pk_hash` into the report
/// data, matching the layout `verify_mock_attestation` expects:
/// `MOCK_QUOTE_HEADER || report_data[64] || zero-pad to 256`, where
/// `report_data = nonce[32] || pk_hash[32]`. The admission handler computes
/// `pk_hash = Sha256(public_key)` and verifies the report data against it, so
/// the caller must pass `Sha256(public_key)` here.
fn mock_quote_bytes(nonce: &[u8; 32], pk_hash: &[u8; 32]) -> Vec<u8> {
    // `nonce[32] || pk_hash[32]`, built via the crate's public helper so the
    // report-data layout stays in sync with production rather than hand-rolled.
    let report_data = calimero_tee_attestation::build_report_data(nonce, Some(pk_hash));

    let mut quote_bytes = Vec::with_capacity(256);
    quote_bytes.extend_from_slice(MOCK_QUOTE_HEADER);
    quote_bytes.extend_from_slice(&report_data);
    quote_bytes.resize(256, 0);
    quote_bytes
}

/// Borsh-encode a `TeeAttestationAnnounce` broadcast and wrap it in a
/// `NetworkEvent::Message` on `topic`, exactly as the gossipsub layer would
/// hand it to the node actor.
fn announce_network_event(
    source: libp2p::PeerId,
    topic: &str,
    quote_bytes: Vec<u8>,
    public_key: PublicKey,
    nonce: [u8; 32],
) -> NetworkEvent {
    let payload = BroadcastMessage::TeeAttestationAnnounce {
        quote_bytes,
        public_key,
        nonce,
        node_type: SpecializedNodeType::ReadOnly,
    };
    let data = borsh::to_vec(&payload).expect("borsh encode TeeAttestationAnnounce");

    NetworkEvent::Message {
        id: MessageId(b"test-announce".to_vec()),
        message: Message {
            source: Some(source),
            data,
            sequence_number: Some(1),
            topic: IdentTopic::new(topic.to_owned()).hash(),
        },
    }
}

/// Provision an owner node so it can act as a TEE-attestation verifier for the
/// namespace `gid`: store its namespace identity (so `node_namespace_identity`
/// resolves and a signing key is available), seed it as group admin, and apply
/// a mock-accepting `TeeAdmissionPolicySet` op that allowlists the mock MRTD.
/// Returns the owner's namespace public key (the admission op's signer).
fn provision_tee_owner(node: &TestNode, gid: &ContextGroupId, rng: &mut OsRng) -> PublicKey {
    let owner_sk = PrivateKey::random(rng);
    let owner_pk = owner_sk.public_key();

    calimero_context::group_store::MetaRepository::new(&node.store)
        .save(gid, &sample_meta(owner_pk))
        .expect("save_group_meta");
    calimero_context::group_store::MembershipRepository::new(&node.store)
        .add_member(gid, &owner_pk, GroupMemberRole::Admin)
        .expect("add owner admin");

    // The namespace identity is what `admit_tee_node` uses as the verifier
    // identity AND signing key. Without it the handler bails with
    // "node has no configured group identity for TEE admission".
    calimero_context::group_store::NamespaceRepository::new(&node.store)
        .store_identity(gid, &owner_pk, &owner_sk, &[0u8; 32])
        .expect("store_namespace_identity");

    // Policy lives on the namespace governance op log; admin-signed.
    let policy_op = SignedGroupOp::sign(
        &owner_sk,
        gid.to_bytes(),
        vec![],
        [0u8; 32],
        1,
        GroupOp::TeeAdmissionPolicySet {
            allowed_mrtd: vec![MOCK_MEASUREMENT_48_HEX.to_owned()],
            allowed_rtmr0: vec![],
            allowed_rtmr1: vec![],
            allowed_rtmr2: vec![],
            allowed_rtmr3: vec![],
            allowed_tcb_statuses: vec![],
            accept_mock: true,
        },
    )
    .expect("sign TeeAdmissionPolicySet");
    apply_local_signed_group_op(&node.store, &policy_op).expect("apply policy op");

    owner_pk
}

/// Poll `cond` until it returns true or the deadline elapses. The admission
/// path is spawned onto the actor's context and crosses an actor boundary
/// (`NodeManager` → `ContextManager`), so the store write lands asynchronously.
async fn wait_until<F: Fn() -> bool>(cond: F) -> bool {
    for _ in 0..100 {
        if cond() {
            return true;
        }
        sleep(Duration::from_millis(50)).await;
    }
    cond()
}

/// End-to-end #2441 regression: a `TeeAttestationAnnounce` (mock quote)
/// delivered on the `ns/<hex(namespace_id)>` topic, through the production
/// `Handler<NetworkEvent>`, drives the owner to admit the announcer as a
/// `ReadOnlyTee` group member — exactly what the `group/` vs `ns/` prefix bug
/// silently prevented. Before the fix the dispatcher dropped the announce on
/// the `ns/` topic, so `count_group_members` would stay at 1 and no
/// `ReadOnlyTee` row would ever appear; this test would time out.
#[tokio::test]
async fn ns_announce_admits_announcer_as_read_only_tee_member() {
    let node = boot_test_node().await;
    let mut rng = OsRng;

    let gid = ContextGroupId::from([0x91u8; 32]);
    let owner_pk = provision_tee_owner(&node, &gid, &mut rng);

    // Sanity: only the owner is a member before the announce.
    assert_eq!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .count(&gid)
            .expect("count"),
        1
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .is_member(&gid, &owner_pk)
            .expect("owner membership"),
        "owner must be the sole member before the announce"
    );

    // The announcing fleet TEE node.
    let announcer_pk = PrivateKey::random(&mut rng).public_key();
    let nonce = [0x7Au8; 32];
    let pk_hash: [u8; 32] = Sha256::digest(*announcer_pk).into();
    let quote_bytes = mock_quote_bytes(&nonce, &pk_hash);

    // Publish on the namespace governance topic, exactly as
    // `NodeClient::publish_on_namespace` does: `ns/<hex(namespace_id)>`.
    let topic = format!("ns/{}", hex::encode(gid.to_bytes()));
    let event = announce_network_event(
        libp2p::PeerId::random(),
        &topic,
        quote_bytes,
        announcer_pk,
        nonce,
    );

    node.node_addr
        .send(event)
        .await
        .expect("deliver NetworkEvent to node actor");

    let admitted = wait_until(|| {
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .member_value(&gid, &announcer_pk)
            .ok()
            .flatten()
            .map(|v| v.role == GroupMemberRole::ReadOnlyTee)
            .unwrap_or(false)
    })
    .await;

    assert!(
        admitted,
        "announcer must be admitted as a ReadOnlyTee member after a TeeAttestationAnnounce on the ns/ topic"
    );
    assert_eq!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .count(&gid)
            .expect("count after admit"),
        2,
        "member_count must increment to 2 (owner + admitted TEE node)"
    );
}

/// Negative guard locking in the fix: a `TeeAttestationAnnounce` arriving on a
/// legacy `group/<hex>` topic must NOT be routed into namespace admission. This
/// is the precise shape of the #2096 bug — if the dispatcher ever resurrects
/// `group/` handling for announces, the announcer would be admitted here and
/// this assertion would fail. The announce is otherwise identical (valid mock
/// quote, allowlisted MRTD), so the ONLY thing keeping the announcer out is the
/// topic-prefix routing decision under test.
#[tokio::test]
async fn group_topic_announce_is_not_routed_as_namespace_admission() {
    let node = boot_test_node().await;
    let mut rng = OsRng;

    let gid = ContextGroupId::from([0x92u8; 32]);
    let _owner_pk = provision_tee_owner(&node, &gid, &mut rng);

    let announcer_pk = PrivateKey::random(&mut rng).public_key();
    let nonce = [0x7Bu8; 32];
    let pk_hash: [u8; 32] = Sha256::digest(*announcer_pk).into();
    let quote_bytes = mock_quote_bytes(&nonce, &pk_hash);

    // Legacy `group/<hex>` topic — the buggy prefix. Same namespace id suffix.
    let topic = format!("group/{}", hex::encode(gid.to_bytes()));
    let event = announce_network_event(
        libp2p::PeerId::random(),
        &topic,
        quote_bytes,
        announcer_pk,
        nonce,
    );

    node.node_addr
        .send(event)
        .await
        .expect("deliver NetworkEvent to node actor");

    // Deterministic signal: `send().await` resolves only after the actor's
    // synchronous `Handler<NetworkEvent>` returns, and the dispatcher rejects a
    // `group/` topic *synchronously* (`parse_namespace_announce_topic` →
    // `NotNamespaceTopic`) without ever reaching the `ctx.spawn` admission path.
    // So the moment we get here, the #2096-shape regression (synchronous
    // mis-routing) is already decided — no member row can exist.
    assert!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .member_value(&gid, &announcer_pk)
            .ok()
            .flatten()
            .is_none(),
        "a group/ announce was routed into admission synchronously (the #2096 bug shape)"
    );

    // Secondary guard for a regression that instead spawned an *async*
    // admission task off this event: give any such task ample time to land a
    // row, then assert none did. (There is no positive signal to await for a
    // correctly-ignored announce without adding a test hook to production code.)
    let leaked = wait_until(|| {
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .member_value(&gid, &announcer_pk)
            .ok()
            .flatten()
            .is_some()
    })
    .await;

    assert!(
        !leaked,
        "a TeeAttestationAnnounce on a group/ topic must not be routed into namespace admission"
    );
    assert_eq!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .count(&gid)
            .expect("count"),
        1,
        "no member should be admitted from a group/ topic announce (owner only)"
    );
}
