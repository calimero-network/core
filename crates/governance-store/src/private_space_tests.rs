//! Structure rules of private spaces, judged where each op is applied.

use std::cell::Cell;

use calimero_account::AccountId;
use calimero_context_client::local_governance::{
    GroupOp, NamespaceOp, RootOp, SignedGroupOp, SignedNamespaceOp,
};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::VisibilityMode;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use eyre::Result as EyreResult;

use crate::namespace::NamespaceGovernance;
use crate::test_fixtures::{
    apply_member_joined, bootstrap_namespace_with_admin_account, derived_group_id, enrol_member,
    group_created, seal_for_test, signed_invitation_for, test_store,
};
use crate::{
    apply_local_signed_group_op, CapabilitiesRepository, MetaRepository,
    NamespaceMembershipService, NamespaceRepository, PrivateSpaceError,
};

const NS: [u8; 32] = [0xB1; 32];

struct Space {
    store: Store,
    sk: PrivateKey,
    admin: AccountId,
    nonce: Cell<u64>,
    tag: Cell<u8>,
}

impl Space {
    fn new() -> Self {
        let store = test_store();
        let ((sk, _), admin) = bootstrap_namespace_with_admin_account(&store, NS);
        Self {
            store,
            sk,
            admin,
            nonce: Cell::new(0),
            tag: Cell::new(0),
        }
    }

    fn next_nonce(&self) -> u64 {
        self.nonce.set(self.nonce.get() + 1);
        self.nonce.get()
    }

    fn root_op(&self, op: RootOp) -> EyreResult<()> {
        self.root_op_as(&self.sk, op)
    }

    fn root_op_as(&self, sk: &PrivateKey, op: RootOp) -> EyreResult<()> {
        let ns = ContextGroupId::from(NS);
        let signed = SignedNamespaceOp::sign(
            sk,
            NS.into(),
            vec![],
            self.next_nonce(),
            seal_for_test(&self.store, ns, op),
        )?;
        NamespaceGovernance::new(&self.store, NS.into())
            .apply_signed_op(&signed)
            .and_then(|applied| applied.refused.map_or(Ok(()), Err))
    }

    fn group_op(&self, group: [u8; 32], op: GroupOp) -> EyreResult<()> {
        let signed = SignedGroupOp::sign(&self.sk, group.into(), vec![], self.next_nonce(), op)?;
        apply_local_signed_group_op(&self.store, &signed)
    }

    fn create(&self, parent: [u8; 32], restricted: bool) -> EyreResult<[u8; 32]> {
        self.tag.set(self.tag.get() + 1);
        let tag = self.tag.get();
        let space_parent = self.in_space(parent);
        self.root_op(group_created(self.admin, parent, restricted, tag))?;
        let created = derived_group_id(&self.admin, parent, restricted, tag);
        // A private space's members hold its key, which seals the ops of the folders inside it.
        if restricted && !space_parent {
            let _ = crate::GroupKeyring::new(&self.store, created.into())
                .store_key(&[tag; 32])
                .expect("mint the space's key");
        }
        Ok(created)
    }

    fn in_space(&self, group: [u8; 32]) -> bool {
        crate::private_space::Tree::live(&self.store, &NS.into())
            .and_then(|tree| tree.in_space(&group.into()))
            .unwrap_or(false)
    }

    fn open(&self, parent: [u8; 32]) -> [u8; 32] {
        self.create(parent, false).expect("create an open folder")
    }

    fn private(&self, parent: [u8; 32]) -> [u8; 32] {
        self.create(parent, true).expect("create a private space")
    }

    fn reparent(&self, child: [u8; 32], new_parent: [u8; 32]) -> EyreResult<()> {
        self.root_op(RootOp::GroupReparented {
            child_group_id: child.into(),
            new_parent_id: new_parent.into(),
        })
    }

    fn delete(&self, group: [u8; 32]) -> EyreResult<()> {
        let payload = NamespaceRepository::new(&self.store)
            .collect_subtree_for_cascade(&group.into())
            .expect("collect the subtree");
        self.root_op(RootOp::GroupDeleted {
            root_group_id: group.into(),
            cascade_group_ids: payload
                .descendant_groups
                .iter()
                .map(|g| g.to_bytes().into())
                .collect(),
            cascade_context_ids: vec![],
        })
    }

    fn set_visibility(&self, group: [u8; 32], mode: VisibilityMode) -> EyreResult<()> {
        self.group_op(group, GroupOp::SubgroupVisibilitySet { mode })
    }

    fn add_member(&self, group: [u8; 32]) -> EyreResult<()> {
        let pk =
            PrivateKey::random(&mut rand::rand_core::UnwrapErr(rand::rngs::SysRng)).public_key();
        let member = enrol_member(&self.store, &ContextGroupId::from(NS), &pk);
        crate::MembershipRepository::new(&self.store)
            .add_member(&NS.into(), &member, GroupMemberRole::Member)
            .unwrap();
        self.group_op(
            group,
            GroupOp::MemberAdded {
                member,
                role: GroupMemberRole::Member,
            },
        )
    }

    fn parent(&self, group: [u8; 32]) -> Option<[u8; 32]> {
        NamespaceRepository::new(&self.store)
            .parent(&group.into())
            .unwrap()
            .map(|p| p.to_bytes())
    }

    fn exists(&self, group: [u8; 32]) -> bool {
        MetaRepository::new(&self.store)
            .load(&group.into())
            .unwrap()
            .is_some()
    }
}

#[test]
fn a_private_space_may_sit_in_the_open_tree() {
    let s = Space::new();
    let folder = s.open(NS);
    let space = s.private(folder);
    assert_eq!(s.parent(space), Some(folder));
    s.open(space);
}

#[test]
fn a_private_space_cannot_be_created_inside_a_private_space() {
    let s = Space::new();
    let space = s.private(NS);
    let inner_folder = s.open(space);

    assert!(s.create(space, true).is_err(), "directly under a space");
    assert!(s.create(inner_folder, true).is_err(), "deeper in a space");
    assert!(s.create(space, false).is_ok(), "an open folder is fine");
}

#[test]
fn making_a_folder_private_is_refused_inside_a_space() {
    let s = Space::new();
    let space = s.private(NS);
    let inner = s.open(space);

    assert!(s.set_visibility(inner, VisibilityMode::Restricted).is_err());
}

#[test]
fn making_a_folder_private_is_refused_when_its_subtree_holds_a_space() {
    let s = Space::new();
    let folder = s.open(NS);
    let _space = s.private(folder);

    assert!(s
        .set_visibility(folder, VisibilityMode::Restricted)
        .is_err());
}

#[test]
fn making_an_open_folder_private_is_allowed_in_the_open_tree() {
    let s = Space::new();
    let folder = s.open(NS);
    s.open(folder);

    s.set_visibility(folder, VisibilityMode::Restricted)
        .expect("a clean open subtree may become a space");
    assert_eq!(
        CapabilitiesRepository::new(&s.store)
            .subgroup_visibility(&folder.into())
            .unwrap(),
        VisibilityMode::Restricted
    );
}

#[test]
fn a_space_cannot_be_moved_under_a_space() {
    let s = Space::new();
    let a = s.private(NS);
    let b = s.private(NS);

    assert!(s.reparent(b, a).is_err());
    assert_eq!(s.parent(b), Some(NS));
}

#[test]
fn a_folder_holding_a_space_cannot_be_moved_into_a_space() {
    let s = Space::new();
    let space = s.private(NS);
    let folder = s.open(NS);
    let _nested = s.private(folder);

    assert!(s.reparent(folder, space).is_err());
}

#[test]
fn a_folder_cannot_leave_a_space() {
    let s = Space::new();
    let space = s.private(NS);
    let inner = s.open(space);
    let outside = s.open(NS);

    assert!(s.reparent(inner, outside).is_err(), "out to an open folder");
    assert!(s.reparent(inner, NS).is_err(), "out to the root");
    assert_eq!(s.parent(inner), Some(space));
}

#[test]
fn folders_move_into_a_space_and_within_it() {
    let s = Space::new();
    let space = s.private(NS);
    let inner_a = s.open(space);
    let inner_b = s.open(space);
    let outside = s.open(NS);

    s.reparent(outside, space).expect("into a space");
    s.reparent(inner_a, inner_b).expect("within a space");

    let folder = s.open(NS);
    let other = s.private(folder);
    s.reparent(other, NS)
        .expect("a space moves in the open tree");
}

#[test]
fn a_group_holding_a_nested_space_cannot_be_deleted() {
    let s = Space::new();
    let folder = s.open(NS);
    let space = s.private(folder);

    assert!(s.delete(folder).is_err());
    assert!(s.exists(folder) && s.exists(space));
    s.delete(space).expect("the space itself can be deleted");
    s.delete(folder).expect("and then the empty folder");
}

#[test]
fn a_space_with_only_open_folders_can_be_deleted() {
    let s = Space::new();
    let space = s.private(NS);
    s.open(space);

    s.delete(space).expect("delete the space and its folders");
    assert!(!s.exists(space));
}

#[test]
fn members_are_added_at_the_root_or_a_space_root() {
    let s = Space::new();
    let folder = s.open(NS);
    let space = s.private(NS);
    let inner = s.open(space);

    assert!(s.add_member(folder).is_err(), "an open folder");
    assert!(s.add_member(inner).is_err(), "a folder in a space");
    s.add_member(NS).expect("the workspace root");
    s.add_member(space).expect("a space root");
}

fn refused_as_invitation_target(err: &eyre::Report) -> bool {
    err.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<PrivateSpaceError>(),
            Some(PrivateSpaceError::InvitationTargetNotRoot(_))
        )
    })
}

#[test]
fn the_responder_gate_admits_invitations_to_the_namespace_root_only() {
    let s = Space::new();
    let folder = s.open(NS);
    let space = s.private(NS);
    let service = NamespaceMembershipService::new(&s.store, NS.into());
    let invite = |group: [u8; 32]| signed_invitation_for(&s.sk, group.into(), [0x31; 32]);

    service
        .validate_open_invitation(&invite(NS), 0)
        .expect("the namespace root");
    for target in [folder, space] {
        let err = service
            .validate_open_invitation(&invite(target), 0)
            .expect_err("a subgroup is not an invitation target");
        assert!(refused_as_invitation_target(&err), "{err:#}");
    }
}

#[test]
fn a_join_naming_a_subgroup_is_refused_when_applied() {
    let s = Space::new();
    let space = s.private(NS);
    let joiner = PrivateKey::random(&mut rand::rand_core::UnwrapErr(rand::rngs::SysRng));

    let invitation = signed_invitation_for(&s.sk, space.into(), [0x32; 32]);
    let err = apply_member_joined(&s.store, NS, &joiner, invitation, 1, &s.sk)
        .expect_err("a join into a space root is not a join");
    assert!(refused_as_invitation_target(&err), "{err:#}");

    let invitation = signed_invitation_for(&s.sk, NS.into(), [0x33; 32]);
    apply_member_joined(&s.store, NS, &joiner, invitation, 2, &s.sk)
        .expect("a join into the namespace root");
}

/// A namespace admin outside a space has no authority in it; the space's own admin has, whether
/// or not it administers the namespace.
#[test]
fn only_a_spaces_own_admins_create_move_or_delete_inside_it() {
    let s = Space::new();
    let space = s.private(NS);
    let folder = s.open(space);
    let account = |role: GroupMemberRole, in_space: Option<GroupMemberRole>| {
        let sk = PrivateKey::random(&mut rand::rand_core::UnwrapErr(rand::rngs::SysRng));
        let who = enrol_member(&s.store, &ContextGroupId::from(NS), &sk.public_key());
        let members = crate::MembershipRepository::new(&s.store);
        members.add_member(&NS.into(), &who, role).unwrap();
        if let Some(role) = in_space {
            members.add_member(&space.into(), &who, role).unwrap();
        }
        (sk, who)
    };
    let (outsider_sk, outsider) = account(GroupMemberRole::Admin, None);
    let (admin_sk, space_admin) = account(GroupMemberRole::Member, Some(GroupMemberRole::Admin));
    let delete = |group: [u8; 32]| RootOp::GroupDeleted {
        root_group_id: group.into(),
        cascade_group_ids: vec![group.into()],
        cascade_context_ids: vec![],
    };
    let reparent = |child: [u8; 32], to: [u8; 32]| RootOp::GroupReparented {
        child_group_id: child.into(),
        new_parent_id: to.into(),
    };

    let created = s.root_op_as(&outsider_sk, group_created(outsider, space, false, 0x71));
    assert!(created.is_err(), "{created:?}");
    assert!(s.root_op_as(&outsider_sk, reparent(folder, NS)).is_err());
    assert!(s.root_op_as(&outsider_sk, delete(folder)).is_err());

    s.root_op_as(&admin_sk, group_created(space_admin, space, false, 0x72))
        .expect("a space admin creates inside its space");
    s.root_op_as(&admin_sk, delete(folder))
        .expect("and deletes there");
}

/// A node that lacks a space's key holds the ops of the space's folders as unread, and its
/// namespace head keeps advancing through them.
#[test]
fn a_node_outside_a_space_keeps_advancing_through_its_inner_folder_ops() {
    let s = Space::new();
    let space = s.private(NS);
    let kept = s.open(space);
    let moved = s.open(space);
    let ns = ContextGroupId::from(NS);
    let create = group_created(s.admin, space, false, 0x71);
    let reparent = RootOp::GroupReparented {
        child_group_id: kept.into(),
        new_parent_id: moved.into(),
    };
    let delete = RootOp::GroupDeleted {
        root_group_id: moved.into(),
        cascade_group_ids: vec![moved.into()],
        cascade_context_ids: vec![],
    };
    let sealed: Vec<_> = [create, reparent, delete]
        .into_iter()
        .map(|op| seal_for_test(&s.store, ns, op))
        .collect();
    assert!(sealed
        .iter()
        .all(|op| matches!(op, NamespaceOp::RootSealedForGroup { group_id, .. } if *group_id == space.into())));

    let keyring = crate::GroupKeyring::new(&s.store, space.into());
    while let Some((key_id, _)) = keyring.load_current_key().unwrap() {
        keyring.delete_key_by_id(&key_id).unwrap();
    }
    for op in sealed {
        let head = crate::NamespaceDagService::new(&s.store, NS.into())
            .read_head_record()
            .unwrap();
        let signed =
            SignedNamespaceOp::sign(&s.sk, NS.into(), head.parent_hashes, s.next_nonce(), op)
                .unwrap();
        let applied = NamespaceGovernance::new(&s.store, NS.into())
            .apply_signed_op(&signed)
            .expect("an op the node cannot read does not stall it");
        assert!(applied.refused.is_none());
        let after = crate::NamespaceDagService::new(&s.store, NS.into())
            .read_head_record()
            .unwrap();
        assert_eq!(after.parent_hashes, vec![signed.content_hash().unwrap()]);
    }
}

/// A space root is deleted by its owner alone: neither a root `CAN_DELETE_SUBGROUP` holder
/// nor a namespace admin who does not own it may.
#[test]
fn only_a_spaces_owner_deletes_its_root() {
    let s = Space::new();
    let space = s.private(NS);
    let account = |role: GroupMemberRole| {
        let sk = PrivateKey::random(&mut rand::rand_core::UnwrapErr(rand::rngs::SysRng));
        let who = enrol_member(&s.store, &ContextGroupId::from(NS), &sk.public_key());
        crate::MembershipRepository::new(&s.store)
            .add_member(&NS.into(), &who, role)
            .unwrap();
        (sk, who)
    };
    let (delegate_sk, delegate) = account(GroupMemberRole::Member);
    crate::CapabilitiesRepository::new(&s.store)
        .set_member_capability(
            &NS.into(),
            &delegate,
            calimero_context_config::MemberCapabilities::CAN_DELETE_SUBGROUP.bits(),
        )
        .unwrap();
    let (admin_sk, _) = account(GroupMemberRole::Admin);
    let delete = RootOp::GroupDeleted {
        root_group_id: space.into(),
        cascade_group_ids: vec![space.into()],
        cascade_context_ids: vec![],
    };

    assert!(s.root_op_as(&delegate_sk, delete.clone()).is_err());
    assert!(s.root_op_as(&admin_sk, delete.clone()).is_err());
    s.root_op(delete).expect("the owner deletes its space");
}

/// A folder an outsider still holds below a space root, though the space's ops removed it, does
/// not split a board verdict: deleting the space applies on the stale node too.
#[test]
fn a_stale_folder_below_a_space_root_does_not_split_a_board_delete() {
    let s = Space::new();
    let space = s.private(NS);
    let _stale = s.open(space);
    let payload = RootOp::GroupDeleted {
        root_group_id: space.into(),
        cascade_group_ids: vec![space.into()],
        cascade_context_ids: vec![],
    };

    s.root_op(payload)
        .expect("the stale folder is the space's to keep current");
    assert!(!s.exists(space));
}

/// A space root is moved by its owner alone: a namespace admin who does not own it has no
/// authority over it.
#[test]
fn a_namespace_admin_who_is_not_the_owner_cannot_move_a_space_root() {
    let s = Space::new();
    let space = s.private(NS);
    let target = s.open(NS);
    let sk = PrivateKey::random(&mut rand::rand_core::UnwrapErr(rand::rngs::SysRng));
    let who = enrol_member(&s.store, &ContextGroupId::from(NS), &sk.public_key());
    crate::MembershipRepository::new(&s.store)
        .add_member(&NS.into(), &who, GroupMemberRole::Admin)
        .unwrap();
    let moves = RootOp::GroupReparented {
        child_group_id: space.into(),
        new_parent_id: target.into(),
    };

    assert!(s.root_op_as(&sk, moves.clone()).is_err());
    s.root_op(moves).expect("its owner moves it");
    assert_eq!(s.parent(space), Some(target));
}

/// A folder in a space has the space's members, so an inherited join into it needs no op, and
/// one that names it is refused rather than announced to the workspace.
#[test]
fn an_inherited_join_into_a_folder_of_a_space_is_refused() {
    let s = Space::new();
    let space = s.private(NS);
    let folder = s.open(space);
    let sk = PrivateKey::random(&mut rand::rand_core::UnwrapErr(rand::rngs::SysRng));
    let account = enrol_member(&s.store, &ContextGroupId::from(NS), &sk.public_key());
    crate::MembershipRepository::new(&s.store)
        .add_member(&NS.into(), &account, GroupMemberRole::Member)
        .unwrap();
    let join = RootOp::MemberJoinedOpen {
        member: account,
        group_id: folder.into(),
        account: crate::test_fixtures::real_join_account(&sk.public_key()),
    };

    let refused = s.root_op_as(&sk, join).expect_err("refused");
    assert!(
        format!("{refused:#}").contains("private space"),
        "{refused:#}"
    );
}

/// An owner that no longer belongs to the workspace has no authority over its space.
#[test]
fn a_removed_owner_deletes_or_moves_nothing() {
    let s = Space::new();
    let space = s.private(NS);
    let target = s.open(NS);
    crate::MembershipRepository::new(&s.store)
        .remove_member(&NS.into(), &s.admin)
        .unwrap();
    let delete = RootOp::GroupDeleted {
        root_group_id: space.into(),
        cascade_group_ids: vec![space.into()],
        cascade_context_ids: vec![],
    };
    let moves = RootOp::GroupReparented {
        child_group_id: space.into(),
        new_parent_id: target.into(),
    };

    assert!(s.root_op(delete).is_err());
    assert!(s.root_op(moves).is_err());
}

/// A space whose owner the workspace has since removed, and an admin to hand it to.
fn space_of_a_removed_owner() -> (Space, [u8; 32], AccountId) {
    let s = Space::new();
    let space = s.private(NS);
    let sk = PrivateKey::random(&mut rand::rand_core::UnwrapErr(rand::rngs::SysRng));
    let accomplice = enrol_member(&s.store, &ContextGroupId::from(NS), &sk.public_key());
    crate::MembershipRepository::new(&s.store)
        .add_member(&NS.into(), &accomplice, GroupMemberRole::Admin)
        .unwrap();
    crate::MembershipRepository::new(&s.store)
        .remove_member(&NS.into(), &s.admin)
        .unwrap();
    (s, space, accomplice)
}

fn guarded(s: &Space, space: [u8; 32], op: GroupOp) -> GroupOp {
    crate::test_fixtures::guarded_group_op(&s.store, &space.into(), &s.sk.public_key(), op)
}

#[test]
fn a_removed_owner_does_not_transfer_its_space_by_group_op() {
    let (s, space, accomplice) = space_of_a_removed_owner();
    let transfer = guarded(
        &s,
        space,
        GroupOp::TransferOwnership {
            new_owner: accomplice,
        },
    );
    assert!(s.group_op(space, transfer).is_err());
}

#[test]
fn a_removed_owner_does_not_delete_its_space_by_group_op() {
    let (s, space, _) = space_of_a_removed_owner();
    let delete = guarded(&s, space, GroupOp::GroupDelete);
    assert!(s.group_op(space, delete).is_err());
    assert!(s.exists(space));
}

/// Making a folder private founds a space with its signer as the first admin: another member
/// can then be removed without tripping the last-admin guard.
#[test]
fn making_a_folder_private_seats_its_signer_as_the_spaces_admin() {
    let s = Space::new();
    let folder = s.open(NS);
    s.set_visibility(folder, VisibilityMode::Restricted)
        .expect("make the folder private");
    let members = crate::MembershipRepository::new(&s.store);
    assert_eq!(
        members.role_of(&folder.into(), &s.admin).unwrap(),
        Some(GroupMemberRole::Admin),
    );

    let sk = PrivateKey::random(&mut rand::rand_core::UnwrapErr(rand::rngs::SysRng));
    let other = enrol_member(&s.store, &ContextGroupId::from(NS), &sk.public_key());
    members
        .add_member(&NS.into(), &other, GroupMemberRole::Member)
        .unwrap();
    s.group_op(
        folder,
        GroupOp::MemberAdded {
            member: other,
            role: GroupMemberRole::Admin,
        },
    )
    .expect("add a second admin");
    s.group_op(
        folder,
        GroupOp::MemberRemoved {
            member: other,
            expected_group_state_hash: [0u8; 32],
            expected_context_state_hashes: Vec::new(),
        },
    )
    .expect("the signer stays, so the guard does not trip");
}

/// The flip's rotation is addressed to the space's direct members, built after the flip applied:
/// its maker, seated by the flip, is among them, so it holds the key it minted.
#[test]
fn the_flips_key_rotation_reaches_the_space_maker() {
    let s = Space::new();
    let folder = s.open(NS);
    s.set_visibility(folder, VisibilityMode::Restricted)
        .expect("make the folder private");

    let recipients = crate::GroupKeyring::new(&s.store, folder.into())
        .current_key_recipients()
        .expect("list the recipients");
    assert!(
        recipients.iter().any(|entitled| entitled.member == s.admin),
        "the maker is entitled to the space's key"
    );
}

/// Making a space root private again is refused, and seats nobody.
#[test]
fn re_flipping_a_space_root_is_refused_and_seats_nobody() {
    let s = Space::new();
    let space = s.private(NS);
    let sk = PrivateKey::random(&mut rand::rand_core::UnwrapErr(rand::rngs::SysRng));
    let other = enrol_member(&s.store, &ContextGroupId::from(NS), &sk.public_key());
    crate::MembershipRepository::new(&s.store)
        .remove_member(&space.into(), &s.admin)
        .unwrap();

    assert!(s.set_visibility(space, VisibilityMode::Restricted).is_err());
    let members = crate::MembershipRepository::new(&s.store);
    assert_eq!(members.role_of(&space.into(), &s.admin).unwrap(), None);
    assert_eq!(members.role_of(&space.into(), &other).unwrap(), None);
}

/// A flip its signer may not make seats nobody: it is refused before anything is written.
#[test]
fn an_unauthorized_flip_seats_nobody() {
    let s = Space::new();
    let folder = s.open(NS);
    let sk = PrivateKey::random(&mut rand::rand_core::UnwrapErr(rand::rngs::SysRng));
    let who = enrol_member(&s.store, &ContextGroupId::from(NS), &sk.public_key());
    crate::MembershipRepository::new(&s.store)
        .add_member(&NS.into(), &who, GroupMemberRole::Member)
        .unwrap();
    let flip = SignedGroupOp::sign(
        &sk,
        folder.into(),
        vec![],
        1,
        GroupOp::SubgroupVisibilitySet {
            mode: VisibilityMode::Restricted,
        },
    )
    .unwrap();

    assert!(apply_local_signed_group_op(&s.store, &flip).is_err());
    assert_eq!(
        crate::MembershipRepository::new(&s.store)
            .role_of(&folder.into(), &who)
            .unwrap(),
        None,
    );
}

/// A node outside a space holds the space's own ops as unread; the owner's deletion of the space,
/// a workspace-sealed board op, still applies there from cleartext alone.
#[test]
fn an_outsider_applies_a_space_roots_delete_past_the_spaces_sealed_ops() {
    let s = Space::new();
    let space = s.private(NS);
    let ns = ContextGroupId::from(NS);
    let key = [0x66u8; 32];
    let keyring = crate::GroupKeyring::new(&s.store, space.into());
    let key_id = keyring.store_key(&key).unwrap();
    let rename = GroupOp::GroupMetadataSet {
        name: Some("sealed".to_owned()),
        data: std::collections::BTreeMap::new(),
    };
    let sealed = NamespaceOp::Group {
        group_id: space.into(),
        key_id: key_id.into(),
        encrypted: crate::GroupKeyring::encrypt_op(&key, &rename).unwrap(),
        key_rotation: None,
        registers: None,
    };
    let head = crate::NamespaceDagService::new(&s.store, NS.into())
        .read_head_record()
        .unwrap();
    let name =
        SignedNamespaceOp::sign(&s.sk, NS.into(), head.parent_hashes, s.next_nonce(), sealed)
            .unwrap();
    keyring.delete_key_by_id(&key_id).unwrap();
    let applied = NamespaceGovernance::new(&s.store, NS.into())
        .apply_signed_op(&name)
        .expect("an op the node cannot read does not stall it");
    assert!(applied.refused.is_none());

    let head = crate::NamespaceDagService::new(&s.store, NS.into())
        .read_head_record()
        .unwrap();
    let delete = seal_for_test(
        &s.store,
        ns,
        RootOp::GroupDeleted {
            root_group_id: space.into(),
            cascade_group_ids: vec![space.into()],
            cascade_context_ids: vec![],
        },
    );
    let signed =
        SignedNamespaceOp::sign(&s.sk, NS.into(), head.parent_hashes, s.next_nonce(), delete)
            .unwrap();
    let applied = NamespaceGovernance::new(&s.store, NS.into())
        .apply_signed_op(&signed)
        .expect("the owner's deletion applies without the space's key");
    assert!(applied.refused.is_none(), "{:?}", applied.refused);
    assert!(!s.exists(space));
}
