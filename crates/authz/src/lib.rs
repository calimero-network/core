//! The **one** authorization fold for the unified causal log (core#2716,
//! Phase 5).
//!
//! [`authorize`] is the single security boundary. It replaces the three
//! causal-auth folds the old model smeared across the codebase — `writers_at`
//! (writer-set), `membership_status_at` (group membership), and the
//! `GovernancePosition` / `GroupIdCheck` chain — with one match over
//! [`OpPayload`] arms against an [`AclView`] resolved at the op's causal cut.
//!
//! **Causal-honor semantics** (the decision recorded for §9.1): an op is
//! authorized against the ACL/membership *as of its own causal parents*, never
//! the receiver's current state. So a write authored before a revocation, in
//! causal order, stays valid regardless of the order a receiver observes the
//! revocation — the forward-only property the P4 `acl_view_at` already
//! provides. The caller produces the [`AclView`] via
//! `ScopeState::acl_view_at(op.parents)` (see `calimero-projection`); this
//! crate is the pure decision over that view.
//!
//! Additive scaffolding — not yet wired into the live apply path.

use std::collections::BTreeMap;

use thiserror::Error as ThisError;

use calimero_context_config::types::ContextGroupId;
use calimero_op::{Op, OpPayload};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;

/// Why an op was refused. One rejection type for every plane — the caller no
/// longer has to know which of three folds said no.
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
    /// Writer/capability set per object (the writer plane — was the rotation
    /// log resolved by `writers_at` / `resolve_local`).
    pub acl: BTreeMap<Id, BTreeMap<PublicKey, OpMask>>,
    /// Group memberships at the cut (the membership plane — was
    /// `membership_status_at`'s walk result).
    pub groups: BTreeMap<ContextGroupId, BTreeMap<PublicKey, GroupMemberRole>>,
    /// The scope's root admin at the cut (the admin plane).
    pub root_admin: Option<PublicKey>,
}

/// Capabilities a scope **member** implicitly holds on a non-restricted
/// entity (§9.2 / slice-2 S2.1 decision: `default-write = membership`):
/// `WRITE` + `DELETE`, but **not** `ADMIN` — rotating an object's writer set
/// still requires an explicit ACL grant (ownership), so a plain member can't
/// lock others out of a default entity.
const DEFAULT_MEMBER_MASK: OpMask = OpMask::WRITE.union(OpMask::DELETE);

impl AclView {
    /// Does `author` hold at least `required` on `entity`?
    ///
    /// Two-tier (S2.1 `default-write = membership`):
    /// 1. **Restricted entity** — an explicit per-object ACL entry exists:
    ///    `author` must be listed with a mask covering `required`. This is the
    ///    writer plane (was `writers_at` / `resolve_local`); a member who isn't
    ///    a listed writer is denied (strictly finer than the old per-delta
    ///    membership gate).
    /// 2. **Non-restricted entity** — no explicit ACL: any scope member holds
    ///    [`DEFAULT_MEMBER_MASK`] (`WRITE`+`DELETE`). This reproduces today's
    ///    "members can write" for ordinary contexts (e.g. kv-store) without
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

    /// Is `author` the owner of `object` — permitted to rotate its writer set?
    ///
    /// Default (§9.2, owner = capability holder): the `ADMIN` bit on the object
    /// confers ownership. Refine here if `owner` becomes distinct from
    /// `writer`/`admin`.
    #[must_use]
    pub fn is_owner(&self, author: &PublicKey, object: Id) -> bool {
        self.may(author, object, OpMask::ADMIN)
    }

    /// Is `author` an `Admin` of `group` at the cut?
    #[must_use]
    pub fn is_group_admin(&self, author: &PublicKey, group: ContextGroupId) -> bool {
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

/// The capability a data op requires of its author.
#[must_use]
pub fn required_mask_for(payload: &OpPayload) -> OpMask {
    match payload {
        OpPayload::Put { .. } => OpMask::WRITE,
        OpPayload::Delete { .. } => OpMask::DELETE,
        _ => OpMask::NONE,
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
        OpPayload::Put { entity, .. } | OpPayload::Delete { entity } => {
            let required = required_mask_for(&op.payload);
            if acl_at_cut.may(&op.author, *entity, required) {
                Ok(())
            } else {
                Err(Rejected::NotPermitted { required })
            }
        }
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

    // ---- S2.1: default-write = membership (the slice-2 example scenarios) ----

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
