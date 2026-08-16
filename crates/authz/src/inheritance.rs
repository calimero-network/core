//! The subgroup-tree walk, and the three questions that share it.
//!
//! `is_member_at_cut`, `is_authorized_admin` and `member_path_at_cut` all climb
//! the same `subgroups` parent chain and stop at the same restricted wall; they
//! differ only in what counts as success and what they report. Keeping them in
//! one file is what makes the differences (documented on each) legible as
//! deliberate rather than accidental drift — they answer three different
//! questions and their walk order is *not* interchangeable.

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_op::ScopeId;
use calimero_primitives::context::GroupMemberRole;

use crate::view::AclView;

/// `CAN_JOIN_OPEN_SUBGROUPS` capability bit — gates inherited membership into an
/// open subgroup (mirrors the live `MemberCapabilities` constant).
pub(crate) const CAN_JOIN_OPEN_SUBGROUPS: u32 = MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits();
/// Max subgroup-tree depth the inheritance walk traverses. Sourced from the
/// single definition in `calimero-context-config` (shared with governance-store
/// and the context client so the walk bound cannot drift between crates).
const MAX_NAMESPACE_DEPTH: usize = calimero_context_config::MAX_NAMESPACE_DEPTH;

/// How `author` reaches membership of a group at a cut — the at-cut analogue of
/// the live `MembershipPath`. Carries enough to derive the enumeration ROLE
/// (mirrors live's `list ∪ enumerate_inherited`): a direct member keeps its
/// folded role; an inherited member is `Admin` when reached via an admin, else
/// its role at the `anchor` it inherits from.
#[derive(Clone, Debug, PartialEq)]
pub enum MemberPathAtCut {
    /// Not a member of the group at this cut.
    None,
    /// A direct member, with its folded role.
    Direct { role: GroupMemberRole },
    /// Inherited over the open-subgroup chain from `anchor`; `via_admin` when the
    /// path was through an admin ancestor (role resolves to `Admin`).
    Inherited {
        anchor: ContextGroupId,
        via_admin: bool,
    },
}

impl AclView {
    /// Is `author` a member of `group` **at this cut** — direct, group admin, or
    /// inherited through an open-subgroup chain — resolved entirely from the
    /// folded view (no live-store reads). Faithful port of the live
    /// `MembershipRepository::check_path` + the `acl_view_at` admin carve-out,
    /// but over the at-cut state, so a membership the cut revoked is not granted.
    ///
    /// `root` is the immutable `(namespace_root_group, genesis_admin)` — the one
    /// admin fact with no governance op (it lives in `GroupMeta` at namespace
    /// genesis); pass `None` if unknown. Every *mutable* input (memberships,
    /// caps, visibility, subgroup tree, subgroup-creator admin) comes from the
    /// view, so the result honors the cut.
    #[must_use]
    pub fn is_member_at_cut(
        &self,
        group: ContextGroupId,
        author: &AccountId,
        root: Option<(ContextGroupId, AccountId)>,
        default_cap_base: u32,
    ) -> bool {
        // Admin of `g` at the cut: a folded group admin (subgroup creator / an
        // `Admin`-role holder) OR the immutable namespace-root genesis admin.
        let is_admin = |g: ContextGroupId| -> bool {
            self.is_group_admin(author, g)
                || root.is_some_and(|(root_g, root_admin)| g == root_g && *author == root_admin)
        };

        // Direct member or admin of the target group. NOTE: an open-subgroup
        // inheritance self-join (`MemberJoinedOpen`) is deliberately NOT folded as
        // a direct membership (it carries no persistent direct row in the live
        // model — its apply requires `check_path == Inherited` and creates no
        // row); such membership is re-derived by the inheritance walk below, so it
        // is correctly revoked when the anchor is removed.
        if is_admin(group)
            || self
                .groups
                .get(&group)
                .is_some_and(|m| m.contains_key(author))
        {
            return true;
        }

        // Inherited: walk parents while the chain stays Open, mirroring
        // `check_path`. The first direct-membership ancestor decides via its
        // `CAN_JOIN_OPEN_SUBGROUPS` cap (recorded, not returned); an admin
        // ancestor reached over the open chain grants immediately.
        //
        // `effective_cap`: the projection folds explicit `DefaultCapabilitiesSet`
        // / `MemberCapabilitySet` ops, but a group's CREATION default cap is a
        // store write, not an op — so when nothing is folded for the anchor we
        // fall back to `default_cap_base` (the materialized default, immutable
        // base state, passed by the caller). This mirrors live's
        // `member_capability` = override.or(default).
        let effective_cap = |g: &ContextGroupId| -> u32 {
            let folded = self.capability(g, author);
            if folded != 0 {
                folded
            } else {
                default_cap_base
            }
        };

        let mut anchor_is_member: Option<bool> = None;
        let mut current = group;
        for _ in 0..=MAX_NAMESPACE_DEPTH {
            // `current` must be Open for inheritance to pass up through it.
            let Some(edge) = self.subgroups.get(&ScopeId::from(current.to_bytes())) else {
                return anchor_is_member.unwrap_or(false);
            };
            if edge.restricted {
                return anchor_is_member.unwrap_or(false);
            }
            let parent = ContextGroupId::from(*edge.parent.as_bytes());
            if is_admin(parent) {
                return true;
            }
            if anchor_is_member.is_none()
                && self
                    .groups
                    .get(&parent)
                    .is_some_and(|m| m.contains_key(author))
            {
                anchor_is_member = Some(effective_cap(&parent) & CAN_JOIN_OPEN_SUBGROUPS != 0);
            }
            current = parent;
        }
        anchor_is_member.unwrap_or(false)
    }

    /// Is `author` authorized as an ADMIN of `group` at the cut — the apply-gate
    /// admin authority. Mirrors live's `is_authorized_with_capability` admin path:
    /// a direct group admin (subgroup creator / `Admin`-role holder), the
    /// namespace ROOT admin (who administers every group — folded, so it tracks
    /// `AdminChanged`, with the genesis `root` carve-out as the un-folded base), OR
    /// an admin of an ANCESTOR reached over the open-subgroup chain (an admin of an
    /// Open parent administers its children). Restricted edges stop the walk.
    ///
    /// Admin-only — unlike [`is_member_at_cut`](Self::is_member_at_cut), a plain
    /// inherited MEMBER is not authorized. Capability holders are checked
    /// separately by the caller.
    #[must_use]
    pub fn is_authorized_admin(
        &self,
        group: ContextGroupId,
        author: &AccountId,
        root: Option<(ContextGroupId, AccountId)>,
    ) -> bool {
        let is_admin = |g: ContextGroupId| -> bool {
            self.is_group_admin(author, g)
                || self.is_root_admin(author)
                || root.is_some_and(|(root_g, root_admin)| g == root_g && *author == root_admin)
        };
        if is_admin(group) {
            return true;
        }
        let mut current = group;
        for _ in 0..=MAX_NAMESPACE_DEPTH {
            let Some(edge) = self.subgroups.get(&ScopeId::from(current.to_bytes())) else {
                return false;
            };
            if edge.restricted {
                return false;
            }
            let parent = ContextGroupId::from(*edge.parent.as_bytes());
            if is_admin(parent) {
                return true;
            }
            current = parent;
        }
        false
    }

    /// `author`'s membership PATH to `group` at the cut — the at-cut analogue of
    /// live `check_path`, returning the role-bearing [`MemberPathAtCut`] for the
    /// enumeration consumers. Mirrors the `is_member_at_cut` walk: direct row first
    /// (with its folded role), then an admin of the group with no row (genesis root
    /// admin), then up the open chain — an admin ancestor yields
    /// `Inherited{via_admin:true}`, the first direct-member ancestor yields
    /// `Inherited{via_admin:false}` iff it holds `CAN_JOIN_OPEN_SUBGROUPS` (else the
    /// recorded decision is `None`, but the walk continues for an admin higher up).
    #[must_use]
    pub fn member_path_at_cut(
        &self,
        group: ContextGroupId,
        author: &AccountId,
        root: Option<(ContextGroupId, AccountId)>,
        default_cap_base: u32,
    ) -> MemberPathAtCut {
        // Narrow admin, IDENTICAL to `is_member_at_cut`'s: a folded group admin or
        // the genesis root admin OF THIS GROUP ONLY. Deliberately NOT the global
        // `is_root_admin` — the root admin is a member of a *subgroup* only over
        // the open-subgroup chain (the walk below), never of a Restricted subgroup.
        // Using the global predicate here would mark the root admin a direct member
        // of every group, diverging from the membership set this path must mirror.
        let is_admin = |g: ContextGroupId| -> bool {
            self.is_group_admin(author, g)
                || root.is_some_and(|(root_g, root_admin)| g == root_g && *author == root_admin)
        };
        // Direct row FIRST (the reverse of `is_member_at_cut`, which only needs a
        // bool so its order is immaterial): when an identity is BOTH a stored
        // member and the genesis admin, live's `list` returns the stored row's
        // role, so the row is authoritative. The `is_admin` carve-out below only
        // supplies a role (`Admin`) when there is NO row to read.
        if let Some(role) = self.groups.get(&group).and_then(|m| m.get(author)) {
            return MemberPathAtCut::Direct { role: role.clone() };
        }
        if is_admin(group) {
            return MemberPathAtCut::Direct {
                role: GroupMemberRole::Admin,
            };
        }
        let effective_cap = |g: &ContextGroupId| -> u32 {
            let folded = self.capability(g, author);
            if folded != 0 {
                folded
            } else {
                default_cap_base
            }
        };
        let mut anchor_decision: Option<MemberPathAtCut> = None;
        let mut current = group;
        for _ in 0..=MAX_NAMESPACE_DEPTH {
            let Some(edge) = self.subgroups.get(&ScopeId::from(current.to_bytes())) else {
                return anchor_decision.unwrap_or(MemberPathAtCut::None);
            };
            if edge.restricted {
                return anchor_decision.unwrap_or(MemberPathAtCut::None);
            }
            let parent = ContextGroupId::from(*edge.parent.as_bytes());
            if is_admin(parent) {
                return MemberPathAtCut::Inherited {
                    anchor: parent,
                    via_admin: true,
                };
            }
            if anchor_decision.is_none()
                && self
                    .groups
                    .get(&parent)
                    .is_some_and(|m| m.contains_key(author))
            {
                anchor_decision = Some(if effective_cap(&parent) & CAN_JOIN_OPEN_SUBGROUPS != 0 {
                    MemberPathAtCut::Inherited {
                        anchor: parent,
                        via_admin: false,
                    }
                } else {
                    MemberPathAtCut::None
                });
            }
            current = parent;
        }
        anchor_decision.unwrap_or(MemberPathAtCut::None)
    }
}
