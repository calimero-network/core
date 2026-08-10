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
use calimero_primitives::context::{GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::db::InMemoryDB;
use calimero_store::key::{GroupMetaValue, GroupParentRef};
use calimero_store::Store;
use rand::rngs::OsRng;
/// A joiner's account credential for tests that only need a join op to be
/// well-formed.
///
/// Structurally valid but not cryptographically meaningful: the certificate's
/// signature is filler. Tests that care whether a credential VERIFIES must build a
/// real one — `apply_link` checks the certificate against the genesis, so this
/// fixture is refused on that path by design, and the join still applies because a
/// refused credential is reported rather than propagated.
pub(super) fn test_join_account() -> Box<JoinAccountCredential> {
    let root = PrivateKey::random(&mut OsRng).public_key();
    let genesis = calimero_account::AccountGenesis::new(root, [0x5A; 16]);
    Box::new(JoinAccountCredential {
        cert: calimero_account::DeviceCert {
            account: genesis.account_id(),
            device: calimero_account::DeviceId::from([0x3E; 32]),
            sign_pk: PrivateKey::random(&mut OsRng).public_key(),
            kem_pk: calimero_account::KemPublicKey::from([0x2B; 32]),
            key_epoch: 0,
            device_epoch: 0,
            signature: [0x11; 64],
        },
        genesis,
        chain: vec![],
    })
}

/// A joiner's account credential that actually VERIFIES, certified for
/// `sign_pk`.
///
/// The counterpart to [`test_join_account`]: use this wherever the test is about
/// what happens when a credential is admitted, since the filler fixture is
/// refused at `apply_link` by design. `device` is a parameter so a test can mint
/// a second credential for the same joiner (a rejoin) or deliberately collide two
/// devices.
/// A fresh account root: its signing key and the genesis that names it.
///
/// Returned as a pair so a test can mint SEVERAL credentials under one account —
/// a rejoin, or a person's second device — which is the whole distinction the
/// account plane exists to draw.
pub(super) fn test_account_root() -> (PrivateKey, calimero_account::AccountGenesis) {
    let root_sk = PrivateKey::random(&mut OsRng);
    let genesis = calimero_account::AccountGenesis::new(root_sk.public_key(), [0x5A; 16]);
    (root_sk, genesis)
}

/// Certify `sign_pk` as `device` under an existing account root.
pub(super) fn join_account_for(
    root_sk: &PrivateKey,
    genesis: calimero_account::AccountGenesis,
    sign_pk: &PublicKey,
    device: [u8; 32],
    device_epoch: u32,
) -> Box<JoinAccountCredential> {
    let cert = calimero_account::sign_device_cert(
        root_sk,
        genesis.account_id(),
        calimero_account::DeviceId::from(device),
        sign_pk,
        &calimero_account::KemPublicKey::from([0x2B; 32]),
        0,
        device_epoch,
    )
    .expect("the account root signs its own device cert");
    Box::new(JoinAccountCredential {
        genesis,
        chain: vec![],
        cert,
    })
}

/// A credential that actually VERIFIES, certified for `sign_pk` under a brand-new
/// account root.
///
/// The counterpart to [`test_join_account`]: use this wherever the test is about
/// what happens when a credential is admitted, since the filler fixture is
/// refused at `apply_link` by design.
pub(super) fn real_join_account(sign_pk: &PublicKey) -> Box<JoinAccountCredential> {
    let (root_sk, genesis) = test_account_root();
    join_account_for(&root_sk, genesis, sign_pk, [0x3E; 32], 0)
}

pub(super) fn test_store() -> Store {
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
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
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
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
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
    let admin_sk_bytes: [u8; 32] = rand::Rng::gen(&mut OsRng);
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
        .store_identity(&ns_gid, &admin_pk, &admin_sk_bytes, &[0u8; 32])
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
    let founder = credential.cert.account;
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
    let genesis = calimero_account::AccountGenesis::new(root_sk.public_key(), [0x5A; 16]);
    join_account_for(&root_sk, genesis, &founder_sk.public_key(), [0x3E; 32], 0)
}

/// The account [`namespace_genesis_for`] will establish for this founder.
pub(super) fn founder_account_for(founder_sk: &PrivateKey) -> AccountId {
    founder_credential(founder_sk).cert.account
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
    let account = credential.cert.account;
    let bindings = crate::AccountBindingRepository::new(store);
    let _ = bindings
        .apply_link(
            namespace,
            &credential.genesis,
            &credential.chain,
            &credential.cert,
        )
        .expect("store the binding");
    bindings
        .record_endorser(namespace, account, &account)
        .expect("record the endorser");
    account
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
