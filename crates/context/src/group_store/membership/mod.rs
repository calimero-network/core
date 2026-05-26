//! Group-membership concerns consolidated from five previously-separate
//! files (`membership.rs`, `membership_status.rs`, `membership_view.rs`,
//! `membership_policy.rs`, `membership_policy_rules.rs`).
//!
//! Submodules group by axis of concern, and the public surface below
//! mirrors what `group_store/mod.rs` previously re-exported so callers
//! continue to see the same symbol set at `calimero_context::group_store::*`.
//!
//! Issue #2306 / epic #2300.

mod core;
mod policy;
pub(crate) mod policy_rules;
mod status;
mod view;

#[cfg(test)]
mod tests;

pub use self::core::{
    add_group_member, add_group_member_with_keys, check_group_membership,
    check_group_membership_path, count_group_admins, count_group_members,
    enumerate_inherited_members, get_effective_member_capabilities, get_group_member_role,
    get_group_member_value, has_direct_group_member, is_authoritative_namespace_identity,
    is_direct_group_admin, is_group_admin, is_group_admin_or_has_capability, is_inherited_admin,
    list_group_members, namespace_member_pubkeys, remove_group_member, require_group_admin,
    require_group_admin_or_capability, set_member_auto_follow, subgroup_visible_to,
    trusted_anchors_for_group, MembershipPath,
};
pub use self::policy::MembershipPolicy;
pub(crate) use self::status::role_from_invited_role;
pub use self::status::{membership_status_at, MembershipStatus};
pub use self::view::GroupMembershipView;
