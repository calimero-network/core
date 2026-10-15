//! A TEE may pay a rotation debt.
//!
//! A departure leaves a `PendingRotation` row that only an admin could pay. An
//! org always runs a TEE replica, which holds every key the org's groups use, so
//! a removal or a self-leave need not wait for an admin to come online: a
//! `GroupKeyRotated { departed }` signed by a TEE is accepted when `departed` is
//! not a member of the group at the op's cut. Nothing else changes for a TEE.
//!
//! These tests pin that rule at the op handler, at the rotation sidecar the
//! namespace layer applies, and at the publisher.

use calimero_account::AccountId;
use calimero_context_client::local_governance::{
    GroupOp, NamespaceOp, SignedGroupOp, SignedNamespaceOp,
};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::{MemberCapabilities, VisibilityMode};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;

use crate::test_fixtures::{
    bootstrap_namespace_with_admin_account, enrol_local_device, enrol_member, nest_for_test,
    sample_meta_with_admin, test_store, UnresolvableAuthorizer,
};
use crate::{
    apply_local_signed_group_op, apply_local_signed_group_op_at_cut, ApplyError,
    CapabilitiesRepository, GroupKeyring, KeyRecipient, MembershipRepository, MetaRepository,
    NamespaceGovernance, NamespaceRepository, PendingRotationRepository,
};

/// A namespace root `ns`, a Restricted group `g` under it, and an Open child `d`
/// of `g` (covered by its own key). `departed` was removed from `g`, which left
/// `PendingRotation(d, departed)`. `staying` still inherits `d` through `g`.
/// `tee` is a TEE at the root and in `g`, so it inherits `d` and holds its key.
struct Fixture {
    store: Store,
    ns: ContextGroupId,
    g: ContextGroupId,
    d: ContextGroupId,
    departed: AccountId,
    staying_sk: PrivateKey,
    staying: AccountId,
    tee_sk: PrivateKey,
    tee: AccountId,
}

fn fixture(tee_role: GroupMemberRole) -> Fixture {
    let store = test_store();
    let ns_bytes = [0xD1u8; 32];
    let ns = ContextGroupId::from(ns_bytes);
    let ((admin_sk, _), admin) = bootstrap_namespace_with_admin_account(&store, ns_bytes);

    let g = ContextGroupId::from([0xD2u8; 32]);
    let d = ContextGroupId::from([0xD3u8; 32]);
    add_group(&store, &ns, &g, admin, VisibilityMode::Restricted);
    add_group(&store, &g, &d, admin, VisibilityMode::Open);

    let departed_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
    let departed = enrol_member(&store, &ns, &departed_sk.public_key());
    let staying_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
    let staying = enrol_member(&store, &ns, &staying_sk.public_key());
    let tee_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
    let tee = enrol_member(&store, &ns, &tee_sk.public_key());
    for (account, role) in [
        (departed, GroupMemberRole::Member),
        (staying, GroupMemberRole::Member),
        (tee, tee_role),
    ] {
        add_inheriting_member(&store, &ns, account, role.clone());
        add_inheriting_member(&store, &g, account, role);
    }
    let members = MembershipRepository::new(&store);
    for account in [departed, staying, tee] {
        assert!(
            members.is_live_member(&d, &account).unwrap(),
            "precondition: every member of g inherits its Open child"
        );
    }

    let removal = SignedGroupOp::sign(
        &admin_sk,
        g.to_bytes().into(),
        vec![],
        1,
        GroupOp::MemberRemoved {
            member: departed,
            expected_group_state_hash: [0u8; 32],
            expected_context_state_hashes: Vec::new(),
        },
    )
    .expect("sign MemberRemoved");
    apply_local_signed_group_op(&store, &removal).expect("apply MemberRemoved");
    assert!(
        PendingRotationRepository::new(&store)
            .is_pending(&d, &departed)
            .unwrap(),
        "precondition: the removal owes a rotation of the Open child"
    );

    Fixture {
        store,
        ns,
        g,
        d,
        departed,
        staying_sk,
        staying,
        tee_sk,
        tee,
    }
}

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

fn add_inheriting_member(
    store: &Store,
    group: &ContextGroupId,
    account: AccountId,
    role: GroupMemberRole,
) {
    MembershipRepository::new(store)
        .add_member(group, &account, role)
        .unwrap();
    CapabilitiesRepository::new(store)
        .set_member_capability(
            group,
            &account,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
        )
        .unwrap();
}

fn rotation_op(signer: &PrivateKey, group: &ContextGroupId, departed: AccountId) -> SignedGroupOp {
    SignedGroupOp::sign(
        signer,
        group.to_bytes().into(),
        vec![],
        1,
        GroupOp::GroupKeyRotated { departed },
    )
    .expect("sign GroupKeyRotated")
}

fn owes(f: &Fixture) -> bool {
    PendingRotationRepository::new(&f.store)
        .is_pending(&f.d, &f.departed)
        .unwrap()
}

#[test]
fn a_tee_pays_the_rotation_a_departed_member_left_owed() {
    let f = fixture(GroupMemberRole::ReadOnlyTee);
    apply_local_signed_group_op(&f.store, &rotation_op(&f.tee_sk, &f.d, f.departed))
        .expect("a TEE rotating out a member who is gone must apply");
    assert!(!owes(&f), "the TEE's rotation must discharge the debt");
}

#[test]
fn a_relay_tee_pays_it_too() {
    let f = fixture(GroupMemberRole::RelayTee);
    apply_local_signed_group_op(&f.store, &rotation_op(&f.tee_sk, &f.d, f.departed))
        .expect("both TEE roles may pay a rotation debt");
    assert!(!owes(&f));
}

#[test]
fn a_tee_cannot_rotate_out_a_current_member() {
    let f = fixture(GroupMemberRole::ReadOnlyTee);
    let _ = apply_local_signed_group_op(&f.store, &rotation_op(&f.tee_sk, &f.d, f.staying))
        .expect_err("a TEE naming a member who is still there must be refused");
    assert!(owes(&f), "a refused rotation pays nothing");
}

#[test]
fn a_plain_member_still_cannot_pay() {
    let f = fixture(GroupMemberRole::ReadOnlyTee);
    let _ = apply_local_signed_group_op(&f.store, &rotation_op(&f.staying_sk, &f.d, f.departed))
        .expect_err("only an admin or a TEE may rotate");
    assert!(
        owes(&f),
        "the debt must survive a rejected rotation, or it is written off unpaid"
    );
}

#[test]
fn a_tee_outside_the_group_cannot_rotate_it() {
    let f = fixture(GroupMemberRole::ReadOnlyTee);
    // A TEE of the namespace that was never let into the Restricted wall: it does
    // not reach the child, so it holds no key of it and has nothing to rotate.
    let outsider_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
    let outsider = enrol_member(&f.store, &f.ns, &outsider_sk.public_key());
    add_inheriting_member(&f.store, &f.ns, outsider, GroupMemberRole::ReadOnlyTee);
    let _ = apply_local_signed_group_op(&f.store, &rotation_op(&outsider_sk, &f.d, f.departed))
        .expect_err("a TEE that is not a member of the group may not rotate it");
    assert!(owes(&f));
    let _ = f.g;
}

#[test]
fn a_tee_rotation_at_an_unresolvable_cut_parks() {
    let f = fixture(GroupMemberRole::ReadOnlyTee);
    let op = SignedGroupOp::sign(
        &f.tee_sk,
        f.d.to_bytes().into(),
        vec![[0xAB; 32]],
        1,
        GroupOp::GroupKeyRotated {
            departed: f.departed,
        },
    )
    .expect("sign GroupKeyRotated");
    let err = apply_local_signed_group_op_at_cut(&f.store, &op, &UnresolvableAuthorizer)
        .expect_err("a cut this replica cannot resolve must not be decided");
    assert!(
        matches!(
            err.downcast_ref::<ApplyError>(),
            Some(ApplyError::AuthorityUndecidable { .. })
        ),
        "expected a park, got: {err:#}"
    );
    assert!(owes(&f), "a parked rotation pays nothing yet");
}

/// The namespace-level apply: a TEE-signed `GroupKeyRotated` carrying the new key
/// as its sidecar, received by a member who stays. The sidecar has its own gate,
/// separate from the op handler's; both must accept a TEE or the debt is cleared
/// while every peer throws the key away.
fn tee_rotation_received_by_a_member(f: &Fixture, departed: AccountId) -> ([u8; 32], Store) {
    let _ = f.tee;
    // The receiving node: a member that stays, holding its own device secret so it
    // can open the envelope addressed to it.
    let local_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
    let (local, _, _) = enrol_local_device(&f.store, &f.ns, &local_sk.public_key());
    add_inheriting_member(&f.store, &f.ns, local, GroupMemberRole::Member);
    add_inheriting_member(&f.store, &f.g, local, GroupMemberRole::Member);
    NamespaceRepository::new(&f.store)
        .replace_identity(&f.ns, &local_sk.public_key(), local_sk.as_bytes())
        .unwrap();

    let keyring = GroupKeyring::new(&f.store, f.d);
    let (key_id, key) = keyring.load_current_key().unwrap().expect("d's key");
    let new_key = [0x5Eu8; 32];
    let recipients: Vec<KeyRecipient> = keyring
        .current_key_recipients()
        .unwrap()
        .into_iter()
        .filter(|entitled| entitled.member != departed)
        .map(|entitled| entitled.recipient)
        .collect();
    let rotation = keyring
        .build_rotation(&new_key, &f.tee_sk, &recipients)
        .unwrap();
    let encrypted = GroupKeyring::encrypt_op(&key, &GroupOp::GroupKeyRotated { departed }).unwrap();
    let op = SignedNamespaceOp::sign(
        &f.tee_sk,
        f.ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Group {
            group_id: f.d.to_bytes().into(),
            key_id: key_id.into(),
            encrypted,
            key_rotation: Some(rotation),
        },
    )
    .unwrap();
    let _ = NamespaceGovernance::new(&f.store, f.ns.to_bytes().into()).apply_signed_op(&op);
    (new_key, f.store.clone())
}

#[test]
fn peers_adopt_the_key_a_tee_rotated_to() {
    let f = fixture(GroupMemberRole::ReadOnlyTee);
    let (new_key, store) = tee_rotation_received_by_a_member(&f, f.departed);
    assert_eq!(
        GroupKeyring::new(&store, f.d)
            .load_current_key()
            .unwrap()
            .map(|(_, key)| key),
        Some(new_key),
        "a member who stays must adopt the key a TEE rotated to"
    );
    assert!(!owes(&f));
}

#[test]
fn peers_ignore_a_tee_rotation_that_names_a_member() {
    let f = fixture(GroupMemberRole::ReadOnlyTee);
    let (new_key, store) = tee_rotation_received_by_a_member(&f, f.staying);
    assert!(
        GroupKeyring::new(&store, f.d)
            .load_key_by_id(&GroupKeyring::key_id_for(&new_key))
            .unwrap()
            .is_none(),
        "a TEE rotation naming a current member must not reach the keyring"
    );
}

/// The publisher refuses to mint a key peers would reject. With this node's
/// namespace identity a TEE, it may now pay a debt, and nothing more.
#[test]
fn a_tee_node_may_publish_a_debt_payment_and_nothing_else() {
    use crate::group_governance_publisher::ensure_rotation_is_publishable_for;

    let f = fixture(GroupMemberRole::ReadOnlyTee);
    NamespaceRepository::new(&f.store)
        .replace_identity(&f.ns, &f.tee_sk.public_key(), f.tee_sk.as_bytes())
        .unwrap();

    ensure_rotation_is_publishable_for(&f.store, f.d, None, Some(&f.departed))
        .expect("a TEE may publish the rotation a departed member left owed");
    let _ = ensure_rotation_is_publishable_for(&f.store, f.d, None, Some(&f.staying))
        .expect_err("a TEE may not rotate out a member who is still there");
    let _ = ensure_rotation_is_publishable_for(&f.store, f.d, None, None)
        .expect_err("a TEE may not remove anyone, so it may not mint a removal's rotation");
    assert!(
        crate::tee_may_pay_rotation(&f.store, &f.d, &f.tee_sk.public_key(), &f.departed).unwrap(),
        "the listener's eligibility check agrees"
    );
    assert!(
        !crate::tee_may_pay_rotation(&f.store, &f.d, &f.tee_sk.public_key(), &f.staying).unwrap()
    );
}

/// A projection that has folded enough to say the signer is no admin at the cut,
/// and nothing else: the TEE half of the gate is then reached and must park
/// rather than read live rows.
struct KnowsOnlyAdmins;

impl crate::authorizer::AtCutAuthorizer for KnowsOnlyAdmins {
    fn is_admin_at_cut(
        &self,
        _group: &ContextGroupId,
        _signer: &calimero_primitives::identity::PublicKey,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        Some(false)
    }

    fn is_admin_or_capability_at_cut(
        &self,
        _group: &ContextGroupId,
        _signer: &calimero_primitives::identity::PublicKey,
        _capability: u32,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }

    fn is_admin_or_capability_account_at_cut(
        &self,
        _group: &ContextGroupId,
        _member: &AccountId,
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
        Some(false)
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

    fn effective_role_at_cut(
        &self,
        _group: &ContextGroupId,
        _member: &AccountId,
        _parents: &[[u8; 32]],
    ) -> Option<Option<GroupMemberRole>> {
        None
    }

    fn context_rotation_group_at_cut(
        &self,
        _group: &ContextGroupId,
        _context: &calimero_primitives::context::ContextId,
        _parents: &[[u8; 32]],
    ) -> Option<Option<ContextGroupId>> {
        None
    }

    fn can_resolve_cut(&self, _group: &ContextGroupId, _parents: &[[u8; 32]]) -> bool {
        false
    }
}

#[test]
fn the_tee_half_of_the_gate_parks_rather_than_reading_live_rows() {
    let f = fixture(GroupMemberRole::ReadOnlyTee);
    let op = SignedGroupOp::sign(
        &f.tee_sk,
        f.d.to_bytes().into(),
        vec![[0xAC; 32]],
        1,
        GroupOp::GroupKeyRotated {
            departed: f.departed,
        },
    )
    .expect("sign GroupKeyRotated");
    let err = apply_local_signed_group_op_at_cut(&f.store, &op, &KnowsOnlyAdmins)
        .expect_err("live rows say the TEE may pay; the cut has not been read");
    assert!(
        matches!(
            err.downcast_ref::<ApplyError>(),
            Some(ApplyError::AuthorityUndecidable { .. })
        ),
        "expected a park, got: {err:#}"
    );
    assert!(owes(&f));
}

/// A debt left on a group that is now covered by the namespace key (its wall
/// was opened after the departure): as for an admin, the payment mints no key,
/// because a per-group key there would seal nothing, and it still settles the
/// row on every peer.
#[test]
fn a_tee_payment_for_a_namespace_covered_group_mints_nothing_and_settles_the_debt() {
    use crate::group_governance_publisher::ensure_rotation_is_publishable_for;
    use crate::pending_rotation::group_rotates_on_departure;

    let f = fixture(GroupMemberRole::ReadOnlyTee);
    CapabilitiesRepository::new(&f.store)
        .set_subgroup_visibility(&f.g, VisibilityMode::Open)
        .unwrap();
    assert!(
        !group_rotates_on_departure(&f.store, &f.d).unwrap(),
        "precondition: the child is now covered by the namespace key"
    );
    assert!(
        !MembershipRepository::new(&f.store)
            .is_live_member(&f.d, &f.departed)
            .unwrap(),
        "precondition: the removal still keeps the departed member out"
    );
    NamespaceRepository::new(&f.store)
        .replace_identity(&f.ns, &f.tee_sk.public_key(), f.tee_sk.as_bytes())
        .unwrap();

    ensure_rotation_is_publishable_for(&f.store, f.d, None, Some(&f.departed))
        .expect("there is no key to mint, so nothing to authorize");
    apply_local_signed_group_op(&f.store, &rotation_op(&f.tee_sk, &f.d, f.departed))
        .expect("the payment applies and carries no key");
    assert!(!owes(&f), "the debt is settled");
}
