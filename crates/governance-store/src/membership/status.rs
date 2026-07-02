//! Invitation-role mapping for the namespace governance apply path.
//!
//! This module was the home of the position-aware **live** membership resolver
//! (`acl_view_at` → `MembershipStatus`, plus the `prefix_walk_membership` BFS
//! and its head-set comparison). That resolver is retired in F5 (core#2716):
//! every delta-auth and sync consumer now resolves membership from the unified
//! projection at the op's governance cut, so the live cut-resolver has no
//! production callers left. What remains is the single shared helper the
//! `MemberJoined` apply path still needs.

use calimero_primitives::context::GroupMemberRole;

/// Map the `invited_role: u8` byte from `GroupInvitationFromAdmin` to the typed
/// [`GroupMemberRole`], used by the `MemberJoined` apply path in
/// `namespace/membership.rs`.
///
/// Thin alias for the canonical [`GroupMemberRole::from_invited_role`] so this
/// apply path and the op-adapter share one decoding and cannot drift. The
/// canonical mapping documents the encoding (0 = Admin, 1 = Member, 2 = ReadOnly)
/// and the least-privilege default for unknown bytes.
pub(crate) fn role_from_invited_role(value: u8) -> GroupMemberRole {
    GroupMemberRole::from_invited_role(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_from_invited_role_maps_invitation_codes_correctly() {
        assert!(matches!(role_from_invited_role(0), GroupMemberRole::Admin));
        assert!(matches!(role_from_invited_role(1), GroupMemberRole::Member));
        assert!(matches!(
            role_from_invited_role(2),
            GroupMemberRole::ReadOnly
        ));
        // Unknown variants must NOT default to Admin — preserve a less
        // privileged classification.
        assert!(matches!(
            role_from_invited_role(99),
            GroupMemberRole::Member
        ));
    }
}
