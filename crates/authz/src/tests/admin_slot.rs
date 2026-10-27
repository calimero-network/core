//! Who is an admin of a group: an `Admin` row, the creator slot unless the creator has
//! left the workspace, and for a folder with neither its lowest member.

use std::collections::BTreeMap;

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_op::ScopeId;
use calimero_primitives::context::GroupMemberRole;

use crate::view::{AclView, SubgroupEdge};

fn root() -> ContextGroupId {
    ContextGroupId::from([9u8; 32])
}

fn folder() -> ContextGroupId {
    ContextGroupId::from([2u8; 32])
}

fn account(byte: u8) -> AccountId {
    AccountId::from([byte; 32])
}

/// `folder` under `root`, created by `creator`, with the given direct rows in each.
fn view(
    creator: AccountId,
    root_rows: &[(AccountId, GroupMemberRole)],
    folder_rows: &[(AccountId, GroupMemberRole)],
) -> AclView {
    let mut groups = BTreeMap::new();
    for (group, rows) in [(root(), root_rows), (folder(), folder_rows)] {
        if !rows.is_empty() {
            let _ = groups.insert(group, rows.iter().cloned().collect());
        }
    }
    let mut subgroups = BTreeMap::new();
    let _ = subgroups.insert(
        ScopeId::from(folder().to_bytes()),
        SubgroupEdge {
            parent: ScopeId::from(root().to_bytes()),
            restricted: true,
        },
    );
    AclView {
        groups,
        subgroups,
        group_admin: BTreeMap::from([(folder(), creator)]),
        ..Default::default()
    }
}

/// [`view`] with `creator` on record as having left the workspace.
fn departed_view(
    creator: AccountId,
    root_rows: &[(AccountId, GroupMemberRole)],
    folder_rows: &[(AccountId, GroupMemberRole)],
) -> AclView {
    let mut v = view(creator, root_rows, folder_rows);
    let _ = v.namespace_departed.insert(creator);
    v
}

#[test]
fn a_creator_who_left_the_workspace_holds_no_admin_slot() {
    let creator = account(0x10);
    let rejoined = departed_view(creator, &[(creator, GroupMemberRole::Member)], &[]);
    let departed = departed_view(creator, &[], &[]);
    let never_joined = view(creator, &[], &[]);

    assert!(rejoined.is_group_admin(&creator, folder()));
    assert!(!departed.is_group_admin(&creator, folder()));
    assert!(never_joined.is_group_admin(&creator, folder()));
}

#[test]
fn the_root_creator_slot_needs_no_row() {
    let founder = account(0x10);
    let mut v = AclView::default();
    let _ = v.group_admin.insert(root(), founder);

    assert!(v.is_group_admin(&founder, root()));
}

#[test]
fn a_folder_left_without_an_admin_is_run_by_its_lowest_account() {
    let creator = account(0x10);
    let rows = [
        (account(0x90), GroupMemberRole::Member),
        (account(0x40), GroupMemberRole::Member),
        (account(0x05), GroupMemberRole::ReadOnly),
        (account(0x06), GroupMemberRole::ReadOnlyTee),
    ];
    let v = departed_view(creator, &rows, &rows);

    assert!(v.is_group_admin(&account(0x40), folder()));
    assert!(!v.is_group_admin(&account(0x90), folder()));
    assert!(!v.is_group_admin(&account(0x05), folder()));
    assert!(!v.is_group_admin(&account(0x06), folder()));
}

#[test]
fn a_folder_with_an_admin_has_no_stand_in() {
    let creator = account(0x10);
    let rows = [
        (account(0x90), GroupMemberRole::Admin),
        (account(0x40), GroupMemberRole::Member),
    ];
    let v = departed_view(creator, &rows, &rows);

    assert!(v.is_group_admin(&account(0x90), folder()));
    assert!(!v.is_group_admin(&account(0x40), folder()));
}

#[test]
fn a_folder_whose_creator_never_left_has_no_stand_in() {
    let creator = account(0x90);
    let v = view(creator, &[], &[(account(0x40), GroupMemberRole::Member)]);

    assert!(v.is_group_admin(&creator, folder()));
    assert!(!v.is_group_admin(&account(0x40), folder()));
}

#[test]
fn the_root_has_no_stand_in() {
    let mut v = AclView::default();
    let _ = v.groups.insert(
        root(),
        BTreeMap::from([(account(0x40), GroupMemberRole::Member)]),
    );

    assert!(!v.is_group_admin(&account(0x40), root()));
}

fn add_op(author: AccountId, member: AccountId, role: GroupMemberRole) -> calimero_op::Op {
    super::support::op_with(
        author,
        calimero_op::OpPayload::MemberAdded {
            group: folder(),
            member,
            role,
        },
    )
}

#[test]
fn a_folder_without_an_admin_refuses_an_add_until_its_successor_promotes_itself() {
    let creator = account(0x10);
    let successor = account(0x40);
    let newcomer = account(0x01);
    let bound =
        |v: AclView| super::support::bind_test_devices(super::support::bind_account(v, newcomer));
    let leaderless = bound(departed_view(
        creator,
        &[(successor, GroupMemberRole::Member)],
        &[(successor, GroupMemberRole::Member)],
    ));

    assert_eq!(
        crate::authorize(
            &add_op(successor, newcomer, GroupMemberRole::Member),
            &leaderless
        ),
        Err(crate::Rejected::GroupHasNoAdmin)
    );
    assert!(!leaderless.is_group_admin(&newcomer, folder()));
    assert!(leaderless.is_group_admin(&successor, folder()));

    assert!(crate::authorize(
        &add_op(successor, successor, GroupMemberRole::Admin),
        &leaderless
    )
    .is_ok());
    let promoted = bound(departed_view(
        creator,
        &[],
        &[(successor, GroupMemberRole::Admin)],
    ));
    assert!(crate::authorize(
        &add_op(successor, newcomer, GroupMemberRole::Member),
        &promoted
    )
    .is_ok());
}

#[test]
fn a_member_who_left_the_workspace_is_never_the_successor() {
    let creator = account(0x10);
    let stale = account(0x40);
    let current = account(0x90);
    let v = departed_view(
        creator,
        &[(current, GroupMemberRole::Member)],
        &[
            (stale, GroupMemberRole::Member),
            (current, GroupMemberRole::Member),
        ],
    );

    assert!(!v.is_group_admin(&stale, folder()));
    assert!(v.is_group_admin(&current, folder()));
}

#[test]
fn a_folder_with_only_read_only_members_is_run_by_the_lowest_of_them() {
    let creator = account(0x10);
    let rows = [
        (account(0x90), GroupMemberRole::ReadOnly),
        (account(0x40), GroupMemberRole::ReadOnly),
        (account(0x05), GroupMemberRole::ReadOnlyTee),
    ];
    let v = departed_view(creator, &rows, &rows);

    assert!(v.is_group_admin(&account(0x40), folder()));
    assert!(!v.is_group_admin(&account(0x90), folder()));
    assert!(!v.is_group_admin(&account(0x05), folder()));
}
