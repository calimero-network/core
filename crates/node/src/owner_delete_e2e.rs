//! An owner's delete reaching a peer: node A owns a namespace and deletes a
//! subgroup, then the namespace, through its real handlers; node B receives
//! each of A's publishes through its own gossip entrypoint.
//!
//! The harness has no transport, so B is given A's namespace key, standing in
//! for the key delivery a join brings.
use std::time::Duration;

use calimero_context_client::group::{CreateGroupRequest, OwnerDeleteGroupRequest};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::governance_broadcast::ns_topic;
use calimero_governance_store::{
    GroupKeyring, MetaRepository, NamespaceRepository, NodeDeviceRepository,
};
use calimero_network_primitives::messages::{Message, MessageId, NetworkEvent};
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::application::ApplicationId;
use serial_test::serial;
use tokio::time::sleep;

use crate::test_node_harness::{boot_test_node, TestNode};

const APP: [u8; 32] = [0xE4; 32];
const SETTLE_POLLS: usize = 200; // 10 s at 50 ms: B applies off its actor

/// Hand every namespace-governance message `from` published past `seen` to
/// `to`'s gossip entrypoint, in publish order.
async fn relay(from: &TestNode, to: &TestNode, seen: &mut usize) {
    let published = from
        .publishes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    for data in &published[*seen..] {
        let Ok(BroadcastMessage::NamespaceGovernanceDelta { namespace_id, .. }) =
            borsh::from_slice(data)
        else {
            continue;
        };
        to.node_addr
            .send(NetworkEvent::Message {
                id: MessageId(b"relayed".to_vec()),
                message: Message {
                    source: Some(libp2p::PeerId::random()),
                    data: data.clone(),
                    sequence_number: Some(1),
                    topic: ns_topic(namespace_id.into()),
                },
            })
            .await
            .expect("deliver to B's node actor");
    }
    *seen = published.len();
}

async fn eventually(cond: impl Fn() -> bool) -> bool {
    for _ in 0..SETTLE_POLLS {
        if cond() {
            return true;
        }
        sleep(Duration::from_millis(50)).await;
    }
    cond()
}

fn has_group(node: &TestNode, group: &ContextGroupId) -> bool {
    MetaRepository::new(&node.store)
        .load(group)
        .expect("read group meta")
        .is_some()
}

fn children(node: &TestNode, group: &ContextGroupId) -> Vec<ContextGroupId> {
    NamespaceRepository::new(&node.store)
        .list_children(group)
        .expect("read children")
}

async fn create(node: &TestNode, parent_group_id: Option<ContextGroupId>) -> ContextGroupId {
    node.context_client
        .create_group(CreateGroupRequest {
            salt: None,
            bytecode_id: None,
            application_id: Some(ApplicationId::from(APP)),
            name: None,
            parent_group_id,
            restricted: false,
        })
        .await
        .expect("create the group")
        .group_id
}

async fn owner_delete(node: &TestNode, group_id: ContextGroupId) {
    node.context_client
        .owner_delete_group(OwnerDeleteGroupRequest {
            group_id,
            root_proof: None,
        })
        .await
        .expect("the owner may delete");
}

#[tokio::test]
#[serial(boot_test_node)]
async fn an_owner_delete_reaches_a_peer_and_leaves_no_ghost_child() {
    let a = boot_test_node().await;
    calimero_context::test_support::install_application(&a.store, ApplicationId::from(APP));
    let _root = NodeDeviceRepository::new(&a.store)
        .provision_account_root()
        .expect("the account root that signs the owner's proofs");
    let namespace = create(&a, None).await;
    let subgroup = create(&a, Some(namespace)).await;

    let b = boot_test_node().await;
    let namespace_key = GroupKeyring::new(&a.store, namespace)
        .load_current_key_record()
        .expect("read A's keyring")
        .expect("A minted the namespace key");
    let _ = GroupKeyring::new(&b.store, namespace)
        .store_key(&namespace_key.group_key)
        .expect("B holds the namespace key");
    let mut seen = 0;
    relay(&a, &b, &mut seen).await;
    assert!(
        eventually(|| has_group(&b, &subgroup)).await,
        "B never synced the subgroup"
    );

    owner_delete(&a, subgroup).await;
    relay(&a, &b, &mut seen).await;

    assert!(
        eventually(|| !has_group(&b, &subgroup)).await,
        "B still has the subgroup its owner deleted"
    );
    assert!(children(&a, &namespace).is_empty(), "A lists a ghost child");
    assert!(children(&b, &namespace).is_empty(), "B lists a ghost child");

    owner_delete(&a, namespace).await;
    relay(&a, &b, &mut seen).await;

    assert!(
        eventually(|| !has_group(&b, &namespace)).await,
        "B still has the namespace its owner deleted"
    );
}
