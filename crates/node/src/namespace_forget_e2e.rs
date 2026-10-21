//! Removing a namespace from a device leaves nothing on disk and nothing follows it
//! after a restart. One node with no peer: sync's leftovers are seeded.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use calimero_context::handlers::delete_context::CONTEXT_ROW_COLUMNS;
use calimero_context::test_support::{enrol, enrol_holder, holder_device_scoped_to};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    register_context_in_group, set_context_service_name, AccountNamespaceSet,
    CapabilitiesRepository, ForgottenNamespaces, GroupKeyring, MembershipRepository,
    MetaRepository, MetadataRepository, NamespaceRepository, NodeDeviceRepository,
};
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_primitives::metadata::MetadataRecord;
use calimero_store::db::Column;
use calimero_store::key::{self, GroupMetaValue, GroupTarget};
use calimero_store::{types, Store};
use serial_test::serial;
use strum::IntoEnumIterator;

use crate::test_node_harness::{boot_test_node, boot_test_node_over};

const APP: [u8; 32] = [0xA7; 32];
const NAMESPACE: [u8; 32] = [0x5A; 32];
const CONTROL: [u8; 32] = [0x5B; 32];
const CONTEXT: [u8; 32] = [0xC1; 32];
const ADMIN_SEED: [u8; 32] = [0x11; 32];
const MEMBER_SEED: [u8; 32] = [0x22; 32];

/// The replay ledger outlives a purge on purpose. `ContextLocal` also keeps the
/// left markers and the forgotten marker; only its 32-byte rows are a context's.
const KEPT: [Column; 1] = [Column::ContextWarrantNonce];

/// The namespace as a sync leaves it on a member whose admin is someone else:
/// the group, both members, this node's identity and key, and a context with
/// state, private state, a delta and an index row.
fn a_member_with_a_synced_namespace(store: &Store, namespace: ContextGroupId) {
    let admin = enrol(
        store,
        &namespace,
        &PrivateKey::from(ADMIN_SEED).public_key(),
    );
    MetaRepository::new(store)
        .save(
            &namespace,
            &GroupMetaValue {
                target: GroupTarget {
                    application_id: ApplicationId::from(APP),
                    bytecode_id: [0xBB; 32],
                    package: Box::default(),
                    version: Box::default(),
                },
                created_at: 1_700_000_000,
                admin_identity: admin,
                owner_identity: admin,
                migration: None,
                auto_join: true,
            },
        )
        .expect("save the namespace");
    MembershipRepository::new(store)
        .add_member(&namespace, &admin, GroupMemberRole::Admin)
        .expect("seat the admin");

    let member_sk = PrivateKey::from(MEMBER_SEED);
    let member_pk = member_sk.public_key();
    NamespaceRepository::new(store)
        .replace_identity(&namespace, &member_pk, member_sk.as_bytes())
        .expect("seat this node's identity");
    let account = enrol_holder(store, &namespace, &member_pk);
    MembershipRepository::new(store)
        .add_member(&namespace, &account, GroupMemberRole::Member)
        .expect("seat this node as a plain member");
    let _key_id = GroupKeyring::new(store, namespace)
        .store_key(&[0x33; 32])
        .expect("store the namespace key");

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
            &key::ContextIdentity::new(context, member_pk),
            &types::ContextIdentity {
                private_key: Some(*member_sk.as_bytes()),
            },
        )
        .expect("put the membership marker");
    drop(handle);
    register_context_in_group(store, &namespace, &context).expect("register the context");
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
    for column in CONTEXT_ROW_COLUMNS {
        store
            .raw_put(column, &CONTEXT, b"value")
            .expect("seed a row");
    }
    // The rows the group plane keeps for the context, the namespace and their aliases.
    MetadataRepository::new(store)
        .set_context(
            &namespace,
            &context,
            &MetadataRecord {
                name: Some("channel".into()),
                data: BTreeMap::new(),
                updated_at: 1,
                updated_by: member_pk,
            },
        )
        .expect("name the context");
    CapabilitiesRepository::new(store)
        .set_context_member(&namespace, &context, &account, 1)
        .expect("grant a context capability");
    set_context_service_name(store, &context, "service").expect("name the service");
    MembershipRepository::new(store)
        .set_bootstrap_inviter(NAMESPACE.into(), admin)
        .expect("record the inviter hint");
    let mut handle = store.handle();
    handle
        .put(&key::GroupOwnerOpCounter::new(NAMESPACE), &1u64)
        .expect("spend an owner counter");
    handle
        .put(
            &key::Alias::new(
                None,
                Alias::<ContextId>::from_str("mine").expect("an alias"),
            )
            .expect("the context alias key"),
            &Hash::from(CONTEXT),
        )
        .expect("alias the context");
    handle
        .put(
            &key::Alias::new(
                Some(context),
                Alias::<PublicKey>::from_str("me").expect("an alias"),
            )
            .expect("the identity alias key"),
            &Hash::from([0x55; 32]),
        )
        .expect("alias an identity of the context");
    drop(handle);
    // Unified op-log rows, scoped to the namespace and to the context.
    for scope in [NAMESPACE, CONTEXT] {
        let key = [scope.to_vec(), vec![0x07; 32]].concat();
        store
            .raw_put(Column::UnifiedOp, &key, b"value")
            .expect("seed an op-log row");
    }
}

/// Every key outside [`KEPT`] and the account namespace's own rows that holds one
/// of `ids` anywhere in it, left and forgotten markers aside.
fn leftovers(
    store: &Store,
    ids: &[[u8; 32]],
    account_namespace: [u8; 32],
) -> Vec<(Column, String)> {
    let mut found = Vec::new();
    for column in Column::iter().filter(|column| !KEPT.contains(column)) {
        let rows = store
            .raw_scan(column, &[], &[0xFF; 128], None)
            .expect("scan the column");
        for (key, value) in rows {
            if column == Column::ContextLocal && key.len() != 32 {
                continue;
            }
            let holds = |id: &[u8; 32]| key.windows(32).any(|window| window == id);
            let aliased = column == Column::Alias && ids.iter().any(|id| value == id);
            if (ids.iter().any(holds) || aliased) && !holds(&account_namespace) {
                found.push((column, hex::encode(key)));
            }
        }
    }
    found
}

async fn eventually(mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
#[serial(boot_test_node)]
async fn a_member_removing_a_namespace_leaves_nothing_and_it_stays_gone_after_a_restart() {
    let node = boot_test_node().await;
    let store = node.store.clone();
    let (account_namespace, _device) = holder_device_scoped_to(&store, &[]);
    let (namespace, control) = (
        ContextGroupId::from(NAMESPACE),
        ContextGroupId::from(CONTROL),
    );
    a_member_with_a_synced_namespace(&store, namespace);
    let account = AccountNamespaceSet::new(&store, account_namespace);
    account
        .record(namespace, Some(ApplicationId::from(APP)))
        .expect("the account holds the namespace");
    account
        .record(control, Some(ApplicationId::from(APP)))
        .expect("the account holds a namespace this node keeps");
    assert!(
        !leftovers(&store, &[NAMESPACE, CONTEXT], account_namespace.to_bytes()).is_empty(),
        "the fixture has to put rows on disk, or the sweep below proves nothing"
    );
    let published_before = node.publishes.lock().expect("publishes").len();

    let deleted = node
        .context_client
        .delete_namespace(calimero_context_client::group::DeleteNamespaceRequest {
            namespace_id: namespace,
        })
        .await
        .expect("a plain member removes the namespace from its own node");
    assert!(deleted.deleted);

    assert_eq!(
        leftovers(&store, &[NAMESPACE, CONTEXT], account_namespace.to_bytes()),
        vec![],
        "nothing of the namespace or its context stays on disk"
    );
    assert_eq!(
        node.publishes.lock().expect("publishes").len(),
        published_before,
        "removing it from this device is no governance op: the others see no change"
    );
    assert!(
        account.contains(namespace).expect("read the set").is_some(),
        "the account stays a member, so its set keeps the namespace"
    );
    let forgotten = ForgottenNamespaces::new(&store)
        .list()
        .expect("list the removals");
    assert_eq!(forgotten.len(), 1);
    assert_eq!(forgotten[0].namespace_id, namespace);
    assert_eq!(forgotten[0].application_id, Some(ApplicationId::from(APP)));

    // A restart: the node boots again over the same disk, and the account's
    // start-up sweep walks the set that still names the namespace.
    drop(node);
    calimero_context::account_follow::shutdown();
    let node = boot_test_node_over(store.clone()).await;
    let participating = || {
        NamespaceRepository::new(&store)
            .participating_namespaces()
            .expect("read the participation rows")
    };
    assert!(
        eventually(|| participating().contains(&control)).await,
        "the sweep ran: it followed the namespace this node keeps"
    );
    assert!(
        !participating().contains(&namespace),
        "and left the removed one alone"
    );
    assert_eq!(
        leftovers(&store, &[NAMESPACE, CONTEXT], account_namespace.to_bytes()),
        vec![],
        "still nothing on disk after the restart"
    );
    drop(node);
}

#[tokio::test]
#[serial(boot_test_node)]
async fn a_node_with_no_account_namespace_keeps_its_device_when_it_removes_its_only_namespace() {
    let node = boot_test_node().await;
    let store = node.store.clone();
    let devices = NodeDeviceRepository::new(&store);
    devices
        .provision_account_root()
        .expect("an initialised node holds an account root");
    let namespace = ContextGroupId::from(NAMESPACE);
    a_member_with_a_synced_namespace(&store, namespace);
    let device = devices
        .get()
        .expect("read the device")
        .expect("enrolling this node mints its device")
        .device();
    assert_eq!(
        NamespaceRepository::new(&store)
            .participating_namespaces()
            .expect("read the participation rows"),
        vec![namespace],
        "the namespace is the only one this node takes part in"
    );

    node.context_client
        .delete_namespace(calimero_context_client::group::DeleteNamespaceRequest {
            namespace_id: namespace,
        })
        .await
        .expect("a plain member removes its only namespace");

    assert_eq!(
        devices.get().expect("read the device").map(|d| d.device()),
        Some(device),
        "a rejoin has to find the device the inviter wrapped its key for"
    );
    assert!(ForgottenNamespaces::new(&store)
        .is_forgotten(&namespace)
        .expect("read the marker"));
    drop(node);
}
