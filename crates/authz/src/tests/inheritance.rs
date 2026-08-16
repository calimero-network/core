//! The subgroup-inheritance walk: an open chain plus the capability, or an
//! admin anywhere on that chain — and nothing through a restricted wall.

use std::collections::BTreeMap;

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_op::ScopeId;

use super::support::inheritance_view;
use crate::inheritance::CAN_JOIN_OPEN_SUBGROUPS;
use crate::view::{AclView, SubgroupEdge};

#[test]
fn inherited_membership_requires_open_chain_and_cap() {
    let parent = ContextGroupId::from([1u8; 32]);
    let child = ContextGroupId::from([2u8; 32]);
    let member = AccountId::from([0x55; 32]);

    // Open child + parent member with CAN_JOIN_OPEN_SUBGROUPS → inherits.
    let v = inheritance_view(parent, child, member, CAN_JOIN_OPEN_SUBGROUPS, false, true);
    assert!(v.is_member_at_cut(child, &member, None, 0));

    // Open child but member lacks the cap → no inheritance.
    let v = inheritance_view(parent, child, member, 0, false, true);
    assert!(!v.is_member_at_cut(child, &member, None, 0));

    // Restricted child → wall, no inheritance even with the cap.
    let v = inheritance_view(parent, child, member, CAN_JOIN_OPEN_SUBGROUPS, true, true);
    assert!(!v.is_member_at_cut(child, &member, None, 0));

    // THE over-auth case: parent membership REVOKED at the cut (parent no
    // longer has the member) → not a member of the child either, even with
    // the cap still set. This is exactly what reading current live state got
    // wrong; the at-cut view has no parent membership, so inheritance fails.
    let v = inheritance_view(parent, child, member, CAN_JOIN_OPEN_SUBGROUPS, false, false);
    assert!(!v.is_member_at_cut(child, &member, None, 0));
}

#[test]
fn inherited_via_parent_admin_and_root_genesis_admin() {
    let parent = ContextGroupId::from([1u8; 32]);
    let child = ContextGroupId::from([2u8; 32]);
    let admin = AccountId::from([0xAA; 32]);

    // Parent's folded group admin, open child → inherits via admin (no cap
    // needed, no direct parent membership row).
    let mut v = inheritance_view(parent, child, admin, 0, false, false);
    v.group_admin.insert(parent, admin);
    assert!(v.is_member_at_cut(child, &admin, None, 0));

    // Namespace-root genesis admin (no op) supplied via `root`: an open child
    // directly under the root inherits for the root admin.
    let root = ContextGroupId::from([9u8; 32]);
    let mut subgroups = BTreeMap::new();
    subgroups.insert(
        ScopeId::from(child.to_bytes()),
        SubgroupEdge {
            parent: ScopeId::from(root.to_bytes()),
            restricted: false,
        },
    );
    let v = AclView {
        subgroups,
        ..Default::default()
    };
    assert!(v.is_member_at_cut(child, &admin, Some((root, admin)), 0));
    // A non-admin without any membership does not inherit.
    let other = AccountId::from([0x33; 32]);
    assert!(!v.is_member_at_cut(child, &other, Some((root, admin)), 0));
}
