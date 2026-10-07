//! Whose identity a delegated read observes.
//!
//! Driven through a live `ContextManager` with a hand-written wasm module whose
//! one view returns `env::device_id()`, so "who did the read run as" is a plain
//! comparison of the method's return value against a key.
//!
//! The bug (seen on a fleet relay): `POST /contexts/:id/query` on an account's
//! session ran the method with the RELAY's key as the device, so a contract's
//! "who am I" answered with the relay for every account, while the same method
//! through a warrant answered with the caller's delegated device key.

use std::sync::Arc;

use calimero_context_client::messages::{ExecuteError, ReadAs};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::{MemberCapabilities, VisibilityMode};
use calimero_governance_store::{
    register_context_in_group, unregister_context_from_group, CapabilitiesRepository,
    DenyListRepository, GroupKeyring, MembershipRepository, MetaRepository, NamespaceRepository,
    NodeDeviceRepository,
};
use calimero_node_primitives::test_fixtures::signed_wasm;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::{DeviceId, PrivateKey, PublicKey};
use calimero_store::db::InMemoryDB;
use calimero_store::key::{self, GroupMetaValue, GroupTarget};
use calimero_store::{types, Store};
use futures_util::io::Cursor;

use crate::test_support::{actor, enrol, enrol_holder};

/// Exports one view, `get_current_user`, returning the 32 bytes of
/// `env::device_id()`. Memory layout: the id lands at 0; the `{ptr, len}`
/// descriptor `read_register` fills from is at 64; the `ValueReturn::Ok`
/// (an 8-byte discriminant of 0 followed by a `{ptr, len}` buffer) is at 96.
const MODULE: &str = r#"
    (module
        (import "env" "device_id" (func $device_id (param i64)))
        (import "env" "read_register" (func $read_register (param i64 i64) (result i32)))
        (import "env" "value_return" (func $value_return (param i64)))
        (memory (export "memory") 1)
        (data (i32.const 64)
            "\00\00\00\00\00\00\00\00\20\00\00\00\00\00\00\00")
        (data (i32.const 96)
            "\00\00\00\00\00\00\00\00"
            "\00\00\00\00\00\00\00\00\20\00\00\00\00\00\00\00")
        (func (export "get_current_user")
            (call $device_id (i64.const 0))
            (drop (call $read_register (i64.const 0) (i64.const 64)))
            (call $value_return (i64.const 96))))
"#;

/// The ABI the module carries: `get_current_user` declared read-only, which is
/// what lets a session (rather than a warrant) call it.
const MANIFEST: &str = r#"{
    "schema_version": "wasm-abi/1",
    "types": {},
    "methods": [{"name": "get_current_user", "params": [], "intent": "read_only"}],
    "events": []
}"#;

/// Exports one view, `read_private`, that reads a private-storage key and
/// returns nothing. Memory layout: the 32-byte zero key at 0, its
/// `{ptr, len}` descriptor at 64.
const PRIVATE_MODULE: &str = r#"
    (module
        (import "env" "private_storage_read" (func $read (param i64 i64) (result i32)))
        (memory (export "memory") 1)
        (data (i32.const 64)
            "\00\00\00\00\00\00\00\00\20\00\00\00\00\00\00\00")
        (func (export "read_private")
            (drop (call $read (i64.const 64) (i64.const 0)))))
"#;

const PRIVATE_MANIFEST: &str = r#"{
    "schema_version": "wasm-abi/1",
    "types": {},
    "methods": [{"name": "read_private", "params": [], "intent": "read_only"}],
    "events": []
}"#;

/// Module compiles run on the node's global runtime, which must be
/// multi-threaded; `actix::test` runs on a current-thread one.
fn global_runtime() {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    let runtime = RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a multi-threaded runtime")
    });
    let _ = std::thread::scope(|scope| {
        scope
            .spawn(|| runtime.block_on(async { calimero_utils_actix::init_global_runtime() }))
            .join()
    });
}

struct Fixture {
    harness: actor::Harness,
    store: Store,
    group_id: ContextGroupId,
    context_id: ContextId,
    /// This node's own signing key in the context: the executor of every run.
    node_key: PublicKey,
    /// A member account that runs no node, with the one device it joined on.
    caller: calimero_account::AccountId,
    caller_device: DeviceId,
    caller_key: PublicKey,
    /// The group admin's key, bound to the admin's account and nobody else's.
    admin_key: PublicKey,
}

/// A one-group namespace with one context on [`MODULE`]: this node a member,
/// and one account that is a member through a device binding and nothing
/// else, as a thin client that joined through a relay is.
async fn fixture() -> Fixture {
    fixture_for(MODULE, MANIFEST).await
}

async fn fixture_for(module: &str, manifest: &str) -> Fixture {
    global_runtime();
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    NodeDeviceRepository::new(&store)
        .provision_account_root()
        .expect("provision the account root an initialised node has");
    let harness = actor::over(store.clone()).await;

    let wasm = wat::parse_str(module).expect("parse the module");
    let manifest: calimero_wasm_abi::schema::Manifest =
        serde_json::from_str(manifest).expect("parse the manifest");
    let wasm = calimero_wasm_abi::embed::write_embedded_state_schema(&wasm, &manifest)
        .expect("embed the manifest");
    let (blob_id, size) = harness
        .node_client
        .add_blob(Cursor::new(signed_wasm(&wasm)), None, None)
        .await
        .expect("store the wasm");
    let application_id = ApplicationId::from([0xA8; 32]);
    let mut handle = store.handle();
    handle
        .put(
            &key::ApplicationMeta::new(application_id),
            &types::ApplicationMeta::new(
                key::BlobMeta::new(blob_id),
                size,
                "file://test.wasm".into(),
                vec![].into(),
                key::BlobMeta::new([0; 32].into()),
                types::PackageInfo {
                    package: "com.test.whoami".into(),
                    version: "1.0.0".into(),
                    signer_id: "did:key:test".into(),
                    state_version: 0,
                },
            ),
        )
        .expect("install the application");

    let group_id = ContextGroupId::from([0x6C; 32]);
    let admin = PrivateKey::from([0x11; 32]).public_key();
    let admin_account = enrol(&store, &group_id, &admin);
    MetaRepository::new(&store)
        .save(
            &group_id,
            &GroupMetaValue {
                target: GroupTarget {
                    application_id,
                    bytecode_id: *blob_id.digest(),
                    package: Box::default(),
                    version: Box::default(),
                },
                created_at: 1_700_000_000,
                admin_identity: admin_account,
                owner_identity: admin_account,
                migration: None,
                auto_join: true,
            },
        )
        .expect("save the group meta");
    MembershipRepository::new(&store)
        .add_member(&group_id, &admin_account, GroupMemberRole::Admin)
        .expect("seat the admin");
    let _key_id = GroupKeyring::new(&store, group_id)
        .store_key(&[0x33; 32])
        .expect("store the group key");

    let node_sk = PrivateKey::from([0x22; 32]);
    let node_key = node_sk.public_key();
    NamespaceRepository::new(&store)
        .replace_identity(&group_id, &node_key, node_sk.as_bytes())
        .expect("seat this node's namespace identity");
    let node_account = enrol_holder(&store, &group_id, &node_key);
    MembershipRepository::new(&store)
        .add_member(&group_id, &node_account, GroupMemberRole::Member)
        .expect("seat the local node");

    // The caller: an account whose device is bound in the namespace, the row a
    // join as an account writes. `enrol` derives the device id from the key.
    let caller_key = PrivateKey::from([0x44; 32]).public_key();
    let caller = enrol(&store, &group_id, &caller_key);
    let caller_device = crate::test_support::credential(&caller_key)
        .statement
        .device;
    MembershipRepository::new(&store)
        .add_member(&group_id, &caller, GroupMemberRole::Member)
        .expect("seat the caller");

    let context_id = ContextId::from([0xC8; 32]);
    handle
        .put(
            &key::ContextMeta::new(context_id),
            &types::ContextMeta::new(
                key::ApplicationMeta::new(application_id),
                [0x01; 32],
                Vec::new(),
                None,
            ),
        )
        .expect("put the context meta");
    handle
        .put(
            &key::ContextConfig::new(context_id),
            &types::ContextConfig::new(0, 0),
        )
        .expect("put the context config");
    register_context_in_group(&store, &group_id, &context_id).expect("register the context");
    handle
        .put(
            &key::ContextIdentity::new(context_id, node_key),
            &types::ContextIdentity {
                private_key: Some(*node_sk.as_bytes()),
            },
        )
        .expect("put the membership marker");

    Fixture {
        harness,
        store,
        group_id,
        context_id,
        node_key,
        caller,
        caller_device,
        caller_key,
        admin_key: admin,
    }
}

impl Fixture {
    /// `get_current_user` as the query route runs it: on a session that names
    /// `device`, executed by this node.
    async fn who_am_i(&self, read_as: ReadAs) -> Result<[u8; 32], ExecuteError> {
        let response = self
            .harness
            .context_client
            .query_as(
                &self.context_id,
                read_as,
                &self.node_key,
                "get_current_user".to_owned(),
                Vec::new(),
            )
            .await?;
        let returns = response
            .returns
            .expect("the view returns")
            .expect("the view returns a value");
        let device: [u8; 32] = returns
            .as_slice()
            .try_into()
            .expect("device_id is 32 bytes");
        Ok(device)
    }
}

/// The prod repro: a read on an account's session must answer "who am I" with
/// the caller's device key, as the same method through a warrant does — not
/// with the relay's.
#[actix::test]
async fn a_read_on_an_accounts_session_observes_the_sessions_device() {
    let fx = fixture().await;
    let device = fx
        .who_am_i(ReadAs {
            account: fx.caller,
            device: Some(fx.caller_device),
        })
        .await
        .expect("a member's read runs");
    assert_eq!(
        device, *fx.caller_key,
        "the read ran as this node ({}) instead of the caller's device ({})",
        fx.node_key, fx.caller_key,
    );
    assert_ne!(device, *fx.node_key);
}

/// A session that names no device is judged by its account alone, and the
/// device half of the principal is then the process actually running the
/// call, as it was before: there is no other key it could honestly name.
#[actix::test]
async fn a_read_on_a_session_naming_no_device_runs_on_this_nodes_key() {
    let fx = fixture().await;
    let device = fx
        .who_am_i(ReadAs {
            account: fx.caller,
            device: None,
        })
        .await
        .expect("a member's read runs");
    assert_eq!(device, *fx.node_key);
}

/// A device the namespace never bound to this account resolves to no key of
/// the caller's, and the read must not pick up someone else's: a device bound
/// to a DIFFERENT account is not this session's device, whatever it claims.
#[actix::test]
async fn a_device_bound_to_another_account_is_not_the_sessions() {
    let fx = fixture().await;
    let device = fx
        .who_am_i(ReadAs {
            account: fx.caller,
            device: Some(
                crate::test_support::credential(&fx.admin_key)
                    .statement
                    .device,
            ),
        })
        .await
        .expect("a member's read runs");
    assert_ne!(
        device, *fx.admin_key,
        "the read adopted a key bound to a different account"
    );
    assert_eq!(
        device, *fx.node_key,
        "an unresolvable device falls back to the executor, never to another account's key"
    );
}

/// The account is still the membership gate: a device alone admits nobody.
#[actix::test]
async fn a_non_member_is_refused_whatever_device_it_names() {
    let fx = fixture().await;
    let stranger_key = PrivateKey::from([0x55; 32]).public_key();
    let stranger = crate::test_support::account_for(&stranger_key);
    let err = fx
        .who_am_i(ReadAs {
            account: stranger,
            device: Some(fx.caller_device),
        })
        .await
        .expect_err("a stranger's read is refused");
    assert!(
        matches!(err, ExecuteError::NotAMember { .. }),
        "expected NotAMember, got {err:?}"
    );
}

/// A read on an account's behalf has no private store: the node-local bucket is
/// keyed by context alone, so handing it over would share it across accounts.
#[actix::test]
async fn a_read_on_an_accounts_behalf_gets_no_private_storage() {
    let fx = fixture_for(PRIVATE_MODULE, PRIVATE_MANIFEST).await;
    let own = fx
        .harness
        .context_client
        .execute(
            &fx.context_id,
            &fx.node_key,
            "read_private".to_owned(),
            Vec::new(),
            None,
        )
        .await
        .expect("control: the node's own run reads its private storage");
    assert!(own.returns.is_ok(), "control: {:?}", own.returns);

    let err = fx
        .harness
        .context_client
        .query_as(
            &fx.context_id,
            ReadAs {
                account: fx.caller,
                device: Some(fx.caller_device),
            },
            &fx.node_key,
            "read_private".to_owned(),
            Vec::new(),
        )
        .await
        .expect_err("a read on an account's behalf must not reach private storage");
    assert!(
        matches!(err, ExecuteError::PrivateStorageUnavailable { .. }),
        "expected PrivateStorageUnavailable, got {err:?}"
    );
}

/// An account that inherits its way into an Open subgroup stops being a member
/// of its contexts once the subgroup deny-lists it, though it keeps the seat above.
#[actix::test]
async fn an_inheritor_removed_from_the_context_group_is_refused() {
    let fx = fixture().await;
    let child = ContextGroupId::from([0x6D; 32]);
    let parent_meta = MetaRepository::new(&fx.store)
        .load(&fx.group_id)
        .expect("load the group meta")
        .expect("the group has meta");
    MetaRepository::new(&fx.store)
        .save(&child, &parent_meta)
        .expect("save the subgroup meta");
    GroupKeyring::new(&fx.store, child)
        .store_key(&[0x33; 32])
        .expect("store the subgroup key");
    NamespaceRepository::new(&fx.store)
        .nest(&fx.group_id, &child)
        .expect("nest the subgroup");
    CapabilitiesRepository::new(&fx.store)
        .set_subgroup_visibility(&child, VisibilityMode::Open)
        .expect("open the subgroup");
    CapabilitiesRepository::new(&fx.store)
        .set_member_capability(
            &fx.group_id,
            &fx.caller,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
        )
        .expect("let the caller join open subgroups");
    unregister_context_from_group(&fx.store, &fx.group_id, &fx.context_id)
        .expect("leave the parent group");
    register_context_in_group(&fx.store, &child, &fx.context_id).expect("join the subgroup");

    let read = ReadAs {
        account: fx.caller,
        device: Some(fx.caller_device),
    };
    fx.who_am_i(read)
        .await
        .expect("control: an inheritor reads the subgroup's context");

    DenyListRepository::new(&fx.store)
        .mark(&child, &fx.caller)
        .expect("remove the caller from the subgroup");
    let err = fx
        .who_am_i(read)
        .await
        .expect_err("a removed inheritor's read is refused");
    assert!(
        matches!(err, ExecuteError::NotAMember { .. }),
        "expected NotAMember, got {err:?}"
    );
}
