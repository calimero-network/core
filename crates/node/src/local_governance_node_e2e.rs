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
use calimero_context_client::group::{CreateGroupRequest, SetMemberAutoFollowRequest};
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
use calimero_store::types::ApplicationMeta as ApplicationMetaValue;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use prometheus_client::registry::Registry;
use rand::rngs::OsRng;
use serial_test::serial;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};
use tokio::time::sleep;

use crate::arbiter_pool::ArbiterPool;
use crate::peer_identity_cache::ObservedMembership;
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
    /// Blob/network client for tests that need to seed real blob bytes
    /// (e.g. the cascade tests' ABI-bearing bytecode fixtures).
    pub(crate) node_client: NodeClient,
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

// These `boot_test_node`-based e2e tests share process-global state (the
// `calimero_context::tee_subgroup_admit` subscriber singleton + the
// `op_events` broadcast channel), so they must not run concurrently.
#[tokio::test]
#[serial(boot_test_node)]
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
#[serial(boot_test_node)]
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
    provision_tee_owner_with_sk(node, gid, rng).0
}

/// Same as [`provision_tee_owner`] but also returns the owner's secret key, for
/// tests that must sign a follow-up admin op as the namespace root (e.g. a
/// root `MemberRemoved` driving the TEE-eviction cascade). The public-key-only
/// `provision_tee_owner` delegates here and drops the secret key.
fn provision_tee_owner_with_sk(
    node: &TestNode,
    gid: &ContextGroupId,
    rng: &mut OsRng,
) -> (PublicKey, PrivateKey) {
    let owner_sk = PrivateKey::random(rng);
    let owner_pk = owner_sk.public_key();

    calimero_context::group_store::MetaRepository::new(&node.store)
        .save(gid, &sample_meta(owner_pk))
        .expect("save_group_meta");
    calimero_context::group_store::MembershipRepository::new(&node.store)
        .add_member(gid, &owner_pk, GroupMemberRole::Admin)
        .expect("add owner admin");

    // Faithful to the production namespace-root creation path
    // (`create_group`/`store_group_meta` both call this): a namespace root's
    // default capabilities include `CAN_JOIN_OPEN_SUBGROUPS`, so non-admin
    // members added to the root — including a `ReadOnlyTee` admitted via the
    // announce path — inherit the bit that gates membership-by-inheritance into
    // Open descendant subgroups. Without this, this shim would diverge from
    // production and a root TEE node would (incorrectly) fail to read Open
    // subgroups. `add_member` reads these defaults at add time, so it must be
    // set before any non-admin member is admitted.
    calimero_context::group_store::CapabilitiesRepository::new(&node.store)
        .set_default_capabilities(
            gid,
            calimero_context_config::MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        )
        .expect("set namespace-root default capabilities");

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

    (owner_pk, owner_sk)
}

/// Create a **Restricted** subgroup nested under `parent_ns` via the production
/// create-group entrypoint (`ContextClient::create_group`), signed by the
/// namespace admin (`admin_pk`, provisioned by `provision_tee_owner`).
///
/// Going through the real handler is load-bearing for this test: it (a) mints
/// and stores the subgroup's `GroupKeyring` key locally (so this node is the
/// key-holder), and (b) applies `RootOp::GroupCreated`, which fires
/// `OpEvent::SubgroupCreated` — the event the `tee_subgroup_admit` subscriber
/// reacts to. We deliberately do NOT hand-roll the op or hand-insert the key.
///
/// Subgroups have no explicit visibility field on `CreateGroupRequest`; subgroup
/// visibility defaults to `Restricted` when unset (see
/// `CapabilitiesRepository::subgroup_visibility`), so a freshly-created subgroup
/// is Restricted, which is exactly what this path exercises.
///
/// Returns the new subgroup's `ContextGroupId`.
async fn create_restricted_subgroup(
    node: &TestNode,
    parent_ns: &ContextGroupId,
    _admin_pk: &PublicKey,
    rng: &mut OsRng,
) -> ContextGroupId {
    // The create handler resolves the target application from the parent's
    // `target_application_id` and reads back its `ApplicationMeta` row (to
    // derive the group's `app_key` from the bytecode blob id). `sample_meta`
    // pins that id to `[0xCC; 32]`; seed a well-formed meta row for it so
    // `load_app_meta` succeeds. No real blob bytes are needed on the
    // caller-omits-app_key path (only `verify_requested_app_key` touches blobs).
    let app_id = ApplicationId::from([0xCCu8; 32]);
    let app_meta = ApplicationMetaValue::new(
        calimero_store::key::BlobMeta::new(calimero_primitives::blobs::BlobId::from([0xDDu8; 32])),
        0,
        "test://app".into(),
        Box::new([]),
        calimero_store::key::BlobMeta::new(calimero_primitives::blobs::BlobId::from([0xDDu8; 32])),
        calimero_store::types::PackageInfo {
            package: "test-package".into(),
            version: "0.0.0".into(),
            signer_id: "test-signer".into(),
        },
    );
    node.store
        .handle()
        .put(
            &calimero_store::key::ApplicationMeta::new(app_id),
            &app_meta,
        )
        .expect("seed application meta");

    let sub_gid = ContextGroupId::from(*PrivateKey::random(rng).public_key());

    let resp = node
        .context_client
        .create_group(CreateGroupRequest {
            group_id: Some(sub_gid),
            app_key: None,
            application_id: app_id,
            upgrade_policy: UpgradePolicy::Automatic,
            name: Some("restricted-sub".to_owned()),
            parent_group_id: Some(*parent_ns),
        })
        .await
        .expect("create_group");

    resp.group_id
}

/// Create an **Open** subgroup nested under `parent_ns`.
///
/// Mirrors [`create_restricted_subgroup`] (same real `ContextClient::create_group`
/// path, same `sample_meta`-derived application row), but then flips the
/// subgroup's visibility to `Open` via the production
/// `ContextClient::set_subgroup_visibility` path — a `GroupOp::SubgroupVisibilitySet`
/// applied through `sign_apply_and_publish`, admin-signed by `admin_pk`.
///
/// Visibility is NOT a field on `CreateGroupRequest`: a freshly-created subgroup
/// defaults to `Restricted` (see `CapabilitiesRepository::subgroup_visibility`).
/// To make the chain `sub → ns` genuinely Open
/// (`is_open_chain_to_namespace(&sub, &ns) == true`) we must apply the visibility
/// op after create, which is what this helper does.
///
/// Returns the new subgroup's `ContextGroupId`.
async fn create_open_subgroup(
    node: &TestNode,
    parent_ns: &ContextGroupId,
    admin_pk: &PublicKey,
    rng: &mut OsRng,
) -> ContextGroupId {
    // Reuse the Restricted-create plumbing (seeds app meta, mints the key,
    // applies `RootOp::GroupCreated`); the only difference is visibility.
    let sub_gid = create_restricted_subgroup(node, parent_ns, admin_pk, rng).await;

    node.context_client
        .set_subgroup_visibility(
            calimero_context_client::group::SetSubgroupVisibilityRequest {
                group_id: sub_gid,
                subgroup_visibility: calimero_context_config::VisibilityMode::Open,
                requester: Some(*admin_pk),
            },
        )
        .await
        .expect("set_subgroup_visibility(Open)");

    sub_gid
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
#[serial(boot_test_node)]
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

/// Precondition proof for the "Open-is-free" property: a TEE node admitted at the
/// namespace ROOT must be able to read Open subgroups WITHOUT any per-subgroup
/// admission. That holds only if the root `ReadOnlyTee` row carries
/// `CAN_JOIN_OPEN_SUBGROUPS`, because the inheritance walk in
/// `membership::check_group_membership_path` requires that capability for a
/// non-admin member to count as an inherited member of an Open descendant.
///
/// Structure: admit a TEE node at root via the announce path, create an **Open**
/// subgroup, then assert the root TEE node has NO direct row in the subgroup yet
/// IS an inherited member of it. If the `is_member` assertion fails, the root
/// admission is not granting `CAN_JOIN_OPEN_SUBGROUPS` and the Open-is-free
/// scoping decision (only Restricted subgroups need explicit TEE admission) would
/// be unsound.
#[tokio::test]
#[serial(boot_test_node)]
async fn root_admitted_tee_is_member_of_open_subgroup() {
    let node = boot_test_node().await;
    let mut rng = OsRng;

    let ns_gid = ContextGroupId::from([0x95u8; 32]);
    let owner_pk = provision_tee_owner(&node, &ns_gid, &mut rng);

    // 1) Admit a TEE node at the namespace root via the announce path.
    let tee_pk = PrivateKey::random(&mut rng).public_key();
    let nonce = [0x7Du8; 32];
    let pk_hash: [u8; 32] = Sha256::digest(*tee_pk).into();
    let quote_bytes = mock_quote_bytes(&nonce, &pk_hash);
    let topic = format!("ns/{}", hex::encode(ns_gid.to_bytes()));
    node.node_addr
        .send(announce_network_event(
            libp2p::PeerId::random(),
            &topic,
            quote_bytes,
            tee_pk,
            nonce,
        ))
        .await
        .expect("deliver announce");
    assert!(
        wait_until(
            || calimero_context::group_store::MembershipRepository::new(&node.store)
                .is_member(&ns_gid, &tee_pk)
                .unwrap_or(false)
        )
        .await,
        "TEE node must be admitted at the namespace root first"
    );

    // Shut the process-global `tee_subgroup_admit` subscriber down before
    // creating the subgroup. We are proving the *inheritance* path (Open is
    // free), so no per-subgroup direct admission must run: the subgroup is
    // created Restricted-by-default and only flipped to Open afterward, so a
    // live subscriber would race in a direct `ReadOnlyTee` row on the
    // momentarily-Restricted subgroup and mask the inheritance we assert on.
    calimero_context::tee_subgroup_admit::shutdown();

    // 2) Create an OPEN subgroup nested under the namespace.
    let open_sub = create_open_subgroup(&node, &ns_gid, &owner_pk, &mut rng).await;

    // Sanity: the subgroup chain to the namespace is genuinely Open.
    assert!(
        calimero_context::group_store::CapabilitiesRepository::new(&node.store)
            .is_open_chain_to_namespace(&open_sub, &ns_gid)
            .expect("is_open_chain_to_namespace"),
        "the created subgroup must be Open all the way to the namespace root"
    );

    // 3) The root TEE node must be an inherited member of the Open subgroup
    //    WITHOUT any direct row in it.
    assert!(
        !calimero_context::group_store::MembershipRepository::new(&node.store)
            .has_direct_member(&open_sub, &tee_pk)
            .unwrap(),
        "no direct row expected in the Open subgroup"
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .is_member(&open_sub, &tee_pk)
            .unwrap(),
        "root TEE node must be an inherited member of the Open subgroup \
         (requires CAN_JOIN_OPEN_SUBGROUPS on the root ReadOnlyTee row)"
    );
}

/// End-to-end regression for Fix B (auto-follow inheritance, Task 1): a TEE node
/// admitted at the namespace ROOT must not only *be authorized for* an Open
/// subgroup's contexts — it must actually start **replicating** them. This is the
/// behavioural pay-off of making `should_auto_follow_contexts` inheritance-aware:
/// the root `ReadOnlyTee` holds NO direct row in the Open subgroup, so before the
/// fix `decide_on_context_registered` resolved to `NotAutoFollowing` and
/// `join_context` never fired; after the fix it resolves the inheritance anchor
/// (the root row, which carries `auto_follow.contexts = true`) and auto-joins.
///
/// Structure (mirrors `root_admitted_tee_is_member_of_open_subgroup`): root-admit
/// a TEE node, create an Open subgroup it inherits into with no direct row, point
/// the node's namespace identity at that TEE, then register a context in the
/// subgroup and let the production auto-follow handler react to the
/// `OpEvent::ContextRegistered` event. Observable: the TEE node ends up with the
/// context's `ContextMeta` written locally (`has_context` is true) — the durable
/// proof that `join_context` ran and the TEE is now replicating it. Pre-fix this
/// poll would time out (no auto-join ⇒ no `ContextMeta`).
#[tokio::test]
#[serial(boot_test_node)]
async fn root_admitted_tee_auto_follows_open_subgroup_context() {
    let node = boot_test_node().await;
    let mut rng = OsRng;

    let ns_gid = ContextGroupId::from([0x96u8; 32]);
    let owner_pk = provision_tee_owner(&node, &ns_gid, &mut rng);

    // 1) Admit a TEE node at the namespace root via the announce path. Keep the
    //    TEE secret key: after the Open subgroup exists we re-point THIS node's
    //    namespace identity to the TEE (step 3b) so the auto-follow joiner IS the
    //    inherited-only TEE — the path the fix changes — rather than the owner,
    //    who is a *direct* member of the subgroup it created.
    let tee_sk = PrivateKey::random(&mut rng);
    let tee_pk = tee_sk.public_key();
    let nonce = [0x7Eu8; 32];
    let pk_hash: [u8; 32] = Sha256::digest(*tee_pk).into();
    let quote_bytes = mock_quote_bytes(&nonce, &pk_hash);
    let topic = format!("ns/{}", hex::encode(ns_gid.to_bytes()));
    node.node_addr
        .send(announce_network_event(
            libp2p::PeerId::random(),
            &topic,
            quote_bytes,
            tee_pk,
            nonce,
        ))
        .await
        .expect("deliver announce");
    assert!(
        wait_until(
            || calimero_context::group_store::MembershipRepository::new(&node.store)
                .is_member(&ns_gid, &tee_pk)
                .unwrap_or(false)
        )
        .await,
        "TEE node must be admitted at the namespace root first"
    );

    // Same race guard as the sibling membership test: shut the process-global
    // `tee_subgroup_admit` subscriber down before the (momentarily-Restricted)
    // subgroup is created, so it can't race in a DIRECT ReadOnlyTee row and mask
    // the *inheritance* path this test asserts on. We need the TEE to have NO
    // direct row in the Open subgroup for the inheritance fall-through to be the
    // thing under test.
    calimero_context::tee_subgroup_admit::shutdown();

    // 2) Create an OPEN subgroup nested under the namespace. The root TEE node is
    //    an inherited member of it with no direct row (proven by the sibling test).
    let open_sub = create_open_subgroup(&node, &ns_gid, &owner_pk, &mut rng).await;
    assert!(
        !calimero_context::group_store::MembershipRepository::new(&node.store)
            .has_direct_member(&open_sub, &tee_pk)
            .unwrap(),
        "the TEE must have NO direct row in the Open subgroup — the inheritance \
         fall-through is the path under test"
    );

    // 3) Re-point THIS node's namespace identity to the root-admitted TEE. The
    //    auto-follow gate resolves the joiner from the node's namespace identity
    //    (`resolve_identity`), and so does `join_context`. After
    //    `provision_tee_owner` that identity is the OWNER — but the owner created
    //    the subgroup and therefore holds a *direct* row in it, which would
    //    exercise the direct-member path (unaffected by the fix) instead of the
    //    inheritance fall-through. Pointing the identity at the TEE makes the
    //    joiner an inherited-only member (root ReadOnlyTee row, no subgroup row) —
    //    exactly the path Task 1 fixed. The TEE row carries the default
    //    `auto_follow.contexts = true` (set by `add_member` at admission), which
    //    the inheritance fall-through must honor via the root anchor.
    calimero_context::group_store::NamespaceRepository::new(&node.store)
        .store_identity(&ns_gid, &tee_pk, &tee_sk, &[0u8; 32])
        .expect("re-point namespace identity to the TEE");

    // 4) Bind the auto-follow handler to THIS node's store/client. Like
    //    `tee_subgroup_admit`, the handler is a process-global singleton bound to
    //    whichever node spawned it first; rebind it (shutdown + spawn) so it acts
    //    on the store we assert on. The handler subscribes to the `op_events`
    //    broadcast channel inside its spawned task, so we re-fire the registration
    //    event in the poll loop below to absorb the spawn/subscribe race.
    calimero_context::auto_follow::shutdown();
    calimero_context::auto_follow::spawn(node.store.clone(), node.context_client.clone());

    // 5) Make the subgroup's application a `calimero://` STUB so `join_context`'s
    //    `sync_context_config` bootstrap does not try to install a real bytecode
    //    blob (it would derive a different application id off the placeholder
    //    fixture and bail with "application mismatch"). A stub source is
    //    install-skipped (`is_stub`), so the bootstrap writes `ContextMeta` and
    //    completes — exactly what makes the auto-join observable. The app id is
    //    the one `sample_meta` pins on the group (`[0xCC; 32]`).
    let app_id = ApplicationId::from([0xCCu8; 32]);
    let stub_blob =
        calimero_store::key::BlobMeta::new(calimero_primitives::blobs::BlobId::from([0u8; 32]));
    let stub_meta = ApplicationMetaValue::new(
        stub_blob,
        0,
        "calimero://stub-app".into(),
        Box::new([]),
        stub_blob,
        calimero_store::types::PackageInfo {
            package: "stub-package".into(),
            version: "0.0.0".into(),
            signer_id: "stub-signer".into(),
        },
    );
    node.store
        .handle()
        .put(
            &calimero_store::key::ApplicationMeta::new(app_id),
            &stub_meta,
        )
        .expect("seed stub application meta");

    // 6) Register a context in the Open subgroup: seed the context->group mapping
    //    (the same mapping `join_context` resolves) the way the in-file resolver
    //    e2e does, then drive the production trigger by emitting the exact
    //    `OpEvent::ContextRegistered` the apply path queues
    //    (`ops/group/context_registered.rs`).
    let context_id = calimero_primitives::context::ContextId::from([0xC0u8; 32]);
    calimero_context::group_store::register_context_in_group(&node.store, &open_sub, &context_id)
        .expect("register context -> open subgroup");

    // 7) The node — now acting as the inherited-only TEE member of the Open
    //    subgroup (no direct row, root ReadOnlyTee anchor) — must auto-join the
    //    context: the inheritance-aware auto-follow gate resolves to
    //    Join, `join_context` runs, and `sync_context_config` writes the context's
    //    `ContextMeta` locally. `has_context` is the durable proof the auto-join
    //    ran. Re-fire the event each poll so a late handler subscribe still lands
    //    the trigger (re-firing is harmless — `join_context` is idempotent once
    //    the context exists). `has_member`/group membership is deliberately NOT
    //    used as the observable: it is true via the group-membership fallback
    //    regardless of whether a join actually happened.
    //
    // Pre-fix this poll times out: with no direct subgroup row the gate returned
    // `NotAutoFollowing`, `join_context` never fired, and no `ContextMeta` was
    // ever written.
    let replicating = wait_until(|| {
        calimero_governance_store::op_events::notify(
            calimero_governance_store::op_events::OpEvent::ContextRegistered {
                group_id: open_sub.to_bytes(),
                context_id,
            },
        );
        node.context_client
            .has_context(&context_id)
            .unwrap_or(false)
    })
    .await;
    assert!(
        replicating,
        "root-admitted TEE (inherited-only Open-subgroup member) must auto-follow \
         (replicate) the Open-subgroup context via the inheritance-aware auto-follow \
         gate — pre-fix it resolved to NotAutoFollowing (no direct row) and never joined"
    );
}

/// Integrated TEE-lifecycle e2e exercising BOTH shipped fixes through the real
/// governance/apply/auto-follow/cascade code with a MOCK attestation (no TDX):
///
/// * **Fix B (auto-follow inheritance):** a root-admitted `ReadOnlyTee`
///   auto-follows (replicates) an OPEN subgroup's context via the
///   inheritance-aware gate — re-confirmed here in the same node fixture that
///   also holds the cascade topology.
/// * **Fix A (scoped root-removal cascade):** a namespace-ROOT `MemberRemoved`
///   of that `ReadOnlyTee`, signed by the root owner `O`, cascades the TEE out
///   of descendant subgroups — INCLUDING a `Restricted` subgroup whose admin is
///   a DIFFERENT identity `M` than the namespace owner (the non-owner case). A
///   normal `Member` in that same Restricted subgroup is NOT cascaded (TEE-
///   scoped; Restricted autonomy / #2256 wall preserved).
///
/// Topology (single node, owns all keys):
/// ```text
///   ns root (admin = O)
///   ├── open_sub        VisibilityMode::Open   (T inherited, no direct row)
///   └── restricted_sub  VisibilityMode::Restricted (admin = M ≠ O)
///                         ├── T        ReadOnlyTee (direct)
///                         └── regular  Member      (direct)
/// ```
///
/// `T` is admitted at the root through the production mock-quote announce path
/// (real `admit_tee_node`). `open_sub` is built via the real
/// `ContextClient::create_group` + visibility-flip path. `restricted_sub` is
/// SEEDED DIRECTLY (nest + Restricted visibility + distinct-admin meta + direct
/// member rows) exactly as the gov-store cascade unit test
/// (`member_removed_root_readonly_tee_cascades_into_restricted_subgroup`) does:
/// the cascade decision keys off the ROOT removal role and the descendant's
/// direct rows, never the subgroup admin, so seeding the rows is a faithful
/// stand-in for a creator-side `tee_subgroup_admit` having admitted `T` there —
/// and it lets us pin the subgroup admin to a non-owner `M`, which the full
/// create path (which uses `sample_meta`'s owner-as-admin) cannot express.
///
/// Fix-gated assertions (would fail on the pre-fix tree):
/// * Fix B — `has_context(ctx)` true after auto-follow (pre-fix:
///   `NotAutoFollowing`, never joins ⇒ no `ContextMeta` ⇒ poll times out).
/// * Fix A — `role_of(restricted_sub, T)` is `None` after the root removal
///   (pre-fix: no cascade ⇒ row survives ⇒ still `Some(ReadOnlyTee)`), and the
///   subgroup deny-list marks `T`.
/// Control assertions (pass regardless of the fix, guarding scope/over-reach):
/// * `regular`'s row in `restricted_sub` survives the `T` removal (cascade is
///   TEE-scoped, not a blanket subtree wipe).
/// * a separate root removal of `regular` does NOT cascade into `restricted_sub`
///   (mirrors the gov-store control at the node level).
#[tokio::test]
#[serial(boot_test_node)]
async fn integrated_tee_lifecycle_open_replication_and_scoped_root_cascade() {
    let node = boot_test_node().await;
    let mut rng = OsRng;

    let ns_gid = ContextGroupId::from([0x97u8; 32]);
    // Retain the owner secret key: the root `MemberRemoved` that drives the
    // Fix-A cascade must be signed by the namespace root admin `O`.
    let (owner_pk, owner_sk) = provision_tee_owner_with_sk(&node, &ns_gid, &mut rng);

    // 1) Admit the TEE node `T` at the namespace root via the production
    //    mock-quote announce path (real `admit_tee_node`). Keep its secret key:
    //    Fix B re-points this node's namespace identity at `T` so the auto-follow
    //    joiner is the inherited-only TEE.
    let tee_sk = PrivateKey::random(&mut rng);
    let tee_pk = tee_sk.public_key();
    let nonce = [0x7Fu8; 32];
    let pk_hash: [u8; 32] = Sha256::digest(*tee_pk).into();
    let quote_bytes = mock_quote_bytes(&nonce, &pk_hash);
    let topic = format!("ns/{}", hex::encode(ns_gid.to_bytes()));
    node.node_addr
        .send(announce_network_event(
            libp2p::PeerId::random(),
            &topic,
            quote_bytes,
            tee_pk,
            nonce,
        ))
        .await
        .expect("deliver announce");
    assert!(
        wait_until(|| {
            calimero_context::group_store::MembershipRepository::new(&node.store)
                .member_value(&ns_gid, &tee_pk)
                .ok()
                .flatten()
                .map(|v| v.role == GroupMemberRole::ReadOnlyTee)
                .unwrap_or(false)
        })
        .await,
        "T must be admitted as a ReadOnlyTee at the namespace root first"
    );

    // Shut the process-global `tee_subgroup_admit` subscriber down before
    // creating the (momentarily-Restricted) Open subgroup, so it can't race in a
    // DIRECT ReadOnlyTee row and mask the *inheritance* path Fix B asserts on.
    calimero_context::tee_subgroup_admit::shutdown();

    // 2) Create the OPEN subgroup via the real create + visibility-flip path. The
    //    root TEE inherits into it with NO direct row.
    let open_sub = create_open_subgroup(&node, &ns_gid, &owner_pk, &mut rng).await;
    assert!(
        !calimero_context::group_store::MembershipRepository::new(&node.store)
            .has_direct_member(&open_sub, &tee_pk)
            .unwrap(),
        "T must have NO direct row in the Open subgroup — the inheritance \
         fall-through is the path Fix B exercises"
    );

    // 3) Seed the RESTRICTED subgroup with a DISTINCT admin `M` (≠ owner `O`),
    //    nested under the root, and give it two direct rows: `T` (ReadOnlyTee)
    //    and `regular` (Member). This is the non-owner Restricted case the
    //    gov-store cascade unit test seeds; the cascade never reads the subgroup
    //    admin, so this faithfully stands in for creator-side
    //    `tee_subgroup_admit` having admitted `T` here.
    let restricted_admin_m = PrivateKey::random(&mut rng).public_key();
    assert_ne!(
        restricted_admin_m, owner_pk,
        "the Restricted subgroup admin M must differ from the namespace owner O"
    );
    let regular = PrivateKey::random(&mut rng).public_key();
    let restricted_sub = ContextGroupId::from(*PrivateKey::random(&mut rng).public_key());

    calimero_context::group_store::NamespaceRepository::new(&node.store)
        .nest(&ns_gid, &restricted_sub)
        .expect("nest restricted_sub under root");
    calimero_context::group_store::CapabilitiesRepository::new(&node.store)
        .set_subgroup_visibility(
            &restricted_sub,
            calimero_context_config::VisibilityMode::Restricted,
        )
        .expect("set restricted visibility");
    calimero_context::group_store::MetaRepository::new(&node.store)
        .save(&restricted_sub, &sample_meta(restricted_admin_m))
        .expect("save restricted_sub meta (admin = M)");
    calimero_context::group_store::MembershipRepository::new(&node.store)
        .add_member(&restricted_sub, &restricted_admin_m, GroupMemberRole::Admin)
        .expect("add M as restricted_sub admin");
    calimero_context::group_store::MembershipRepository::new(&node.store)
        .add_member(&restricted_sub, &tee_pk, GroupMemberRole::ReadOnlyTee)
        .expect("add T as ReadOnlyTee in restricted_sub");
    calimero_context::group_store::MembershipRepository::new(&node.store)
        .add_member(&restricted_sub, &regular, GroupMemberRole::Member)
        .expect("add regular Member in restricted_sub");

    // Sanity: the seeded rows are present before any removal.
    assert_eq!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .role_of(&restricted_sub, &tee_pk)
            .unwrap(),
        Some(GroupMemberRole::ReadOnlyTee),
        "T must be a direct ReadOnlyTee member of restricted_sub before removal"
    );
    assert_eq!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .role_of(&restricted_sub, &regular)
            .unwrap(),
        Some(GroupMemberRole::Member),
        "regular must be a direct Member of restricted_sub before removal"
    );

    // -----------------------------------------------------------------
    // Fix B — Open-subgroup context replication (auto-follow inheritance).
    // -----------------------------------------------------------------

    // Re-point THIS node's namespace identity to `T` so the auto-follow joiner
    // is the inherited-only TEE (root anchor, no open_sub row) — the path Fix B
    // changed. (See `root_admitted_tee_auto_follows_open_subgroup_context`.)
    calimero_context::group_store::NamespaceRepository::new(&node.store)
        .store_identity(&ns_gid, &tee_pk, &tee_sk, &[0u8; 32])
        .expect("re-point namespace identity to T");

    // Rebind the process-global auto-follow handler to this node's store/client.
    calimero_context::auto_follow::shutdown();
    calimero_context::auto_follow::spawn(node.store.clone(), node.context_client.clone());

    // Make the subgroup app a `calimero://` STUB so `join_context`'s bootstrap
    // does not try to install a real bytecode blob (install-skipped on stubs),
    // letting the auto-join write `ContextMeta` and complete.
    let app_id = ApplicationId::from([0xCCu8; 32]);
    let stub_blob =
        calimero_store::key::BlobMeta::new(calimero_primitives::blobs::BlobId::from([0u8; 32]));
    let stub_meta = ApplicationMetaValue::new(
        stub_blob,
        0,
        "calimero://stub-app".into(),
        Box::new([]),
        stub_blob,
        calimero_store::types::PackageInfo {
            package: "stub-package".into(),
            version: "0.0.0".into(),
            signer_id: "stub-signer".into(),
        },
    );
    node.store
        .handle()
        .put(
            &calimero_store::key::ApplicationMeta::new(app_id),
            &stub_meta,
        )
        .expect("seed stub application meta");

    // Register a context in the Open subgroup and drive the production
    // auto-follow trigger; assert `T` replicates it (durable `ContextMeta`).
    let context_id = calimero_primitives::context::ContextId::from([0xC1u8; 32]);
    calimero_context::group_store::register_context_in_group(&node.store, &open_sub, &context_id)
        .expect("register context -> open subgroup");

    let replicating = wait_until(|| {
        calimero_governance_store::op_events::notify(
            calimero_governance_store::op_events::OpEvent::ContextRegistered {
                group_id: open_sub.to_bytes(),
                context_id,
            },
        );
        node.context_client
            .has_context(&context_id)
            .unwrap_or(false)
    })
    .await;
    assert!(
        replicating,
        "Fix B: root-admitted TEE (inherited-only Open-subgroup member) must \
         auto-follow (replicate) the Open-subgroup context — pre-fix it resolved \
         to NotAutoFollowing and never joined"
    );

    // -----------------------------------------------------------------
    // Fix A — namespace-root MemberRemoved of T cascades, TEE-scoped.
    // -----------------------------------------------------------------

    // Quiesce the process-global auto-follow handler before driving the cascade.
    // Fix B's `join_context` spawns background work (a `sync_context_config`
    // bootstrap) bound to this store; left running it races the cascade applies
    // and the `op_events` channel we read below. The replication assertion above
    // already proved the join happened, so we no longer need the handler live.
    calimero_context::auto_follow::shutdown();

    // Subscribe to the op-events broadcast BEFORE applying, so we can observe the
    // per-subgroup `TeeMemberRemoved` the cascade emits. (Process-global channel;
    // these tests are `#[serial]`, so we only see events from this apply.)
    let mut events = calimero_governance_store::op_events::subscribe();

    // Root admin `O` removes the TEE at the namespace root. Mirror the gov-store
    // cascade test: the signed `expected_group_state_hash` is the pre-removal
    // hash (divergence is a non-fatal detection signal; the op still applies and
    // the cascade still runs).
    let pre_hash = calimero_context::group_store::MetaRepository::new(&node.store)
        .compute_state_hash(&ns_gid)
        .expect("compute pre-removal state hash");
    // The owner already signed ops (the policy set + the TEE admission via
    // `sign_apply_and_publish`), so pick the next free nonce off the persisted
    // window floor rather than a hard-coded value, which would replay.
    let next_owner_nonce = get_local_gov_nonce(&node.store, &ns_gid, &owner_pk)
        .expect("read owner nonce floor")
        .unwrap_or(0)
        + 1;
    let remove_tee_op = SignedGroupOp::sign(
        &owner_sk,
        ns_gid.to_bytes(),
        vec![],
        pre_hash,
        next_owner_nonce,
        GroupOp::MemberRemoved {
            member: tee_pk,
            expected_group_state_hash: pre_hash,
            expected_context_state_hashes: Vec::new(),
        },
    )
    .expect("sign root MemberRemoved(T)");
    apply_local_signed_group_op(&node.store, &remove_tee_op).expect("apply root MemberRemoved(T)");

    // Fix-A core proof: T's root row AND its non-owner-Restricted-subgroup row
    // are both gone (the cascade crossed into the Restricted subgroup whose
    // admin is M ≠ O).
    assert_eq!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .role_of(&ns_gid, &tee_pk)
            .unwrap(),
        None,
        "Fix A: T's root row must be removed"
    );
    assert_eq!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .role_of(&restricted_sub, &tee_pk)
            .unwrap(),
        None,
        "Fix A: root TEE removal MUST cascade into the non-owner Restricted \
         subgroup (admin M ≠ O) — pre-fix this row survives"
    );
    // The cascade deny-lists T in the subgroup.
    assert!(
        calimero_context::group_store::DenyListRepository::new(&node.store)
            .is_denied(&restricted_sub, &tee_pk)
            .unwrap(),
        "Fix A: cascade must deny-list T in the Restricted subgroup"
    );

    // Scope guard: the regular Member is STILL a member of restricted_sub —
    // the cascade is TEE-scoped, not a blanket subtree wipe.
    assert_eq!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .role_of(&restricted_sub, &regular)
            .unwrap(),
        Some(GroupMemberRole::Member),
        "Fix A: the cascade is TEE-scoped — regular Member must survive in \
         restricted_sub (Restricted autonomy / #2256 wall preserved)"
    );

    // Observe the per-subgroup `TeeMemberRemoved` the cascade emitted. We count
    // only events for `restricted_sub` + `T`; the root also emits its own pair,
    // so we don't assert an exact channel total. Drain non-blockingly: by the
    // time `apply_local_signed_group_op` returned, every event is already queued.
    let mut saw_subgroup_tee_removed = false;
    while let Ok(ev) = events.try_recv() {
        if let calimero_governance_store::op_events::OpEvent::TeeMemberRemoved {
            group_id,
            member,
        } = ev
        {
            if group_id == restricted_sub.to_bytes() && member == tee_pk {
                saw_subgroup_tee_removed = true;
            }
        }
    }
    assert!(
        saw_subgroup_tee_removed,
        "Fix A: cascade must emit a per-subgroup TeeMemberRemoved for T in \
         restricted_sub"
    );

    // Note on the non-TEE control: the mirror-image guard — a root removal of a
    // regular `Member` does NOT cascade into a Restricted subgroup — is covered
    // exhaustively by the gov-store unit test
    // `member_removed_root_regular_member_does_not_cascade`, which seeds the same
    // topology and asserts the subgroup row survives. It is deliberately NOT
    // re-driven here: this node fixture keeps process-global background workers
    // (`auto_follow`'s `join_context` bootstrap) alive after the Fix-B join, and
    // a SECOND root apply in the same test races their in-flight namespace work
    // non-deterministically. The TEE-scoping is already proven above by the
    // surviving `regular` Member row after the TEE removal — that assertion IS
    // the node-level control (a blanket subtree wipe would have taken `regular`
    // too), so the contract is covered without a redundant, flaky second apply.
}

/// End-to-end create-time path (proposal.md §12d, Phase 1): with a TEE node
/// already admitted at the namespace root, creating a **Restricted** subgroup on
/// the same node must transparently give that TEE node a direct `ReadOnlyTee`
/// row in the subgroup AND the subgroup's key. The subgroup is created through
/// the production `ContextClient::create_group` path, so the subgroup key is
/// minted locally and `OpEvent::SubgroupCreated` fires — driving the
/// `tee_subgroup_admit` subscriber (spawned in `ContextManager::started`) to
/// admit the existing root TEE member into the new Restricted subgroup.
#[tokio::test]
#[serial(boot_test_node)]
async fn restricted_subgroup_created_admits_existing_tee_member() {
    let node = boot_test_node().await;
    let mut rng = OsRng;

    let ns_gid = ContextGroupId::from([0x93u8; 32]);
    let owner_pk = provision_tee_owner(&node, &ns_gid, &mut rng);

    // 1) Admit a TEE node at the namespace root via the announce path, exactly
    //    like `ns_announce_admits_announcer_as_read_only_tee_member`.
    let tee_pk = PrivateKey::random(&mut rng).public_key();
    let nonce = [0x7Bu8; 32];
    let pk_hash: [u8; 32] = Sha256::digest(*tee_pk).into();
    let quote_bytes = mock_quote_bytes(&nonce, &pk_hash);
    let topic = format!("ns/{}", hex::encode(ns_gid.to_bytes()));
    node.node_addr
        .send(announce_network_event(
            libp2p::PeerId::random(),
            &topic,
            quote_bytes,
            tee_pk,
            nonce,
        ))
        .await
        .expect("deliver announce");

    let admitted_root = wait_until(|| {
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .member_value(&ns_gid, &tee_pk)
            .ok()
            .flatten()
            .map(|v| v.role == GroupMemberRole::ReadOnlyTee)
            .unwrap_or(false)
    })
    .await;
    assert!(
        admitted_root,
        "TEE node must be admitted as a ReadOnlyTee at the namespace root first"
    );

    // The `tee_subgroup_admit` subscriber is a PROCESS-GLOBAL singleton
    // (`OnceLock` op-events notifier + a `Mutex<Option<_>>` spawn guard), bound
    // to whichever node booted first in the test binary. Other e2e tests in this
    // crate (`cascade_dispatch_e2e`) also `boot_test_node`, so by the time this
    // test runs the subscriber may be operating on a *different* node's store and
    // would never see THIS node's key — defeating the create-time admission.
    // Rebind the global subscriber to this test's store/client right before the
    // subgroup create so it acts on the same store we assert on. (`shutdown` +
    // `spawn` because `spawn` alone is first-wins and won't rebind.) `spawn`
    // subscribes synchronously before returning, so no sleep is needed: the
    // subscriber is guaranteed registered before the op below fires.
    calimero_context::tee_subgroup_admit::shutdown();
    calimero_context::tee_subgroup_admit::spawn(node.store.clone(), node.context_client.clone());

    // 2) Create a RESTRICTED subgroup on this node (mints + holds its key, fires
    //    OpEvent::SubgroupCreated).
    let sub_gid = create_restricted_subgroup(&node, &ns_gid, &owner_pk, &mut rng).await;

    // 3) The subscriber must admit the existing root TEE member into the new
    //    Restricted subgroup.
    let admitted_sub = wait_until(|| {
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .member_value(&sub_gid, &tee_pk)
            .ok()
            .flatten()
            .map(|v| v.role == GroupMemberRole::ReadOnlyTee)
            .unwrap_or(false)
    })
    .await;
    assert!(
        admitted_sub,
        "TEE node must gain a ReadOnlyTee row in the Restricted subgroup after creation"
    );

    // 4) And the creator node must hold the subgroup key (minted at create time
    //    and used by `admit_tee_node` to deliver to the admitted member).
    assert!(
        calimero_context::group_store::GroupKeyring::new(&node.store, sub_gid)
            .load_current_key()
            .expect("load key")
            .is_some(),
        "subgroup must have a current key on this (creator) node"
    );
}

/// End-to-end join-into-existing path (proposal.md §12d, Phase 1): a
/// **Restricted** subgroup already exists on this node (it holds the subgroup
/// key) when a TEE node is then admitted at the namespace root. An
/// `OpEvent::TeeMemberAdmitted` at the root drives the `tee_subgroup_admit`
/// subscriber's `handle_new_tee_member` (Task 4) to fan the new root TEE member
/// into every descendant Restricted subgroup whose key this node holds — here,
/// the pre-existing subgroup. This complements the create-time path proven by
/// `restricted_subgroup_created_admits_existing_tee_member`.
///
/// Race note: `handle_new_tee_member` reuses the root admission verdict via
/// `tee_admission_record`, which scans the namespace governance op-log. That
/// op-log entry is recorded *after* `apply_group_op_mutations` fires the
/// `TeeMemberAdmitted` event (see `apply_local_signed_group_op`), so the event
/// emitted *during* root admission races the op-log write. The subscriber
/// absorbs this with a bounded wake-then-reread retry (see `tee_subgroup_admit`),
/// so the production IMMEDIATE path works without any manual re-fire: this test
/// rebinds the subscriber, announces at the root, and asserts the fan-in lands
/// directly from that single root admission.
#[tokio::test]
#[serial(boot_test_node)]
async fn tee_admitted_after_restricted_subgroup_exists_is_fanned_in() {
    let node = boot_test_node().await;
    let mut rng = OsRng;

    let ns_gid = ContextGroupId::from([0x94u8; 32]);
    let owner_pk = provision_tee_owner(&node, &ns_gid, &mut rng);

    // 1) A RESTRICTED subgroup exists FIRST, before any TEE member is admitted at
    //    the root. Going through the real create handler mints + stores the
    //    subgroup key locally, so THIS node is the key-holder that
    //    `handle_new_tee_member` needs to fan a later root TEE member into it.
    let sub_gid = create_restricted_subgroup(&node, &ns_gid, &owner_pk, &mut rng).await;

    // Sanity: the subgroup key is held on this node (the fan-in delivers under it).
    assert!(
        calimero_context::group_store::GroupKeyring::new(&node.store, sub_gid)
            .load_current_key()
            .expect("load key")
            .is_some(),
        "creator node must hold the pre-existing subgroup key before root admission"
    );

    // 2) Rebind the PROCESS-GLOBAL `tee_subgroup_admit` subscriber to THIS node's
    //    store/client BEFORE the announce, so the root admission's own
    //    `TeeMemberAdmitted` event is the trigger under test — the production
    //    immediate path, not a manually-driven recovery. (`shutdown` + `spawn`
    //    because `spawn` alone is first-wins and won't rebind.)
    // `spawn` subscribes synchronously before returning, so the subscriber is
    // registered before the announce below — no sleep needed.
    calimero_context::tee_subgroup_admit::shutdown();
    calimero_context::tee_subgroup_admit::spawn(node.store.clone(), node.context_client.clone());

    // 3) Admit a TEE node at the namespace ROOT via the announce path, exactly
    //    like the sibling tests (mock quote on the `ns/<hex>` topic). This emits
    //    `OpEvent::TeeMemberAdmitted` at the root, which the (now-bound)
    //    subscriber reacts to. Its bounded wake-then-reread retry absorbs the
    //    emit-before-persist op-log race, so the fan-in lands immediately.
    let tee_pk = PrivateKey::random(&mut rng).public_key();
    let nonce = [0x7Cu8; 32];
    let pk_hash: [u8; 32] = Sha256::digest(*tee_pk).into();
    let quote_bytes = mock_quote_bytes(&nonce, &pk_hash);
    let topic = format!("ns/{}", hex::encode(ns_gid.to_bytes()));
    node.node_addr
        .send(announce_network_event(
            libp2p::PeerId::random(),
            &topic,
            quote_bytes,
            tee_pk,
            nonce,
        ))
        .await
        .expect("deliver announce");

    // 4) The single root admission must fan into the pre-existing Restricted
    //    subgroup (Task 4: OpEvent::TeeMemberAdmitted at root →
    //    handle_new_tee_member → admit into descendants we hold keys for).
    let fanned_in = wait_until(|| {
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .member_value(&sub_gid, &tee_pk)
            .ok()
            .flatten()
            .map(|v| v.role == GroupMemberRole::ReadOnlyTee)
            .unwrap_or(false)
    })
    .await;
    assert!(
        fanned_in,
        "root TEE admission must fan into the pre-existing Restricted subgroup"
    );

    // 5) The creator node still holds the subgroup key (fan-in delivers under it).
    assert!(
        calimero_context::group_store::GroupKeyring::new(&node.store, sub_gid)
            .load_current_key()
            .expect("load key")
            .is_some(),
        "subgroup must still have a current key on this (creator) node after fan-in"
    );
}

/// ACCEPTANCE GATE for bug #2848 (mock-TEE harness Task 16 / R1).
///
/// Reproduces the "stranded buffered `ContextRegistered`" bug as a deterministic,
/// in-process, store-only test. On a (re-)admitted member, a buffered encrypted
/// `GroupOp::ContextRegistered` for a Restricted subgroup is stranded forever
/// when its subgroup's `GroupCreated` applies AFTER the `KeyDelivery`:
///
/// 1. The encrypted `ContextRegistered` arrives without the subgroup key, so the
///    `NamespaceOp::Group` arm in `apply_signed_op` cannot resolve a key and
///    effect-SKIPS the op (`governance.rs:357`) — but still LOGS it to the
///    namespace op-log (`governance.rs:428`). Drop path B.
/// 2. `KeyDelivery` arrives → `apply_received_group_key` stores the key and calls
///    `retry_encrypted_ops_for_group`, which re-feeds the buffered op through
///    `decrypt_and_apply_group_op` → `apply_group_op_inner`.
/// 3. That retry FAILS at the staleness check (`governance.rs:1231-1232`):
///    `compute_state_hash(group_id)` raises `MetaError::GroupNotFoundForHash`
///    (`meta.rs:90-93`) because the subgroup's `GroupMeta` row — written only by
///    `GroupCreated` apply (`group_created.rs:98`) — is not present yet. The op
///    stays stranded.
/// 4. `KeyDelivery` is the ONLY retry trigger. Applying `GroupCreated` later does
///    NOT re-drive the buffered op, so the context never registers and
///    `OpEvent::ContextRegistered` is never emitted.
///
/// This test drives the bug-triggering ORDER (buffer → KeyDelivery → GroupCreated)
/// and asserts the OUTCOME the fix will provide: after `GroupCreated` applies, the
/// buffered op is re-driven, the context becomes registered, and
/// `OpEvent::ContextRegistered` fires.
///
/// On CURRENT master this test FAILS at step (a)/(b) — the op stays stranded.
/// That failing state IS the deliverable: it proves the harness reproduces the
/// bug. The later #2848 fix turns it green.
///
/// Store-only (no actor): the bug lives entirely in the synchronous gov-store
/// apply path, so this drives `apply_signed_namespace_op` directly. That keeps it
/// fully deterministic — no actor mailbox, no spawned task, no timing — except for
/// the bounded `op_events` drain at the end (a process-global broadcast channel).
#[tokio::test]
#[serial(boot_test_node)]
async fn restricted_ctx_redriven_after_group_created() {
    use calimero_context::group_store::{
        apply_signed_namespace_op, get_group_for_context, GroupKeyring, MembershipRepository,
        MetaRepository, NamespaceRepository,
    };
    use calimero_context_client::local_governance::{
        GroupOp, NamespaceOp, RootOp, SignedNamespaceOp,
    };

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let mut rng = OsRng;

    // ---- Namespace root provisioning (receiver side) -------------------------
    // The namespace IS its root group. `ns_gid.to_bytes()` is the namespace id.
    let ns_gid = ContextGroupId::from([0xD8u8; 32]);
    let namespace_id = ns_gid.to_bytes();

    // `owner` is the namespace-root admin AND the holder/minter of the subgroup
    // key. It signs the buffered `ContextRegistered`, the `KeyDelivery`, and the
    // `GroupCreated` (all three require/derive root-admin authority).
    let owner_sk = PrivateKey::random(&mut rng);
    let owner_pk = owner_sk.public_key();

    // `member` is THIS receiver node's namespace identity — the recipient the
    // `KeyDelivery` envelope is wrapped for, and whose `identity_record`
    // `apply_received_group_key` reads to unwrap it. Distinct from the owner.
    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();
    assert_ne!(
        member_pk, owner_pk,
        "receiver identity must differ from the owner/admin"
    );

    // Seed the namespace root meta (admin = owner) so `GroupCreated`'s parent
    // lookup (`MetaRepository::load(parent)`) succeeds and its authorization
    // (`is_admin(ns, signer=owner)`) passes.
    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta(owner_pk))
        .expect("save namespace root meta");
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &owner_pk, GroupMemberRole::Admin)
        .expect("add owner as namespace-root admin");

    // The receiver's namespace identity (member_sk). `apply_received_group_key`
    // unwraps the `KeyDelivery` envelope with THIS key, so the envelope below
    // must be wrapped for `member_pk`.
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &member_pk, &member_sk, &[0u8; 32])
        .expect("store receiver namespace identity");

    // ---- The Restricted subgroup (NOT yet created on the receiver) -----------
    // We pick its id and mint its key OWNER-side. The receiver does NOT hold the
    // key nor the subgroup meta yet — that is the whole point: the encrypted op
    // arrives before either is locally present.
    let sub_gid = ContextGroupId::from(*PrivateKey::random(&mut rng).public_key());
    let subgroup_key: [u8; 32] = {
        use rand::RngCore;
        let mut k = [0u8; 32];
        rng.fill_bytes(&mut k);
        k
    };
    let key_id = GroupKeyring::key_id_for(&subgroup_key);

    // The context the buffered op registers.
    let context_id = calimero_primitives::context::ContextId::from([0xC8u8; 32]);

    // A NON-ZERO state_hash on the buffered op is load-bearing: the receiver's
    // staleness check only calls `compute_state_hash` (which raises
    // `GroupNotFoundForHash` on a meta-absent subgroup) when
    // `state_hash != [0u8; 32]`. In production the publisher (subgroup creator)
    // DOES hold the subgroup meta and so signs a non-zero hash; this stand-in
    // reproduces that exact condition. (Any non-zero value reaches the meta load
    // before any hash comparison, so the precise bytes don't matter.)
    let nonzero_state_hash = [0x11u8; 32];

    // ---- Subscribe to op-events BEFORE driving anything ----------------------
    // Process-global broadcast; `#[serial]` keeps cross-test events out, but we
    // still filter strictly on (sub_gid, context_id) when draining at the end.
    let mut events = calimero_governance_store::op_events::subscribe();

    // ---- Step 1: buffer the encrypted ContextRegistered (effect-skipped) -----
    let inner_op = GroupOp::ContextRegistered {
        context_id,
        application_id: calimero_primitives::application::ApplicationId::from([0xCCu8; 32]),
        blob_id: calimero_primitives::blobs::BlobId::from([0xDDu8; 32]),
        source: "calimero://stub-app".to_owned(),
        service_name: None,
    };
    let encrypted = GroupKeyring::encrypt_op(&subgroup_key, &inner_op).expect("encrypt group op");

    let ctx_registered_op = SignedNamespaceOp::sign(
        &owner_sk,
        namespace_id,
        vec![],
        nonzero_state_hash,
        1,
        NamespaceOp::Group {
            group_id: sub_gid.to_bytes(),
            key_id,
            encrypted,
            key_rotation: None,
        },
    )
    .expect("sign NamespaceOp::Group(ContextRegistered)");

    apply_signed_namespace_op(&store, &ctx_registered_op)
        .expect("apply buffered ContextRegistered (effect-skipped, logged)");

    // The op is logged but its effect was skipped: no key locally, so the
    // context is NOT registered and the subgroup meta does NOT exist.
    assert_eq!(
        get_group_for_context(&store, &context_id).expect("get_group_for_context"),
        None,
        "buffered ContextRegistered must be effect-skipped before the key arrives \
         (logged-but-not-applied — drop path B)"
    );
    assert!(
        MetaRepository::new(&store)
            .load(&sub_gid)
            .expect("load subgroup meta")
            .is_none(),
        "subgroup meta must NOT exist yet (GroupCreated not applied)"
    );

    // ---- Step 2: KeyDelivery → retry fires, fails meta-absent (stranded) -----
    // Wrap the subgroup key for the receiver's namespace identity (member_pk),
    // exactly as `admit_tee_node` / `add_group_members` would.
    let envelope = GroupKeyring::wrap_for_member(&owner_sk, &member_pk, &subgroup_key)
        .expect("wrap subgroup key for receiver");

    let key_delivery_op = SignedNamespaceOp::sign(
        &owner_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        2,
        NamespaceOp::Root(RootOp::KeyDelivery {
            group_id: sub_gid.to_bytes(),
            envelope,
        }),
    )
    .expect("sign KeyDelivery");

    apply_signed_namespace_op(&store, &key_delivery_op).expect("apply KeyDelivery");

    // The key landed (so retry HAD a candidate and DID run)...
    assert!(
        GroupKeyring::new(&store, sub_gid)
            .load_key_by_id(&key_id)
            .expect("load key by id")
            .is_some(),
        "KeyDelivery must store the subgroup key (the retry trigger ran)"
    );
    // ...but the retry FAILED at the staleness check (subgroup meta absent →
    // GroupNotFoundForHash), so the context is STILL not registered: stranded.
    // (Watch the test log for "group not found for state hash computation" /
    // "failed to retry encrypted op after KeyDelivery" — that proves the retry
    // ran and bailed for the RIGHT reason.)
    assert_eq!(
        get_group_for_context(&store, &context_id).expect("get_group_for_context"),
        None,
        "after KeyDelivery the retry must fail (subgroup meta absent) and leave \
         the ContextRegistered stranded — this is the #2848 trap"
    );

    // ---- Step 3: GroupCreated for the subgroup applies LAST ------------------
    // On master this does NOT re-drive the stranded op. The #2848 fix makes
    // GroupCreated re-trigger the buffered-op retry.
    let group_created_op = SignedNamespaceOp::sign(
        &owner_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        3,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sub_gid.to_bytes(),
            parent_id: namespace_id,
        }),
    )
    .expect("sign GroupCreated");

    apply_signed_namespace_op(&store, &group_created_op).expect("apply GroupCreated");

    // Sanity: GroupCreated itself landed (subgroup meta now present).
    assert!(
        MetaRepository::new(&store)
            .load(&sub_gid)
            .expect("load subgroup meta")
            .is_some(),
        "GroupCreated must have written the subgroup meta row"
    );

    // ---- (a) the previously-buffered ContextRegistered is now applied --------
    assert_eq!(
        get_group_for_context(&store, &context_id).expect("get_group_for_context"),
        Some(sub_gid),
        "#2848: after GroupCreated applies, the previously-stranded \
         ContextRegistered must be re-driven and the context registered to the \
         subgroup (FAILS on master — the op stays stranded)"
    );

    // ---- (b) OpEvent::ContextRegistered fired for this context --------------
    // Bounded drain: by the time the apply returns, any synchronously-emitted
    // event is already queued. Allow a short poll window for robustness against
    // the broadcast channel's async delivery.
    let mut saw_context_registered = false;
    'drain: for _ in 0..40 {
        loop {
            match events.try_recv() {
                Ok(calimero_governance_store::op_events::OpEvent::ContextRegistered {
                    group_id,
                    context_id: ev_ctx,
                }) if group_id == sub_gid.to_bytes() && ev_ctx == context_id => {
                    saw_context_registered = true;
                    break 'drain;
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(_) => break,
            }
        }
        sleep(Duration::from_millis(25)).await;
    }
    assert!(
        saw_context_registered,
        "#2848: re-driving the buffered ContextRegistered must emit \
         OpEvent::ContextRegistered for (subgroup, context) (FAILS on master — \
         no re-drive, so the event never fires)"
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
#[serial(boot_test_node)]
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

// ===========================================================================
// TEE timing × visibility scenario matrix
// ===========================================================================
//
// Six deterministic cells = {Open, Restricted} × {created-after-join,
// late-join, join-with-created}. Each cell drives an explicit ORDER of
//   (a) admitting a root `ReadOnlyTee`,
//   (b) creating the subgroup,
//   (c) registering a context in it,
// then asserts BOTH:
//   * Membership — the TEE is a member of the subgroup (a direct `ReadOnlyTee`
//     row for Restricted; an inherited member with NO direct row for Open).
//   * Replication — the subgroup's context is actually known/registered on the
//     TEE side and the auto-follow path resolves to JOIN. For Restricted the
//     observable is `get_group_for_context(ctx) == Some(subgroup)` (the context
//     is registered on the receiver after the buffered-op re-drive). For Open
//     the observable is `has_context(ctx)` becoming true via the live
//     inheritance-aware auto-follow handler (the durable proof `join_context`
//     ran). Membership alone is NOT sufficient — every cell asserts replication.
//
// The three orderings:
//   * created-after-join: admit the root TEE first, THEN create the subgroup +
//     register the context (live fan-out — `tee_subgroup_admit` for Restricted;
//     inheritance/auto-follow for Open).
//   * late-join: create the subgroup + register the context FIRST, THEN admit
//     the root TEE / deliver its key, which must backfill pre-existing state
//     (for Restricted this is the buffered-ops re-drive path of #2848/#2771;
//     for Open the inheritance/auto-follow fall-through over a pre-existing
//     context).
//   * join-with-created: interleave — the subgroup is created, then the TEE is
//     admitted, then the context is registered. Explicit and deterministic.
//
// CELL → TEST mapping (which test covers each of the 6 cells):
//
//   Restricted / created-after-join:
//     membership  → `restricted_subgroup_created_admits_existing_tee_member`
//     replication → `restricted_ctx_redriven_after_group_created` (R1, #2848:
//                   buffered ContextRegistered re-driven once the held key's
//                   subgroup GroupCreated applies — the created-after-join
//                   receiver-backfill replication case)
//   Restricted / late-join:
//     both        → `tee_matrix_restricted_late_join` (NEW; store-only:
//                   GroupCreated + buffered ContextRegistered exist FIRST, the
//                   TEE row is seeded, THEN KeyDelivery arrives and the retry
//                   re-drives the buffered op — passes ONLY because of the
//                   #2848/#2771 retry trigger). Membership fan-in for the same
//                   ordering is also covered by
//                   `tee_admitted_after_restricted_subgroup_exists_is_fanned_in`.
//   Restricted / join-with-created:
//     both        → `tee_matrix_restricted_join_with_created` (NEW; actor
//                   harness: subgroup created, then root TEE admitted, then
//                   context registered — interleaved fan-in + replication).
//   Open / created-after-join:
//     membership  → `root_admitted_tee_is_member_of_open_subgroup`
//     replication → `root_admitted_tee_auto_follows_open_subgroup_context`
//                   (also re-confirmed by
//                   `integrated_tee_lifecycle_open_replication_and_scoped_root_cascade`)
//   Open / late-join:
//     both        → `tee_matrix_open_late_join` (NEW; actor harness: Open
//                   subgroup + context exist FIRST, THEN the root TEE is
//                   admitted and must auto-follow the pre-existing context via
//                   the inheritance fall-through).
//   Open / join-with-created:
//     both        → `tee_matrix_open_join_with_created` (NEW; actor harness:
//                   Open subgroup created, then root TEE admitted, THEN context
//                   registered — interleaved inheritance membership + replication).
//
// All matrix cells are DETERMINISTIC store/actor-level tests (no real libp2p);
// op order is controlled exactly, which is the whole point of the matrix.
// ===========================================================================

/// Matrix cell — **Restricted / late-join** (membership + replication).
///
/// Late-join ordering: the subgroup and its context exist on the receiver
/// FIRST (GroupCreated applied, an encrypted ContextRegistered buffered but
/// effect-skipped for want of the key), the TEE already holds a direct
/// `ReadOnlyTee` row in the subgroup (seeded as a prior root fan-in would have
/// left it), and only THEN does the subgroup key arrive via `KeyDelivery`. The
/// `KeyDelivery` retry must re-drive the buffered op so the context becomes
/// registered.
///
/// This is the genuine "member joins / key lands after the state already
/// exists, and backfills it" path. It passes ONLY because of the #2848/#2771
/// retry trigger on `apply_received_group_key` — on the pre-fix tree the
/// buffered op stays stranded and `get_group_for_context` returns `None`.
///
/// Store-only (mirrors R1): the backfill lives entirely in the synchronous
/// gov-store apply path, so this drives `apply_signed_namespace_op` directly,
/// keeping it fully deterministic.
#[tokio::test]
#[serial(boot_test_node)]
async fn tee_matrix_restricted_late_join() {
    use calimero_context::group_store::{
        apply_signed_namespace_op, get_group_for_context, GroupKeyring, MembershipRepository,
        MetaRepository, NamespaceRepository,
    };
    use calimero_context_client::local_governance::{
        GroupOp, NamespaceOp, RootOp, SignedNamespaceOp,
    };

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let mut rng = OsRng;

    // ---- Namespace root (receiver side) -------------------------------------
    let ns_gid = ContextGroupId::from([0xDAu8; 32]);
    let namespace_id = ns_gid.to_bytes();

    let owner_sk = PrivateKey::random(&mut rng);
    let owner_pk = owner_sk.public_key();

    // This receiver node's namespace identity (the `KeyDelivery` recipient).
    let member_sk = PrivateKey::random(&mut rng);
    let member_pk = member_sk.public_key();

    MetaRepository::new(&store)
        .save(&ns_gid, &sample_meta(owner_pk))
        .expect("save namespace root meta");
    MembershipRepository::new(&store)
        .add_member(&ns_gid, &owner_pk, GroupMemberRole::Admin)
        .expect("add owner as namespace-root admin");
    NamespaceRepository::new(&store)
        .store_identity(&ns_gid, &member_pk, &member_sk, &[0u8; 32])
        .expect("store receiver namespace identity");

    // The Restricted subgroup: id + key minted owner-side. The receiver does
    // not hold the key yet.
    let sub_gid = ContextGroupId::from(*PrivateKey::random(&mut rng).public_key());
    let subgroup_key: [u8; 32] = {
        use rand::RngCore;
        let mut k = [0u8; 32];
        rng.fill_bytes(&mut k);
        k
    };
    let key_id = GroupKeyring::key_id_for(&subgroup_key);
    let context_id = calimero_primitives::context::ContextId::from([0xCAu8; 32]);
    let nonzero_state_hash = [0x22u8; 32];

    // The TEE node whose membership the late join must end with. In the
    // late-join ordering the TEE row in the subgroup pre-exists the key (a
    // prior root fan-in / create-time admission left it); the new event is the
    // KeyDelivery that lets the held context op finally apply.
    let tee_pk = PrivateKey::random(&mut rng).public_key();

    let mut events = calimero_governance_store::op_events::subscribe();

    // ---- Step 1 (late-join): the subgroup EXISTS first (GroupCreated) -------
    let group_created_op = SignedNamespaceOp::sign(
        &owner_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        1,
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: sub_gid.to_bytes(),
            parent_id: namespace_id,
        }),
    )
    .expect("sign GroupCreated");
    apply_signed_namespace_op(&store, &group_created_op).expect("apply GroupCreated");
    assert!(
        MetaRepository::new(&store)
            .load(&sub_gid)
            .expect("load subgroup meta")
            .is_some(),
        "subgroup meta must exist after GroupCreated (the late-join precondition)"
    );

    // The subgroup is Restricted by default; seed the TEE's direct ReadOnlyTee
    // row as a prior root fan-in would have. This is the MEMBERSHIP half.
    MembershipRepository::new(&store)
        .add_member(&sub_gid, &tee_pk, GroupMemberRole::ReadOnlyTee)
        .expect("seed pre-existing ReadOnlyTee row in subgroup");

    // ---- Step 2 (late-join): the context op is buffered, effect-skipped -----
    // It cannot apply yet — the key has not arrived.
    let inner_op = GroupOp::ContextRegistered {
        context_id,
        application_id: calimero_primitives::application::ApplicationId::from([0xCCu8; 32]),
        blob_id: calimero_primitives::blobs::BlobId::from([0xDDu8; 32]),
        source: "calimero://stub-app".to_owned(),
        service_name: None,
    };
    let encrypted = GroupKeyring::encrypt_op(&subgroup_key, &inner_op).expect("encrypt group op");
    let ctx_registered_op = SignedNamespaceOp::sign(
        &owner_sk,
        namespace_id,
        vec![],
        nonzero_state_hash,
        2,
        NamespaceOp::Group {
            group_id: sub_gid.to_bytes(),
            key_id,
            encrypted,
            key_rotation: None,
        },
    )
    .expect("sign NamespaceOp::Group(ContextRegistered)");
    apply_signed_namespace_op(&store, &ctx_registered_op)
        .expect("apply buffered ContextRegistered (effect-skipped, logged)");

    assert_eq!(
        get_group_for_context(&store, &context_id).expect("get_group_for_context"),
        None,
        "pre-key: the context must NOT be registered yet (buffered/effect-skipped)"
    );

    // ---- Step 3 (late-join): the key/admission lands LAST (KeyDelivery) -----
    // This is the "member joins after the state already exists" moment. The
    // retry on apply_received_group_key must re-drive the buffered op; because
    // the subgroup meta already exists (GroupCreated applied in step 1), the
    // staleness check passes and the context registers.
    let envelope = GroupKeyring::wrap_for_member(&owner_sk, &member_pk, &subgroup_key)
        .expect("wrap subgroup key for receiver");
    let key_delivery_op = SignedNamespaceOp::sign(
        &owner_sk,
        namespace_id,
        vec![],
        [0u8; 32],
        3,
        NamespaceOp::Root(RootOp::KeyDelivery {
            group_id: sub_gid.to_bytes(),
            envelope,
        }),
    )
    .expect("sign KeyDelivery");
    apply_signed_namespace_op(&store, &key_delivery_op).expect("apply KeyDelivery");

    // ---- MEMBERSHIP: the TEE is a direct ReadOnlyTee member of the subgroup --
    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&sub_gid, &tee_pk)
            .expect("role_of"),
        Some(GroupMemberRole::ReadOnlyTee),
        "late-join Restricted: the TEE must be a direct ReadOnlyTee member"
    );

    // ---- REPLICATION: the buffered context op was re-driven on KeyDelivery ---
    assert_eq!(
        get_group_for_context(&store, &context_id).expect("get_group_for_context"),
        Some(sub_gid),
        "late-join Restricted: KeyDelivery must re-drive the buffered \
         ContextRegistered (#2848/#2771) so the context is registered on the \
         receiver — pre-fix the op stays stranded and this is None"
    );

    // The auto-follow Join decision is observable here as the emitted
    // OpEvent::ContextRegistered (the trigger the auto-follow handler joins on).
    let mut saw_context_registered = false;
    'drain: for _ in 0..40 {
        loop {
            match events.try_recv() {
                Ok(calimero_governance_store::op_events::OpEvent::ContextRegistered {
                    group_id,
                    context_id: ev_ctx,
                }) if group_id == sub_gid.to_bytes() && ev_ctx == context_id => {
                    saw_context_registered = true;
                    break 'drain;
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(_) => break,
            }
        }
        sleep(Duration::from_millis(25)).await;
    }
    assert!(
        saw_context_registered,
        "late-join Restricted: the re-drive must emit OpEvent::ContextRegistered \
         (the auto-follow Join trigger) for (subgroup, context)"
    );
}

/// Matrix cell — **Restricted / join-with-created** (membership + replication).
///
/// Interleaved ordering: the Restricted subgroup is CREATED first (so this node
/// mints + holds its key), then the root TEE is ADMITTED (live fan-in via the
/// `tee_subgroup_admit` subscriber's `handle_new_tee_member`), then the CONTEXT
/// is registered. This is the "concurrent" cell: admission lands between the
/// subgroup's creation and the context's registration.
///
/// Membership: the TEE gains a direct `ReadOnlyTee` row in the subgroup via the
/// live fan-in. Replication: the context registered after the fan-in is known on
/// this node (`get_group_for_context == Some(sub)`) and the auto-follow Join
/// trigger (`OpEvent::ContextRegistered`) fires for the subgroup.
#[tokio::test]
#[serial(boot_test_node)]
async fn tee_matrix_restricted_join_with_created() {
    let node = boot_test_node().await;
    let mut rng = OsRng;

    let ns_gid = ContextGroupId::from([0x98u8; 32]);
    let owner_pk = provision_tee_owner(&node, &ns_gid, &mut rng);

    // Rebind the process-global subscriber to THIS node before any subgroup op
    // fires (it is a first-wins singleton; other e2e tests may have bound it to
    // a different store). `spawn` subscribes synchronously, so no sleep needed.
    calimero_context::tee_subgroup_admit::shutdown();
    calimero_context::tee_subgroup_admit::spawn(node.store.clone(), node.context_client.clone());

    // (b) CREATE the Restricted subgroup first (mints + holds its key).
    let sub_gid = create_restricted_subgroup(&node, &ns_gid, &owner_pk, &mut rng).await;
    assert!(
        calimero_context::group_store::GroupKeyring::new(&node.store, sub_gid)
            .load_current_key()
            .expect("load key")
            .is_some(),
        "creator node must hold the subgroup key before admission (join-with-created)"
    );

    // (a) ADMIT the root TEE (interleaved — after create, before context). The
    // root admission's `TeeMemberAdmitted` event drives `handle_new_tee_member`
    // to fan the TEE into the already-created Restricted subgroup we hold the
    // key for.
    let tee_pk = PrivateKey::random(&mut rng).public_key();
    let nonce = [0x80u8; 32];
    let pk_hash: [u8; 32] = Sha256::digest(*tee_pk).into();
    let quote_bytes = mock_quote_bytes(&nonce, &pk_hash);
    let topic = format!("ns/{}", hex::encode(ns_gid.to_bytes()));
    node.node_addr
        .send(announce_network_event(
            libp2p::PeerId::random(),
            &topic,
            quote_bytes,
            tee_pk,
            nonce,
        ))
        .await
        .expect("deliver announce");

    // ---- MEMBERSHIP: the TEE is fanned into the pre-created subgroup ---------
    let fanned_in = wait_until(|| {
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .member_value(&sub_gid, &tee_pk)
            .ok()
            .flatten()
            .map(|v| v.role == GroupMemberRole::ReadOnlyTee)
            .unwrap_or(false)
    })
    .await;
    assert!(
        fanned_in,
        "join-with-created Restricted: root admission must fan the TEE into the \
         already-created Restricted subgroup"
    );

    // (c) REGISTER the context LAST (interleave point: after admission). This
    // node holds the subgroup key, so the registration applies directly and
    // emits OpEvent::ContextRegistered — the auto-follow Join trigger.
    let mut events = calimero_governance_store::op_events::subscribe();
    let context_id = calimero_primitives::context::ContextId::from([0xCBu8; 32]);
    calimero_context::group_store::register_context_in_group(&node.store, &sub_gid, &context_id)
        .expect("register context -> restricted subgroup");
    // `register_context_in_group` only writes the mapping; emit the production
    // trigger event the apply path would queue so the auto-follow Join decision
    // is observable for the TEE member.
    calimero_governance_store::op_events::notify(
        calimero_governance_store::op_events::OpEvent::ContextRegistered {
            group_id: sub_gid.to_bytes(),
            context_id,
        },
    );

    // ---- REPLICATION: the context is registered/known on this node ----------
    assert_eq!(
        calimero_context::group_store::get_group_for_context(&node.store, &context_id)
            .expect("get_group_for_context"),
        Some(sub_gid),
        "join-with-created Restricted: the post-admission context must be \
         registered to the subgroup on this node"
    );

    let mut saw_context_registered = false;
    'drain: for _ in 0..40 {
        loop {
            match events.try_recv() {
                Ok(calimero_governance_store::op_events::OpEvent::ContextRegistered {
                    group_id,
                    context_id: ev_ctx,
                }) if group_id == sub_gid.to_bytes() && ev_ctx == context_id => {
                    saw_context_registered = true;
                    break 'drain;
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(_) => break,
            }
        }
        sleep(Duration::from_millis(25)).await;
    }
    assert!(
        saw_context_registered,
        "join-with-created Restricted: OpEvent::ContextRegistered (the \
         auto-follow Join trigger) must fire for (subgroup, context)"
    );
}

/// Shared driver for the Open matrix cells that end on the auto-follow
/// replication observable. Re-points THIS node's namespace identity to the
/// inherited-only TEE, (re)binds the process-global auto-follow handler to this
/// node, seeds a `calimero://` stub app so `join_context`'s bootstrap completes,
/// registers `context_id` in `open_sub`, then polls until the TEE replicates it
/// (`has_context` true) — re-firing the production `OpEvent::ContextRegistered`
/// trigger each poll to absorb the handler spawn/subscribe race.
///
/// Returns whether the TEE ended up replicating the context. Mirrors the body
/// of `root_admitted_tee_auto_follows_open_subgroup_context`.
async fn drive_open_auto_follow_replication(
    node: &TestNode,
    ns_gid: &ContextGroupId,
    open_sub: &ContextGroupId,
    tee_sk: &PrivateKey,
    tee_pk: &PublicKey,
    context_id: calimero_primitives::context::ContextId,
) -> bool {
    // The auto-follow gate and `join_context` resolve the joiner from the
    // node's namespace identity; point it at the inherited-only TEE so the
    // inheritance fall-through (root anchor, no subgroup row) is exercised.
    calimero_context::group_store::NamespaceRepository::new(&node.store)
        .store_identity(ns_gid, tee_pk, tee_sk, &[0u8; 32])
        .expect("re-point namespace identity to the TEE");

    calimero_context::auto_follow::shutdown();
    calimero_context::auto_follow::spawn(node.store.clone(), node.context_client.clone());

    // `calimero://` stub app so `join_context`'s bootstrap is install-skipped
    // and writes `ContextMeta` (the durable replication proof).
    let app_id = ApplicationId::from([0xCCu8; 32]);
    let stub_blob =
        calimero_store::key::BlobMeta::new(calimero_primitives::blobs::BlobId::from([0u8; 32]));
    let stub_meta = ApplicationMetaValue::new(
        stub_blob,
        0,
        "calimero://stub-app".into(),
        Box::new([]),
        stub_blob,
        calimero_store::types::PackageInfo {
            package: "stub-package".into(),
            version: "0.0.0".into(),
            signer_id: "stub-signer".into(),
        },
    );
    node.store
        .handle()
        .put(
            &calimero_store::key::ApplicationMeta::new(app_id),
            &stub_meta,
        )
        .expect("seed stub application meta");

    calimero_context::group_store::register_context_in_group(&node.store, open_sub, &context_id)
        .expect("register context -> open subgroup");

    wait_until(|| {
        calimero_governance_store::op_events::notify(
            calimero_governance_store::op_events::OpEvent::ContextRegistered {
                group_id: open_sub.to_bytes(),
                context_id,
            },
        );
        node.context_client
            .has_context(&context_id)
            .unwrap_or(false)
    })
    .await
}

/// Matrix cell — **Open / late-join** (membership + replication).
///
/// Late-join ordering: an Open subgroup AND a context registered in it exist
/// FIRST, and only THEN is the root TEE admitted. The TEE must backfill: become
/// an inherited member of the pre-existing Open subgroup (no direct row) and
/// auto-follow the pre-existing context via the inheritance fall-through.
///
/// This differs from the Open created-after-join cell
/// (`root_admitted_tee_auto_follows_open_subgroup_context`), where the TEE is
/// admitted before the subgroup/context exist. Here the state pre-exists the
/// admission, so the auto-follow handler must pick up an already-registered
/// context rather than reacting to a fresh registration.
#[tokio::test]
#[serial(boot_test_node)]
async fn tee_matrix_open_late_join() {
    let node = boot_test_node().await;
    let mut rng = OsRng;

    let ns_gid = ContextGroupId::from([0x99u8; 32]);
    let owner_pk = provision_tee_owner(&node, &ns_gid, &mut rng);

    // Keep `tee_subgroup_admit` off: we assert the Open INHERITANCE path, so no
    // per-subgroup direct admission must race in a direct row.
    calimero_context::tee_subgroup_admit::shutdown();

    // (b) + (c) FIRST: create the Open subgroup and register a context in it,
    // before any TEE is admitted.
    let open_sub = create_open_subgroup(&node, &ns_gid, &owner_pk, &mut rng).await;
    assert!(
        calimero_context::group_store::CapabilitiesRepository::new(&node.store)
            .is_open_chain_to_namespace(&open_sub, &ns_gid)
            .expect("is_open_chain_to_namespace"),
        "the created subgroup must be Open all the way to the namespace root"
    );
    let context_id = calimero_primitives::context::ContextId::from([0xCDu8; 32]);
    calimero_context::group_store::register_context_in_group(&node.store, &open_sub, &context_id)
        .expect("pre-register context -> open subgroup (late-join precondition)");
    assert_eq!(
        calimero_context::group_store::get_group_for_context(&node.store, &context_id)
            .expect("get_group_for_context"),
        Some(open_sub),
        "the context must be registered to the Open subgroup before admission"
    );

    // (a) THEN admit the root TEE via the announce path. Keep its secret key:
    // we re-point this node's namespace identity to it for the auto-follow.
    let tee_sk = PrivateKey::random(&mut rng);
    let tee_pk = tee_sk.public_key();
    let nonce = [0x81u8; 32];
    let pk_hash: [u8; 32] = Sha256::digest(*tee_pk).into();
    let quote_bytes = mock_quote_bytes(&nonce, &pk_hash);
    let topic = format!("ns/{}", hex::encode(ns_gid.to_bytes()));
    node.node_addr
        .send(announce_network_event(
            libp2p::PeerId::random(),
            &topic,
            quote_bytes,
            tee_pk,
            nonce,
        ))
        .await
        .expect("deliver announce");
    assert!(
        wait_until(
            || calimero_context::group_store::MembershipRepository::new(&node.store)
                .is_member(&ns_gid, &tee_pk)
                .unwrap_or(false)
        )
        .await,
        "the TEE must be admitted at the namespace root"
    );

    // ---- MEMBERSHIP: inherited member of the Open subgroup, no direct row ----
    assert!(
        !calimero_context::group_store::MembershipRepository::new(&node.store)
            .has_direct_member(&open_sub, &tee_pk)
            .unwrap(),
        "late-join Open: no direct row expected — inheritance is the path"
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .is_member(&open_sub, &tee_pk)
            .unwrap(),
        "late-join Open: the root TEE must be an inherited member of the \
         pre-existing Open subgroup"
    );

    // ---- REPLICATION: the TEE backfills the PRE-EXISTING context ------------
    let replicating =
        drive_open_auto_follow_replication(&node, &ns_gid, &open_sub, &tee_sk, &tee_pk, context_id)
            .await;
    assert!(
        replicating,
        "late-join Open: the root-admitted TEE (inherited-only member) must \
         auto-follow (replicate) the PRE-EXISTING Open-subgroup context via the \
         inheritance fall-through"
    );
}

/// Matrix cell — **Open / join-with-created** (membership + replication).
///
/// Interleaved ordering: the Open subgroup is CREATED first, then the root TEE
/// is ADMITTED, then the CONTEXT is registered. Admission lands between the
/// subgroup's creation and the context's registration — the "concurrent" cell
/// for the Open visibility.
///
/// Membership: the TEE is an inherited member of the Open subgroup with no
/// direct row. Replication: the context registered after admission is
/// auto-followed (replicated) by the inherited-only TEE.
#[tokio::test]
#[serial(boot_test_node)]
async fn tee_matrix_open_join_with_created() {
    let node = boot_test_node().await;
    let mut rng = OsRng;

    let ns_gid = ContextGroupId::from([0x9Au8; 32]);
    let owner_pk = provision_tee_owner(&node, &ns_gid, &mut rng);

    // Keep `tee_subgroup_admit` off: the Open inheritance path must not be
    // masked by a racing direct row on the momentarily-Restricted subgroup.
    calimero_context::tee_subgroup_admit::shutdown();

    // (b) CREATE the Open subgroup first.
    let open_sub = create_open_subgroup(&node, &ns_gid, &owner_pk, &mut rng).await;
    assert!(
        calimero_context::group_store::CapabilitiesRepository::new(&node.store)
            .is_open_chain_to_namespace(&open_sub, &ns_gid)
            .expect("is_open_chain_to_namespace"),
        "the created subgroup must be Open all the way to the namespace root"
    );

    // (a) ADMIT the root TEE (interleaved — after create, before context).
    let tee_sk = PrivateKey::random(&mut rng);
    let tee_pk = tee_sk.public_key();
    let nonce = [0x82u8; 32];
    let pk_hash: [u8; 32] = Sha256::digest(*tee_pk).into();
    let quote_bytes = mock_quote_bytes(&nonce, &pk_hash);
    let topic = format!("ns/{}", hex::encode(ns_gid.to_bytes()));
    node.node_addr
        .send(announce_network_event(
            libp2p::PeerId::random(),
            &topic,
            quote_bytes,
            tee_pk,
            nonce,
        ))
        .await
        .expect("deliver announce");
    assert!(
        wait_until(
            || calimero_context::group_store::MembershipRepository::new(&node.store)
                .is_member(&ns_gid, &tee_pk)
                .unwrap_or(false)
        )
        .await,
        "the TEE must be admitted at the namespace root"
    );

    // ---- MEMBERSHIP: inherited member of the Open subgroup, no direct row ----
    assert!(
        !calimero_context::group_store::MembershipRepository::new(&node.store)
            .has_direct_member(&open_sub, &tee_pk)
            .unwrap(),
        "join-with-created Open: no direct row expected — inheritance is the path"
    );
    assert!(
        calimero_context::group_store::MembershipRepository::new(&node.store)
            .is_member(&open_sub, &tee_pk)
            .unwrap(),
        "join-with-created Open: the root TEE must be an inherited member of the \
         Open subgroup"
    );

    // (c) REGISTER the context LAST and assert the inherited-only TEE replicates
    // it via the auto-follow inheritance fall-through.
    let context_id = calimero_primitives::context::ContextId::from([0xCEu8; 32]);
    let replicating =
        drive_open_auto_follow_replication(&node, &ns_gid, &open_sub, &tee_sk, &tee_pk, context_id)
            .await;
    assert!(
        replicating,
        "join-with-created Open: the inherited-only root TEE must auto-follow \
         (replicate) the post-admission Open-subgroup context"
    );
}

/// Build a standalone `SyncManager` against an in-memory store, without
/// the surrounding `NodeManager` actor — enough to exercise the
/// synchronous peer-selection helpers (`member_peers_for_context`)
/// end-to-end against real governance state. Returns the manager, the
/// shared store, the shared `NodeState` (its peer-identity cache is an
/// `Arc` shared with the manager's `state_access`, so seeding it here is
/// visible to the manager), and the `TempDir` guard the blob fs needs.
async fn build_standalone_sync_manager() -> (SyncManager, Store, NodeState, TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = Store::new(Arc::new(InMemoryDB::owned()));

    let blob_store_config =
        BlobStoreConfig::new(tmp.path().to_path_buf().try_into().expect("utf8 blob path"));
    let file_system = FileSystem::new(&blob_store_config).await.expect("blob fs");
    let blob_store = BlobStore::new(store.clone(), file_system);
    let blob_manager = BlobManager::new(blob_store);

    let node_recipient = LazyRecipient::<NodeMessage>::new();
    let context_recipient = LazyRecipient::new();
    let network_recipient = LazyRecipient::new();
    let network_client = NetworkClient::new(network_recipient);
    let (event_sender, _) = broadcast::channel(16);
    let (ctx_sync_tx, ctx_sync_rx) = mpsc::channel(64);
    let (ns_sync_tx, ns_sync_rx) = mpsc::channel(16);
    let (ns_join_tx, ns_join_rx) = mpsc::channel(16);
    let (open_subgroup_join_tx, open_subgroup_join_rx) = mpsc::channel(16);
    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

    let node_client = NodeClient::new(
        store.clone(),
        blob_manager,
        network_client.clone(),
        node_recipient,
        event_sender,
        sync_client,
        String::new(),
        None,
    );
    let context_client = ContextClient::new(store.clone(), node_client.clone(), context_recipient);
    let node_state = NodeState::new(false, NodeMode::Standard);

    let sync_manager = SyncManager::new(
        SyncConfig::default(),
        node_client,
        context_client,
        network_client,
        node_state.clone(),
        ctx_sync_rx,
        ns_sync_rx,
        ns_join_rx,
        open_subgroup_join_rx,
    );

    (sync_manager, store, node_state, tmp)
}

/// End-to-end resolver (#2642): with a context registered to a group in
/// the real governance store and the durable peer-identity cache seeded
/// via the authenticated observe path, `member_peers_for_context`
/// resolves `context → group → cached member peers`, deduped by role.
#[tokio::test]
async fn member_peers_for_context_resolves_cached_members_end_to_end() {
    let (sync_manager, store, node_state, _tmp) = build_standalone_sync_manager().await;

    let group_id = ContextGroupId::from([0x11; 32]);
    let other_group = ContextGroupId::from([0xAA; 32]);
    let context_id = calimero_primitives::context::ContextId::from([0x22; 32]);
    calimero_context::group_store::register_context_in_group(&store, &group_id, &context_id)
        .expect("register context -> group");

    // Real (random) member identities, matching the convention of the
    // other tests in this file.
    let admin_id = PrivateKey::random(&mut OsRng).public_key();
    let member_id = PrivateKey::random(&mut OsRng).public_key();
    let admin_second_id = PrivateKey::random(&mut OsRng).public_key();
    let other_id = PrivateKey::random(&mut OsRng).public_key();
    let admin_peer = libp2p::PeerId::random();
    let member_peer = libp2p::PeerId::random();
    let other_peer = libp2p::PeerId::random();

    // Seed the shared cache through the same gate the production receive
    // paths use (group + role at the cross-DAG cut).
    let observe = |peer, identity, group, role| {
        node_state.observe_peer_identity(
            peer,
            identity,
            Some(ObservedMembership {
                group_id: group,
                role,
            }),
        );
    };
    observe(admin_peer, admin_id, group_id, GroupMemberRole::Admin);
    observe(member_peer, member_id, group_id, GroupMemberRole::Member);
    // Same peer observed again under a second identity at a weaker role —
    // the dedup must keep the strongest role (Admin) for admin_peer,
    // exercising dedup_peers_by_strongest_role through the full resolver.
    observe(
        admin_peer,
        admin_second_id,
        group_id,
        GroupMemberRole::Member,
    );
    // A member of a DIFFERENT group must not leak into this context's
    // result (guards group-scoping of cached_member_peers_for_group).
    observe(other_peer, other_id, other_group, GroupMemberRole::Admin);

    let resolved: std::collections::BTreeMap<_, _> = sync_manager
        .member_peers_for_context(&context_id)
        .into_iter()
        .collect();
    assert_eq!(resolved.len(), 2, "only this group's members resolve");
    assert_eq!(
        resolved.get(&admin_peer),
        Some(&GroupMemberRole::Admin),
        "dedup keeps the strongest role for a peer seen at two roles"
    );
    assert_eq!(resolved.get(&member_peer), Some(&GroupMemberRole::Member));
    assert!(
        !resolved.contains_key(&other_peer),
        "a member cached under a different group is excluded"
    );

    // A context with no group mapping resolves to nothing (caller then
    // falls back to topic discovery).
    let unregistered = calimero_primitives::context::ContextId::from([0x99; 32]);
    assert!(
        sync_manager
            .member_peers_for_context(&unregistered)
            .is_empty(),
        "unregistered context yields no cached members"
    );
}
