//! `ContextClient::apply_signed_group_op` → `group_store`.
//!
//! Complements `calimero-context` store-only tests and `calimero-network` gossipsub tests.

use std::sync::Arc;
use std::time::Duration;

use actix::Actor;
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_context::group_store::{
    add_group_member, check_group_membership, get_group_member_value, get_local_gov_nonce,
    save_group_meta, store_group_signing_key,
};
use calimero_context::ContextManager;
use calimero_context_client::client::ContextClient;
use calimero_context_client::group::SetMemberAutoFollowRequest;
use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_node_primitives::messages::NodeMessage;
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
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};
use tokio::time::sleep;

use crate::arbiter_pool::ArbiterPool;
use crate::sync::{SyncConfig, SyncManager};
use crate::{NodeManager, NodeState};

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
struct TestNode {
    _pool: ArbiterPool,
    _tmp: TempDir,
    store: Store,
    context_client: ContextClient,
}

/// Boots a `ContextManager` + `NodeManager` against an in-memory store and
/// a tempdir-backed blobstore, with no peer wired up (the network client's
/// recipient is a never-initialised `LazyRecipient`, so any outbound op
/// publish becomes a local-only apply). Sufficient for governance handlers
/// that just need the actor mailbox and the datastore.
async fn boot_test_node() -> TestNode {
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
    let _node_addr = Actor::start_in_arbiter(&arb2, move |ctx| {
        assert!(node_recipient.init(ctx), "node recipient");
        node_manager
    });

    sleep(Duration::from_millis(50)).await;

    TestNode {
        _pool: pool,
        _tmp: tmp,
        store,
        context_client,
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

    save_group_meta(&node.store, &gid, &sample_meta(admin_pk)).expect("save_group_meta");
    add_group_member(&node.store, &gid, &admin_pk, GroupMemberRole::Admin).expect("add admin");

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
        check_group_membership(&node.store, &gid, &new_member).expect("check_group_membership"),
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

    save_group_meta(&node.store, &gid, &sample_meta(admin_sk.public_key())).unwrap();
    add_group_member(
        &node.store,
        &gid,
        &admin_sk.public_key(),
        GroupMemberRole::Admin,
    )
    .unwrap();
    add_group_member(
        &node.store,
        &gid,
        &alice_sk.public_key(),
        GroupMemberRole::Member,
    )
    .unwrap();

    // Admin needs a signing key registered so preflight can resolve one
    // when admin acts as requester.
    store_group_signing_key(&node.store, &gid, &admin_sk.public_key(), &admin_sk).unwrap();

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
    let alice_row = get_group_member_value(&node.store, &gid, &alice_sk.public_key())
        .unwrap()
        .expect("alice row");
    assert!(alice_row.auto_follow.contexts);
    assert!(!alice_row.auto_follow.subgroups);
}
