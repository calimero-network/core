//! The subgroup-tree walk, and the three questions that share it.
//!
//! # Why it is shaped this way
//!
//! **One climb, three success conditions.** `is_member_at_cut`,
//! `is_authorized_admin` and `member_path_at_cut` all follow the same
//! `subgroups` parent chain and stop at the same restricted wall. That shape is
//! now [`AclView::open_ancestors`], written once, so the three questions differ
//! only in what they treat as success — which is the part that is genuinely
//! load-bearing. Three hand-written copies of the climb meant three places a
//! bound, a wall check, or a parent lookup could drift, and a drift there is not
//! a bug in one question: it is one question granting what another refuses,
//! about the same author and the same group.
//!
//! **`is_member_at_cut` is `member_path_at_cut` with the role discarded.** The
//! two walks differed in one respect — the path checks the direct row before the
//! admin carve-out so a stored role wins over the genesis admin's implied one,
//! matching live's `list` semantics. That ordering decides which *role* is
//! reported and cannot change *whether* the author is a member, so the predicate
//! is now defined in terms of the path. `is_member_defined_by_the_path_walk`
//! pins the equivalence exhaustively over every small tree rather than leaving it
//! asserted.
//!
//! This matters beyond tidiness: `calimero-context` filters its candidate set
//! with `is_member_at_cut` and then resolves each survivor's role with
//! `member_path_at_cut`. If the two ever disagreed, the projection would emit a
//! member with no role or a role for a non-member. That agreement used to rest on
//! two implementations being kept in step by comment; it is now the same code.
//!
//! **A restricted edge is a hard wall.** Hitting one stops the climb
//! immediately, even when an admin sits further up past it — visibility is not
//! something an ancestor's authority reaches through.
//!
//! **`root` is the one un-folded fact.** The namespace's genesis admin has no
//! governance op (it is set at backfill), so it arrives as an explicit parameter
//! rather than from the view. Every *mutable* input — memberships, caps,
//! visibility, the subgroup tree, the subgroup-creator admin — comes from the
//! view, which is what makes the result honor the cut.

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
    /// The ancestors of `group` reachable while every edge crossed is **Open**,
    /// nearest first — the climb all three inheritance questions share.
    ///
    /// Ends at the first group with no subgroup edge (the namespace root) or at
    /// the first `restricted` edge, and is bounded by `MAX_NAMESPACE_DEPTH` so a
    /// cyclic or adversarial tree cannot spin. Yields parents only; `group`
    /// itself is the caller's to check, because each question treats it
    /// differently.
    fn open_ancestors(&self, group: ContextGroupId) -> impl Iterator<Item = ContextGroupId> + '_ {
        let mut current = Some(group);
        core::iter::from_fn(move || {
            let edge = self.subgroups.get(&ScopeId::from(current?.to_bytes()))?;
            if edge.restricted {
                current = None;
                return None;
            }
            let parent = ContextGroupId::from(*edge.parent.as_bytes());
            current = Some(parent);
            Some(parent)
        })
        // One more than the depth limit, matching the bound the three
        // hand-written climbs used.
        .take(MAX_NAMESPACE_DEPTH + 1)
    }

    /// `group` and every ancestor above it, nearest first — the set of groups
    /// whose ops can move an authority question asked about `group`.
    ///
    /// Deliberately climbs THROUGH a `restricted` edge, unlike
    /// [`Self::open_ancestors`]. That walk stops there because inheritance stops
    /// there; this one cannot, because whether an edge is restricted is itself
    /// decided by a `SubgroupVisibilitySet` op of that subgroup — which may be
    /// exactly the op a reader cannot decrypt. Stopping at an edge whose state is
    /// unknown would silently shorten the set and let an unreadable ancestor pass
    /// as irrelevant.
    ///
    /// Same `MAX_NAMESPACE_DEPTH` bound as the inheritance climb, so a cyclic or
    /// adversarial tree cannot spin.
    pub fn group_and_ancestors(
        &self,
        group: ContextGroupId,
    ) -> impl Iterator<Item = ContextGroupId> + '_ {
        let mut current = Some(group);
        core::iter::once(group).chain(
            core::iter::from_fn(move || {
                let edge = self.subgroups.get(&ScopeId::from(current?.to_bytes()))?;
                let parent = ContextGroupId::from(*edge.parent.as_bytes());
                current = Some(parent);
                Some(parent)
            })
            .take(MAX_NAMESPACE_DEPTH + 1),
        )
    }

    /// Does `author` hold a direct membership row in `group`?
    fn has_direct_row(&self, group: ContextGroupId, author: &AccountId) -> bool {
        self.groups
            .get(&group)
            .is_some_and(|members| members.contains_key(author))
    }

    /// Admin of `g` for **membership** purposes: a folded group admin (subgroup
    /// creator / `Admin`-role holder) or the immutable namespace-root genesis
    /// admin.
    ///
    /// Deliberately **not** the global [`AclView::is_root_admin`]: the root admin
    /// is a member of a *subgroup* only over the open chain, never of a Restricted
    /// one. Using the global predicate here would make the root admin a direct
    /// member of every group, diverging from the membership set these walks must
    /// mirror. [`AclView::is_authorized_admin`] does include it, because admin
    /// *authority* is global in a way membership is not.
    fn is_membership_admin(
        &self,
        group: ContextGroupId,
        author: &AccountId,
        root: Option<(ContextGroupId, AccountId)>,
    ) -> bool {
        self.is_group_admin(author, group)
            || root.is_some_and(|(root_g, root_admin)| group == root_g && *author == root_admin)
    }

    /// `author`'s effective capability at `group`, falling back to
    /// `default_cap_base`.
    ///
    /// The projection folds explicit `DefaultCapabilitiesSet` / `MemberCapabilitySet`
    /// ops, but a group's CREATION default cap is a store write rather than an op —
    /// so when nothing is folded we fall back to the materialized default the
    /// caller passes. Mirrors live's `member_capability` = override.or(default).
    fn effective_cap(
        &self,
        group: ContextGroupId,
        author: &AccountId,
        default_cap_base: u32,
    ) -> u32 {
        let folded = self.capability(&group, author);
        if folded == 0 {
            default_cap_base
        } else {
            folded
        }
    }

    /// Is `author` a member of `group` **at this cut** — direct, group admin, or
    /// inherited through an open-subgroup chain?
    ///
    /// Exactly [`AclView::member_path_at_cut`] with the role discarded; see the
    /// module docs for why that is safe despite the two walks having once
    /// differed in order.
    #[must_use]
    pub fn is_member_at_cut(
        &self,
        group: ContextGroupId,
        author: &AccountId,
        root: Option<(ContextGroupId, AccountId)>,
        default_cap_base: u32,
    ) -> bool {
        !matches!(
            self.member_path_at_cut(group, author, root, default_cap_base),
            MemberPathAtCut::None
        )
    }

    /// Is `author` authorized as an ADMIN of `group` at the cut — the apply-gate
    /// admin authority?
    ///
    /// A direct group admin (subgroup creator / `Admin`-role holder), the
    /// namespace ROOT admin (who administers every group — folded, so it tracks
    /// `AdminChanged`, with the genesis `root` carve-out as the un-folded base),
    /// or an admin of an ANCESTOR reached over the open chain (an admin of an Open
    /// parent administers its children).
    ///
    /// Admin-only — unlike [`AclView::is_member_at_cut`], a plain inherited MEMBER
    /// is not authorized, and no capability bit grants admin authority. Capability
    /// holders are checked separately by the caller.
    #[must_use]
    pub fn is_authorized_admin(
        &self,
        group: ContextGroupId,
        author: &AccountId,
        root: Option<(ContextGroupId, AccountId)>,
    ) -> bool {
        // The one place the GLOBAL root admin counts: admin authority reaches
        // every group, where membership does not.
        let is_admin = |g: ContextGroupId| {
            self.is_membership_admin(g, author, root) || self.is_root_admin(author)
        };
        is_admin(group) || self.open_ancestors(group).any(is_admin)
    }

    /// `author`'s membership PATH to `group` at the cut — the role-bearing
    /// verdict the enumeration consumers need.
    ///
    /// The direct row is checked **first**, before the admin carve-out: when an
    /// identity is both a stored member and the genesis admin, live's `list`
    /// returns the stored row's role, so the row is authoritative. The carve-out
    /// only supplies a role (`Admin`) when there is no row to read.
    ///
    /// Then up the open chain: an admin ancestor yields
    /// `Inherited { via_admin: true }` immediately; the **first** direct-member
    /// ancestor yields `Inherited { via_admin: false }` iff it holds
    /// `CAN_JOIN_OPEN_SUBGROUPS`. If it does not, that records a `None` verdict but
    /// does **not** stop the climb — an admin further up still grants.
    #[must_use]
    pub fn member_path_at_cut(
        &self,
        group: ContextGroupId,
        author: &AccountId,
        root: Option<(ContextGroupId, AccountId)>,
        default_cap_base: u32,
    ) -> MemberPathAtCut {
        if let Some(role) = self.groups.get(&group).and_then(|m| m.get(author)) {
            return MemberPathAtCut::Direct { role: role.clone() };
        }
        if self.is_membership_admin(group, author, root) {
            return MemberPathAtCut::Direct {
                role: GroupMemberRole::Admin,
            };
        }

        let mut anchor_decision: Option<MemberPathAtCut> = None;
        for parent in self.open_ancestors(group) {
            if self.is_membership_admin(parent, author, root) {
                return MemberPathAtCut::Inherited {
                    anchor: parent,
                    via_admin: true,
                };
            }
            if anchor_decision.is_none() && self.has_direct_row(parent, author) {
                anchor_decision = Some(
                    if self.effective_cap(parent, author, default_cap_base)
                        & CAN_JOIN_OPEN_SUBGROUPS
                        != 0
                    {
                        MemberPathAtCut::Inherited {
                            anchor: parent,
                            via_admin: false,
                        }
                    } else {
                        MemberPathAtCut::None
                    },
                );
            }
        }
        anchor_decision.unwrap_or(MemberPathAtCut::None)
    }
}
