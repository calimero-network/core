//! Who is an admin of a group: an `Admin` row, the creator slot while the creator is
//! still in the workspace, and for a folder with neither its lowest member.

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

#[test]
fn a_creator_outside_the_workspace_holds_no_admin_slot() {
    let creator = account(0x10);
    let inside = view(creator, &[(creator, GroupMemberRole::Member)], &[]);
    let outside = view(creator, &[], &[]);

    assert!(inside.is_group_admin(&creator, folder()));
    assert!(!outside.is_group_admin(&creator, folder()));
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
    let v = view(creator, &[], &rows);

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
    let v = view(creator, &[], &rows);

    assert!(v.is_group_admin(&account(0x90), folder()));
    assert!(!v.is_group_admin(&account(0x40), folder()));
}

#[test]
fn a_folder_whose_creator_is_still_in_the_workspace_has_no_stand_in() {
    let creator = account(0x90);
    let v = view(
        creator,
        &[(creator, GroupMemberRole::Member)],
        &[(account(0x40), GroupMemberRole::Member)],
    );

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
