//! `NetworkEvent::Message` → [`crate::NodeManager`] → `ContextClient::apply_signed_group_op` → `group_store`.
//!
//! Complements `calimero-context` store-only tests and `calimero-network` gossipsub tests.

use std::sync::Arc;
use std::time::Duration;

use actix::Actor;
use borsh::to_vec as borsh_to_vec;
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager, FileSystem};
use calimero_context::group_store::{
    add_group_member, check_group_membership, get_local_gov_nonce, save_group_meta,
};
use calimero_context::ContextManager;
use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::local_governance::{GroupOp, SignedGroupOp};
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::messages::NetworkEvent;
use calimero_node_primitives::client::NodeClient;
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
use libp2p::gossipsub::{IdentTopic, Message, MessageId};
use libp2p::PeerId;
use prometheus_client::registry::Registry;
use rand::rngs::OsRng;
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
        migration: None,
        auto_join: true,
    }
}

#[tokio::test]
async fn network_message_signed_group_op_applies_via_node_manager() {
    let mut pool = ArbiterPool::new().await.expect("arbiter pool");
    let tmp = tempfile::tempdir().expect("tempdir");

    let db = InMemoryDB::owned();
    let store = Store::new(Arc::new(db));

    let blob_store_config =
        BlobStoreConfig::new(tmp.path().to_path_buf().try_into().expect("utf8 blob path"));
    let file_system = FileSystem::new(&blob_store_config).await.expect("blob fs");
    let blobstore = BlobManager::new(store.clone(), file_system);

    let node_recipient = LazyRecipient::<NodeMessage>::new();
    let context_recipient = LazyRecipient::new();
    let network_recipient = LazyRecipient::new();

    let network_client = NetworkClient::new(network_recipient.clone());
    let (event_sender, _) = broadcast::channel(16);
    let (ctx_sync_tx, ctx_sync_rx) = mpsc::channel(64);

    let node_client = NodeClient::new(
        store.clone(),
        blobstore.clone(),
        network_client.clone(),
        node_recipient.clone(),
        event_sender,
        ctx_sync_tx,
        String::new(),
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
        None,
        Some(&mut registry),
    );

    let node_state = NodeState::new(false, NodeMode::Standard);

    let sync_manager = SyncManager::new(
        SyncConfig::default(),
        node_client.clone(),
        context_client.clone(),
        network_client.clone(),
        node_state.clone(),
        ctx_sync_rx,
    );

    let node_manager = NodeManager::new(
        blobstore,
        sync_manager,
        context_client.clone(),
        node_client,
        node_state,
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

    sleep(Duration::from_millis(50)).await;

    let mut rng = OsRng;
    let gid = ContextGroupId::from([0x77u8; 32]);
    let gid_bytes = gid.to_bytes();

    let admin_sk = PrivateKey::random(&mut rng);
    let admin_pk = admin_sk.public_key();

    save_group_meta(&store, &gid, &sample_meta(admin_pk)).expect("save_group_meta");
    add_group_member(&store, &gid, &admin_pk, GroupMemberRole::Admin).expect("add_group_member");

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

    let inner = borsh_to_vec(&op).expect("borsh SignedGroupOp");
    let broadcast = BroadcastMessage::SignedGroupOpV1 { payload: inner };
    let broadcast_bytes = borsh_to_vec(&broadcast).expect("borsh BroadcastMessage");

    let topic = IdentTopic::new(format!("group/{}", hex::encode(gid_bytes)));
    let topic_hash = topic.hash();

    let event = NetworkEvent::Message {
        id: MessageId::new(b"e2e"),
        message: Message {
            source: Some(PeerId::random()),
            data: broadcast_bytes,
            sequence_number: Some(1),
            topic: topic_hash,
        },
    };

    node_addr.send(event).await.expect("send NetworkEvent");

    for _ in 0..50 {
        if check_group_membership(&store, &gid, &new_member).unwrap_or(false) {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }

    assert!(
        check_group_membership(&store, &gid, &new_member).expect("check_group_membership"),
        "member should be present after inbound gossip path"
    );
    assert_eq!(
        get_local_gov_nonce(&store, &gid, &admin_pk)
            .expect("get_local_gov_nonce")
            .expect("nonce row"),
        1
    );
}
