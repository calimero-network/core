//! Which executions may commit state, by the local node's role in the
//! context's group.
//!
//! Driven through a live `ContextManager` with a real (hand-written) wasm
//! module, because the gate under test sits between the wasm run and the
//! commit: a module that always commits a known root hash makes "was the write
//! kept?" a plain read of the context's root hash afterwards.

use std::sync::Arc;

use calimero_account::AccountId;
use calimero_context_client::messages::{
    ExecuteError, ExecuteRequest, ExecuteResponse, WriteSource,
};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::{MemberCapabilities, VisibilityMode};
use calimero_governance_store::{
    register_context_in_group, CapabilitiesRepository, GroupKeyring, MembershipRepository,
    MetaRepository, NamespaceRepository, NodeDeviceRepository,
};
use calimero_node_primitives::test_fixtures::signed_wasm;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::db::InMemoryDB;
use calimero_store::key::{self, GroupMetaValue, GroupTarget};
use calimero_store::{types, Store};
use futures_util::io::Cursor;

use crate::test_support::{actor, enrol, enrol_holder};

/// The root hash every run of [`MODULE`] commits.
pub(super) const COMMITTED_ROOT: [u8; 32] = [0x5A; 32];

/// The root hash the context starts from, before any run.
pub(super) const INITIAL_ROOT: [u8; 32] = [0x01; 32];

/// Exports the two kinds of run the gate tells apart: `__calimero_sync_next`
/// (the merge-apply of a delta) and `set` (an ordinary mutating method). Both
/// just `commit` [`COMMITTED_ROOT`] with a one-byte artifact, which is all the
/// gate looks at. Memory layout: the root at 0, the artifact at 32, and the two
/// `{ptr: u64, len: u64}` descriptors `commit` reads at 64 and 80.
const MODULE: &str = r#"
    (module
        (import "env" "commit" (func $commit (param i64 i64)))
        (memory (export "memory") 1)
        (data (i32.const 0)
            "\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a\5a")
        (data (i32.const 32) "\01")
        (data (i32.const 64)
            "\00\00\00\00\00\00\00\00\20\00\00\00\00\00\00\00"
            "\20\00\00\00\00\00\00\00\01\00\00\00\00\00\00\00")
        (func (export "__calimero_sync_next") (call $commit (i64.const 64) (i64.const 80)))
        (func (export "set") (call $commit (i64.const 64) (i64.const 80))))
"#;

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

/// How the local node relates to the context's group.
#[derive(Clone, Debug)]
pub(super) enum LocalRole {
    /// A row with this role.
    Role(GroupMemberRole),
    /// A row with this role at the namespace ROOT, with the right to join Open
    /// subgroups, and none in the group owning the context: an Open subgroup
    /// of that root. The node reaches the context by inheritance only.
    InheritedFromRoot(GroupMemberRole),
    /// Holds the context's membership marker but no row — a node whose row was
    /// removed and whose marker has not (yet) been.
    MarkerOnly,
    /// Neither a row nor a marker: what a removed member's node is left with.
    Outsider,
}

pub(super) struct Fixture {
    pub(super) harness: actor::Harness,
    pub(super) context_id: ContextId,
    pub(super) executor: PublicKey,
    pub(super) store: Store,
    /// The account the local node speaks for.
    pub(super) account: AccountId,
    /// The group that owns the context.
    pub(super) group_id: ContextGroupId,
}

/// A one-group namespace with one context on [`MODULE`], and the local node
/// holding `role` in it. Someone else is the group's admin, so the local node
/// is never an admin by accident.
async fn fixture(role: LocalRole) -> Fixture {
    fixture_running(role, |_| MODULE.to_owned()).await
}

/// [`fixture`], running the module `module` writes for the account the local node speaks for.
pub(super) async fn fixture_running(
    role: LocalRole,
    module: impl FnOnce(AccountId) -> String,
) -> Fixture {
    global_runtime();
    let holds_marker = !matches!(role, LocalRole::Outsider);
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    NodeDeviceRepository::new(&store)
        .provision_account_root()
        .expect("provision the account root an initialised node has");
    let harness = actor::over(store.clone()).await;

    let group_id = ContextGroupId::from([0x6A; 32]);
    let executor_sk = PrivateKey::from([0x22; 32]);
    let executor = executor_sk.public_key();
    let account = enrol_holder(&store, &group_id, &executor);

    let wasm = wat::parse_str(module(account)).expect("parse the module");
    let (blob_id, size) = harness
        .node_client
        .add_blob(Cursor::new(signed_wasm(&wasm)), None, None)
        .await
        .expect("store the wasm");
    let application_id = ApplicationId::from([0xA7; 32]);
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
                    package: "com.test.gate".into(),
                    version: "1.0.0".into(),
                    signer_id: "did:key:test".into(),
                    state_version: 0,
                },
            ),
        )
        .expect("install the application");

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

    NamespaceRepository::new(&store)
        .replace_identity(&group_id, &executor, executor_sk.as_bytes())
        .expect("seat this node's namespace identity");
    // The group that owns the context: the root itself, or an Open subgroup
    // of it when the node's role is inherited.
    let owning_group = match role {
        LocalRole::Role(role) => {
            MembershipRepository::new(&store)
                .add_member(&group_id, &account, role)
                .expect("seat the local node");
            group_id
        }
        LocalRole::InheritedFromRoot(role) => {
            MembershipRepository::new(&store)
                .add_member(&group_id, &account, role)
                .expect("seat the local node at the root");
            CapabilitiesRepository::new(&store)
                .set_member_capability(
                    &group_id,
                    &account,
                    MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
                )
                .expect("let it join Open subgroups");
            let sub = ContextGroupId::from([0x6B; 32]);
            let sub_meta = MetaRepository::new(&store)
                .load(&group_id)
                .expect("read the root meta")
                .expect("the root meta exists");
            MetaRepository::new(&store)
                .save(&sub, &sub_meta)
                .expect("save the subgroup meta");
            NamespaceRepository::new(&store)
                .nest(&group_id, &sub)
                .expect("nest the subgroup under the root");
            CapabilitiesRepository::new(&store)
                .set_subgroup_visibility(&sub, VisibilityMode::Open)
                .expect("open the subgroup");
            let _key_id = GroupKeyring::new(&store, sub)
                .store_key(&[0x34; 32])
                .expect("store the subgroup key");
            sub
        }
        LocalRole::MarkerOnly | LocalRole::Outsider => group_id,
    };

    let context_id = ContextId::from([0xC7; 32]);
    handle
        .put(
            &key::ContextMeta::new(context_id),
            &types::ContextMeta::new(
                key::ApplicationMeta::new(application_id),
                INITIAL_ROOT,
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
    register_context_in_group(&store, &owning_group, &context_id).expect("register the context");
    if holds_marker {
        handle
            .put(
                &key::ContextIdentity::new(context_id, executor),
                &types::ContextIdentity {
                    private_key: Some(*executor_sk.as_bytes()),
                },
            )
            .expect("put the membership marker");
    }

    Fixture {
        harness,
        context_id,
        executor,
        store,
        account,
        group_id,
    }
}

impl Fixture {
    /// Merge-apply a delta a peer authored, the way the node's delta applier
    /// hands it to the executor.
    async fn apply_remote_delta(&self) -> Result<(), ExecuteError> {
        self.harness
            .context_client
            .apply_remote_delta(&self.context_id, &self.executor, Vec::new(), None, None)
            .await
            .map(drop)
    }

    /// Run `method` as an ordinary, locally-invoked call (JSON-RPC, an event
    /// handler, an xcall).
    pub(super) async fn call_locally(&self, method: &str) -> Result<ExecuteResponse, ExecuteError> {
        self.harness
            .context_client
            .execute(
                &self.context_id,
                &self.executor,
                method.to_owned(),
                Vec::new(),
                None,
            )
            .await
    }

    pub(super) fn root(&self) -> Hash {
        let meta: types::ContextMeta = self
            .store
            .handle()
            .get(&key::ContextMeta::new(self.context_id))
            .expect("read the context meta")
            .expect("the context meta exists");
        meta.root_hash.into()
    }

    pub(super) fn write_was_kept(&self) -> bool {
        let root = self.root();
        assert!(
            root == Hash::from(COMMITTED_ROOT) || root == Hash::from(INITIAL_ROOT),
            "the root is either the module's or the initial one, got {root}"
        );
        root == Hash::from(COMMITTED_ROOT)
    }
}

/// The bug: a read-only replica threw away every delta it was sent, so it
/// served stale state until a snapshot repaired it.
#[actix::test]
async fn a_read_only_member_applies_a_delta_a_peer_authored() {
    for role in [
        GroupMemberRole::ReadOnly,
        GroupMemberRole::ReadOnlyTee,
        GroupMemberRole::RelayTee,
    ] {
        let fx = fixture(LocalRole::Role(role.clone())).await;
        fx.apply_remote_delta().await.expect("the apply runs");
        assert!(
            fx.write_was_kept(),
            "a {role:?} member must apply a verified delta from a peer"
        );
    }
}

/// The control: a writer applies a peer's delta, before the fix and after.
#[actix::test]
async fn a_member_applies_a_delta_a_peer_authored() {
    let fx = fixture(LocalRole::Role(GroupMemberRole::Member)).await;
    fx.apply_remote_delta().await.expect("the apply runs");
    assert!(fx.write_was_kept());
}

/// Applying is not authoring: a read-only node's own mutating call is still
/// discarded.
#[actix::test]
async fn a_read_only_member_still_cannot_write_locally() {
    for role in [
        GroupMemberRole::ReadOnly,
        GroupMemberRole::ReadOnlyTee,
        GroupMemberRole::RelayTee,
    ] {
        let fx = fixture(LocalRole::Role(role.clone())).await;
        let response = fx.call_locally("set").await.expect("the call runs");
        assert!(
            !fx.write_was_kept(),
            "a {role:?} member's own write must be discarded"
        );
        assert!(
            response.read_only_write_discarded,
            "the response says the {role:?}'s write was dropped, so RPC can refuse it"
        );
    }
}

/// The gap: a node read-only at the namespace root that reaches an Open-subgroup
/// context by inheritance only held no row there, so the read-only discard did
/// not fire and its own writes committed (and were signed and published). The
/// role it inherits is its role in the context, as a direct row would be.
#[actix::test]
async fn an_inherited_read_only_member_cannot_write_locally() {
    for role in [
        GroupMemberRole::ReadOnly,
        GroupMemberRole::ReadOnlyTee,
        GroupMemberRole::RelayTee,
    ] {
        for method in ["set", "__calimero_sync_next"] {
            let fx = fixture(LocalRole::InheritedFromRoot(role.clone())).await;
            let response = fx.call_locally(method).await.expect("the call runs");
            assert!(
                !fx.write_was_kept(),
                "a {role:?} inherited from the root must not commit its own `{method}`"
            );
            // `__calimero_sync_next` named locally is a state op, which the
            // authorship gate drops rather than the read-only one.
            assert_eq!(
                response.read_only_write_discarded,
                method == "set",
                "the response says the {role:?}'s `{method}` was dropped as a read-only write"
            );
        }
    }
}

/// Inheriting a read-only role still makes the node a replica: it applies a
/// delta a peer authored, as a direct read-only member does.
#[actix::test]
async fn an_inherited_read_only_member_applies_a_delta_a_peer_authored() {
    for role in [
        GroupMemberRole::ReadOnly,
        GroupMemberRole::ReadOnlyTee,
        GroupMemberRole::RelayTee,
    ] {
        let fx = fixture(LocalRole::InheritedFromRoot(role.clone())).await;
        fx.apply_remote_delta().await.expect("the apply runs");
        assert!(
            fx.write_was_kept(),
            "a {role:?} inherited from the root must apply a verified delta from a peer"
        );
    }
}

/// The control: an inherited writer's own call commits.
#[actix::test]
async fn an_inherited_member_writes_locally() {
    let fx = fixture(LocalRole::InheritedFromRoot(GroupMemberRole::Member)).await;
    let response = fx.call_locally("set").await.expect("the call runs");
    assert!(fx.write_was_kept());
    assert!(!response.read_only_write_discarded);
}

/// A node that stopped being a member keeps no marker, so it cannot run the
/// context at all — neither its own call nor a peer's delta.
#[actix::test]
async fn an_outsider_can_neither_write_nor_apply() {
    let fx = fixture(LocalRole::Outsider).await;
    assert!(matches!(
        fx.call_locally("set").await,
        Err(ExecuteError::Unauthorized { .. })
    ));
    assert!(matches!(
        fx.apply_remote_delta().await,
        Err(ExecuteError::Unauthorized { .. })
    ));
    assert!(!fx.write_was_kept());
}

/// A node still holding the marker without a membership row is not a replica:
/// a delta applied there is discarded, as it always was.
#[actix::test]
async fn a_marker_without_membership_does_not_apply_deltas() {
    let fx = fixture(LocalRole::MarkerOnly).await;
    fx.apply_remote_delta().await.expect("the apply runs");
    assert!(!fx.write_was_kept());
}

/// The relaxation is reachable only through the applier's entry point: naming
/// `__calimero_sync_next` in an ordinary call (as a JSON-RPC client can) is
/// still this node writing, and a read-only node's write is still discarded.
#[actix::test]
async fn a_read_only_member_cannot_author_by_calling_the_apply_method() {
    for role in [
        GroupMemberRole::ReadOnly,
        GroupMemberRole::ReadOnlyTee,
        GroupMemberRole::RelayTee,
    ] {
        let fx = fixture(LocalRole::Role(role.clone())).await;
        fx.call_locally("__calimero_sync_next")
            .await
            .expect("the call runs");
        assert!(
            !fx.write_was_kept(),
            "a {role:?} member naming the apply method must not commit"
        );
    }
}

/// A remote-delta marker on any other method is refused outright, not run
/// under either rule.
#[actix::test]
async fn a_remote_delta_marker_on_an_ordinary_method_is_refused() {
    let fx = fixture(LocalRole::Role(GroupMemberRole::ReadOnly)).await;
    let outcome = fx
        .harness
        .manager
        .send(ExecuteRequest {
            context: fx.context_id,
            executor: fx.executor,
            method: "set".to_owned(),
            payload: Vec::new(),
            atomic: None,
            xcall_origin: None,
            xcall_depth: 0,
            delegation: None,
            read_as: None,
            tee_trigger: None,
            event_handler: false,
            write_source: WriteSource::RemoteDelta,
            governance_position: None,
        })
        .await
        .expect("mailbox");
    assert!(matches!(outcome, Err(ExecuteError::Unauthorized { .. })));
    assert!(!fx.write_was_kept());
}
