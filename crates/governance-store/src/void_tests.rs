//! A removal voids the concurrent group ops of the account it removes. The
//! authorizer here reads the op-store the apply writes, through the projection.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use calimero_account::AccountId;
use calimero_context_client::local_governance::{GroupOp, NamespaceOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_op::{Op, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_projection::{AuthorityBase, ScopeState};
use calimero_store::Store;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;

use crate::authorizer::{AtCutAuthorizer, AtCutMembershipPath, GroupRows};
use crate::test_fixtures::{enrol_local_device, enrol_member, sample_meta_with_admin, test_store};
use crate::{
    CapabilitiesRepository, GroupKeyring, MembershipRepository, MetaRepository,
    NamespaceGovernance, NamespaceRepository,
};

const NS: [u8; 32] = [0xB8; 32];

/// The authorizer a node runs, built on the op-store the apply writes.
struct LogAuthorizer<'s> {
    store: &'s Store,
}

impl LogAuthorizer<'_> {
    fn group() -> ContextGroupId {
        ContextGroupId::from(NS)
    }

    fn log(&self) -> Vec<Op> {
        let scope = ScopeId::from(NS);
        let handle = self.store.handle();
        let mut iter = handle
            .iter::<calimero_store::key::ScopeUnifiedOp>()
            .expect("iterate the op-store");
        let start = calimero_store::key::ScopeUnifiedOp::new(*scope.as_bytes(), [0u8; 32]);
        let mut ops = Vec::new();
        if let Some(first) = iter.seek(start).expect("seek") {
            if first.scope() == *scope.as_bytes() {
                let value = handle.get(&first).expect("read").expect("row");
                ops.push(borsh::from_slice::<Op>(value.as_ref()).expect("decode"));
                for (key, value) in iter.entries() {
                    let key = key.expect("key");
                    if key.scope() != *scope.as_bytes() {
                        break;
                    }
                    let value = value.expect("value");
                    ops.push(borsh::from_slice::<Op>(value.as_ref()).expect("decode"));
                }
            }
        }
        // A group op stored before its key arrived folds as a hole; once the key
        // is held it reads as what it is, as the node's own load re-derives it.
        let op_log = crate::NamespaceOpLogService::new(self.store, NS.into());
        for op in &mut ops {
            if !matches!(
                op.payload,
                calimero_op::OpPayload::Opaque { .. } | calimero_op::OpPayload::Noop
            ) {
                continue;
            }
            let Ok(Some(signed)) = op_log.get_signed_op(op.id()) else {
                continue;
            };
            let NamespaceOp::Group {
                group_id,
                key_id,
                encrypted,
                ..
            } = &signed.op
            else {
                continue;
            };
            let Some(inner) = crate::decrypt_group_op(
                self.store,
                NS.into(),
                *group_id,
                key_id.as_bytes(),
                encrypted,
            )
            .ok()
            .flatten() else {
                continue;
            };
            let binding = crate::unified_op_decode::signer_binding_for(
                self.store,
                &Self::group(),
                &signed.signer,
            );
            *op = crate::unified_op_decode::op_from_namespace_op_with_binding(
                &signed,
                Some(&inner),
                None,
                binding,
                op.id(),
                op.hlc,
                &op.parents,
            );
        }
        ops
    }

    fn base(&self) -> AuthorityBase {
        let group = Self::group();
        AuthorityBase {
            root: MetaRepository::new(self.store)
                .load(&group)
                .expect("meta")
                .map(|meta| (group, meta.admin_identity)),
            default_cap_base: CapabilitiesRepository::new(self.store)
                .default_capabilities(&group)
                .expect("caps")
                .unwrap_or(0),
        }
    }

    fn account_of(&self, key: &PublicKey) -> Option<AccountId> {
        crate::unified_op_decode::signer_binding_for(self.store, &Self::group(), key)
            .map(|(account, _)| account)
    }

    fn view(&self, parents: &[[u8; 32]]) -> calimero_authz::AclView {
        ScopeState::acl_view_at_with_base(&self.log(), parents, self.base())
    }

    fn holds(
        &self,
        view: &calimero_authz::AclView,
        group: &ContextGroupId,
        account: &AccountId,
        capability: u32,
    ) -> bool {
        let base = self.base();
        if view.is_authorized_admin(*group, account, base.root) {
            return true;
        }
        let member = view
            .groups
            .get(group)
            .is_some_and(|members| members.contains_key(account));
        let folded = view.capability(group, account);
        let effective = if folded != 0 {
            folded
        } else {
            base.default_cap_base
        };
        member && effective & capability != 0
    }
}

impl AtCutAuthorizer for LogAuthorizer<'_> {
    fn is_admin_at_cut(
        &self,
        group: &ContextGroupId,
        signer: &PublicKey,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        if parents.is_empty() {
            return None;
        }
        let view = self.view(parents);
        Some(
            self.account_of(signer).is_some_and(|account| {
                view.is_authorized_admin(*group, &account, self.base().root)
            }),
        )
    }

    fn is_admin_or_capability_at_cut(
        &self,
        group: &ContextGroupId,
        signer: &PublicKey,
        capability: u32,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        if parents.is_empty() {
            return None;
        }
        let view = self.view(parents);
        Some(
            self.account_of(signer)
                .is_some_and(|account| self.holds(&view, group, &account, capability)),
        )
    }

    fn is_admin_or_capability_account_at_cut(
        &self,
        group: &ContextGroupId,
        member: &AccountId,
        capability: u32,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        if parents.is_empty() {
            return None;
        }
        Some(self.holds(&self.view(parents), group, member, capability))
    }

    fn is_admin_account_at_cut(
        &self,
        group: &ContextGroupId,
        member: &AccountId,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        if parents.is_empty() {
            return None;
        }
        Some(
            self.view(parents)
                .is_authorized_admin(*group, member, self.base().root),
        )
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
    ) -> Option<AtCutMembershipPath> {
        None
    }

    fn op_is_void(&self, group: &ContextGroupId, op: &Op) -> Option<bool> {
        if op.parents.is_empty() {
            return Some(false);
        }
        let log = self.log();
        Some(
            ScopeState::void_ops_with(&log, self.base(), Some((op, Some(*group))))
                .contains(&op.id()),
        )
    }

    fn voided_ops(
        &self,
        _group: &ContextGroupId,
        applied: Option<&Op>,
    ) -> Option<BTreeSet<[u8; 32]>> {
        let log = self.log();
        Some(ScopeState::void_ops_with(
            &log,
            self.base(),
            applied.map(|op| (op, None)),
        ))
    }

    fn group_rows(&self, group: &ContextGroupId, applied: Option<&Op>) -> Option<GroupRows> {
        let mut log = self.log();
        if let Some(op) = applied {
            if !log.iter().any(|held| held.id() == op.id()) {
                log.push(op.clone());
            }
        }
        let heads: Vec<[u8; 32]> = crate::NamespaceDagService::new(self.store, NS.into())
            .read_head_record()
            .ok()?
            .parent_hashes;
        let view = ScopeState::acl_view_at_with_base(&log, &heads, self.base());
        Some(GroupRows {
            members: view.groups.get(group).cloned().unwrap_or_default(),
            member_caps: view
                .member_caps
                .iter()
                .filter(|((g, _), _)| g == group)
                .map(|((_, account), caps)| (*account, *caps))
                .collect::<BTreeMap<_, _>>(),
            default_caps: view.default_caps.get(group).copied(),
        })
    }
}

struct Person {
    sk: PrivateKey,
    pk: PublicKey,
    account: AccountId,
}

/// The keys that make one set of people, so a second node can hold the same ones.
#[derive(Clone, Copy)]
struct Seeds {
    people: [[u8; 32]; 5],
    local: [u8; 32],
}

impl Seeds {
    fn random() -> Self {
        let mut rng = UnwrapErr(SysRng);
        Self {
            people: std::array::from_fn(|_| rand::RngExt::random(&mut rng)),
            local: rand::RngExt::random(&mut rng),
        }
    }
}

struct World {
    store: Store,
    seeds: Seeds,
    owner: Person,
    alice: Person,
    sam: Person,
    bob: Person,
    xavier: Person,
    k0: [u8; 32],
    nonces: RefCell<BTreeMap<PublicKey, u64>>,
}

impl World {
    fn new() -> Self {
        Self::build(Seeds::random(), true)
    }

    /// A second node of the same namespace, which holds the group key or not.
    fn replica(&self, keyed: bool) -> Self {
        Self::build(self.seeds, keyed)
    }

    fn build(seeds: Seeds, keyed: bool) -> Self {
        let store = test_store();
        let ns = LogAuthorizer::group();
        let person = |store: &Store, bytes: [u8; 32]| {
            let sk = PrivateKey::from(bytes);
            let pk = sk.public_key();
            let account = enrol_member(store, &ns, &pk);
            Person { sk, pk, account }
        };
        let [owner, alice, sam, bob, xavier] = seeds.people.map(|bytes| person(&store, bytes));

        // The local node is an ordinary member whose device rotations address.
        let local_sk = PrivateKey::from(seeds.local);
        let local_pk = local_sk.public_key();
        let (local_account, _, _) = enrol_local_device(&store, &ns, &local_pk);
        NamespaceRepository::new(&store)
            .store_identity(&ns, &local_pk, &seeds.local)
            .expect("store the node identity");

        MetaRepository::new(&store)
            .save(&ns, &sample_meta_with_admin(owner.account))
            .expect("root meta");
        let members = MembershipRepository::new(&store);
        members
            .add_member(&ns, &owner.account, GroupMemberRole::Admin)
            .expect("owner");
        members
            .add_member(&ns, &local_account, GroupMemberRole::Member)
            .expect("local");
        let k0 = [0x97u8; 32];
        if keyed {
            let _ = GroupKeyring::new(&store, ns).store_key(&k0).expect("k0");
        }

        Self {
            store,
            seeds,
            owner,
            alice,
            sam,
            bob,
            xavier,
            k0,
            nonces: RefCell::default(),
        }
    }

    fn next_nonce(&self, who: &Person) -> u64 {
        let mut nonces = self.nonces.borrow_mut();
        let next = nonces.entry(who.pk).or_insert(0);
        *next += 1;
        *next
    }

    fn sign(
        &self,
        who: &Person,
        parents: &[&SignedNamespaceOp],
        op: &GroupOp,
        rotation: Option<&[u8; 32]>,
    ) -> SignedNamespaceOp {
        self.sign_under(&self.k0, who, parents, op, rotation)
    }

    /// [`Self::sign`], sealed under `key` rather than the genesis key.
    fn sign_under(
        &self,
        key: &[u8; 32],
        who: &Person,
        parents: &[&SignedNamespaceOp],
        op: &GroupOp,
        rotation: Option<&[u8; 32]>,
    ) -> SignedNamespaceOp {
        let ns = LogAuthorizer::group();
        let encrypted = GroupKeyring::encrypt_op(key, op).expect("encrypt");
        let key_rotation = rotation.map(|new_key| {
            let keyring = GroupKeyring::new(&self.store, ns);
            let recipients: Vec<crate::KeyRecipient> = keyring
                .current_key_recipients()
                .expect("recipients")
                .into_iter()
                .map(|entitled| entitled.recipient)
                .collect();
            keyring
                .build_rotation(new_key, &who.sk, &recipients)
                .expect("rotation")
        });
        let nonce = self.next_nonce(who);
        SignedNamespaceOp::sign(
            &who.sk,
            NS.into(),
            parents
                .iter()
                .map(|p| p.content_hash().expect("hash"))
                .collect(),
            nonce,
            NamespaceOp::Group {
                group_id: NS.into(),
                key_id: GroupKeyring::key_id_for(key).into(),
                encrypted,
                key_rotation,
            },
        )
        .expect("sign")
    }

    fn apply(&self, op: &SignedNamespaceOp) -> eyre::Result<()> {
        let authorizer = LogAuthorizer { store: &self.store };
        NamespaceGovernance::new(&self.store, NS.into())
            .with_apply_auth(&op.parent_op_hashes, &authorizer)
            .apply_signed_op(op)
            .map(|_| ())
    }

    fn add(
        &self,
        by: &Person,
        parents: &[&SignedNamespaceOp],
        who: &Person,
        role: GroupMemberRole,
    ) -> SignedNamespaceOp {
        self.sign(
            by,
            parents,
            &GroupOp::MemberAdded {
                member: who.account,
                role,
            },
            None,
        )
    }

    fn remove(
        &self,
        by: &Person,
        parents: &[&SignedNamespaceOp],
        who: &Person,
        rotation: Option<&[u8; 32]>,
    ) -> SignedNamespaceOp {
        self.sign(
            by,
            parents,
            &GroupOp::MemberRemoved {
                member: who.account,
                expected_group_state_hash: [0u8; 32],
                expected_context_state_hashes: Vec::new(),
            },
            rotation,
        )
    }

    /// Owner adds Alice and Sam as admins and Bob as a member; all applied.
    fn founded(&self) -> [SignedNamespaceOp; 3] {
        let a = self.add(&self.owner, &[], &self.alice, GroupMemberRole::Admin);
        self.apply(&a).expect("owner adds alice");
        let s = self.add(&self.owner, &[&a], &self.sam, GroupMemberRole::Admin);
        self.apply(&s).expect("owner adds sam");
        let b = self.add(&self.owner, &[&s], &self.bob, GroupMemberRole::Member);
        self.apply(&b).expect("owner adds bob");
        [a, s, b]
    }

    fn role(&self, who: &Person) -> Option<GroupMemberRole> {
        MembershipRepository::new(&self.store)
            .role_of(&LogAuthorizer::group(), &who.account)
            .expect("role")
    }

    fn key_held(&self, key: &[u8; 32]) -> bool {
        GroupKeyring::new(&self.store, LogAuthorizer::group())
            .load_key_by_id(&GroupKeyring::key_id_for(key))
            .expect("keyring")
            .is_some()
    }

    fn current_key(&self) -> [u8; 32] {
        GroupKeyring::new(&self.store, LogAuthorizer::group())
            .load_current_key()
            .expect("keyring")
            .expect("a current key")
            .1
    }
}

/// What Sam, who has not seen his removal, sends from the cut before it.
struct FromTheOldCut {
    readd: SignedNamespaceOp,
    promote: SignedNamespaceOp,
    kick_and_rotate: SignedNamespaceOp,
}

const K_SAM: [u8; 32] = [0x42; 32];
const K_ALICE: [u8; 32] = [0x43; 32];

fn sam_from_the_old_cut(w: &World, head: &SignedNamespaceOp) -> FromTheOldCut {
    FromTheOldCut {
        readd: w.add(&w.sam, &[head], &w.sam, GroupMemberRole::Admin),
        promote: w.add(&w.sam, &[head], &w.xavier, GroupMemberRole::Admin),
        kick_and_rotate: w.remove(&w.sam, &[head], &w.bob, Some(&K_SAM)),
    }
}

type Rows = (
    Vec<(AccountId, GroupMemberRole)>,
    Vec<(AccountId, Option<u32>)>,
    [u8; 32],
);

/// Everything the tests compare between two nodes: the members and their roles,
/// every capability row, and the key the group encrypts under.
fn rows(w: &World) -> Rows {
    let ns = LogAuthorizer::group();
    let mut members = MembershipRepository::new(&w.store)
        .list(&ns, 0, usize::MAX)
        .expect("members");
    members.sort_by_key(|(account, _)| *account);
    let caps = CapabilitiesRepository::new(&w.store);
    let mut granted: Vec<(AccountId, Option<u32>)> = members
        .iter()
        .map(|(account, _)| {
            (
                *account,
                caps.member_capability(&ns, account).expect("caps"),
            )
        })
        .collect();
    granted.sort_by_key(|(account, _)| *account);
    (members, granted, w.current_key())
}

#[test]
fn ops_a_removed_admin_sends_from_a_cut_before_its_removal_have_no_effect() {
    let w = World::new();
    let [_, s, _] = w.founded();

    // Alice removes Sam (and rotates, as a removal does), then Sam's ops from the
    // cut before it arrive.
    let removal = w.remove(&w.alice, &[&s], &w.sam, Some(&K_ALICE));
    w.apply(&removal).expect("alice removes sam");
    assert_eq!(w.role(&w.sam), None);
    assert!(w.key_held(&K_ALICE));
    let current = w.current_key();

    let old = sam_from_the_old_cut(&w, &s);
    for op in [&old.readd, &old.promote, &old.kick_and_rotate] {
        w.apply(op)
            .expect("the op is stored; it just carries no authority");
    }

    assert_eq!(w.role(&w.sam), None, "Sam did not re-add himself");
    assert_eq!(
        w.role(&w.xavier),
        None,
        "the admin Sam added is not an admin"
    );
    assert_eq!(
        w.role(&w.bob),
        Some(GroupMemberRole::Member),
        "Bob was not removed"
    );
    assert!(
        !w.key_held(&K_SAM),
        "the key Sam rotated to was never taken"
    );
    assert_eq!(w.current_key(), current);
}

#[test]
fn a_removal_that_arrives_after_the_ops_it_voids_rebuilds_the_group() {
    let w = World::new();
    let [_, s, _] = w.founded();

    // Sam's ops arrive first and take effect: nothing yet says he was removed.
    let old = sam_from_the_old_cut(&w, &s);
    for op in [&old.promote, &old.kick_and_rotate] {
        w.apply(op)
            .expect("sam's op applies while his removal is unknown");
    }
    assert_eq!(w.role(&w.xavier), Some(GroupMemberRole::Admin));
    assert_eq!(w.role(&w.bob), None);
    assert_eq!(w.current_key(), K_SAM);

    // The removal rotates nothing, so the key can only go back if Sam's is undone.
    let removal = w.remove(&w.alice, &[&s], &w.sam, None);
    w.apply(&removal).expect("alice removes sam");

    assert_eq!(w.role(&w.sam), None);
    assert_eq!(
        w.role(&w.xavier),
        None,
        "the admin Sam added is taken back out"
    );
    assert_eq!(
        w.role(&w.bob),
        Some(GroupMemberRole::Member),
        "the member Sam removed is back"
    );
    assert_eq!(
        w.current_key(),
        w.k0,
        "the key Sam rotated to is not current"
    );
    assert!(
        w.key_held(&K_SAM),
        "it stays held, for what was sealed under it meanwhile"
    );
}

#[test]
fn an_op_the_projection_models_nothing_about_is_not_applied_either() {
    let w = World::new();
    let [_, s, _] = w.founded();
    let removal = w.remove(&w.alice, &[&s], &w.sam, None);
    w.apply(&removal).expect("alice removes sam");

    // Group metadata is nothing the membership plane folds, so only the apply
    // itself can refuse it.
    let set = w.sign(
        &w.sam,
        &[&s],
        &GroupOp::GroupMetadataSet {
            name: Some("takeover".to_owned()),
            data: Default::default(),
        },
        None,
    );
    w.apply(&set).expect("logged");
    assert_eq!(
        crate::MetadataRepository::new(&w.store)
            .group_metadata(&LogAuthorizer::group())
            .expect("metadata")
            .map(|record| record.name),
        None,
        "the metadata a removed admin set from the old cut was not written"
    );
}

#[test]
fn a_node_reaches_the_same_rows_whichever_of_the_op_and_the_removal_it_sees_first() {
    let removal_first = World::new();
    let [_, s, _] = removal_first.founded();
    let removal = removal_first.remove(
        &removal_first.alice,
        &[&s],
        &removal_first.sam,
        Some(&K_ALICE),
    );
    removal_first.apply(&removal).expect("removal");
    let old = sam_from_the_old_cut(&removal_first, &s);
    for op in [&old.readd, &old.promote, &old.kick_and_rotate] {
        removal_first.apply(op).expect("op");
    }

    let op_first = World::new();
    let [_, s2, _] = op_first.founded();
    let old2 = sam_from_the_old_cut(&op_first, &s2);
    for op in [&old2.readd, &old2.promote, &old2.kick_and_rotate] {
        op_first.apply(op).expect("op");
    }
    let removal2 = op_first.remove(&op_first.alice, &[&s2], &op_first.sam, Some(&K_ALICE));
    op_first.apply(&removal2).expect("removal");

    // The two worlds hold different accounts (the keys are random), so compare
    // what each holds by role.
    let shape = |w: &World| {
        let (members, caps, key) = rows(w);
        let roles: Vec<GroupMemberRole> = members.into_iter().map(|(_, role)| role).collect();
        (
            roles.len(),
            roles
                .iter()
                .filter(|r| **r == GroupMemberRole::Admin)
                .count(),
            caps.len(),
            key,
        )
    };
    assert_eq!(shape(&removal_first), shape(&op_first));
    assert_eq!(removal_first.role(&removal_first.xavier), None);
    assert_eq!(op_first.role(&op_first.xavier), None);
    assert_eq!(op_first.role(&op_first.bob), Some(GroupMemberRole::Member));
    assert_eq!(op_first.current_key(), K_ALICE);
    assert_eq!(removal_first.current_key(), K_ALICE);
}

#[test]
fn another_admins_concurrent_op_survives_the_removal() {
    let w = World::new();
    let [_, s, _] = w.founded();

    let removal = w.remove(&w.alice, &[&s], &w.sam, None);
    // The owner, concurrently, adds Xavier.
    let by_owner = w.add(&w.owner, &[&s], &w.xavier, GroupMemberRole::Member);
    w.apply(&removal).expect("removal");
    w.apply(&by_owner).expect("the owner's op");
    assert_eq!(w.role(&w.xavier), Some(GroupMemberRole::Member));
    assert_eq!(w.role(&w.sam), None);
}

#[test]
fn two_admins_removing_each_other_are_both_removed_in_either_order() {
    for alice_first in [true, false] {
        let w = World::new();
        let [_, s, _] = w.founded();
        let alice_removes_sam = w.remove(&w.alice, &[&s], &w.sam, None);
        let sam_removes_alice = w.remove(&w.sam, &[&s], &w.alice, None);
        if alice_first {
            w.apply(&alice_removes_sam).expect("first");
            w.apply(&sam_removes_alice).expect("second");
        } else {
            w.apply(&sam_removes_alice).expect("first");
            w.apply(&alice_removes_sam).expect("second");
        }
        assert_eq!(
            w.role(&w.sam),
            None,
            "Sam is removed (alice_first = {alice_first})"
        );
        assert_eq!(
            w.role(&w.alice),
            None,
            "Alice is removed (alice_first = {alice_first})"
        );
        assert_eq!(w.role(&w.owner), Some(GroupMemberRole::Admin));
    }
}

#[test]
fn the_namespace_owner_cannot_be_removed_and_its_old_cut_ops_stand() {
    let w = World::new();
    let [_, s, _] = w.founded();

    // Alice tries to remove the owner. The owner's own op, sent from the cut
    // before it, stands whichever arrives first.
    let removal = w.remove(&w.alice, &[&s], &w.owner, None);
    let by_owner = w.add(&w.owner, &[&s], &w.xavier, GroupMemberRole::Member);
    assert!(
        w.apply(&removal).is_err(),
        "the owner is immune from removal"
    );
    w.apply(&by_owner).expect("the owner's op");
    assert_eq!(w.role(&w.owner), Some(GroupMemberRole::Admin));
    assert_eq!(w.role(&w.xavier), Some(GroupMemberRole::Member));
}

#[test]
fn a_voided_key_is_never_current_yet_still_opens_what_was_sealed_under_it() {
    let store = test_store();
    let ns = LogAuthorizer::group();
    let keyring = GroupKeyring::new(&store, ns);
    let genesis = [0x11u8; 32];
    let rotated = [0x22u8; 32];
    let _ = keyring.store_key(&genesis).expect("genesis");
    let rotated_id = keyring.store_key_with_epoch(&rotated, 5).expect("rotated");
    assert_eq!(keyring.load_current_key().unwrap().unwrap().1, rotated);

    keyring.void_key(&rotated_id).expect("void it");

    assert_eq!(
        keyring.load_current_key().unwrap().unwrap().1,
        genesis,
        "the key a void rotation introduced is not the one to encrypt under"
    );
    assert_eq!(
        keyring.load_key_by_id(&rotated_id).unwrap(),
        Some(rotated),
        "what a peer sealed under it in the meantime still opens"
    );
}

#[test]
fn a_key_voided_before_it_is_held_stays_void_when_it_arrives() {
    let store = test_store();
    let ns = LogAuthorizer::group();
    let keyring = GroupKeyring::new(&store, ns);
    let genesis = [0x11u8; 32];
    let rotated = [0x22u8; 32];
    let _ = keyring.store_key(&genesis).expect("genesis");

    keyring
        .void_key(&GroupKeyring::key_id_for(&rotated))
        .expect("void a key not held yet");
    assert_eq!(
        keyring
            .load_key_by_id(&GroupKeyring::key_id_for(&rotated))
            .unwrap(),
        None,
        "a mark is not a key"
    );
    assert!(keyring.holds_any_key().unwrap());

    // A pull from a peer that has not learned of the removal hands it over.
    let _ = keyring.store_key(&rotated).expect("pulled");
    let _ = keyring
        .store_key_with_epoch(&rotated, 9)
        .expect("delivered");

    assert_eq!(keyring.load_current_key().unwrap().unwrap().1, genesis);
    assert_eq!(
        keyring
            .load_key_by_id(&GroupKeyring::key_id_for(&rotated))
            .unwrap(),
        Some(rotated)
    );
}

#[test]
fn a_root_op_a_removed_admin_sends_from_a_cut_before_its_removal_has_no_effect() {
    let w = World::new();
    let [_, s, _] = w.founded();
    let removal = w.remove(&w.alice, &[&s], &w.sam, None);
    w.apply(&removal).expect("alice removes sam");

    // Sam, from the old cut, creates a subgroup of the namespace.
    let created = ContextGroupId::from([0xC1; 32]);
    let root_op = crate::test_fixtures::seal_for_test(
        &w.store,
        LogAuthorizer::group(),
        calimero_context_client::local_governance::RootOp::GroupCreated {
            group_id: created,
            parent_id: LogAuthorizer::group(),
            restricted: false,
            admin: w.sam.account,
        },
    );
    let op = SignedNamespaceOp::sign(
        &w.sam.sk,
        NS.into(),
        vec![s.content_hash().expect("hash")],
        w.next_nonce(&w.sam),
        root_op,
    )
    .expect("sign");
    w.apply(&op)
        .expect("the op is logged; it carries no authority");

    assert!(
        MetaRepository::new(&w.store)
            .load(&created)
            .expect("meta")
            .is_none(),
        "the subgroup Sam created from the old cut does not exist"
    );
}

#[test]
fn ops_replayed_when_the_group_key_arrives_are_judged_like_ops_received_with_it() {
    // The node that signs the ops, and a replica that holds no key when they
    // reach it, so every one of them is parked.
    let w = World::new();
    let [a, s, b] = w.founded();
    let removal = w.remove(&w.alice, &[&s], &w.sam, None);
    w.apply(&removal).expect("alice removes sam");
    let old = sam_from_the_old_cut(&w, &s);

    let replica = w.replica(false);
    for op in [
        &a,
        &s,
        &b,
        &removal,
        &old.readd,
        &old.promote,
        &old.kick_and_rotate,
    ] {
        replica.apply(op).expect("parked until the key arrives");
    }
    assert_eq!(
        replica.role(&replica.alice),
        None,
        "nothing applied without the key"
    );

    let ns = LogAuthorizer::group();
    let _ = GroupKeyring::new(&replica.store, ns)
        .store_key(&replica.k0)
        .expect("the key arrives");
    let authorizer = LogAuthorizer {
        store: &replica.store,
    };
    let _ = NamespaceGovernance::new(&replica.store, NS.into())
        .with_apply_auth(&[], &authorizer)
        .retry_encrypted_ops_for_group(NS)
        .expect("replay the parked ops");

    assert_eq!(replica.role(&replica.alice), Some(GroupMemberRole::Admin));
    assert_eq!(replica.role(&replica.sam), None, "Sam is removed");
    assert_eq!(
        replica.role(&replica.xavier),
        None,
        "the admin Sam added from the old cut is not"
    );
    assert_eq!(
        replica.role(&replica.bob),
        Some(GroupMemberRole::Member),
        "nor did Sam remove Bob"
    );
}

#[test]
fn a_removal_replayed_when_its_key_arrives_rebuilds_what_the_ops_before_it_wrote() {
    let w = World::new();
    let [a, s, b] = w.founded();
    // The removal is sealed under a key the replica does not hold yet.
    let k1 = [0x55u8; 32];
    let removal = w.sign_under(
        &k1,
        &w.alice,
        &[&s],
        &GroupOp::MemberRemoved {
            member: w.sam.account,
            expected_group_state_hash: [0u8; 32],
            expected_context_state_hashes: Vec::new(),
        },
        None,
    );
    let old = sam_from_the_old_cut(&w, &s);

    let replica = w.replica(true);
    for op in [&a, &s, &b, &old.promote, &old.kick_and_rotate] {
        replica.apply(op).expect("applies: no removal is known");
    }
    replica
        .apply(&removal)
        .expect("parked: its key is not held");
    assert_eq!(replica.role(&replica.xavier), Some(GroupMemberRole::Admin));
    assert_eq!(replica.role(&replica.sam), Some(GroupMemberRole::Admin));

    let ns = LogAuthorizer::group();
    let _ = GroupKeyring::new(&replica.store, ns)
        .store_key(&k1)
        .expect("the key arrives");
    let authorizer = LogAuthorizer {
        store: &replica.store,
    };
    let _ = NamespaceGovernance::new(&replica.store, NS.into())
        .with_apply_auth(&[], &authorizer)
        .retry_encrypted_ops_for_group(NS)
        .expect("replay the parked removal");

    assert_eq!(replica.role(&replica.sam), None, "Sam is removed");
    assert_eq!(
        replica.role(&replica.xavier),
        None,
        "what Sam did from the old cut is taken back out"
    );
    assert_eq!(replica.role(&replica.bob), Some(GroupMemberRole::Member));
}

#[test]
fn a_sealed_root_op_replayed_when_the_key_arrives_is_judged_like_one_received_with_it() {
    let w = World::new();
    let [a, s, b] = w.founded();
    let removal = w.remove(&w.alice, &[&s], &w.sam, None);
    w.apply(&removal).expect("alice removes sam");

    let created = ContextGroupId::from([0xC2; 32]);
    let root_op = crate::test_fixtures::seal_for_test(
        &w.store,
        LogAuthorizer::group(),
        calimero_context_client::local_governance::RootOp::GroupCreated {
            group_id: created,
            parent_id: LogAuthorizer::group(),
            restricted: false,
            admin: w.sam.account,
        },
    );
    let from_the_old_cut = SignedNamespaceOp::sign(
        &w.sam.sk,
        NS.into(),
        vec![s.content_hash().expect("hash")],
        w.next_nonce(&w.sam),
        root_op,
    )
    .expect("sign");

    let replica = w.replica(false);
    for op in [&a, &s, &b, &removal, &from_the_old_cut] {
        replica.apply(op).expect("parked until the key arrives");
    }
    // A replayed sealed root op is gated on live rows, so hold Sam's admin row:
    // the void verdict is then all that stands between the op and its effect.
    let ns = LogAuthorizer::group();
    let members = MembershipRepository::new(&replica.store);
    for admin in [&replica.alice, &replica.sam] {
        members
            .add_member(&ns, &admin.account, GroupMemberRole::Admin)
            .expect("an admin row");
    }
    let _ = GroupKeyring::new(&replica.store, ns)
        .store_key(&replica.k0)
        .expect("the key arrives");
    let authorizer = LogAuthorizer {
        store: &replica.store,
    };
    let _ = NamespaceGovernance::new(&replica.store, NS.into())
        .with_apply_auth(&[], &authorizer)
        .retry_encrypted_ops_for_group(NS)
        .expect("replay");

    assert!(
        MetaRepository::new(&replica.store)
            .load(&created)
            .expect("meta")
            .is_none(),
        "the subgroup Sam created from the old cut does not exist"
    );
}

fn unreadable_op(who: &Person, nonce: u64) -> SignedNamespaceOp {
    SignedNamespaceOp::sign(
        &who.sk,
        NS.into(),
        Vec::new(),
        nonce,
        NamespaceOp::Group {
            group_id: NS.into(),
            // A key this node does not hold.
            key_id: GroupKeyring::key_id_for(&[0xEE; 32]).into(),
            encrypted: calimero_context_client::local_governance::EncryptedGroupOp {
                nonce: [0u8; 12],
                ciphertext: vec![nonce as u8; 1 << 20],
            },
            key_rotation: None,
        },
    )
    .expect("sign")
}

#[test]
fn the_unreadable_ops_a_signer_leaves_stored_are_bounded() {
    let w = World::new();
    let attacker = Person {
        sk: PrivateKey::from([0xA7; 32]),
        pk: PrivateKey::from([0xA7; 32]).public_key(),
        account: AccountId::from([0xA7; 32]),
    };

    let mut stored = 0u64;
    let mut refused = None;
    for nonce in 1..=40 {
        match w.apply(&unreadable_op(&attacker, nonce)) {
            Ok(()) => stored += 1,
            Err(err) => {
                refused = Some(err);
                break;
            }
        }
    }
    let refused = refused.expect("a signer cannot park unreadable ops without end");
    assert!(
        matches!(
            refused.downcast_ref::<crate::ApplyError>(),
            Some(crate::ApplyError::UnreadableOpBudgetExceeded { .. })
        ),
        "{refused:#}"
    );
    assert!(
        stored >= 10,
        "an honest volume is far below the budget: {stored}"
    );

    // What the node can read is not held up by what it cannot.
    let [_, _, _] = w.founded();
    assert_eq!(w.role(&w.alice), Some(GroupMemberRole::Admin));
}

fn caps(w: &World, who: &Person) -> Option<u32> {
    CapabilitiesRepository::new(&w.store)
        .member_capability(&LogAuthorizer::group(), &who.account)
        .expect("caps")
}

#[test]
fn a_capability_a_void_op_granted_is_taken_back_when_the_removal_arrives() {
    let w = World::new();
    let [_, s, _] = w.founded();
    let before = caps(&w, &w.bob);

    let grant = w.sign(
        &w.sam,
        &[&s],
        &GroupOp::MemberCapabilitySet {
            member: w.bob.account,
            capabilities: calimero_context_config::MemberCapabilities::CAN_INVITE_MEMBERS,
        },
        None,
    );
    w.apply(&grant)
        .expect("applies while the removal is unknown");
    assert_eq!(
        caps(&w, &w.bob),
        Some(calimero_context_config::MemberCapabilities::CAN_INVITE_MEMBERS.bits())
    );

    let removal = w.remove(&w.alice, &[&s], &w.sam, None);
    w.apply(&removal).expect("alice removes sam");
    assert_eq!(
        caps(&w, &w.bob),
        before,
        "Bob holds what he did before Sam's grant"
    );
}

#[test]
fn a_default_a_void_op_set_is_taken_back_when_the_removal_arrives() {
    let w = World::new();
    let [_, s, _] = w.founded();
    let ns = LogAuthorizer::group();
    let before = CapabilitiesRepository::new(&w.store)
        .default_capabilities(&ns)
        .expect("default");

    let set = w.sign(
        &w.sam,
        &[&s],
        &GroupOp::DefaultCapabilitiesSet {
            capabilities: calimero_context_config::MemberCapabilities::CAN_INVITE_MEMBERS,
        },
        None,
    );
    w.apply(&set).expect("applies while the removal is unknown");
    assert_ne!(
        CapabilitiesRepository::new(&w.store)
            .default_capabilities(&ns)
            .expect("default"),
        before
    );

    let removal = w.remove(&w.alice, &[&s], &w.sam, None);
    w.apply(&removal).expect("alice removes sam");
    assert_eq!(
        CapabilitiesRepository::new(&w.store)
            .default_capabilities(&ns)
            .expect("default"),
        before
    );
}

#[test]
fn a_role_a_void_op_gave_is_taken_back_when_the_removal_arrives() {
    let w = World::new();
    let [_, s, _] = w.founded();

    // Sam promotes Bob, an existing member, from the old cut.
    let promote = w.add(&w.sam, &[&s], &w.bob, GroupMemberRole::Admin);
    w.apply(&promote)
        .expect("applies while the removal is unknown");
    assert_eq!(w.role(&w.bob), Some(GroupMemberRole::Admin));

    let removal = w.remove(&w.alice, &[&s], &w.sam, None);
    w.apply(&removal).expect("alice removes sam");
    assert_eq!(w.role(&w.bob), Some(GroupMemberRole::Member));
}

#[test]
fn a_mark_with_no_key_behind_it_is_not_a_held_key() {
    let store = test_store();
    let keyring = GroupKeyring::new(&store, LogAuthorizer::group());
    keyring
        .void_key(&GroupKeyring::key_id_for(&[0x22; 32]))
        .expect("mark a key not held");
    assert!(!keyring.holds_any_key().unwrap());
    assert_eq!(keyring.load_current_key().unwrap(), None);
}

#[test]
fn unreadable_sealed_root_ops_are_charged_to_their_signers_budget() {
    use calimero_context_client::local_governance::EncryptedRootOp;

    let w = World::new();
    let signer = Person {
        sk: PrivateKey::from([0xA8; 32]),
        pk: PrivateKey::from([0xA8; 32]).public_key(),
        account: AccountId::from([0xA8; 32]),
    };
    let key_id: calimero_governance_types::KeyId = GroupKeyring::key_id_for(&[0xEE; 32]).into();
    let sealed = || EncryptedRootOp {
        nonce: [0u8; 12],
        ciphertext: vec![7u8; 4096],
    };
    let shapes = [
        NamespaceOp::RootSealed {
            key_id,
            encrypted: sealed(),
        },
        NamespaceOp::RootSealedForGroup {
            group_id: ContextGroupId::from([0xC3; 32]),
            key_id,
            encrypted: sealed(),
        },
        NamespaceOp::RootRelaySealed {
            key_id,
            encrypted: calimero_governance_types::EncryptedRelayedOp {
                nonce: [0u8; 12],
                ciphertext: vec![7u8; 4096],
            },
        },
    ];
    for (nonce, shape) in (1u64..).zip(shapes) {
        let who = Person {
            sk: PrivateKey::from([0xA8 + nonce as u8; 32]),
            pk: PrivateKey::from([0xA8 + nonce as u8; 32]).public_key(),
            account: signer.account,
        };
        let op =
            SignedNamespaceOp::sign(&who.sk, NS.into(), Vec::new(), nonce, shape).expect("sign");
        let size = borsh::to_vec(&op).expect("encode").len() as u64;
        w.apply(&op).expect("parked until its key arrives");

        // The op was charged: little room is left beyond what it cost.
        let budget = crate::unreadable_budget::UnreadableOpBudget::new(&w.store);
        assert!(
            budget
                .admit(
                    &NS.into(),
                    &who.pk,
                    crate::unreadable_budget::MAX_BYTES_PER_SIGNER - size + 1,
                )
                .is_err(),
            "an unreadable op of shape {nonce} was not charged"
        );
    }
}
