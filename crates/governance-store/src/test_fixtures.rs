//! Shared test helpers for `group_store` unit tests.
//!
//! Extracted from `tests.rs` so the membership-specific test module
//! (`membership/tests.rs`, added in #2306) can share the same setup
//! without duplicating fixtures. Crate-internal: visible to all
//! submodules under `group_store/`, invisible outside.

use super::{MembershipRepository, MetaRepository, NamespaceRepository};
use std::sync::Arc;

use calimero_account::AccountId;
use calimero_context_client::local_governance::{GroupOp, JoinAccountCredential};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::db::InMemoryDB;
use calimero_store::key::{GroupMetaValue, GroupParentRef, GroupTarget};
use calimero_store::Store;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;

/// A fresh account root: its signing key and the genesis that names it.
///
/// Returned as a pair so a test can mint SEVERAL credentials under one account —
/// a rejoin, or a person's second device — which is the whole distinction the
/// account plane exists to draw.
pub(super) fn test_account_root() -> (PrivateKey, calimero_account::AccountGenesis) {
    let root_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
    let genesis = calimero_account::AccountGenesis::new(root_sk.public_key());
    (root_sk, genesis)
}

/// The agreement secret a fixture-issued device holds, derived from the device
/// seed so any test can recover it.
///
/// Production mints this pair on the node and certifies only the public half, so a
/// fixture that invented a `kem_pk` with no private counterpart produced devices
/// that could never open an envelope addressed to them. That is not a cosmetic
/// gap: it silently made every "the recipient cannot decrypt this" assertion pass
/// for the wrong reason, since no device in the crate could decrypt anything.
/// Deriving both halves from the seed keeps the fixture faithful and lets a test
/// assert the positive direction too.
pub(super) fn device_kem_secret(device: [u8; 32]) -> calimero_crypto::X25519SecretKey {
    calimero_crypto::X25519SecretKey::from(device)
}

/// Certify `sign_pk` as `device` under an existing account root.
pub(super) fn join_account_for(
    root_sk: &PrivateKey,
    genesis: calimero_account::AccountGenesis,
    sign_pk: &PublicKey,
    device: [u8; 32],
    device_epoch: u32,
) -> Box<JoinAccountCredential> {
    let cert = calimero_account::DeviceCert::sign(
        root_sk,
        genesis.account_id(),
        calimero_account::DeviceId::from(device),
        sign_pk,
        // The real public half of `device_kem_secret(device)`, so an envelope
        // sealed to this device can actually be opened by a test holding it.
        &calimero_account::KemPublicKey::from(*device_kem_secret(device).public_key().as_bytes()),
        0,
        device_epoch,
    )
    .expect("the account root signs its own device cert");
    Box::new(JoinAccountCredential {
        genesis,
        chain: vec![],
        statement: cert,
    })
}

/// A credential that VERIFIES, certified for `sign_pk` under an account root
/// derived from that same key.
///
/// There is no filler counterpart any more: a structurally-valid-but-unverifiable
/// credential can no longer reach the code a test would be aiming at, because
/// naming a member means naming the account its credential certifies and the
/// signer/member check refuses the pair first.
pub(super) fn real_join_account(sign_pk: &PublicKey) -> Box<JoinAccountCredential> {
    // The account root is derived from the signing key, NOT random, so the same
    // key always yields the same account. Tests name a member once and then use
    // it across several ops — a rejoin, a removal, a later assertion — and a
    // random root would make each of those a DIFFERENT principal. It also lets
    // [`account_for`] answer "which account will this key speak for" anywhere,
    // including before anything has been applied.
    let root_sk = PrivateKey::from(*(*sign_pk));
    let genesis = calimero_account::AccountGenesis::new(root_sk.public_key());
    // The device id is derived from the signing key, NOT fixed. A constant here
    // made every credential claim the SAME device, so the second enrolment in
    // any store was refused as an `AccountReassignment` — one device cannot
    // speak for two accounts. That looked like a flip bug and was a fixture bug.
    join_account_for(&root_sk, genesis, sign_pk, *sign_pk.as_ref(), 0)
}

/// The account [`real_join_account`] certifies for this signing key.
pub(super) fn account_for(sign_pk: &PublicKey) -> AccountId {
    real_join_account(sign_pk).statement.account
}

/// A store shaped like an initialised node's: it has an account root.
///
/// `merod init` provisions one, so every node that has ever run has a root
/// unless it was started with `--no-account-root`. A fixture without one models
/// a state production does not reach, and it used to pass only because
/// `ensure_account_root` minted lazily — the fixture was relying on the very
/// side effect that made a root-free node impossible.
///
/// Use [`test_store_without_account_root`] to model the root-free node
/// deliberately.
pub(super) fn test_store() -> Store {
    let store = test_store_without_account_root();
    crate::NodeDeviceRepository::new(&store)
        .provision_account_root()
        .expect("provision the account root an initialised node has");
    store
}

/// A store shaped like a node started with `--no-account-root`.
///
/// It holds no signing root, so anything that must certify a device — its own or
/// anyone's — fails. That is the point: such a node's device is enabled by a
/// certificate its account root signed elsewhere.
pub(super) fn test_store_without_account_root() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

pub(super) fn test_group_id() -> ContextGroupId {
    ContextGroupId::from([0xAA; 32])
}

/// Build a `MemberRemoved` op with placeholder cross-DAG claims for
/// tests that don't exercise the convergence-detection path. The
/// claims here are deliberately zero/empty so a receiver verifying
/// against actual post-apply state will see a mismatch — tests that
/// hit the apply path either ignore the mismatch (it's a warn-log,
/// not a hard reject) or use the real `compute_*` helpers.
pub(super) fn dummy_member_removed_op(member: AccountId) -> GroupOp {
    GroupOp::MemberRemoved {
        member,
        expected_group_state_hash: [0u8; 32],
        expected_context_state_hashes: Vec::new(),
    }
}

pub(super) fn test_meta() -> GroupMetaValue {
    GroupMetaValue {
        target: GroupTarget {
            application_id: ApplicationId::from([0xCC; 32]),
            bytecode_id: [0xBB; 32],
            package: Box::default(),
            version: Box::default(),
        },
        created_at: 1_700_000_000,
        admin_identity: AccountId::from([0x01; 32]),
        owner_identity: AccountId::from([0x01; 32]),
        migration: None,
        auto_join: true,
    }
}

/// Variant of [`test_meta`] that wires both the admin and owner pin to the
/// supplied account. Used by tests that want a specific admin.
pub(super) fn sample_meta_with_admin(admin: AccountId) -> GroupMetaValue {
    GroupMetaValue {
        target: GroupTarget {
            application_id: ApplicationId::from([0xCC; 32]),
            bytecode_id: [0xBB; 32],
            package: Box::default(),
            version: Box::default(),
        },
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
    }
}

/// Bootstrap a namespace root with a freshly-generated admin: writes the
/// root meta (`admin == owner`), an `Admin` member row, and the admin's
/// stored identity. Returns the admin's `(PrivateKey, PublicKey)` so the
/// caller can sign ops and seed subgroup metas. Collapses the
/// meta-save + add_member + store_identity setup duplicated across the
/// namespace apply tests.
pub(super) fn bootstrap_namespace_with_admin(
    store: &Store,
    ns_id: [u8; 32],
) -> (PrivateKey, PublicKey) {
    bootstrap_namespace_with_admin_account(store, ns_id).0
}

/// [`bootstrap_namespace_with_admin`], also returning the admin's account.
///
/// **This enrols the admin, it does not merely add a row.** Membership names an
/// account, and a signed op resolves its signer through the binding rows — so a
/// fixture that wrote the row without the binding would produce an admin whose
/// own ops are refused, and every apply test built on it would fail for a reason
/// that has nothing to do with what it is testing.
pub(super) fn bootstrap_namespace_with_admin_account(
    store: &Store,
    ns_id: [u8; 32],
) -> ((PrivateKey, PublicKey), AccountId) {
    let admin_sk_bytes: [u8; 32] = rand::RngExt::random(&mut UnwrapErr(SysRng));
    let admin_sk = PrivateKey::from(admin_sk_bytes);
    let admin_pk = admin_sk.public_key();
    let ns_gid = ContextGroupId::from(ns_id);
    let admin_account = enrol_member(store, &ns_gid, &admin_pk);
    MetaRepository::new(store)
        .save(&ns_gid, &sample_meta_with_admin(admin_account))
        .unwrap();
    MembershipRepository::new(store)
        .add_member(&ns_gid, &admin_account, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(store)
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes)
        .unwrap();
    ((admin_sk, admin_pk), admin_account)
}

/// The genesis op a namespace founder signs, and the account it establishes.
///
/// `NamespaceCreated` carries a credential now — the founder is the one member
/// no join op ever admits, so this is the only place its device can be bound —
/// and the apply verifies the credential certifies the key that signed the op.
/// A test that hand-built the op without one would be rejected before it
/// reached whatever it meant to exercise.
pub(super) fn namespace_genesis_for(
    founder_sk: &PrivateKey,
) -> (
    calimero_context_client::local_governance::NamespaceOp,
    AccountId,
) {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp};
    let credential = founder_credential(founder_sk);
    let founder = credential.statement.account;
    (
        NamespaceOp::Root(RootOp::NamespaceCreated {
            founder,
            account: credential,
        }),
        founder,
    )
}

/// The founder's credential, derived DETERMINISTICALLY from its signing key.
///
/// A random root would be fine for building the op, but a test that asserts
/// "the meta names the founder" then has to receive the account back from
/// whatever built it. Deriving the account root from the signing key means
/// [`founder_account_for`] can answer the same question anywhere in the test,
/// including in blocks that never build a genesis at all.
fn founder_credential(founder_sk: &PrivateKey) -> Box<JoinAccountCredential> {
    let root_sk = PrivateKey::from(*founder_sk.public_key());
    let genesis = calimero_account::AccountGenesis::new(root_sk.public_key());
    join_account_for(&root_sk, genesis, &founder_sk.public_key(), [0x3E; 32], 0)
}

/// The account [`namespace_genesis_for`] will establish for this founder.
pub(super) fn founder_account_for(founder_sk: &PrivateKey) -> AccountId {
    founder_credential(founder_sk).statement.account
}

/// A genesis op that DECLARES `founder` while carrying `signer_sk`'s own
/// credential — the forgery shape.
///
/// When the two disagree the apply refuses, which is the point: naming somebody
/// else as founder no longer needs a separate `signer == founder` check,
/// because the credential cannot certify a key it was not issued for.
pub(super) fn namespace_genesis_naming(
    founder: AccountId,
    signer_sk: &PrivateKey,
) -> calimero_context_client::local_governance::NamespaceOp {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp};
    NamespaceOp::Root(RootOp::NamespaceCreated {
        founder,
        account: real_join_account(&signer_sk.public_key()),
    })
}

/// An enrolled participant: a deterministic signing key plus the account it
/// speaks for, already bound in `namespace`.
///
/// Most tests name a participant once and then use it in BOTH spaces — the
/// repository rows take the account, the gates take the key they sign with — so
/// returning the pair keeps a test from having to say which it meant twice.
///
/// The key is derived from `seed` so a test that wants two distinct
/// participants gets them, and a test that re-derives one gets the same key.
pub(super) fn enrolled(
    store: &Store,
    namespace: &ContextGroupId,
    seed: u8,
) -> (PublicKey, AccountId) {
    let sign_pk = PublicKey::from([seed; 32]);
    let account = enrol_member(store, namespace, &sign_pk);
    (sign_pk, account)
}

/// Bind `sign_pk` to a fresh account in `namespace` and return that account.
///
/// The fixture form of what a join op does: writes the device binding AND the
/// endorser row, which is what makes `member_account_in_namespace` resolve the
/// key afterwards. Without the endorser the binding is recorded and inert.
///
/// Self-endorsing is fine here for the same reason genesis self-endorses: the
/// caller writes the member row alongside, so the endorser IS a member.
pub(super) fn enrol_member(
    store: &Store,
    namespace: &ContextGroupId,
    sign_pk: &PublicKey,
) -> AccountId {
    let credential = real_join_account(sign_pk);
    let account = credential.statement.account;
    let bindings = crate::AccountBindingRepository::new(store);
    let _ = bindings
        .apply_link(
            namespace,
            &credential.genesis,
            &credential.chain,
            &credential.statement,
        )
        .expect("store the binding");
    bindings
        .record_endorser(namespace, account, &account)
        .expect("record the endorser");
    account
}

/// The [`crate::DeviceSecret`] belonging to a member enrolled by [`enrol_member`].
///
/// [`real_join_account`] derives the device id from the signing key and
/// [`device_kem_secret`] derives the agreement secret from that same device id, so
/// this reconstructs what the member's own node would hold — which is what lets a
/// test open an envelope addressed to that device, and prove the leaver's cannot.
pub(super) fn device_secret_for(sign_pk: &PublicKey) -> crate::DeviceSecret {
    let device: [u8; 32] = *sign_pk.as_ref();
    crate::DeviceSecret {
        device: calimero_account::DeviceId::from(device),
        kem_secret: device_kem_secret(device),
    }
}

/// Shortcut for nesting one group under another inside tests, unwrapping
/// the result. Used by membership-path tests across both `tests.rs` and
/// `membership/tests.rs`.
pub(super) fn nest_for_test(store: &Store, parent: &ContextGroupId, child: &ContextGroupId) {
    NamespaceRepository::new(store).nest(parent, child).unwrap();
}

/// Like [`nest_for_test`] but writes the parent edge directly to the
/// store, bypassing `NamespaceRepository::nest`'s `MAX_NAMESPACE_DEPTH`
/// guard. Used by tests that need to construct chains longer than the
/// walkers tolerate (depth-overflow regression tests for
/// `enumerate_inherited`, `is_open_chain_to_namespace`, etc.). The
/// resulting tree is intentionally malformed from the production API's
/// perspective — only the walker bail-out path should ever observe it.
///
/// **Asymmetric edge.** Only writes the child→parent `GroupParentRef`
/// edge. The parent→child `GroupChildIndex` edge that real `nest`
/// writes is *not* set, so `list_children` / `collect_descendants` /
/// any downward walk will not see these synthetic edges. Use this
/// helper only for tests that walk upward (resolve, check_path,
/// is_open_chain_to_namespace, enumerate_inherited).
pub(super) fn nest_for_test_unchecked(
    store: &Store,
    parent: &ContextGroupId,
    child: &ContextGroupId,
) {
    let mut handle = store.handle();
    handle
        .put(&GroupParentRef::new(child.to_bytes()), &parent.to_bytes())
        .unwrap();
}

/// An [`AtCutAuthorizer`](crate::authorizer::AtCutAuthorizer) that answers every
/// gate with one fixed verdict, standing in for a projection that HAS folded the
/// op's cited ancestry.
///
/// Its whole purpose is to DISAGREE with the live store rows, so a test can prove
/// which resolver an apply gate actually consults. A gate that honors this
/// authorizer decides identically on every replica regardless of fold progress;
/// a gate that falls through to the live rows does not, which is the divergence
/// these tests guard.
///
/// Tests must pass a NON-EMPTY `parents`: the empty-cut contract requires real
/// authorizers to abstain (`None`) on an empty cut, and a test that passed `&[]`
/// would silently be exercising the live path it means to rule out.
pub(super) struct FixedAuthorizer(pub(super) bool);

impl crate::authorizer::AtCutAuthorizer for FixedAuthorizer {
    fn is_admin_at_cut(
        &self,
        _group: &ContextGroupId,
        _signer: &PublicKey,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        Some(self.0)
    }

    fn is_admin_or_capability_at_cut(
        &self,
        _group: &ContextGroupId,
        _signer: &PublicKey,
        _capability: u32,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        Some(self.0)
    }

    fn is_admin_account_at_cut(
        &self,
        _group: &ContextGroupId,
        _member: &AccountId,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        Some(self.0)
    }

    fn is_last_admin_at_cut(
        &self,
        _group: &ContextGroupId,
        _member: &AccountId,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        Some(false)
    }

    fn membership_path_at_cut(
        &self,
        _group: &ContextGroupId,
        _member: &AccountId,
        _parents: &[[u8; 32]],
    ) -> Option<crate::authorizer::AtCutMembershipPath> {
        None
    }
}

/// A non-empty causal cut for apply-auth tests. Value is irrelevant — only
/// non-emptiness matters (see [`FixedAuthorizer`]).
pub(super) const TEST_CUT: [[u8; 32]; 1] = [[0xAB; 32]];

/// An [`AtCutAuthorizer`](crate::authorizer::AtCutAuthorizer) standing in for a
/// projection that has NOT folded the ancestry the op's cut cites — the
/// catching-up replica, mid-backfill.
///
/// It abstains from every gate (`None`) AND reports the cut as unresolvable. That
/// pairing is the whole point: an abstention alone used to be indistinguishable
/// from "no apply-auth context", so the gate quietly answered from the live rows —
/// a different cut — and two replicas decided the same op differently. A gate that
/// honors `can_resolve_cut` refuses to answer instead.
pub(super) struct UnresolvableAuthorizer;

impl crate::authorizer::AtCutAuthorizer for UnresolvableAuthorizer {
    fn is_admin_at_cut(
        &self,
        _group: &ContextGroupId,
        _signer: &PublicKey,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn is_admin_or_capability_at_cut(
        &self,
        _group: &ContextGroupId,
        _signer: &PublicKey,
        _capability: u32,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn is_admin_account_at_cut(
        &self,
        _group: &ContextGroupId,
        _member: &AccountId,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn is_last_admin_at_cut(
        &self,
        _group: &ContextGroupId,
        _member: &AccountId,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn membership_path_at_cut(
        &self,
        _group: &ContextGroupId,
        _member: &AccountId,
        _parents: &[[u8; 32]],
    ) -> Option<crate::authorizer::AtCutMembershipPath> {
        None
    }

    fn can_resolve_cut(&self, _group: &ContextGroupId, _parents: &[[u8; 32]]) -> bool {
        false
    }
}

/// Enrol `sign_pk` as THIS NODE's device in `namespace`.
///
/// The difference from [`enrol_member`] is the secret half. `enrol_member`
/// records a binding whose `kem_pk` is a placeholder with no private key behind
/// it, which is fine while a test only needs the key to RESOLVE to an account.
/// It is not fine the moment a test needs to OPEN something: scope keys are
/// wrapped to a device now, and an envelope addressed to a placeholder can never
/// be decrypted.
///
/// This writes both halves — the node's own `NodeDeviceIdentity` (with the
/// matching X25519 secret) and the binding that names it — so the node can be
/// addressed by a rotation and actually unwrap what it receives.
/// Returns the account, the device it minted, and the credential that proves the
/// pair — the credential so a SECOND store can record the same binding, which is
/// what a cross-store test needs to have both ends agree on one device.
pub(super) fn enrol_local_device(
    store: &Store,
    namespace: &ContextGroupId,
    sign_pk: &PublicKey,
) -> (
    AccountId,
    calimero_account::DeviceId,
    Box<JoinAccountCredential>,
) {
    let root_sk = PrivateKey::from(*(*sign_pk));
    let genesis = calimero_account::AccountGenesis::new(root_sk.public_key());
    let node = crate::NodeDeviceRepository::new(store)
        .ensure_enrolled_into(&[*namespace], genesis)
        .expect("mint this node's device");
    let cert = calimero_account::DeviceCert::sign(
        &root_sk,
        node.account,
        node.secret.device,
        sign_pk,
        &calimero_account::KemPublicKey::from(*node.secret.kem_secret.public_key().as_bytes()),
        0,
        0,
    )
    .expect("the account root certifies its own device");
    let credential = Box::new(JoinAccountCredential {
        genesis,
        chain: vec![],
        statement: cert,
    });
    record_credential(store, namespace, &credential);
    (node.account, node.secret.device, credential)
}

/// Record `credential` as a binding in `store`, endorsed by its own account.
///
/// Split out so a cross-store test can put the SAME device in both ends: the
/// responder has to resolve the requester's device to decide it is live, and the
/// requester has to hold the secret to open what comes back.
pub(super) fn record_credential(
    store: &Store,
    namespace: &ContextGroupId,
    credential: &JoinAccountCredential,
) {
    let bindings = crate::AccountBindingRepository::new(store);
    bindings
        .record_endorser(
            namespace,
            credential.statement.account,
            &credential.statement.account,
        )
        .expect("endorse");
    let _ = bindings
        .apply_link(
            namespace,
            &credential.genesis,
            &credential.chain,
            &credential.statement,
        )
        .expect("record the binding");
}

/// Stub `NetworkManager` for tests that call
/// `NamespaceGovernance::sign_apply_and_publish[_returning_op]` end to end:
/// resolves only the two `NetworkMessage` variants that path touches
/// (`Publish`, `MeshPeerCount`) and drops the rest, so the publish step
/// completes without a live libp2p swarm. Mirrors the `CountingNetworkActor`
/// pattern in `calimero_node_primitives::client::publish_on_namespace_now_tests`.
struct StubNetworkActor;

impl actix::Actor for StubNetworkActor {
    type Context = actix::Context<Self>;
}

impl actix::Handler<calimero_network_primitives::messages::NetworkMessage> for StubNetworkActor {
    type Result = ();

    fn handle(
        &mut self,
        msg: calimero_network_primitives::messages::NetworkMessage,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        use calimero_network_primitives::messages::{MessageId, NetworkMessage};

        match msg {
            NetworkMessage::MeshPeerCount { outcome, .. } => {
                let _ = outcome.send(0);
            }
            NetworkMessage::Publish { outcome, .. } => {
                let _ = outcome.send(Ok(MessageId(b"stub".to_vec())));
            }
            _ => {}
        }
    }
}

/// Captures every `NodeMessage` the publish path enqueues, so a test can assert
/// on the signals it fires rather than on log output. Forwards on an unbounded
/// channel: the test then AWAITS the message it expects instead of sleeping for
/// the actor to run.
struct CapturingNodeActor {
    seen: tokio::sync::mpsc::UnboundedSender<calimero_node_primitives::messages::NodeMessage>,
}

impl actix::Actor for CapturingNodeActor {
    type Context = actix::Context<Self>;
}

impl actix::Handler<calimero_node_primitives::messages::NodeMessage> for CapturingNodeActor {
    type Result = ();

    fn handle(
        &mut self,
        msg: calimero_node_primitives::messages::NodeMessage,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let _ = self.seen.send(msg);
    }
}

/// Build a real `NodeClient`/`AckRouter` pair for tests that publish end to end -
/// the namespace governance path and the device-link path both use it: a namespace
/// with a bootstrapped admin (returned as the signing key), and a
/// `NodeClient` whose network side is wired to `StubNetworkActor` so the
/// publish step resolves without a swarm. The `TempDir` keeps the stub
/// blobstore filesystem alive for the caller's duration (`sign_apply_and_publish`
/// never touches it, but `NodeClient::new` requires a real `BlobManager`).
pub(super) async fn namespace_publish_fixture() -> (
    Store,
    calimero_node_primitives::client::NodeClient,
    calimero_context_client::local_governance::AckRouter,
    calimero_governance_types::NamespaceId,
    PrivateKey,
    tempfile::TempDir,
    tokio::sync::mpsc::UnboundedReceiver<calimero_node_primitives::messages::NodeMessage>,
) {
    use actix::Actor;
    use calimero_network_primitives::client::NetworkClient;
    use calimero_network_primitives::messages::NetworkMessage;
    use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
    use calimero_utils_actix::LazyRecipient;

    let store = test_store();
    let ns_id: [u8; 32] = [0x91; 32];
    let (sk, _pk) = bootstrap_namespace_with_admin(&store, ns_id);

    let tmp = tempfile::tempdir().expect("tempdir");
    let blob_cfg = calimero_blobstore::config::BlobStoreConfig::new(
        tmp.path().to_path_buf().try_into().expect("utf8 blob path"),
    );
    let fs = calimero_blobstore::FileSystem::new(&blob_cfg)
        .await
        .expect("blob fs");
    let blob_manager = BlobManager::new(calimero_blobstore::BlobManager::new(store.clone(), fs));

    let network_recipient = LazyRecipient::<NetworkMessage>::new();
    let network_client = NetworkClient::new(network_recipient.clone());
    let _addr = StubNetworkActor::create(move |ctx| {
        assert!(network_recipient.init(ctx), "network recipient init");
        StubNetworkActor
    });

    let (event_sender, _) = tokio::sync::broadcast::channel(16);
    let (ctx_sync_tx, _ctx_sync_rx) = tokio::sync::mpsc::channel(8);
    let (ns_sync_tx, _ns_sync_rx) = tokio::sync::mpsc::channel(8);
    let (ns_join_tx, _ns_join_rx) = tokio::sync::mpsc::channel(8);
    let (open_subgroup_join_tx, _open_rx) = tokio::sync::mpsc::channel(8);
    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

    // The node-manager side is wired to a capturing actor rather than left
    // uninitialized: the publish path now enqueues the local-apply feed there,
    // and a test that wants to see it needs a live recipient.
    let (seen_tx, seen_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_recipient = LazyRecipient::<calimero_node_primitives::messages::NodeMessage>::new();
    let capture_recipient = node_recipient.clone();
    let _node_addr = CapturingNodeActor::create(move |ctx| {
        assert!(capture_recipient.init(ctx), "node recipient init");
        CapturingNodeActor { seen: seen_tx }
    });

    let node_client = NodeClient::new(
        store.clone(),
        blob_manager,
        network_client,
        node_recipient,
        event_sender,
        sync_client,
        None,
    );

    let ack_router = calimero_context_client::local_governance::AckRouter::default();

    (
        store,
        node_client,
        ack_router,
        ns_id.into(),
        sk,
        tmp,
        seen_rx,
    )
}

/// Seal a root op the way a publisher does, so a test exercises the path
/// production takes rather than one only tests can use.
///
/// Apply refuses a sealable root op that arrives in the clear, which is the whole
/// point of that rule — so a test that hand-built `NamespaceOp::Root(GroupCreated
/// { .. })` was constructing something no peer will accept. Sealing here keeps
/// those tests about what they were about (parents, cascades, idempotency) while
/// putting them on the real path.
///
/// Mints the namespace key if the fixture has not, because a fixture that skips it
/// is under-building the namespace: production keys a namespace at creation, since
/// its root is a group and `create_group` keys whatever group it creates.
///
/// Non-sealable variants pass through untouched, so the namespace genesis and
/// the two invitation joins still travel in the clear exactly as they must.
pub(super) fn seal_for_test(
    store: &Store,
    ns_gid: ContextGroupId,
    op: calimero_context_client::local_governance::RootOp,
) -> calimero_context_client::local_governance::NamespaceOp {
    if crate::GroupKeyring::new(store, ns_gid)
        .load_current_key()
        .expect("read namespace keyring")
        .is_none()
    {
        let _ = crate::GroupKeyring::new(store, ns_gid)
            .store_key(&[0x5Au8; 32])
            .expect("mint the namespace key the fixture omitted");
    }
    crate::seal_root_op_for_publish(store, ns_gid.to_bytes().into(), op)
        .expect("seal a root op for a test")
}
