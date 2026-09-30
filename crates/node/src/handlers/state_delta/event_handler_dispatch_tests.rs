//! A delta's author names the handlers its receivers run; only declared handlers may run.
//!
//! Driven through a real `ContextManager` and a hand-written module whose every
//! export commits [`COMMITTED_ROOT`], so "did it run?" is a read of the root.

use calimero_context::test_support::{enrol, enrol_holder};
use calimero_context_client::messages::ExecuteError;
use calimero_context_client::tee_trigger::TeeTriggerCause;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    register_context_in_group, GroupKeyring, MembershipRepository, MetaRepository,
    NamespaceRepository, NodeDeviceRepository,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::events::ExecutionEvent;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{self, GroupMetaValue, GroupTarget};
use calimero_store::types;
use calimero_wasm_abi::schema::{Manifest, Method};
use futures_util::io::Cursor;
use serial_test::serial;

use super::events::execute_event_handlers_parsed;
use crate::test_node_harness::{boot_test_node, TestNode};

const COMMITTED_ROOT: [u8; 32] = [0x5A; 32]; // what every export of MODULE commits
const INITIAL_ROOT: [u8; 32] = [0x01; 32]; // the context's root before any run

/// Three exports that each `commit` [`COMMITTED_ROOT`] with a one-byte artifact:
/// the root at 0, the artifact at 32, the `{ptr, len}` descriptors at 64 and 80.
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
        (func (export "on_event") (call $commit (i64.const 64) (i64.const 80)))
        (func (export "transfer") (call $commit (i64.const 64) (i64.const 80))))
"#;

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
    // Already initialised by an earlier test in this process is fine.
    let _ = std::thread::scope(|scope| {
        scope
            .spawn(|| runtime.block_on(async { calimero_utils_actix::init_global_runtime() }))
            .join()
    });
}

fn method(name: &str, handler: bool) -> Method {
    Method {
        name: name.to_owned(),
        handler,
        ..Method::default()
    }
}

/// [`MODULE`], with an ABI declaring `on_event` a handler. It also declares the
/// SDK's `__calimero_sync_next` one, which the node must refuse regardless.
fn module_declaring_on_event() -> Vec<u8> {
    let manifest = Manifest {
        schema_version: "wasm-abi/1".to_owned(),
        types: Default::default(),
        methods: vec![
            method("__calimero_sync_next", true),
            method("on_event", true),
            method("transfer", false),
        ],
        events: Vec::new(),
        state_root: None,
        state_version: None,
        migrations: Vec::new(),
    };
    let wasm = wat::parse_str(MODULE).expect("parse the module");
    calimero_wasm_abi::embed::write_embedded_state_schema(&wasm, &manifest).expect("embed the ABI")
}

struct Fixture {
    node: TestNode,
    context_id: ContextId,
    application_id: ApplicationId,
    executor: PublicKey,
    group_id: ContextGroupId,
    blob: [u8; 32],
}

/// A one-group namespace with one context on `wasm`, and `node` a member of it.
async fn fixture(node: TestNode, wasm: Vec<u8>) -> Fixture {
    global_runtime();
    let store = node.store.clone();
    let _root = NodeDeviceRepository::new(&store)
        .provision_account_root()
        .expect("provision the account root an initialised node has");

    let (blob_id, size) = node
        .node_client
        .add_blob(Cursor::new(wasm.clone()), Some(wasm.len() as u64), None)
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
                    package: "com.test.handlers".into(),
                    version: "1.0.0".into(),
                    signer_id: "did:key:test".into(),
                    state_version: 0,
                },
            ),
        )
        .expect("install the application");

    let group_id = ContextGroupId::from([0x6A; 32]);
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

    let executor_sk = PrivateKey::from([0x22; 32]);
    let executor = executor_sk.public_key();
    NamespaceRepository::new(&store)
        .replace_identity(&group_id, &executor, executor_sk.as_bytes())
        .expect("seat this node's namespace identity");
    let account = enrol_holder(&store, &group_id, &executor);
    MembershipRepository::new(&store)
        .add_member(&group_id, &account, GroupMemberRole::Member)
        .expect("seat the local node");

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
    register_context_in_group(&store, &group_id, &context_id).expect("register the context");
    handle
        .put(
            &key::ContextIdentity::new(context_id, executor),
            &types::ContextIdentity {
                private_key: Some(*executor_sk.as_bytes()),
            },
        )
        .expect("put the membership marker");

    Fixture {
        node,
        context_id,
        application_id,
        executor,
        group_id,
        blob: *blob_id.digest(),
    }
}

fn event(handler: &str) -> ExecutionEvent {
    ExecutionEvent {
        kind: "Named".to_owned(),
        data: b"{}".to_vec(),
        handler: Some(handler.to_owned()),
    }
}

impl Fixture {
    /// Whether receiving `events` settles them, rather than keeping them for replay.
    async fn settles(&self, events: &[ExecutionEvent]) -> bool {
        execute_event_handlers_parsed(
            &self.node.context_client,
            &calimero_governance_store::NotFolded,
            &self.context_id,
            &self.executor,
            &[0x33; 32],
            events,
        )
        .await
        .expect("run the events")
    }

    /// Leave this context running its blob while the group moves to another.
    fn move_group_past_this_node(&self) {
        let repo = MetaRepository::new(&self.node.store);
        let mut meta = repo
            .load(&self.group_id)
            .expect("read the group meta")
            .expect("the group meta exists");
        meta.target.bytecode_id = [0xB2; 32];
        repo.save(&self.group_id, &meta)
            .expect("save the group meta");
        calimero_context::activation::record_activation(
            &self.node.store,
            &self.context_id,
            self.blob,
        );
    }

    /// Run `events` the way a receiver runs a peer's delta's events, and report
    /// whether any of their handlers committed.
    async fn receive(&self, events: &[ExecutionEvent]) -> bool {
        let _ = execute_event_handlers_parsed(
            &self.node.context_client,
            &calimero_governance_store::NotFolded,
            &self.context_id,
            &self.executor,
            &[0x33; 32],
            events,
        )
        .await;
        let meta: types::ContextMeta = self
            .node
            .store
            .handle()
            .get(&key::ContextMeta::new(self.context_id))
            .expect("read the context meta")
            .expect("the context meta exists");
        assert!(
            meta.root_hash == COMMITTED_ROOT || meta.root_hash == INITIAL_ROOT,
            "the root is either the module's or the initial one"
        );
        meta.root_hash == COMMITTED_ROOT
    }
}

#[tokio::test]
#[serial(boot_test_node)]
async fn a_received_event_does_not_run_a_method_the_app_never_declared_a_handler() {
    let fx = fixture(boot_test_node().await, module_declaring_on_event()).await;

    // An owner-only mutation the ABI does not declare, and the sync export it
    // does, which is never an event's to run.
    assert!(
        !fx.receive(&[event("transfer"), event("__calimero_sync_next")])
            .await,
        "an event handler the app never declared ran as this node"
    );
}

#[tokio::test]
#[serial(boot_test_node)]
async fn a_refused_handler_names_its_context_and_application() {
    let fx = fixture(boot_test_node().await, module_declaring_on_event()).await;

    let refusal = fx
        .node
        .context_client
        .execute_event_handler(&fx.context_id, &fx.executor, "transfer".to_owned(), vec![])
        .await;

    assert!(
        matches!(
            refusal,
            Err(ExecuteError::NotAnEventHandler { context_id, application_id })
                if context_id == fx.context_id && application_id == fx.application_id
        ),
        "{refusal:?}"
    );
}

#[tokio::test]
#[serial(boot_test_node)]
async fn a_declared_handler_runs() {
    let fx = fixture(boot_test_node().await, module_declaring_on_event()).await;

    assert!(
        fx.receive(&[event("on_event")]).await,
        "a handler the app declared did not run"
    );
}

#[tokio::test]
#[serial(boot_test_node)]
async fn an_app_whose_abi_cannot_be_read_runs_no_handler() {
    let wasm = wat::parse_str(MODULE).expect("parse the module");
    let fx = fixture(boot_test_node().await, wasm).await;

    assert!(
        !fx.receive(&[event("on_event")]).await,
        "a handler ran for an app with no readable ABI"
    );
}

#[tokio::test]
#[serial(boot_test_node)]
async fn a_tee_trigger_on_an_event_must_name_a_declared_handler() {
    let fx = fixture(boot_test_node().await, module_declaring_on_event()).await;
    let fire = |method: &str| {
        fx.node.context_client.execute_tee_trigger(
            &fx.context_id,
            &fx.executor,
            b"{}".to_vec(),
            TeeTriggerCause::Event {
                cause: [0x33; 32],
                method: method.to_owned(),
            },
        )
    };

    assert!(matches!(
        fire("transfer").await,
        Err(ExecuteError::NotAnEventHandler { .. })
    ));
    // Past the handler gate, it stops at the next one: this node is no TEE.
    assert!(matches!(
        fire("on_event").await,
        Err(ExecuteError::Unauthorized { .. })
    ));
}

#[tokio::test]
#[serial(boot_test_node)]
async fn a_refused_handler_at_the_groups_version_is_settled() {
    let fx = fixture(boot_test_node().await, module_declaring_on_event()).await;

    assert!(
        fx.settles(&[event("transfer")]).await,
        "a refusal this node's version will always repeat was kept for replay"
    );
}

#[tokio::test]
#[serial(boot_test_node)]
async fn a_refused_handler_waits_while_this_node_runs_an_older_version() {
    let fx = fixture(boot_test_node().await, module_declaring_on_event()).await;
    fx.move_group_past_this_node();

    let refusal = fx
        .node
        .context_client
        .execute_event_handler(&fx.context_id, &fx.executor, "transfer".to_owned(), vec![])
        .await;
    assert!(
        matches!(
            refusal,
            Err(ExecuteError::EventHandlerAwaitsUpgrade { context_id, application_id })
                if context_id == fx.context_id && application_id == fx.application_id
        ),
        "{refusal:?}"
    );
    assert!(
        !fx.settles(&[event("transfer")]).await,
        "a call the group's newer version may declare was settled on an older one"
    );
    assert!(
        !fx.receive(&[event("transfer")]).await,
        "an undeclared handler ran while waiting"
    );
}
