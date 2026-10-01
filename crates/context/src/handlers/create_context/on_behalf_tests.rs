//! A context created on a member's behalf, driven through a live
//! `ContextManager` with a real (hand-written) wasm module.
//!
//! The module's `init` commits the account it runs as as its root hash, so
//! "whose authority did `init` run under?" is a plain read of the new context's
//! root afterwards — the author's account for a delegated creation, which is
//! the property that makes the member, not the relay, the context's owner.

use std::sync::Arc;

use calimero_account::{ContextCreationDelegation, ContextCreationTerms, ContextCreationWarrant};
use calimero_context_client::messages::{CreateContextRequest, CreateContextResponse};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_governance_store::{
    get_group_for_context, CapabilitiesRepository, GroupKeyring, MembershipRepository,
    MetaRepository, MetadataRepository, NamespaceRepository, NodeDeviceRepository,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::db::InMemoryDB;
use calimero_store::key::{self, GroupMetaValue, GroupTarget};
use calimero_store::{types, Store};
use futures_util::io::Cursor;

use crate::test_support::{actor, credential, enrol, enrol_holder};

/// `init` reads the executing account (`account_id`) into memory and commits
/// it as the root hash, with an empty artifact; `set` does the same with a
/// one-byte artifact, as a method run must carry one. Layout: the account at 0,
/// the artifact byte at 32, the root descriptor at 64 (`{ptr: 0, len: 32}`),
/// the empty-artifact descriptor at 80 (`{ptr: 32, len: 0}`), the register-read
/// descriptor at 96 (`{ptr: 0, len: 32}`), and the one-byte artifact descriptor
/// at 112 (`{ptr: 32, len: 1}`).
const MODULE: &str = r#"
    (module
        (import "env" "account_id" (func $account_id (param i64)))
        (import "env" "read_register" (func $read_register (param i64 i64) (result i32)))
        (import "env" "commit" (func $commit (param i64 i64)))
        (memory (export "memory") 1)
        (data (i32.const 64)
            "\00\00\00\00\00\00\00\00\20\00\00\00\00\00\00\00"
            "\20\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00"
            "\00\00\00\00\00\00\00\00\20\00\00\00\00\00\00\00"
            "\20\00\00\00\00\00\00\00\01\00\00\00\00\00\00\00")
        (data (i32.const 32) "\01")
        (func (export "init")
            (call $account_id (i64.const 0))
            (drop (call $read_register (i64.const 0) (i64.const 96)))
            (call $commit (i64.const 64) (i64.const 80)))
        (func (export "set")
            (call $account_id (i64.const 0))
            (drop (call $read_register (i64.const 0) (i64.const 96)))
            (call $commit (i64.const 64) (i64.const 112))))
"#;

const GROUP: [u8; 32] = [0x6B; 32];
const APP: [u8; 32] = [0xA8; 32];
const SEED: [u8; 32] = [0x5E; 32];
const INIT: &[u8] = b"{}";

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

/// How the relay (this node) stands in the group.
#[derive(Clone, Copy, Debug)]
enum Standing {
    /// A `Member` holding `CAN_AUTHOR_ON_BEHALF` — a self-hosted
    /// `--delegated-access` node.
    Granted,
    /// A TEE admitted in relay mode.
    RelayTee,
    /// A TEE replica, which never acts for members.
    ReadOnlyTee,
    /// A `Member` with no grant.
    Ungranted,
}

struct Fixture {
    harness: actor::Harness,
    store: Store,
    group: ContextGroupId,
    application_id: ApplicationId,
    author_sk: PrivateKey,
    author: calimero_account::AccountId,
    relay_pk: PublicKey,
    relay: calimero_account::AccountId,
}

async fn fixture(relay_standing: Standing, author_may_create: bool) -> Fixture {
    global_runtime();
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    NodeDeviceRepository::new(&store)
        .provision_account_root()
        .expect("provision the account root an initialised node has");
    let harness = actor::over(store.clone()).await;

    let wasm = wat::parse_str(MODULE).expect("parse the module");
    let (blob_id, size) = harness
        .node_client
        .add_blob(Cursor::new(wasm.clone()), Some(wasm.len() as u64), None)
        .await
        .expect("store the wasm");
    let application_id = ApplicationId::from(APP);
    store
        .handle()
        .put(
            &key::ApplicationMeta::new(application_id),
            &types::ApplicationMeta::new(
                key::BlobMeta::new(blob_id),
                size,
                "file://test.wasm".into(),
                vec![].into(),
                key::BlobMeta::new([0; 32].into()),
                types::PackageInfo {
                    package: "com.test.create".into(),
                    version: "1.0.0".into(),
                    signer_id: "did:key:test".into(),
                    state_version: 0,
                },
            ),
        )
        .expect("install the application");

    let group = ContextGroupId::from(GROUP);
    let admin = PrivateKey::from([0x11; 32]).public_key();
    let admin_account = enrol(&store, &group, &admin);
    MetaRepository::new(&store)
        .save(
            &group,
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
    let membership = MembershipRepository::new(&store);
    membership
        .add_member(&group, &admin_account, GroupMemberRole::Admin)
        .expect("seat the admin");
    let _key_id = GroupKeyring::new(&store, group)
        .store_key(&[0x33; 32])
        .expect("store the group key");

    // The author: a member with no node here. Only its binding is folded, as
    // it would be after it joined through an admitter.
    let author_sk = PrivateKey::from([0x44; 32]);
    let author = enrol(&store, &group, &author_sk.public_key());
    membership
        .add_member(&group, &author, GroupMemberRole::Member)
        .expect("seat the author");
    if author_may_create {
        CapabilitiesRepository::new(&store)
            .set_member_capability(
                &group,
                &author,
                MemberCapabilities::CAN_CREATE_CONTEXT.bits(),
            )
            .expect("the author may create contexts");
    }

    // The relay: this node, holding no create rights of its own.
    let relay_sk = PrivateKey::from([0x22; 32]);
    let relay_pk = relay_sk.public_key();
    NamespaceRepository::new(&store)
        .replace_identity(&group, &relay_pk, relay_sk.as_bytes())
        .expect("seat this node's namespace identity");
    let relay = enrol_holder(&store, &group, &relay_pk);
    let (role, caps) = match relay_standing {
        Standing::Granted => (
            GroupMemberRole::Member,
            MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
        ),
        Standing::RelayTee => (GroupMemberRole::RelayTee, 0),
        Standing::ReadOnlyTee => (
            GroupMemberRole::ReadOnlyTee,
            MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
        ),
        Standing::Ungranted => (GroupMemberRole::Member, 0),
    };
    membership
        .add_member(&group, &relay, role)
        .expect("seat the relay");
    CapabilitiesRepository::new(&store)
        .set_member_capability(&group, &relay, caps)
        .expect("set the relay's capabilities");

    Fixture {
        harness,
        store,
        group,
        application_id,
        author_sk,
        author,
        relay_pk,
        relay,
    }
}

impl Fixture {
    fn terms(&self) -> ContextCreationTerms {
        ContextCreationTerms {
            group: self.group.to_bytes(),
            seed: SEED,
            author_account: self.author,
            executor: self.relay,
            application_id: self.application_id,
            service_name: None,
            name: Some("general".to_owned()),
            init_hash: ContextCreationWarrant::init_hash(INIT),
            account_heads: vec![],
            governance_floor: vec![],
            nonce: 1,
            not_after: u64::MAX,
        }
    }

    /// The bundle the server assembles: the author's warrant and proof, this
    /// node's own credential and key.
    fn delegation_with(&self, terms: ContextCreationTerms) -> ContextCreationDelegation {
        ContextCreationDelegation {
            warrant: Box::new(ContextCreationWarrant::sign(&self.author_sk, terms).expect("sign")),
            author_proof: credential(&self.author_sk.public_key()),
            executor_proof: crate::join_credential::build(&self.store, &self.group, &self.relay_pk)
                .expect("this node's credential"),
            executor_key: self.relay_pk,
        }
    }

    fn delegation(&self) -> ContextCreationDelegation {
        self.delegation_with(self.terms())
    }

    async fn create(
        &self,
        delegation: ContextCreationDelegation,
    ) -> eyre::Result<CreateContextResponse> {
        self.harness
            .context_client
            .create_context_on_behalf(delegation, INIT.to_vec())
            .await
    }

    fn context_id(&self) -> ContextId {
        ContextId::from_seed(SEED)
    }

    fn created(&self) -> bool {
        self.store
            .handle()
            .has(&key::ContextMeta::new(self.context_id()))
            .expect("read the context meta")
    }

    fn root(&self) -> [u8; 32] {
        let meta: types::ContextMeta = self
            .store
            .handle()
            .get(&key::ContextMeta::new(self.context_id()))
            .expect("read the context meta")
            .expect("the context meta exists");
        meta.root_hash
    }
}

#[actix::test]
async fn a_relay_creates_a_context_for_a_member_and_init_runs_as_the_member() {
    for standing in [Standing::RelayTee] {
        let fx = fixture(standing, true).await;
        let created = fx
            .create(fx.delegation())
            .await
            .unwrap_or_else(|err| panic!("{standing:?}: must create: {err:?}"));

        assert_eq!(created.context_id, fx.context_id(), "the seed fixes the id");
        assert_eq!(
            created.identity, fx.relay_pk,
            "this node holds the context's keys"
        );
        assert_eq!(created.group_id, Some(fx.group));
        assert_eq!(
            get_group_for_context(&fx.store, &fx.context_id()).expect("read"),
            Some(fx.group),
            "{standing:?}: registered in the group"
        );
        assert_eq!(
            fx.root(),
            *fx.author.as_bytes(),
            "{standing:?}: init must run as the AUTHOR's account, not the relay's"
        );
        assert_ne!(fx.root(), *fx.relay.as_bytes());
        let name = MetadataRepository::new(&fx.store)
            .context_metadata(&fx.group, &fx.context_id())
            .expect("read")
            .and_then(|record| record.name);
        assert_eq!(
            name.as_deref(),
            Some("general"),
            "{standing:?}: the name rides in the op"
        );
        assert!(
            fx.store
                .handle()
                .has(&key::ContextIdentity::new(fx.context_id(), fx.relay_pk))
                .expect("read"),
            "{standing:?}: the relay keeps an identity to sign the member's later writes"
        );
    }
}

/// A `Member` holding `CAN_AUTHOR_ON_BEHALF` passes the creation gate, but every
/// peer refuses the entries `init` would write for the member under its key:
/// only a `RelayTee` signs on a member's behalf. So it is refused before `init`
/// runs, and nothing is created or published.
#[actix::test]
async fn a_relay_that_is_not_a_relay_tee_is_refused_before_init_runs() {
    use calimero_governance_store::OnBehalfRefusal;

    let mut fx = fixture(Standing::Granted, true).await;
    let _ = fx.harness.broadcast_topics();
    let err = fx.create(fx.delegation()).await.expect_err("refused");
    assert_eq!(
        err.downcast_ref::<OnBehalfRefusal>(),
        Some(&OnBehalfRefusal::SignerNotARelay),
        "{err:?}"
    );
    assert!(!fx.created());
    assert!(
        fx.harness.broadcast_topics().is_empty(),
        "a refused creation publishes nothing"
    );
}

/// A delegated write through a relay that is no longer a `RelayTee` is refused
/// before it runs, with a reason the API reports as a 403, rather than run and
/// published as entries every peer refuses.
#[actix::test]
async fn a_delegated_write_through_a_relay_that_is_not_a_relay_tee_is_refused() {
    use calimero_context_client::messages::{DelegatedWriteRefusal, ExecuteError};

    let fx = fixture(Standing::RelayTee, true).await;
    let created = fx.create(fx.delegation()).await.expect("create");
    let root = fx.root();
    MembershipRepository::new(&fx.store)
        .set_role(&fx.group, &fx.relay, GroupMemberRole::Member)
        .expect("the relay is now a plain member");
    CapabilitiesRepository::new(&fx.store)
        .set_member_capability(
            &fx.group,
            &fx.relay,
            MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
        )
        .expect("still holding the authorship bit, which the warrant gate honours");

    let args = br#"{}"#.to_vec();
    let warrant = calimero_account::Warrant::sign(
        &fx.author_sk,
        calimero_account::WarrantTerms {
            context: created.context_id,
            author_account: fx.author,
            executor: fx.relay,
            app_version: fx.application_id,
            method: "set".to_owned(),
            intent_hash: calimero_account::Warrant::intent_hash("set", &args),
            account_heads: vec![],
            governance_floor: vec![],
            nonce: 2,
            not_after: u64::MAX,
        },
    )
    .expect("sign");
    let delegation = calimero_account::Delegation {
        warrant: Box::new(warrant),
        author_proof: credential(&fx.author_sk.public_key()),
        executor_proof: crate::join_credential::build(&fx.store, &fx.group, &fx.relay_pk)
            .expect("this node's credential"),
        executor_key: fx.relay_pk,
    };
    let err = fx
        .harness
        .context_client
        .execute_with_origin(
            &created.context_id,
            &created.identity,
            "set".to_owned(),
            args,
            None,
            None,
            0,
            Some(Box::new(delegation)),
        )
        .await
        .expect_err("refused before it runs");
    assert!(
        matches!(
            err,
            ExecuteError::DelegatedWriteRefused {
                reason: DelegatedWriteRefusal::ExecutorIsNotARelay,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(fx.root(), root, "nothing ran");
}

/// Published, not just applied locally: peers learn of the context from the
/// namespace topic.
#[actix::test]
async fn the_delegated_registration_is_broadcast() {
    let mut fx = fixture(Standing::RelayTee, true).await;
    let _ = fx.harness.broadcast_topics();
    fx.create(fx.delegation()).await.expect("create");
    assert!(
        !fx.harness.broadcast_topics().is_empty(),
        "the registration must reach the namespace topic"
    );
}

#[actix::test]
async fn an_author_without_create_rights_is_refused_before_init_runs() {
    let fx = fixture(Standing::Granted, false).await;
    let err = fx.create(fx.delegation()).await.expect_err("refused");
    assert_eq!(
        err.downcast_ref::<calimero_governance_store::creation_gate::CreationRefusal>(),
        Some(&calimero_governance_store::creation_gate::CreationRefusal::AuthorMayNotCreate),
        "{err:?}"
    );
    assert!(!fx.created(), "a refusal must not leave a context behind");
    assert_eq!(
        get_group_for_context(&fx.store, &fx.context_id()).expect("read"),
        None
    );
}

#[actix::test]
async fn a_relay_that_may_not_act_for_members_is_refused() {
    for standing in [Standing::Ungranted, Standing::ReadOnlyTee] {
        let fx = fixture(standing, true).await;
        let err = fx.create(fx.delegation()).await.expect_err("refused");
        assert!(
            matches!(
                err.downcast_ref::<calimero_governance_store::creation_gate::CreationRefusal>(),
                Some(calimero_governance_store::creation_gate::CreationRefusal::Executor(_))
            ),
            "{standing:?}: {err:?}"
        );
        assert!(!fx.created(), "{standing:?}");
    }
}

/// The gap this feature closes, pinned from the other side: the same relay,
/// creating as itself, is refused — it holds no create rights of its own.
#[actix::test]
async fn the_relay_cannot_create_as_itself() {
    let fx = fixture(Standing::Granted, true).await;
    let result = fx
        .harness
        .manager
        .send(CreateContextRequest {
            protocol: "local".to_owned(),
            seed: Some(SEED),
            application_id: fx.application_id,
            service_name: None,
            identity_secret: None,
            init_params: INIT.to_vec(),
            group_id: fx.group,
            name: None,
            delegation: None,
        })
        .await
        .expect("mailbox");
    let err = result.expect_err("a relay is not a context creator");
    assert!(err.to_string().contains("CAN_CREATE_CONTEXT"), "{err}");
    assert!(!fx.created());
}

#[actix::test]
async fn a_caller_chosen_identity_is_refused_with_a_warrant() {
    let fx = fixture(Standing::Granted, true).await;
    let delegation = fx.delegation();
    let result = fx
        .harness
        .manager
        .send(CreateContextRequest {
            protocol: "local".to_owned(),
            seed: Some(SEED),
            application_id: fx.application_id,
            service_name: None,
            identity_secret: Some(PrivateKey::from([0x99; 32])),
            init_params: INIT.to_vec(),
            group_id: fx.group,
            name: Some("general".to_owned()),
            delegation: Some(Box::new(delegation)),
        })
        .await
        .expect("mailbox");
    let err = result.expect_err("refused");
    assert!(err.to_string().contains("identity_secret"), "{err}");
    assert!(!fx.created());
}

/// A request that asks for anything other than what the warrant pins is
/// refused, whichever field differs.
#[actix::test]
async fn a_request_that_departs_from_its_warrant_is_refused() {
    let fx = fixture(Standing::Granted, true).await;
    let base = || CreateContextRequest {
        protocol: "local".to_owned(),
        seed: Some(SEED),
        application_id: fx.application_id,
        service_name: None,
        identity_secret: None,
        init_params: INIT.to_vec(),
        group_id: fx.group,
        name: Some("general".to_owned()),
        delegation: Some(Box::new(fx.delegation())),
    };
    let variants = [
        CreateContextRequest {
            seed: Some([0x01; 32]),
            ..base()
        },
        CreateContextRequest {
            seed: None,
            ..base()
        },
        CreateContextRequest {
            name: Some("random".to_owned()),
            ..base()
        },
        CreateContextRequest {
            name: None,
            ..base()
        },
        CreateContextRequest {
            service_name: Some("other".to_owned()),
            ..base()
        },
    ];
    for request in variants {
        let result = fx.harness.manager.send(request).await.expect("mailbox");
        let err = result.expect_err("a departure from the warrant must be refused");
        assert!(
            err.to_string().contains("exactly as its warrant pins"),
            "{err}"
        );
    }
    assert!(!fx.created());
}

#[actix::test]
async fn a_warrant_for_an_application_the_group_does_not_target_is_refused() {
    let fx = fixture(Standing::Granted, true).await;
    let other = ApplicationId::from([0x01; 32]);
    let delegation = fx.delegation_with(ContextCreationTerms {
        application_id: other,
        ..fx.terms()
    });
    let err = fx.create(delegation).await.expect_err("refused");
    assert!(
        matches!(
            err.downcast_ref::<crate::error::ContextError>(),
            Some(crate::error::ContextError::DelegatedApplicationNotTargeted { .. })
        ),
        "{err:?}"
    );
    assert!(!fx.created());
}

#[actix::test]
async fn a_spent_creation_warrant_cannot_create_twice() {
    let fx = fixture(Standing::RelayTee, true).await;
    fx.create(fx.delegation()).await.expect("first");
    let _refused = fx
        .create(fx.delegation())
        .await
        .expect_err("the same warrant must not create a second time");
}

/// A warrant another operator was issued is not spendable here: this node's
/// credential does not match the executor account the author named.
#[actix::test]
async fn a_warrant_issued_to_another_relay_is_refused() {
    let fx = fixture(Standing::Granted, true).await;
    let other = crate::test_support::account_for(&PrivateKey::from([0x77; 32]).public_key());
    let delegation = fx.delegation_with(ContextCreationTerms {
        executor: other,
        ..fx.terms()
    });
    let _refused = fx.create(delegation).await.expect_err("refused");
    assert!(!fx.created());
}

/// The relay creating the context is the node holding it, so the member's
/// first write through that relay lands at once — no wait for a sync that
/// would otherwise answer the first write with "not initialized yet".
#[actix::test]
async fn the_first_delegated_write_after_a_delegated_creation_lands_immediately() {
    let fx = fixture(Standing::RelayTee, true).await;
    let created = fx.create(fx.delegation()).await.expect("create");

    let args = br#"{}"#.to_vec();
    let warrant = calimero_account::Warrant::sign(
        &fx.author_sk,
        calimero_account::WarrantTerms {
            context: created.context_id,
            author_account: fx.author,
            executor: fx.relay,
            app_version: fx.application_id,
            method: "set".to_owned(),
            intent_hash: calimero_account::Warrant::intent_hash("set", &args),
            account_heads: vec![],
            governance_floor: vec![],
            // A nonce the creation warrant did not spend in this context's
            // ledger: the creation spent 1.
            nonce: 2,
            not_after: u64::MAX,
        },
    )
    .expect("sign");
    let delegation = calimero_account::Delegation {
        warrant: Box::new(warrant),
        author_proof: credential(&fx.author_sk.public_key()),
        executor_proof: crate::join_credential::build(&fx.store, &fx.group, &fx.relay_pk)
            .expect("this node's credential"),
        executor_key: fx.relay_pk,
    };
    let outcome = fx
        .harness
        .context_client
        .execute_with_origin(
            &created.context_id,
            &created.identity,
            "set".to_owned(),
            args,
            None,
            None,
            0,
            Some(Box::new(delegation)),
        )
        .await
        .expect("the first delegated write must land, not answer 'not initialized'");
    let _ = outcome;
    assert_eq!(fx.root(), *fx.author.as_bytes(), "and it ran as the member");
}

/// The creation warrant spends its nonce in the new context's ledger, which
/// later delegated writes into that context draw from — so a write reusing the
/// creation's nonce is a replay.
#[actix::test]
async fn a_write_reusing_the_creation_nonce_is_refused() {
    let fx = fixture(Standing::RelayTee, true).await;
    let created = fx.create(fx.delegation()).await.expect("create");
    let args = br#"{}"#.to_vec();
    let warrant = calimero_account::Warrant::sign(
        &fx.author_sk,
        calimero_account::WarrantTerms {
            context: created.context_id,
            author_account: fx.author,
            executor: fx.relay,
            app_version: fx.application_id,
            method: "set".to_owned(),
            intent_hash: calimero_account::Warrant::intent_hash("set", &args),
            account_heads: vec![],
            governance_floor: vec![],
            nonce: 1,
            not_after: u64::MAX,
        },
    )
    .expect("sign");
    let delegation = calimero_account::Delegation {
        warrant: Box::new(warrant),
        author_proof: credential(&fx.author_sk.public_key()),
        executor_proof: crate::join_credential::build(&fx.store, &fx.group, &fx.relay_pk)
            .expect("credential"),
        executor_key: fx.relay_pk,
    };
    let _refused = fx
        .harness
        .context_client
        .execute_with_origin(
            &created.context_id,
            &created.identity,
            "set".to_owned(),
            args,
            None,
            None,
            0,
            Some(Box::new(delegation)),
        )
        .await
        .expect_err("nonce 1 was spent by the creation");
}

/// The gap a namespace founded through a relay hit: it targets no application,
/// so a creation warrant for any application is refused. The member chooses
/// the application through the same relay, and the creation then succeeds.
#[actix::test]
async fn a_member_chooses_the_first_application_through_the_relay_then_creates() {
    use calimero_account::{GovernanceDelegation, GovernanceOpKind, GovernanceTerms};
    use calimero_context_client::group::{DelegatedGovernanceOp, GovernOnBehalfRequest};
    use calimero_context_client::local_governance::GroupOp;
    use calimero_context_config::types::BytecodeId;
    use calimero_primitives::application::ZERO_APPLICATION_ID;

    let fx = fixture(Standing::RelayTee, true).await;
    let meta = MetaRepository::new(&fx.store);
    let mut group_meta = meta.load(&fx.group).expect("read").expect("meta");
    let bundle = group_meta.target.bytecode_id;
    group_meta.target = GroupTarget {
        application_id: ZERO_APPLICATION_ID,
        bytecode_id: [0; 32],
        package: Box::default(),
        version: Box::default(),
    };
    meta.save(&fx.group, &group_meta)
        .expect("no application yet");
    CapabilitiesRepository::new(&fx.store)
        .set_member_capability(
            &fx.group,
            &fx.author,
            (MemberCapabilities::CAN_CREATE_CONTEXT | MemberCapabilities::MANAGE_APPLICATION)
                .bits(),
        )
        .expect("the author may choose and create");

    let err = fx.create(fx.delegation()).await.expect_err("no target yet");
    assert!(
        matches!(
            err.downcast_ref::<crate::error::ContextError>(),
            Some(crate::error::ContextError::DelegatedApplicationNotTargeted { .. })
        ),
        "{err:?}"
    );

    // What the member signs leaves `bytecode_id` for the relay; what the relay
    // publishes carries the bundle it resolved.
    let chosen = |bytecode_id| GroupOp::TargetApplicationSet {
        bytecode_id: BytecodeId::from(bytecode_id),
        target_application_id: fx.application_id,
        package: "com.test.create".to_owned(),
        version: "1.0.0".to_owned(),
    };
    let form = borsh::to_vec(&chosen([0; 32])).expect("encode");
    let delegation = GovernanceDelegation {
        warrant: Box::new(
            calimero_account::GovernanceWarrant::sign(
                &fx.author_sk,
                GovernanceTerms {
                    scope: fx.group.to_bytes(),
                    kind: GovernanceOpKind::Group,
                    author_account: fx.author,
                    executor: fx.relay,
                    op_hash: calimero_account::GovernanceWarrant::op_hash(
                        GovernanceOpKind::Group,
                        &form,
                    ),
                    account_heads: vec![],
                    governance_floor: vec![],
                    nonce: 1,
                    not_after: u64::MAX,
                },
            )
            .expect("sign"),
        ),
        author_proof: credential(&fx.author_sk.public_key()),
        executor_proof: crate::join_credential::build(&fx.store, &fx.group, &fx.relay_pk)
            .expect("this node's credential"),
        executor_key: fx.relay_pk,
    };
    let _published = fx
        .harness
        .context_client
        .govern_on_behalf(GovernOnBehalfRequest {
            delegation,
            op: DelegatedGovernanceOp::Group {
                group_id: fx.group,
                op: chosen(bundle),
            },
        })
        .await
        .expect("the first application rides the relay");
    let target = meta.load(&fx.group).expect("read").expect("meta").target;
    assert_eq!(target.application_id, fx.application_id);
    assert_eq!(target.bytecode_id, bundle);

    let _created = fx
        .create(fx.delegation())
        .await
        .expect("the group now targets the warrant's application");
    assert!(fx.created());
}
