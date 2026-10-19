//! A purging leave empties the leaving node and, applied as the account namespace's
//! purge op, any node of the account; neither follows the namespace after a restart.

use std::sync::Arc;

use calimero_context::test_support::{ack_every_publish, enrol, holders_account_namespace};
use calimero_context_client::group::{CreateGroupRequest, LeaveNamespaceRequest};
use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    register_context_in_group, AccountNamespaceSet, ForgottenNamespaces, MembershipRepository,
    MetaRepository, NamespaceRepository, NodeDeviceRepository,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PrivateKey;
use calimero_store::db::Column;
use calimero_store::key;
use calimero_store::{types, Store};
use serial_test::serial;

use crate::namespace_forget_e2e::{eventually, leftovers};
use crate::test_node_harness::{
    boot_test_node, boot_test_node_over, boot_test_node_with_a_subscriber, TestNode,
};

const APP: [u8; 32] = [0xA8; 32];
const CONTEXT: [u8; 32] = [0xC2; 32];
const CONTROL: [u8; 32] = [0x6B; 32];

/// The second admin's key, which acks as the reachable owner would.
fn the_other_admin() -> PrivateKey {
    PrivateKey::from([0xF2; 32])
}

/// A holder whose namespace, with a context and state, is owned by a second admin
/// (an owner cannot leave); it and a namespace the account keeps are in the set.
async fn a_holder_in_a_namespace(node: &TestNode) -> (ContextGroupId, ContextGroupId) {
    let store = &node.store;
    let _root = NodeDeviceRepository::new(store)
        .provision_account_root()
        .expect("the holder's root");
    let account_namespace = holders_account_namespace(store, &node.context_client).await;
    let namespace = node
        .context_client
        .create_group(CreateGroupRequest {
            salt: None,
            bytecode_id: None,
            application_id: None,
            name: None,
            parent_group_id: None,
            restricted: true,
        })
        .await
        .expect("the namespace is created")
        .group_id;

    let other = enrol(store, &namespace, &the_other_admin().public_key());
    MembershipRepository::new(store)
        .add_member(&namespace, &other, GroupMemberRole::Admin)
        .expect("a second admin");
    let meta = MetaRepository::new(store);
    let mut row = meta
        .load(&namespace)
        .expect("read the meta")
        .expect("the namespace has one");
    row.owner_identity = other;
    meta.save(&namespace, &row).expect("hand it over");

    a_context_with_state(store, &namespace);
    let set = AccountNamespaceSet::new(store, account_namespace);
    for held in [namespace, ContextGroupId::from(CONTROL)] {
        set.record(held, Some(ApplicationId::from(APP)))
            .expect("the account holds the namespace");
    }
    assert!(
        !leftovers(store, &ids(namespace), account_namespace.to_bytes()).is_empty(),
        "the fixture has to put rows on disk, or the sweep below proves nothing"
    );
    (namespace, account_namespace)
}

/// The rows a synced context leaves on a member: meta, config, its identity, and
/// state, private state, a delta, an index row and blob rows.
fn a_context_with_state(store: &Store, namespace: &ContextGroupId) {
    let context = ContextId::from(CONTEXT);
    let mut handle = store.handle();
    handle
        .put(
            &key::ContextMeta::new(context),
            &types::ContextMeta::new(
                key::ApplicationMeta::new(ApplicationId::from(APP)),
                [0x01; 32],
                Vec::new(),
                None,
            ),
        )
        .expect("put the context meta");
    handle
        .put(
            &key::ContextConfig::new(context),
            &types::ContextConfig::new(0, 0),
        )
        .expect("put the context config");
    handle
        .put(
            &key::ContextIdentity::new(context, PrivateKey::from([0x23; 32]).public_key()),
            &types::ContextIdentity { private_key: None },
        )
        .expect("put the membership marker");
    drop(handle);
    register_context_in_group(store, namespace, &context).expect("register the context");
    for (column, tail) in [
        (Column::State, 0x01),
        (Column::PrivateState, 0x02),
        (Column::Delta, 0x03),
        (Column::SortedIndex, 0x04),
        (Column::ContextBlob, 0x05),
        (Column::BlobOwner, 0x06),
    ] {
        let key = [CONTEXT.to_vec(), vec![tail; 32]].concat();
        store.raw_put(column, &key, b"value").expect("seed a row");
    }
}

fn ids(namespace: ContextGroupId) -> [[u8; 32]; 2] {
    [namespace.to_bytes(), CONTEXT]
}

/// What a device must hold once the purge ran, and again after a restart.
fn nothing_of_the_namespace(
    store: &Store,
    namespace: ContextGroupId,
    account_namespace: ContextGroupId,
) {
    assert_eq!(
        leftovers(store, &ids(namespace), account_namespace.to_bytes()),
        vec![],
        "nothing of the namespace or its context stays on disk"
    );
    assert_eq!(
        ForgottenNamespaces::new(store)
            .list()
            .expect("list the removals"),
        vec![],
        "the account left, so there is nothing to restore"
    );
    assert_eq!(
        AccountNamespaceSet::new(store, account_namespace)
            .contains(namespace)
            .expect("read the set"),
        None,
        "the account's set drops it, so no device follows it from there"
    );
}

/// Boot again over `store`; the start-up sweep follows `CONTROL` and leaves the
/// purged namespace alone.
async fn restart(node: TestNode, namespace: ContextGroupId, account_namespace: ContextGroupId) {
    let store = node.store.clone();
    drop(node);
    calimero_context::account_follow::shutdown();
    let node = boot_test_node_over(store.clone()).await;
    let participating = || {
        NamespaceRepository::new(&store)
            .participating_namespaces()
            .expect("read the participation rows")
    };
    assert!(
        eventually(|| participating().contains(&ContextGroupId::from(CONTROL))).await,
        "the sweep ran: it followed the namespace the account keeps"
    );
    assert!(!participating().contains(&namespace));
    nothing_of_the_namespace(&store, namespace, account_namespace);
    drop(node);
    calimero_context::account_follow::shutdown();
}

#[tokio::test]
#[serial(boot_test_node)]
async fn the_leaving_device_keeps_nothing_and_it_stays_gone_after_a_restart() {
    calimero_context::account_follow::shutdown();
    let node = boot_test_node_with_a_subscriber().await;
    let (namespace, account_namespace) = a_holder_in_a_namespace(&node).await;

    let acker = ack_every_publish(
        Arc::clone(node.context_client.ack_router()),
        the_other_admin(),
    );
    let left = node
        .context_client
        .leave_namespace(LeaveNamespaceRequest {
            namespace_id: namespace,
            purge: true,
        })
        .await
        .expect("a second admin leaves with a purge");
    acker.abort();
    assert!(left.purged);

    nothing_of_the_namespace(&node.store, namespace, account_namespace);
    restart(node, namespace, account_namespace).await;
}

#[tokio::test]
#[serial(boot_test_node)]
async fn applying_the_account_purge_op_empties_this_node() {
    calimero_context::account_follow::shutdown();
    let node = boot_test_node().await;
    let (namespace, account_namespace) = a_holder_in_a_namespace(&node).await;
    let (_, signer_sk) = NamespaceRepository::new(&node.store)
        .identity(&account_namespace)
        .expect("read the identity")
        .expect("this device signs in its account namespace");

    // The leave has folded here: the account is no longer a member.
    let account = NodeDeviceRepository::new(&node.store)
        .get()
        .expect("read the device")
        .expect("this node is a device")
        .account;
    MembershipRepository::new(&node.store)
        .remove_member(&namespace, &account)
        .expect("the account left");

    // What the leaving device publishes into the account namespace.
    let _report = calimero_governance_store::sign_apply_and_publish(
        &node.store,
        &node.node_client,
        node.context_client.ack_router(),
        &account_namespace,
        &PrivateKey::from(signer_sk),
        GroupOp::AccountNamespacePurged { namespace },
    )
    .await
    .expect("the purge op applies");

    assert!(
        eventually(
            || leftovers(&node.store, &ids(namespace), account_namespace.to_bytes()).is_empty()
        )
        .await,
        "the listener takes the namespace off this device: {:?}",
        leftovers(&node.store, &ids(namespace), account_namespace.to_bytes())
    );
    nothing_of_the_namespace(&node.store, namespace, account_namespace);
    restart(node, namespace, account_namespace).await;
}
