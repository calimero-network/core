//! [`AclView`] — the authorization-relevant slice of projected state at a
//! causal cut — and the flat predicates over it.
//!
//! The three questions that walk the subgroup tree live in
//! [`crate::inheritance`] instead; everything here answers from a single map
//! lookup.

use std::collections::{BTreeMap, BTreeSet};

use calimero_account::{AccountId, DeviceId, KemPublicKey};
use calimero_context_config::types::ContextGroupId;
use calimero_op::ScopeId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;

/// The authorization-relevant slice of a `ScopeState` (`calimero-projection`)
/// **at a causal cut** — the value [`crate::authorize`] decides against. Produced by
/// `ScopeState::acl_view_at(parents)`; this crate never walks the DAG itself
/// (that's the projection's job), keeping the decision pure and unit-testable.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AclView {
    /// Writer/capability set per object (the writer plane).
    pub acl: BTreeMap<Id, BTreeMap<AccountId, OpMask>>,
    /// Group memberships at the cut (the membership plane).
    pub groups: BTreeMap<ContextGroupId, BTreeMap<AccountId, GroupMemberRole>>,
    /// The scope's root admin at the cut (the admin plane).
    pub root_admin: Option<AccountId>,
    /// Per-group default capability bitmask at the cut (capability plane).
    pub default_caps: BTreeMap<ContextGroupId, u32>,
    /// Per-(group, member) explicit capability override at the cut. Takes
    /// precedence over the group default for that member.
    pub member_caps: BTreeMap<(ContextGroupId, AccountId), u32>,
    /// Live subgroup tree at the cut: child scope → (parent scope, restricted).
    /// Only scopes whose latest `exists` is true appear. Drives the inherited-
    /// membership parent walk (open chain to an ancestor the author belongs to).
    pub subgroups: BTreeMap<ScopeId, SubgroupEdge>,
    /// Per-group genesis admin at the cut (the subgroup creator, or the
    /// namespace-root admin seeded at backfill). Mirrors the live
    /// `GroupMeta.admin_identity`. An identity is a group admin iff it is this
    /// or holds the `Admin` role in `groups[group]`.
    pub group_admin: BTreeMap<ContextGroupId, AccountId>,
    /// Each account's **resolved** root key at the cut (the account plane).
    ///
    /// Derived by the projection by walking the account's handoff chain, so
    /// this is the key that may currently mint device certificates — not merely
    /// some key that once could.
    pub accounts: BTreeMap<AccountId, AccountBinding>,
    /// Device→account bindings in force at the cut.
    ///
    /// This is what turns an authenticated *signature* into an authorized
    /// *account*. A device absent here speaks for nobody, barring the
    /// self-binding rule (see [`crate::authorize`]).
    pub devices: BTreeMap<DeviceId, DeviceBinding>,
    /// Devices whose binding has been withdrawn, and the account they were
    /// withdrawn from.
    ///
    /// Separate from [`devices`](Self::devices) and **grow-only**, which is
    /// what makes revocation order-independent: a revocation that folds
    /// *before* the link it withdraws still wins, because every link consults
    /// this set. Were revocation merely a flag on the binding, a
    /// revoke-then-link arrival order would silently resurrect the device.
    pub revoked_devices: BTreeSet<DeviceId>,
}

/// An account's resolved root key at a cut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountBinding {
    /// Highest root-key epoch established at this cut.
    pub epoch: u32,
    /// The root key at [`epoch`](Self::epoch) — the only key that may sign a
    /// device certificate this scope will still accept.
    pub root_pk: PublicKey,
}

/// A device's binding to an account at a cut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceBinding {
    /// The account this device speaks for.
    pub account: AccountId,
    /// The key whose signature counts as this device's.
    pub sign_pk: PublicKey,
    /// Where wrapped scope keys are delivered for this device.
    pub kem_pk: KemPublicKey,
    /// Device key-rotation epoch; a link must strictly exceed it to supersede.
    pub device_epoch: u32,
    /// Account root-key epoch that signed this device's certificate.
    ///
    /// Retained so the projection can drop a binding whose signing epoch the
    /// account has since rotated past. The check has to happen when the view is
    /// read rather than when the link folds, because the account's final epoch
    /// is not known until every op in the cut has been seen.
    pub key_epoch: u32,
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
    ///    `DEFAULT_MEMBER_MASK` (`WRITE`+`DELETE`). This gives "members can
    ///    write" for ordinary contexts (e.g. a key-value store) without
    ///    enumerating a per-entity writer set for every key.
    #[must_use]
    pub fn may(&self, author: &AccountId, entity: Id, required: OpMask) -> bool {
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
    pub fn is_scope_member(&self, author: &AccountId) -> bool {
        self.groups
            .values()
            .any(|members| members.contains_key(author))
    }

    /// `member`'s effective capability bitmask in `group` at the cut: the
    /// explicit per-member override if present, else the group default, else
    /// `0`. Mirrors the live `member_capability` read used by inherited-
    /// membership resolution (the `CAN_JOIN_OPEN_SUBGROUPS` gate).
    #[must_use]
    pub fn capability(&self, group: &ContextGroupId, member: &AccountId) -> u32 {
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
    pub fn is_owner(&self, author: &AccountId, object: Id) -> bool {
        self.may(author, object, OpMask::ADMIN)
    }

    /// Is `author` an `Admin` of `group` at the cut?
    #[must_use]
    pub fn is_group_admin(&self, author: &AccountId, group: ContextGroupId) -> bool {
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
    pub fn is_root_admin(&self, author: &AccountId) -> bool {
        self.root_admin.as_ref() == Some(author)
    }
}
