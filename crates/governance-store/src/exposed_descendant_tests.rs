//! Forward secrecy for the own-keyed groups a departed member reached only by
//! inheritance.
//!
//! An Open subgroup below a Restricted group is covered by its OWN key
//! (`key_covering_group`). A member of the Restricted parent who holds
//! `CAN_JOIN_OPEN_SUBGROUPS` inherits into it and may pull that key. Removing the
//! member from the parent (or the member leaving it) ends the inheritance, but the
//! child's key was never rotated: the departed node kept reading every op and
//! state delta written there afterwards.
//!
//! These tests pin which descendants the departure leaves owing a rotation, and
//! that the rotation's recipients include the members who still inherit.

use calimero_account::AccountId;
use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::{MemberCapabilities, VisibilityMode};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;

use crate::test_fixtures::{
    bootstrap_namespace_with_admin_account, enrol_member, nest_for_test, sample_meta_with_admin,
    test_store,
};
use crate::{
    apply_local_signed_group_op, CapabilitiesRepository, GroupKeyring, MembershipRepository,
    MetaRepository, PendingRotationRepository,
};

/// A namespace root and a Restricted group `g` beneath it. `admin` administers
/// both; `departing` and `staying` are plain members of `g` holding
/// `CAN_JOIN_OPEN_SUBGROUPS`, so each inherits into any Open child of `g`.
struct Fixture {
    store: Store,
    ns: ContextGroupId,
    g: ContextGroupId,
    admin_sk: PrivateKey,
    admin: AccountId,
    departing_sk: PrivateKey,
    departing: AccountId,
    staying: AccountId,
}

fn fixture() -> Fixture {
    let store = test_store();
    let ns_bytes = [0xC1u8; 32];
    let ns = ContextGroupId::from(ns_bytes);
    let ((admin_sk, _), admin) = bootstrap_namespace_with_admin_account(&store, ns_bytes);

    let g = ContextGroupId::from([0xC2u8; 32]);
    add_group(&store, &ns, &g, admin, VisibilityMode::Restricted);

    let departing_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
    let departing = enrol_member(&store, &ns, &departing_sk.public_key());
    let staying = enrol_member(
        &store,
        &ns,
        &PrivateKey::random(&mut UnwrapErr(SysRng)).public_key(),
    );
    for account in [departing, staying] {
        add_inheriting_member(&store, &ns, account);
        add_inheriting_member(&store, &g, account);
    }

    Fixture {
        store,
        ns,
        g,
        admin_sk,
        admin,
        departing_sk,
        departing,
        staying,
    }
}

/// A group `child` under `parent`, administered by `admin`, with its own birth key
/// (production mints one for every group it creates).
fn add_group(
    store: &Store,
    parent: &ContextGroupId,
    child: &ContextGroupId,
    admin: AccountId,
    visibility: VisibilityMode,
) {
    MetaRepository::new(store)
        .save(child, &sample_meta_with_admin(admin))
        .unwrap();
    MembershipRepository::new(store)
        .add_member(child, &admin, GroupMemberRole::Admin)
        .unwrap();
    nest_for_test(store, parent, child);
    CapabilitiesRepository::new(store)
        .set_subgroup_visibility(child, visibility)
        .unwrap();
    let _ = GroupKeyring::new(store, *child)
        .store_key_with_epoch(&[child.to_bytes()[0]; 32], 1)
        .unwrap();
}

fn add_inheriting_member(store: &Store, group: &ContextGroupId, account: AccountId) {
    MembershipRepository::new(store)
        .add_member(group, &account, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(store)
        .set_member_capability(
            group,
            &account,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
        )
        .unwrap();
}

fn remove(f: &Fixture, group: &ContextGroupId) {
    let op = SignedGroupOp::sign(
        &f.admin_sk,
        group.to_bytes().into(),
        vec![],
        1,
        GroupOp::MemberRemoved {
            member: f.departing,
            expected_group_state_hash: [0u8; 32],
            expected_context_state_hashes: Vec::new(),
        },
    )
    .expect("sign MemberRemoved");
    apply_local_signed_group_op(&f.store, &op).expect("apply MemberRemoved");
}

fn leave(f: &Fixture, group: &ContextGroupId) {
    let op = SignedGroupOp::sign(
        &f.departing_sk,
        group.to_bytes().into(),
        vec![],
        1,
        GroupOp::MemberLeft {
            member: f.departing,
            expected_group_state_hash: [0u8; 32],
            expected_context_state_hashes: Vec::new(),
        },
    )
    .expect("sign MemberLeft");
    apply_local_signed_group_op(&f.store, &op).expect("apply MemberLeft");
}

fn owes(f: &Fixture, group: &ContextGroupId) -> bool {
    PendingRotationRepository::new(&f.store)
        .is_pending(group, &f.departing)
        .unwrap()
}

fn inherits(f: &Fixture, group: &ContextGroupId, account: &AccountId) -> bool {
    MembershipRepository::new(&f.store)
        .is_live_member(group, account)
        .unwrap()
}

#[test]
fn removal_from_a_restricted_group_owes_a_rotation_of_its_open_child() {
    let f = fixture();
    let d = ContextGroupId::from([0xC3u8; 32]);
    add_group(&f.store, &f.g, &d, f.admin, VisibilityMode::Open);
    assert!(
        inherits(&f, &d, &f.departing),
        "precondition: the member reaches the Open child by inheritance"
    );

    remove(&f, &f.g);

    assert!(
        !inherits(&f, &d, &f.departing),
        "precondition: the removal ends the inheritance"
    );
    assert!(
        owes(&f, &d),
        "the Open child is covered by its own key, which the removed member could pull \
         while it inherited; the removal must leave a rotation of it owed"
    );
}

#[test]
fn leaving_a_restricted_group_owes_a_rotation_of_its_open_child() {
    let f = fixture();
    let d = ContextGroupId::from([0xC3u8; 32]);
    add_group(&f.store, &f.g, &d, f.admin, VisibilityMode::Open);

    leave(&f, &f.g);

    assert!(owes(&f, &f.g), "the group left still owes its own rotation");
    assert!(
        owes(&f, &d),
        "a leaver who inherited the Open child holds its key too, so the leave must owe \
         a rotation of it"
    );
}

#[test]
fn a_namespace_covered_child_owes_nothing() {
    // `g` Open on an all-Open chain: `g` and its Open child are both covered by the
    // namespace key, which the removed member keeps as a namespace member. Rotating a
    // per-subgroup key there would revoke nothing.
    let f = fixture();
    CapabilitiesRepository::new(&f.store)
        .set_subgroup_visibility(&f.g, VisibilityMode::Open)
        .unwrap();
    let d = ContextGroupId::from([0xC3u8; 32]);
    add_group(&f.store, &f.g, &d, f.admin, VisibilityMode::Open);

    remove(&f, &f.g);

    assert!(!owes(&f, &d), "a namespace-covered child owes no rotation");
    assert!(!owes(&f, &f.g));
}

#[test]
fn a_child_the_member_still_belongs_to_owes_nothing() {
    let f = fixture();
    let d = ContextGroupId::from([0xC3u8; 32]);
    add_group(&f.store, &f.g, &d, f.admin, VisibilityMode::Open);
    MembershipRepository::new(&f.store)
        .add_member(&d, &f.departing, GroupMemberRole::Member)
        .unwrap();

    remove(&f, &f.g);

    assert!(
        inherits(&f, &d, &f.departing),
        "precondition: the direct row keeps the member in the child"
    );
    assert!(
        !owes(&f, &d),
        "a member who still belongs to the child is entitled to its key"
    );
}

#[test]
fn a_restricted_descendant_the_member_never_reached_owes_nothing() {
    // Own-keyed, no direct row, and no membership after the removal - but no
    // membership before it either, so the member never held its key.
    let f = fixture();
    let r = ContextGroupId::from([0xC4u8; 32]);
    add_group(&f.store, &f.g, &r, f.admin, VisibilityMode::Restricted);
    assert!(!inherits(&f, &r, &f.departing), "precondition");

    remove(&f, &f.g);

    assert!(
        !owes(&f, &r),
        "a group the member never belonged to holds no key the member has; rotating it \
         would be work for nothing"
    );
}

#[test]
fn every_open_level_below_the_wall_owes_a_rotation() {
    // g (Restricted) -> p (Open) -> d (Open): both p and d are covered by their own
    // keys, and the member inherited into both.
    let f = fixture();
    let p = ContextGroupId::from([0xC5u8; 32]);
    let d = ContextGroupId::from([0xC6u8; 32]);
    add_group(&f.store, &f.g, &p, f.admin, VisibilityMode::Open);
    add_group(&f.store, &p, &d, f.admin, VisibilityMode::Open);
    assert!(inherits(&f, &d, &f.departing), "precondition");

    remove(&f, &f.g);

    assert!(owes(&f, &p), "the Open child owes a rotation");
    assert!(owes(&f, &d), "the Open grandchild owes a rotation");
}

#[test]
fn leaving_the_namespace_owes_a_rotation_of_an_open_child_of_a_restricted_group_left_with_it() {
    // The namespace-leave cascade removes the leaver's direct row in `g`, which ends
    // its inheritance into `g`'s Open child: that child is exposed too.
    let f = fixture();
    let d = ContextGroupId::from([0xC3u8; 32]);
    add_group(&f.store, &f.g, &d, f.admin, VisibilityMode::Open);

    leave(&f, &f.ns);

    assert!(owes(&f, &f.ns));
    assert!(owes(&f, &f.g));
    assert!(
        owes(&f, &d),
        "the Open child of a Restricted group the leaver left with the namespace owes a \
         rotation"
    );
}

#[test]
fn an_own_keyed_open_group_wraps_its_rotation_for_those_who_inherit_it() {
    // Inheritance through Open writes no row, so a recipient list of direct rows
    // only left every inheritor out of the rotation of an own-keyed Open group.
    let f = fixture();
    let d = ContextGroupId::from([0xC3u8; 32]);
    add_group(&f.store, &f.g, &d, f.admin, VisibilityMode::Open);

    remove(&f, &f.g);

    let entitled: Vec<AccountId> = GroupKeyring::new(&f.store, d)
        .current_key_recipients()
        .unwrap()
        .into_iter()
        .map(|e| e.member)
        .collect();
    assert!(
        entitled.contains(&f.staying),
        "a member who still inherits the group must receive its next key"
    );
    assert!(entitled.contains(&f.admin));
    assert!(
        !entitled.contains(&f.departing),
        "the removed member no longer inherits and must not be a recipient"
    );
}

#[test]
fn a_restricted_group_wraps_for_its_direct_members_only() {
    // The widening applies to Open groups covered by their own key; a Restricted
    // group has no inheritors, so its recipients stay its direct rows.
    let f = fixture();
    let entitled: Vec<AccountId> = GroupKeyring::new(&f.store, f.g)
        .current_key_recipients()
        .unwrap()
        .into_iter()
        .map(|e| e.member)
        .collect();
    let mut expected = vec![f.admin, f.departing, f.staying];
    expected.sort();
    let mut got = entitled;
    got.sort();
    got.dedup();
    assert_eq!(got, expected);
}

#[test]
fn an_admin_of_the_group_left_may_pay_the_debt_of_its_open_child() {
    // Whoever may remove a member from an own-keyed group is an admin of it, and
    // admin authority cascades down the Open chain into the exposed child. Pinned
    // with an admin of the wall alone: no row in the child, none at the root.
    let f = fixture();
    let d = ContextGroupId::from([0xC3u8; 32]);
    add_group(&f.store, &f.g, &d, f.admin, VisibilityMode::Open);
    let wall_admin_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
    let wall_admin = enrol_member(&f.store, &f.ns, &wall_admin_sk.public_key());
    MembershipRepository::new(&f.store)
        .add_member(&f.g, &wall_admin, GroupMemberRole::Admin)
        .unwrap();

    let removal = SignedGroupOp::sign(
        &wall_admin_sk,
        f.g.to_bytes().into(),
        vec![],
        1,
        GroupOp::MemberRemoved {
            member: f.departing,
            expected_group_state_hash: [0u8; 32],
            expected_context_state_hashes: Vec::new(),
        },
    )
    .expect("sign MemberRemoved");
    apply_local_signed_group_op(&f.store, &removal).expect("the wall's admin removes");
    assert!(owes(&f, &d));

    let rotation = SignedGroupOp::sign(
        &wall_admin_sk,
        d.to_bytes().into(),
        vec![],
        2,
        GroupOp::GroupKeyRotated {
            departed: f.departing,
        },
    )
    .expect("sign GroupKeyRotated");
    apply_local_signed_group_op(&f.store, &rotation)
        .expect("an admin of the wall is an admin of its Open child and may rotate it");
    assert!(!owes(&f, &d), "the rotation discharges the debt");
}
