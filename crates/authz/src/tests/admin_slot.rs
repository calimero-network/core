//! Who is an admin of a group: an `Admin` row, the creator slot while the creator is
//! still in the workspace.

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
