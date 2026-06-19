//! The **one** authorization fold for the unified causal log.
//!
//! [`authorize`] is the single security boundary: one match over [`OpPayload`]
//! arms against an [`AclView`] resolved at the op's causal cut. It unifies what
//! were three separate causal-auth checks — writer-set resolution, group
//! membership resolution, and the per-delta governance-position gate.
//!
//! **Causal-honor semantics:** an op is authorized against the ACL/membership
//! *as of its own causal parents*, never the receiver's current state. So a
//! write authored before a revocation, in causal order, stays valid regardless
//! of the order a receiver observes the revocation (the forward-only property).
//! The caller produces the [`AclView`] via `ScopeState::acl_view_at(op.parents)`
//! (see `calimero-projection`); this crate is the pure decision over that view.

use std::collections::BTreeMap;

use thiserror::Error as ThisError;

use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_op::{Op, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;

/// `CAN_JOIN_OPEN_SUBGROUPS` capability bit — gates inherited membership into an
/// open subgroup (mirrors the live `MemberCapabilities` constant).
const CAN_JOIN_OPEN_SUBGROUPS: u32 = MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS;
/// Max subgroup-tree depth the inheritance walk traverses (mirrors the live
/// `MAX_NAMESPACE_DEPTH`).
const MAX_NAMESPACE_DEPTH: usize = 16;

/// Why an op was refused. One rejection type for every plane — the caller
/// doesn't have to know which plane said no.
#[derive(Clone, Debug, PartialEq, Eq, ThisError)]
pub enum Rejected {
    /// Author lacks the required capability on a data entity.
    #[error("author not permitted to write entity (needs {required:?})")]
    NotPermitted { required: OpMask },
    /// Author is not the owner of the object whose writers are being set.
    #[error("author is not the owner of the object")]
    NotOwner,
    /// Author is not an admin of the group being mutated.
    #[error("author is not an admin of the group at the cut")]
    NotGroupAdmin,
    /// Author is not the scope's root admin.
    #[error("author is not the scope root admin at the cut")]
    NotRootAdmin,
}

/// The authorization-relevant slice of a [`ScopeState`](calimero_projection)
/// **at a causal cut** — the value [`authorize`] decides against. Produced by
/// `ScopeState::acl_view_at(parents)`; this crate never walks the DAG itself
/// (that's the projection's job), keeping the decision pure and unit-testable.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AclView {
    /// Writer/capability set per object (the writer plane).
    pub acl: BTreeMap<Id, BTreeMap<PublicKey, OpMask>>,
    /// Group memberships at the cut (the membership plane).
    pub groups: BTreeMap<ContextGroupId, BTreeMap<PublicKey, GroupMemberRole>>,
    /// The scope's root admin at the cut (the admin plane).
    pub root_admin: Option<PublicKey>,
    /// Per-group default capability bitmask at the cut (capability plane).
    pub default_caps: BTreeMap<ContextGroupId, u32>,
    /// Per-(group, member) explicit capability override at the cut. Takes
    /// precedence over the group default for that member.
    pub member_caps: BTreeMap<(ContextGroupId, PublicKey), u32>,
    /// Live subgroup tree at the cut: child scope → (parent scope, restricted).
    /// Only scopes whose latest `exists` is true appear. Drives the inherited-
    /// membership parent walk (open chain to an ancestor the author belongs to).
    pub subgroups: BTreeMap<ScopeId, SubgroupEdge>,
    /// Per-group genesis admin at the cut (the subgroup creator, or the
    /// namespace-root admin seeded at backfill). Mirrors the live
    /// `GroupMeta.admin_identity`. An identity is a group admin iff it is this
    /// or holds the `Admin` role in `groups[group]`.
    pub group_admin: BTreeMap<ContextGroupId, PublicKey>,
}

/// A live subgroup's tree position + visibility at the cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubgroupEdge {
    /// Parent scope this subgroup is nested under.
    pub parent: ScopeId,
    /// `true` = Restricted (a visibility wall that blocks inheritance through
    /// it); `false` = Open.
    pub restricted: bool,
}

/// Capabilities a scope **member** implicitly holds on a non-restricted
/// entity (`default-write = membership`): `WRITE` + `DELETE`, but **not**
/// `ADMIN` — rotating an object's writer set still requires an explicit ACL
/// grant (ownership), so a plain member can't lock others out of a default
/// entity.
///
/// Implication, by design: any member can write **and delete** any
/// non-restricted entity in the scope (a single compromised member can wipe
/// default data) — this matches a shared key-value store, where membership is
/// the write boundary. Data that needs a narrower writer/deleter set must be a
/// restricted object with an explicit ACL.
const DEFAULT_MEMBER_MASK: OpMask = OpMask::WRITE.union(OpMask::DELETE);

impl AclView {
    /// Does `author` hold at least `required` on `entity`?
    ///
    /// Two-tier (`default-write = membership`):
    /// 1. **Restricted entity** — an explicit per-object ACL entry exists:
    ///    `author` must be listed with a mask covering `required`. A member who
    ///    isn't a listed writer is denied.
    /// 2. **Non-restricted entity** — no explicit ACL: any scope member holds
    ///    [`DEFAULT_MEMBER_MASK`] (`WRITE`+`DELETE`). This gives "members can
    ///    write" for ordinary contexts (e.g. a key-value store) without
    ///    enumerating a per-entity writer set for every key.
    #[must_use]
    pub fn may(&self, author: &PublicKey, entity: Id, required: OpMask) -> bool {
        if let Some(writers) = self.acl.get(&entity) {
            // Restricted object: explicit ACL is authoritative.
            return writers
                .get(author)
                .is_some_and(|held| held.contains(required));
        }
        // Non-restricted: default-write = membership.
        self.is_scope_member(author) && DEFAULT_MEMBER_MASK.contains(required)
    }

    /// Is `author` a member of this view's scope (a member of any group in the
    /// view)? An `AclView` resolved for one scope carries that scope's
    /// membership; this is the predicate behind `default-write` for
    /// non-restricted entities.
    #[must_use]
    pub fn is_scope_member(&self, author: &PublicKey) -> bool {
        self.groups
            .values()
            .any(|members| members.contains_key(author))
    }

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
        author: &PublicKey,
        root: Option<(ContextGroupId, PublicKey)>,
    ) -> bool {
        // Admin of `g` at the cut: a folded group admin (subgroup creator / an
        // `Admin`-role holder) OR the immutable namespace-root genesis admin.
        let is_admin = |g: ContextGroupId| -> bool {
            self.is_group_admin(author, g)
                || root.is_some_and(|(root_g, root_admin)| g == root_g && *author == root_admin)
        };

        // Direct member or admin of the target group.
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
                let caps = self.capability(&parent, author);
                anchor_is_member = Some(caps & CAN_JOIN_OPEN_SUBGROUPS != 0);
            }
            current = parent;
        }
        anchor_is_member.unwrap_or(false)
    }

    /// `member`'s effective capability bitmask in `group` at the cut: the
    /// explicit per-member override if present, else the group default, else
    /// `0`. Mirrors the live `member_capability` read used by inherited-
    /// membership resolution (the `CAN_JOIN_OPEN_SUBGROUPS` gate).
    #[must_use]
    pub fn capability(&self, group: &ContextGroupId, member: &PublicKey) -> u32 {
        self.member_caps
            .get(&(*group, *member))
            .copied()
            .or_else(|| self.default_caps.get(group).copied())
            .unwrap_or(0)
    }

    /// Is `author` the owner of `object` — permitted to rotate its writer set?
    ///
    /// The `ADMIN` bit on the object confers ownership (owner = capability
    /// holder). Refine here if `owner` ever becomes distinct from
    /// `writer`/`admin`.
    #[must_use]
    pub fn is_owner(&self, author: &PublicKey, object: Id) -> bool {
        self.may(author, object, OpMask::ADMIN)
    }

    /// Is `author` an `Admin` of `group` at the cut?
    #[must_use]
    pub fn is_group_admin(&self, author: &PublicKey, group: ContextGroupId) -> bool {
        if self.group_admin.get(&group) == Some(author) {
            return true;
        }
        matches!(
            self.groups.get(&group).and_then(|m| m.get(author)),
            Some(GroupMemberRole::Admin)
        )
    }

    /// Is `author` the scope's root admin at the cut?
    #[must_use]
    pub fn is_root_admin(&self, author: &PublicKey) -> bool {
        self.root_admin.as_ref() == Some(author)
    }
}

/// The capability a **data** op requires of its author, or `None` for a
/// non-data op (whose authority is decided by ownership/admin, not a mask).
///
/// Returning `None` rather than `OpMask::NONE` is deliberate: the empty mask is
/// contained by *every* mask, so a `NONE` requirement fed to [`AclView::may`]
/// would authorize anyone — a footgun if a non-data payload ever reached a
/// `may` check. `None` makes that misuse impossible to express.
#[must_use]
pub fn required_mask_for(payload: &OpPayload) -> Option<OpMask> {
    match payload {
        OpPayload::Put { .. } => Some(OpMask::WRITE),
        OpPayload::Delete { .. } => Some(OpMask::DELETE),
        _ => None,
    }
}

/// `Ok` iff `author` holds `required` on `entity` (the data-plane check).
fn check_data(
    acl_at_cut: &AclView,
    author: &PublicKey,
    entity: Id,
    required: OpMask,
) -> Result<(), Rejected> {
    if acl_at_cut.may(author, entity, required) {
        Ok(())
    } else {
        Err(Rejected::NotPermitted { required })
    }
}

/// Authorize `op` against `acl_at_cut` — the [`AclView`] resolved at
/// `op.parents`. The **only** causal-auth decision in the unified model.
///
/// # Errors
/// Returns the plane-specific [`Rejected`] reason when the author lacks the
/// authority the op's payload requires.
pub fn authorize(op: &Op, acl_at_cut: &AclView) -> Result<(), Rejected> {
    match &op.payload {
        // Split per data op so each carries its literal required mask — no
        // `Option` to unwrap, so there is no unreachable fallback that could
        // silently deny (or panic) if the arms ever drift. `required_mask_for`
        // remains the public helper for external callers.
        OpPayload::Put { entity, .. } => check_data(acl_at_cut, &op.author, *entity, OpMask::WRITE),
        OpPayload::Delete { entity } => check_data(acl_at_cut, &op.author, *entity, OpMask::DELETE),
        OpPayload::SetWriters { object, .. } => {
            if acl_at_cut.is_owner(&op.author, *object) {
                Ok(())
            } else {
                Err(Rejected::NotOwner)
            }
        }
        OpPayload::MemberAdded { group, .. } | OpPayload::MemberRemoved { group, .. } => {
            if acl_at_cut.is_group_admin(&op.author, *group) {
                Ok(())
            } else {
                Err(Rejected::NotGroupAdmin)
            }
        }
        OpPayload::SubgroupVisibilitySet { scope, .. } => {
            // Visibility is a property of the subgroup; its admin sets it.
            if acl_at_cut.is_group_admin(&op.author, ContextGroupId::from(*scope.as_bytes())) {
                Ok(())
            } else {
                Err(Rejected::NotGroupAdmin)
            }
        }
        OpPayload::AdminChanged { .. }
        | OpPayload::PolicyUpdated { .. }
        | OpPayload::SubgroupCreated { .. }
        | OpPayload::SubgroupReparented { .. }
        | OpPayload::SubgroupDeleted { .. } => {
            if acl_at_cut.is_root_admin(&op.author) {
                Ok(())
            } else {
                Err(Rejected::NotRootAdmin)
            }
        }
        // Capability changes are an admin action on the target group.
        OpPayload::DefaultCapabilitiesSet { group, .. }
        | OpPayload::MemberCapabilitySet { group, .. } => {
            if acl_at_cut.is_group_admin(&op.author, *group) {
                Ok(())
            } else {
                Err(Rejected::NotGroupAdmin)
            }
        }
        // A graph-only node mutates nothing, so there is nothing to authorize.
        OpPayload::Noop => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_op::ScopeId;
    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
    use core::num::NonZeroU128;

    fn hlc0() -> HybridTimestamp {
        HybridTimestamp::new(Timestamp::new(
            NTP64(0),
            ID::from(NonZeroU128::new(1).unwrap()),
        ))
    }

    fn op_with(author: PublicKey, payload: OpPayload) -> Op {
        Op {
            id: [0u8; 32],
            scope: ScopeId::from([0u8; 32]),
            parents: vec![],
            author,
            hlc: hlc0(),
            payload,
            expected_scope_root: [0u8; 32],
            signature: [0u8; 64],
        }
    }

    fn view_with_writer(entity: Id, who: PublicKey, mask: OpMask) -> AclView {
        let mut acl = BTreeMap::new();
        acl.insert(entity, [(who, mask)].into_iter().collect());
        AclView {
            acl,
            ..Default::default()
        }
    }

    // Build a view: parent group with `member` (holding `caps`), an open
    // subgroup `child` nested under `parent`. Mirrors the inheritance scenario.
    fn inheritance_view(
        parent: ContextGroupId,
        child: ContextGroupId,
        member: PublicKey,
        caps: u32,
        child_restricted: bool,
        parent_has_member: bool,
    ) -> AclView {
        let mut groups: BTreeMap<ContextGroupId, BTreeMap<PublicKey, GroupMemberRole>> =
            BTreeMap::new();
        if parent_has_member {
            groups.insert(
                parent,
                [(member, GroupMemberRole::Member)].into_iter().collect(),
            );
        }
        let mut member_caps = BTreeMap::new();
        member_caps.insert((parent, member), caps);
        let mut subgroups = BTreeMap::new();
        subgroups.insert(
            ScopeId::from(child.to_bytes()),
            SubgroupEdge {
                parent: ScopeId::from(parent.to_bytes()),
                restricted: child_restricted,
            },
        );
        AclView {
            groups,
            member_caps,
            subgroups,
            ..Default::default()
        }
    }

    #[test]
    fn inherited_membership_requires_open_chain_and_cap() {
        let parent = ContextGroupId::from([1u8; 32]);
        let child = ContextGroupId::from([2u8; 32]);
        let member = PublicKey::from([0x55; 32]);

        // Open child + parent member with CAN_JOIN_OPEN_SUBGROUPS → inherits.
        let v = inheritance_view(parent, child, member, CAN_JOIN_OPEN_SUBGROUPS, false, true);
        assert!(v.is_member_at_cut(child, &member, None));

        // Open child but member lacks the cap → no inheritance.
        let v = inheritance_view(parent, child, member, 0, false, true);
        assert!(!v.is_member_at_cut(child, &member, None));

        // Restricted child → wall, no inheritance even with the cap.
        let v = inheritance_view(parent, child, member, CAN_JOIN_OPEN_SUBGROUPS, true, true);
        assert!(!v.is_member_at_cut(child, &member, None));

        // THE over-auth case: parent membership REVOKED at the cut (parent no
        // longer has the member) → not a member of the child either, even with
        // the cap still set. This is exactly what reading current live state got
        // wrong; the at-cut view has no parent membership, so inheritance fails.
        let v = inheritance_view(parent, child, member, CAN_JOIN_OPEN_SUBGROUPS, false, false);
        assert!(!v.is_member_at_cut(child, &member, None));
    }

    #[test]
    fn inherited_via_parent_admin_and_root_genesis_admin() {
        let parent = ContextGroupId::from([1u8; 32]);
        let child = ContextGroupId::from([2u8; 32]);
        let admin = PublicKey::from([0xAA; 32]);

        // Parent's folded group admin, open child → inherits via admin (no cap
        // needed, no direct parent membership row).
        let mut v = inheritance_view(parent, child, admin, 0, false, false);
        v.group_admin.insert(parent, admin);
        assert!(v.is_member_at_cut(child, &admin, None));

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
        assert!(v.is_member_at_cut(child, &admin, Some((root, admin))));
        // A non-admin without any membership does not inherit.
        let other = PublicKey::from([0x33; 32]);
        assert!(!v.is_member_at_cut(child, &other, Some((root, admin))));
    }

    #[test]
    fn put_requires_write_capability() {
        let author = PublicKey::from([1u8; 32]);
        let entity = Id::new([2u8; 32]);
        let op = op_with(
            author,
            OpPayload::Put {
                entity,
                value: vec![1],
            },
        );

        // Writer with WRITE → ok.
        assert!(authorize(&op, &view_with_writer(entity, author, OpMask::WRITE)).is_ok());
        // No entry → rejected.
        assert_eq!(
            authorize(&op, &AclView::default()),
            Err(Rejected::NotPermitted {
                required: OpMask::WRITE
            })
        );
        // A different writer holding the cap doesn't authorize this author.
        let other = PublicKey::from([9u8; 32]);
        assert!(authorize(&op, &view_with_writer(entity, other, OpMask::FULL)).is_err());
    }

    #[test]
    fn delete_requires_delete_capability() {
        let author = PublicKey::from([1u8; 32]);
        let entity = Id::new([2u8; 32]);
        let op = op_with(author, OpPayload::Delete { entity });
        // WRITE alone is not enough for a delete.
        assert!(authorize(&op, &view_with_writer(entity, author, OpMask::WRITE)).is_err());
        assert!(authorize(&op, &view_with_writer(entity, author, OpMask::FULL)).is_ok());
    }

    #[test]
    fn set_writers_requires_owner_admin_bit() {
        let author = PublicKey::from([1u8; 32]);
        let object = Id::new([2u8; 32]);
        let op = op_with(
            author,
            OpPayload::SetWriters {
                object,
                writers: BTreeMap::new(),
            },
        );
        // WRITE-only is not ownership.
        assert_eq!(
            authorize(&op, &view_with_writer(object, author, OpMask::WRITE)),
            Err(Rejected::NotOwner)
        );
        // ADMIN bit confers ownership.
        assert!(authorize(&op, &view_with_writer(object, author, OpMask::ADMIN)).is_ok());
    }

    #[test]
    fn member_change_requires_group_admin() {
        let admin = PublicKey::from([1u8; 32]);
        let stranger = PublicKey::from([2u8; 32]);
        let group = ContextGroupId::from([3u8; 32]);
        let newcomer = PublicKey::from([4u8; 32]);

        let mut groups = BTreeMap::new();
        groups.insert(
            group,
            [(admin, GroupMemberRole::Admin)].into_iter().collect(),
        );
        let view = AclView {
            groups,
            ..Default::default()
        };

        let by_admin = op_with(
            admin,
            OpPayload::MemberAdded {
                group,
                member: newcomer,
                role: GroupMemberRole::Member,
            },
        );
        let by_stranger = op_with(
            stranger,
            OpPayload::MemberRemoved {
                group,
                member: admin,
            },
        );
        assert!(authorize(&by_admin, &view).is_ok());
        assert_eq!(authorize(&by_stranger, &view), Err(Rejected::NotGroupAdmin));
    }

    #[test]
    fn admin_ops_require_root_admin() {
        let root = PublicKey::from([1u8; 32]);
        let other = PublicKey::from([2u8; 32]);
        let view = AclView {
            root_admin: Some(root),
            ..Default::default()
        };
        let op = op_with(other, OpPayload::AdminChanged { new_admin: other });
        assert_eq!(authorize(&op, &view), Err(Rejected::NotRootAdmin));
        let op_ok = op_with(
            root,
            OpPayload::PolicyUpdated {
                policy_bytes: vec![],
            },
        );
        assert!(authorize(&op_ok, &view).is_ok());
    }

    // ---- default-write = membership ----

    fn membership_view(group: ContextGroupId, member: PublicKey, role: GroupMemberRole) -> AclView {
        let mut groups = BTreeMap::new();
        groups.insert(group, [(member, role)].into_iter().collect());
        AclView {
            groups,
            ..Default::default()
        }
    }

    #[test]
    fn default_write_lets_a_member_write_a_non_restricted_entity() {
        // kv-store context: Bob is a member, no per-key ACL. Bob may Put/Delete
        // any key; Carol (non-member) may not.
        let group = ContextGroupId::from([0x33; 32]);
        let bob = PublicKey::from([0xB0; 32]);
        let carol = PublicKey::from([0xC0; 32]);
        let view = membership_view(group, bob, GroupMemberRole::Member);
        let x = Id::new([0x11; 32]);

        assert!(authorize(
            &op_with(
                bob,
                OpPayload::Put {
                    entity: x,
                    value: vec![5]
                }
            ),
            &view
        )
        .is_ok());
        assert!(authorize(&op_with(bob, OpPayload::Delete { entity: x }), &view).is_ok());
        assert_eq!(
            authorize(
                &op_with(
                    carol,
                    OpPayload::Put {
                        entity: x,
                        value: vec![5]
                    }
                ),
                &view
            ),
            Err(Rejected::NotPermitted {
                required: OpMask::WRITE
            })
        );
    }

    #[test]
    fn default_write_does_not_grant_a_member_setwriters() {
        // A plain member gets WRITE+DELETE on default entities but NOT ADMIN —
        // rotating an object's writer set needs an explicit ownership grant.
        let group = ContextGroupId::from([0x33; 32]);
        let bob = PublicKey::from([0xB0; 32]);
        let view = membership_view(group, bob, GroupMemberRole::Member);
        let x = Id::new([0x11; 32]);
        assert_eq!(
            authorize(
                &op_with(
                    bob,
                    OpPayload::SetWriters {
                        object: x,
                        writers: BTreeMap::new()
                    }
                ),
                &view
            ),
            Err(Rejected::NotOwner)
        );
    }

    #[test]
    fn explicit_acl_overrides_default_write_for_restricted_objects() {
        // `secret` carries an explicit ACL {Alice: FULL}. Bob is a context
        // member but NOT a writer of `secret` → denied (the old coarse
        // per-delta gate would have let him through; the unified check is
        // strictly tighter). Alice → ok.
        let group = ContextGroupId::from([0x33; 32]);
        let alice = PublicKey::from([0xA1; 32]);
        let bob = PublicKey::from([0xB0; 32]);
        let secret = Id::new([0x5E; 32]);

        let mut view = membership_view(group, bob, GroupMemberRole::Member);
        // Both are members; only Alice is a writer of the restricted object.
        view.groups
            .get_mut(&group)
            .unwrap()
            .insert(alice, GroupMemberRole::Admin);
        view.acl
            .insert(secret, [(alice, OpMask::FULL)].into_iter().collect());

        assert!(authorize(
            &op_with(
                alice,
                OpPayload::Put {
                    entity: secret,
                    value: vec![1]
                }
            ),
            &view
        )
        .is_ok());
        assert_eq!(
            authorize(
                &op_with(
                    bob,
                    OpPayload::Put {
                        entity: secret,
                        value: vec![1]
                    }
                ),
                &view
            ),
            Err(Rejected::NotPermitted {
                required: OpMask::WRITE
            })
        );
    }
}
