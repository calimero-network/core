//! Which bytecode a group context runs when the node's application row, shared by
//! every group naming its id, holds a blob the context's own group never named.

use calimero_context::test_support::{enrol, enrol_holder};
use calimero_context_client::client::CreateContextParams;
use calimero_context_client::local_governance::{
    GroupOp, NamespaceOp, SignedGroupOp, SignedNamespaceOp,
};
use calimero_context_client::messages::ExecuteError;
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_governance_store::{
    apply_local_signed_group_op, apply_signed_namespace_op, register_context_in_group,
    CapabilitiesRepository, GroupKeyring, MembershipRepository, MetaRepository,
    NamespaceRepository, NodeDeviceRepository,
};
use calimero_node_primitives::test_fixtures::signed_wasm;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{self, GroupMetaValue, GroupTarget};
use calimero_store::types;
use calimero_wasm_abi::embed::write_embedded_state_schema;
use calimero_wasm_abi::schema::{Manifest, MigrationEdgeAbi};
use futures_util::io::Cursor;
use serial_test::serial;

use crate::test_node_harness::{boot_test_node, TestNode};

/// The application id every group here names.
const APP: [u8; 32] = [0xA7; 32];

/// A release a group names: its application id, bytecode blob and version.
type Release<'a> = (ApplicationId, BlobId, &'a str);
/// The root a context starts from, before any run.
const INITIAL_ROOT: [u8; 32] = [0x01; 32];
const HONEST_ROOT: u8 = 0xB0; // committed by the group's own release
const SQUAT_ROOT: u8 = 0xEE; // committed by the squatted release
const NEWER_ROOT: u8 = 0x33; // committed by another group's newer release

/// A module whose `init` and `set` commit `[root; 32]` with an empty `Actions` artifact. Memory:
/// the root at 0, the artifact at 32, the `{ptr, len}` descriptors at 64 and 80.
fn module(root: u8) -> Vec<u8> {
    let root = format!("\\{root:02x}").repeat(32);
    wat::parse_str(format!(
        r#"(module
            (import "env" "commit" (func $commit (param i64 i64)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{root}")
            (data (i32.const 32) "\00\00\00\00\00")
            (data (i32.const 64)
                "\00\00\00\00\00\00\00\00\20\00\00\00\00\00\00\00"
                "\20\00\00\00\00\00\00\00\05\00\00\00\00\00\00\00")
            (func (export "init") (call $commit (i64.const 64) (i64.const 80)))
            (func (export "set") (call $commit (i64.const 64) (i64.const 80))))"#
    ))
    .expect("parse the module")
}

/// [`module`] with an embedded ABI declaring `state_version`.
fn module_at(root: u8, state_version: u32, edge: Option<(&str, u32)>) -> Vec<u8> {
    let mut manifest = Manifest::new();
    manifest.state_version = Some(state_version);
    if let Some((method, from_version)) = edge {
        manifest.migrations = vec![MigrationEdgeAbi {
            method: method.to_owned(),
            from_version,
        }];
    }
    write_embedded_state_schema(&module(root), &manifest).expect("embed the ABI")
}

/// Module compiles run on the node's global runtime, which must be
/// multi-threaded, and outlive any one test.
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

struct Node {
    node: TestNode,
    executor: PublicKey,
    executor_sk: PrivateKey,
}

impl core::ops::Deref for Node {
    type Target = TestNode;

    fn deref(&self) -> &TestNode {
        &self.node
    }
}

impl Node {
    fn new(node: TestNode) -> Self {
        global_runtime();
        let _root = NodeDeviceRepository::new(&node.store)
            .provision_account_root()
            .expect("provision the account root an initialised node has");
        let executor_sk = PrivateKey::from([0x22; 32]);
        Self {
            node,
            executor: executor_sk.public_key(),
            executor_sk,
        }
    }

    /// Store `wasm` as the signed bundle a node runs: raw wasm never runs.
    async fn add_blob(&self, wasm: &[u8]) -> (BlobId, u64) {
        self.node_client
            .add_blob(Cursor::new(signed_wasm(wasm)), None, None)
            .await
            .expect("store the blob")
    }

    /// A group whose admin is someone else, created at the first of `releases` and
    /// moved through the rest, each by a real `TargetApplicationSet`.
    fn group(&self, group_id: ContextGroupId, admin_sk: &PrivateKey, releases: &[Release<'_>]) {
        let admin = enrol(&self.store, &group_id, &admin_sk.public_key());
        let (first_app, first_blob, _) = releases[0];
        MetaRepository::new(&self.store)
            .save(
                &group_id,
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id: first_app,
                        bytecode_id: *first_blob.digest(),
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
            .expect("save the group meta");
        MembershipRepository::new(&self.store)
            .add_member(&group_id, &admin, GroupMemberRole::Admin)
            .expect("seat the admin");
        for (nonce, (application_id, blob, version)) in (1..).zip(releases) {
            let op = SignedGroupOp::sign(
                admin_sk,
                group_id.to_bytes().into(),
                vec![],
                nonce,
                GroupOp::TargetApplicationSet {
                    bytecode_id: (*blob.digest()).into(),
                    target_application_id: *application_id,
                    package: "com.acme.app".to_owned(),
                    version: (*version).to_owned(),
                },
            )
            .expect("sign the target");
            apply_local_signed_group_op(&self.store, &op).expect("apply the target");
        }
    }

    /// The row a local install writes for `application_id`; `signed` for a
    /// verified bundle of the id's publisher.
    fn install(&self, application_id: ApplicationId, blob: BlobId, version: &str, signed: bool) {
        self.store
            .handle()
            .put(
                &key::ApplicationMeta::new(application_id),
                &types::ApplicationMeta::new(
                    key::BlobMeta::new(blob),
                    1,
                    "https://apps.example/app.mpk".into(),
                    Box::default(),
                    key::BlobMeta::new(BlobId::from([0; 32])),
                    types::PackageInfo {
                        package: "com.acme.app".into(),
                        version: version.into(),
                        signer_id: if signed { "did:key:publisher" } else { "" }.into(),
                        state_version: 0,
                    },
                ),
            )
            .expect("install the release");
    }

    /// A context in `group_id` this node joined but has not run yet: no
    /// activation marker, bound to `application_id`.
    fn joined_context(
        &self,
        group_id: ContextGroupId,
        context_id: ContextId,
        application_id: ApplicationId,
    ) {
        NamespaceRepository::new(&self.store)
            .replace_identity(&group_id, &self.executor, self.executor_sk.as_bytes())
            .expect("seat this node's namespace identity");
        let account = enrol_holder(&self.store, &group_id, &self.executor);
        MembershipRepository::new(&self.store)
            .add_member(&group_id, &account, GroupMemberRole::Member)
            .expect("seat the local node");
        let _key_id = GroupKeyring::new(&self.store, group_id)
            .store_key(&[0x33; 32])
            .expect("store the group key");
        let mut handle = self.store.handle();
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
        handle
            .put(
                &key::ContextIdentity::new(context_id, self.executor),
                &types::ContextIdentity {
                    private_key: Some(*self.executor_sk.as_bytes()),
                },
            )
            .expect("put the membership marker");
        drop(handle);
        register_context_in_group(&self.store, &group_id, &context_id)
            .expect("register the context");
    }

    async fn set(&self, context_id: ContextId) -> Result<(), ExecuteError> {
        self.context_client
            .execute(
                &context_id,
                &self.executor,
                "set".to_owned(),
                Vec::new(),
                None,
            )
            .await
            .map(drop)
    }

    fn root(&self, context_id: ContextId) -> Hash {
        let meta: types::ContextMeta = self
            .store
            .handle()
            .get(&key::ContextMeta::new(context_id))
            .expect("read the context meta")
            .expect("the context meta exists");
        meta.root_hash.into()
    }
}

/// What a group admin did to squat `APP` on every node of its group: register
/// a context naming `APP` with its own `squat`, which a raw-wasm bind once filled.
async fn squat_application(node: &Node, squat: &[u8]) {
    let (blob, _size) = node.add_blob(squat).await;
    let admin_sk = PrivateKey::from([0x66; 32]);
    let ns = ContextGroupId::from([0x61; 32]);
    let admin = enrol(&node.store, &ns, &admin_sk.public_key());
    MetaRepository::new(&node.store)
        .save(
            &ns,
            &GroupMetaValue {
                target: GroupTarget {
                    application_id: ApplicationId::from(APP),
                    bytecode_id: *blob.digest(),
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
        .expect("save the squatting group's meta");
    MembershipRepository::new(&node.store)
        .add_member(&ns, &admin, GroupMemberRole::Admin)
        .expect("seat the squatting admin");
    let group_key = [0x62; 32];
    let key_id = GroupKeyring::new(&node.store, ns)
        .store_key(&group_key)
        .expect("store the squatting group's key");
    let registered = GroupOp::ContextRegistered {
        context_id: ContextId::from([0x63; 32]),
        application_id: ApplicationId::from(APP),
        blob_id: blob,
        source: "https://squat.example/app.wasm".to_owned(),
        service_name: None,
        package: "com.acme.app".to_owned(),
        version: "1.0.0".to_owned(),
    };
    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Group {
            group_id: ns.to_bytes().into(),
            key_id: key_id.into(),
            encrypted: GroupKeyring::encrypt_op(&group_key, &registered).expect("encrypt"),
            key_rotation: None,
        },
    )
    .expect("sign the registration");
    apply_signed_namespace_op(&node.store, &op).expect("apply the registration");
    // The unsigned row that bind left behind.
    node.install(ApplicationId::from(APP), blob, "1.0.0", false);
}

/// A node holding a squatted row joins an honest group for the same id before
/// fetching its release: it refuses to run rather than run the squatted wasm.
#[tokio::test]
#[serial(boot_test_node)]
async fn a_squatted_row_never_runs_in_another_groups_context() {
    let node = Node::new(boot_test_node().await);
    squat_application(&node, &module(SQUAT_ROOT)).await;

    // The group's release, not yet fetched by this node.
    let honest = module(HONEST_ROOT);
    let (honest_blob, _) = node.add_blob(&honest).await;
    let _ = node
        .node_client
        .delete_blob(honest_blob)
        .await
        .expect("drop the unfetched release");
    let group_id = ContextGroupId::from([0x6A; 32]);
    node.group(
        group_id,
        &PrivateKey::from([0x11; 32]),
        &[(ApplicationId::from(APP), honest_blob, "1.0.0")],
    );
    let context_id = ContextId::from([0xC7; 32]);
    node.joined_context(group_id, context_id, ApplicationId::from(APP));

    let outcome = node.set(context_id).await;
    assert_ne!(
        node.root(context_id),
        Hash::from([SQUAT_ROOT; 32]),
        "the squatted wasm ran on the honest group's context"
    );
    assert!(
        matches!(outcome, Err(ExecuteError::ApplicationNotInstalled { .. })),
        "the run must be refused until the group's release is here, got {outcome:?}"
    );
    assert_eq!(
        calimero_context::activation::activated_bytecode(&node.store, &context_id),
        None,
        "nothing may bind the context to the squatted blob"
    );

    // Once the group's own release arrives, the context runs it.
    let (fetched, _) = node.add_blob(&honest).await;
    assert_eq!(fetched, honest_blob, "the fixture names the release's blob");
    node.set(context_id)
        .await
        .expect("the group's release runs");
    assert_eq!(node.root(context_id), Hash::from([HONEST_ROOT; 32]));
}

/// A context lagging in one group is not moved to the newer release this node
/// installed for another group: it keeps running its own group's release.
#[tokio::test]
#[serial(boot_test_node)]
async fn a_lagging_context_runs_its_own_groups_release() {
    let node = Node::new(boot_test_node().await);
    let own = module_at(HONEST_ROOT, 1, None);
    let newer = module_at(NEWER_ROOT, 3, Some(("migrate_v2_to_v3", 2)));
    let (own_blob, _) = node.add_blob(&own).await;
    let (newer_blob, _) = node.add_blob(&newer).await;

    // Another group on this node moved to the newer release, and this node
    // installed it under the shared id.
    node.group(
        ContextGroupId::from([0x5A; 32]),
        &PrivateKey::from([0x12; 32]),
        &[(ApplicationId::from(APP), newer_blob, "3.0.0")],
    );
    node.install(ApplicationId::from(APP), newer_blob, "3.0.0", true);

    let group_id = ContextGroupId::from([0x6A; 32]);
    node.group(
        group_id,
        &PrivateKey::from([0x11; 32]),
        &[(ApplicationId::from(APP), own_blob, "1.0.0")],
    );
    let context_id = ContextId::from([0xC7; 32]);
    node.joined_context(group_id, context_id, ApplicationId::from(APP));

    node.set(context_id).await.expect("the context runs");
    assert_eq!(
        node.root(context_id),
        Hash::from([HONEST_ROOT; 32]),
        "the context must run its own group's release, not the node's newer row"
    );
    assert_ne!(
        calimero_context::activation::activated_bytecode(&node.store, &context_id),
        Some(*newer_blob.digest()),
        "the context must not be bound to another group's release"
    );
}

/// Creating a context in an honest group before this node has its release must
/// not init the new context with the squatted row's wasm.
#[tokio::test]
#[serial(boot_test_node)]
async fn a_squatted_row_never_inits_a_new_context_in_another_group() {
    let node = Node::new(boot_test_node().await);
    squat_application(&node, &module(SQUAT_ROOT)).await;
    let honest = module(HONEST_ROOT);
    let (honest_blob, _) = node.add_blob(&honest).await;
    let _ = node
        .node_client
        .delete_blob(honest_blob)
        .await
        .expect("drop the unfetched release");
    let group_id = ContextGroupId::from([0x6A; 32]);
    node.group(
        group_id,
        &PrivateKey::from([0x11; 32]),
        &[(ApplicationId::from(APP), honest_blob, "1.0.0")],
    );
    node.joined_context(
        group_id,
        ContextId::from([0xC7; 32]),
        ApplicationId::from(APP),
    );
    let account = enrol_holder(&node.store, &group_id, &node.executor);
    CapabilitiesRepository::new(&node.store)
        .set_member_capability(
            &group_id,
            &account,
            MemberCapabilities::CAN_CREATE_CONTEXT.bits(),
        )
        .expect("let this node create contexts");

    let created = node
        .context_client
        .create_context(
            "local".to_owned(),
            &ApplicationId::from(APP),
            CreateContextParams {
                service_name: None,
                identity_secret: None,
                init_params: Vec::new(),
                seed: Some([0x5E; 32]),
                group_id,
                name: None,
            },
        )
        .await;
    let err = created.expect_err("the squatted row must not init a context");
    assert!(
        err.to_string()
            .contains("holds a release its group never named"),
        "{err}"
    );
}

/// The control: a member holding its group's release runs it and is bound to it.
#[tokio::test]
#[serial(boot_test_node)]
async fn a_member_runs_its_groups_release() {
    let node = Node::new(boot_test_node().await);
    let (blob, _) = node.add_blob(&module(HONEST_ROOT)).await;
    let group_id = ContextGroupId::from([0x6A; 32]);
    node.group(
        group_id,
        &PrivateKey::from([0x11; 32]),
        &[(ApplicationId::from(APP), blob, "1.0.0")],
    );
    node.install(ApplicationId::from(APP), blob, "1.0.0", true);
    let context_id = ContextId::from([0xC7; 32]);
    node.joined_context(group_id, context_id, ApplicationId::from(APP));

    node.set(context_id).await.expect("the release runs");
    assert_eq!(node.root(context_id), Hash::from([HONEST_ROOT; 32]));
    assert_eq!(
        calimero_context::activation::activated_bytecode(&node.store, &context_id),
        Some(*blob.digest())
    );
}

/// The control: a member on its group's first release replays the ladder from it.
/// The harness has no source for the next rung, so the hop stays pending.
#[tokio::test]
#[serial(boot_test_node)]
async fn a_member_behind_its_group_replays_from_its_own_release() {
    let node = Node::new(boot_test_node().await);
    let (first, _) = node.add_blob(&module_at(HONEST_ROOT, 1, None)).await;
    let (second, _) = node.add_blob(&module_at(NEWER_ROOT, 1, None)).await;
    let (first_app, second_app) = (
        ApplicationId::from([0xA1; 32]),
        ApplicationId::from([0xA2; 32]),
    );
    let group_id = ContextGroupId::from([0x6A; 32]);
    node.group(
        group_id,
        &PrivateKey::from([0x11; 32]),
        &[(first_app, first, "1.0.0"), (second_app, second, "2.0.0")],
    );
    node.install(first_app, first, "1.0.0", false);
    node.install(second_app, second, "2.0.0", false);
    let context_id = ContextId::from([0xC7; 32]);
    node.joined_context(group_id, context_id, first_app);

    node.set(context_id).await.expect("the context runs");
    assert_eq!(node.root(context_id), Hash::from([HONEST_ROOT; 32]));
    assert_eq!(
        calimero_context::activation::activated_bytecode(&node.store, &context_id),
        Some(*first.digest()),
        "the replay starts from the release the member runs"
    );
}
