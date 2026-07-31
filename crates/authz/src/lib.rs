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

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error as ThisError;

use calimero_account::{
    AccountGenesis, AccountId, DeviceCert, DeviceId, KemPublicKey, RootKeyHandoff,
    VerifiedDeviceCert,
};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_op::{Op, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;

/// `CAN_JOIN_OPEN_SUBGROUPS` capability bit — gates inherited membership into an
/// open subgroup (mirrors the live `MemberCapabilities` constant).
const CAN_JOIN_OPEN_SUBGROUPS: u32 = MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits();
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
    /// The signing device has no binding at this cut, so it speaks for nobody.
    #[error("device {device} is not linked to any account at the cut")]
    DeviceNotLinked {
        /// The unbound device.
        device: DeviceId,
    },
    /// The device is bound, but to a different account than the op claims.
    #[error("device {device} speaks for {bound}, not the claimed {claimed}")]
    DeviceAccountMismatch {
        /// The device that signed.
        device: DeviceId,
        /// The account the device is actually bound to.
        bound: AccountId,
        /// The account the op claimed.
        claimed: AccountId,
    },
    /// A root-key rotation authored by an account other than the one it rotates.
    ///
    /// Its own variant rather than [`Rejected::DeviceAccountMismatch`], which used
    /// to be reused here: that one describes a device bound to a different account
    /// than the op claims, and a rotation involves no device binding at all. Reusing
    /// it produced a message that named the two accounts in the wrong roles — the
    /// author's account is the *established* one (`check_device_speaks_for_author`
    /// has already proved it), and the handoff's account is the claim.
    #[error("rotation of {account} was authored by {author}, which is not that account")]
    RotationNotByAccount {
        /// The account whose root key the handoff would roll.
        account: AccountId,
        /// The account that actually authored the op.
        author: AccountId,
    },
    /// The op was signed with a key the device has since rotated away from.
    #[error("device {device} signed with a superseded key")]
    DeviceKeyStale {
        /// The device whose key is out of date.
        device: DeviceId,
    },
    /// The device's binding has been withdrawn at or before this cut.
    #[error("device {device} was revoked at or before this cut")]
    DeviceRevoked {
        /// The withdrawn device.
        device: DeviceId,
    },
    /// A device certificate did not verify against its self-certifying genesis.
    #[error("device certificate is not internally valid: {reason}")]
    CredentialInvalid {
        /// Why `calimero-account` refused the credential.
        reason: calimero_account::AccountError,
    },
    /// A credential minted by a root key this scope has already superseded.
    #[error("credential signed by superseded key epoch {signed} (current is {current})")]
    CredentialSuperseded {
        /// Epoch the credential was signed under.
        signed: u32,
        /// Epoch currently in force at the cut.
        current: u32,
    },
    /// The link does not advance the device's rotation epoch, so it grants
    /// nothing and would only let an old certificate be replayed.
    #[error("device link at epoch {offered} does not supersede the folded epoch {folded}")]
    DeviceEpochNotAdvanced {
        /// Epoch offered by the incoming link.
        offered: u32,
        /// Epoch already in force.
        folded: u32,
    },
    /// A device may not be moved between accounts; enroll a fresh device id.
    #[error("device is already bound to a different account")]
    DeviceAccountReassignment,
    /// The account is not a member of this scope, so its devices may not link
    /// themselves in.
    #[error("account is not a member of this scope at the cut")]
    AccountNotMember,
    /// A key rotation for an account this scope has never seen, or one that
    /// does not continue the established chain.
    #[error("key rotation does not continue this account's chain at the cut")]
    RotationNotContinuous,
    /// A key rotation not signed by the outgoing root key.
    #[error("key rotation is not signed by the outgoing root key")]
    RotationSignatureInvalid,
}

/// The authorization-relevant slice of a [`ScopeState`](calimero_projection)
/// **at a causal cut** — the value [`authorize`] decides against. Produced by
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
    /// self-binding rule (see [`authorize`]).
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
    ///    [`DEFAULT_MEMBER_MASK`] (`WRITE`+`DELETE`). This gives "members can
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

/// Decide whether a `DeviceLinked` credential takes effect at a cut.
///
/// **This is the single definition of the rule.** Both [`authorize`] and the
/// projection's fold call it, so the log can never contain a link the state
/// ignored, nor fold a link authorization refused. Duplicating the rule in two
/// places is exactly how one node authorizes an op its peer folds differently,
/// and that is a `scope_root` divergence.
///
/// The checks, in order:
/// 1. the credential is internally valid — the genesis addresses the claimed
///    account and the handoff chain carries valid signatures up to the
///    certificate's epoch (`calimero-account`);
/// 2. the signing epoch has not been superseded at this cut, so rotating the
///    root key actually withdraws the old key's authority instead of merely
///    adding a new key beside it;
/// 3. the device has not been revoked — read from the grow-only revoked set,
///    which is what makes a revocation that folds *before* its link still win;
/// 4. a device is never reassigned to another account;
/// 5. a re-link strictly advances the device's rotation epoch, so an old
///    certificate cannot be replayed to reinstate a retired key;
/// 6. on first link, no other device in the scope already claims the same
///    replica seed prefix — which turns RGA id uniqueness from a birthday
///    argument into a checked invariant.
///
/// Deliberately **not** checked here: whether the account is a member of the
/// scope. That is policy rather than credential validity, it only bears on the
/// authorization decision, and leaving it out keeps the fold a pure function of
/// already-authorized ops.
///
/// # Errors
/// The `Credential*` / `Device*` variants of [`Rejected`], one per rule above.
pub fn admit_device_link(
    accounts: &BTreeMap<AccountId, AccountBinding>,
    devices: &BTreeMap<DeviceId, DeviceBinding>,
    revoked: &BTreeSet<DeviceId>,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    cert: &DeviceCert,
) -> Result<VerifiedDeviceCert, Rejected> {
    let verified = fold_device_link(devices, revoked, genesis, chain, cert)?;

    // Supersession is checked HERE and not in [`fold_device_link`], because the
    // two callers need it at different moments. `authorize` decides against a
    // fixed causal cut, so "has this epoch been superseded *at the cut*" is a
    // well-defined question. The fold walks ops one at a time, where the same
    // question would read whatever epoch happened to be folded so far — making
    // admission depend on delivery order, and the projection non-convergent.
    // The fold therefore records the link and filters superseded ones out when
    // the view is read, once the final epoch is known. Both paths agree on the
    // observable state; they just get there in the order each can afford.
    if let Some(binding) = accounts.get(&verified.account) {
        if verified.key_epoch < binding.epoch {
            return Err(Rejected::CredentialSuperseded {
                signed: verified.key_epoch,
                current: binding.epoch,
            });
        }
    }

    Ok(verified)
}

/// The order-independent half of [`admit_device_link`] — every rule whose
/// answer cannot change as more ops fold in.
///
/// Each check here is monotone: a revocation tombstone is never removed, a
/// device is never un-assigned from its account, and the device epoch only ever
/// rises. So folding these in any order reaches the same result, which is what
/// keeps `scope_root` convergent.
///
/// # Errors
/// The `Credential*` / `Device*` variants of [`Rejected`], excluding
/// [`Rejected::CredentialSuperseded`] — see [`admit_device_link`].
pub fn fold_device_link(
    devices: &BTreeMap<DeviceId, DeviceBinding>,
    revoked: &BTreeSet<DeviceId>,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    cert: &DeviceCert,
) -> Result<VerifiedDeviceCert, Rejected> {
    let verified = calimero_account::verify_device_cert(cert.account, genesis, chain, cert)
        .map_err(|reason| Rejected::CredentialInvalid { reason })?;

    if revoked.contains(&verified.device) {
        return Err(Rejected::DeviceRevoked {
            device: verified.device,
        });
    }

    match devices.get(&verified.device) {
        Some(existing) => {
            if existing.account != verified.account {
                return Err(Rejected::DeviceAccountReassignment);
            }
            if verified.device_epoch <= existing.device_epoch {
                return Err(Rejected::DeviceEpochNotAdvanced {
                    offered: verified.device_epoch,
                    folded: existing.device_epoch,
                });
            }
        }
        None => {
            // No seed-collision check here. On a prefix collision the LOWER
            // device id wins, but *which* device that is cannot be decided as
            // each link folds: rejecting the newcomer only when an already-folded
            // device compares lower is order-dependent in the direction it does
            // not check, so low-then-high left one device live while
            // high-then-low left both. `ScopeState::live_devices` applies the
            // rule over the folded set instead, where it is a function of the op
            // set and every replica reaches the same view.
        }
    }

    Ok(verified)
}

/// Decide whether an `AccountKeysRotated` handoff takes effect at a cut.
///
/// Shared by [`authorize`] and the fold, for the same no-drift reason as
/// [`admit_device_link`]. A rotation is admissible only if the scope already
/// knows the account (it learned the genesis from a device link) and the
/// handoff continues the chain from the epoch currently in force, signed by the
/// key currently in force.
///
/// # Errors
/// [`Rejected::RotationNotContinuous`] or [`Rejected::RotationSignatureInvalid`].
pub fn admit_key_rotation(
    accounts: &BTreeMap<AccountId, AccountBinding>,
    handoff: &RootKeyHandoff,
) -> Result<(), Rejected> {
    let Some(binding) = accounts.get(&handoff.account) else {
        return Err(Rejected::RotationNotContinuous);
    };
    if handoff.from_epoch != binding.epoch {
        return Err(Rejected::RotationNotContinuous);
    }
    if binding
        .root_pk
        .verify_raw_signature(&handoff.payload(), &handoff.signature)
        .is_err()
    {
        return Err(Rejected::RotationSignatureInvalid);
    }
    Ok(())
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
    author: &AccountId,
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
    // Stage one: does the key that signed this op currently speak for the
    // account the op claims? `Op::verify` already proved the signature genuine;
    // that is integrity, not authority. Only the cut knows which links and
    // revocations are in force, which is why the binding is resolved here and
    // never from live store — a verdict that depended on receiver state would
    // let two nodes disagree about the same op and diverge on `scope_root`.
    //
    // `DeviceLinked` is exempt because it is the op that *establishes* a
    // binding; its own admission rules stand in for this check.
    if !matches!(op.payload, OpPayload::DeviceLinked { .. }) {
        check_device_speaks_for_author(op, acl_at_cut)?;
    }

    // Stage two: does that account hold the authority this payload needs?
    match &op.payload {
        // Split per data op so each carries its literal required mask — no
        // `Option` to unwrap, so there is no unreachable fallback that could
        // silently deny (or panic) if the arms ever drift. `required_mask_for`
        // remains the public helper for external callers.
        OpPayload::Put { entity, .. } => {
            check_data(acl_at_cut, &op.author(), *entity, OpMask::WRITE)
        }
        OpPayload::Delete { entity } => {
            check_data(acl_at_cut, &op.author(), *entity, OpMask::DELETE)
        }
        OpPayload::SetWriters { object, .. } => {
            if acl_at_cut.is_owner(&op.author(), *object) {
                Ok(())
            } else {
                Err(Rejected::NotOwner)
            }
        }
        OpPayload::MemberAdded { group, .. } | OpPayload::MemberRemoved { group, .. } => {
            if acl_at_cut.is_group_admin(&op.author(), *group) {
                Ok(())
            } else {
                Err(Rejected::NotGroupAdmin)
            }
        }
        OpPayload::SubgroupVisibilitySet { scope, .. } => {
            // Visibility is a property of the subgroup; its admin sets it.
            if acl_at_cut.is_group_admin(&op.author(), ContextGroupId::from(*scope.as_bytes())) {
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
            if acl_at_cut.is_root_admin(&op.author()) {
                Ok(())
            } else {
                Err(Rejected::NotRootAdmin)
            }
        }
        // Capability changes are an admin action on the target group.
        OpPayload::DefaultCapabilitiesSet { group, .. }
        | OpPayload::MemberCapabilitySet { group, .. } => {
            if acl_at_cut.is_group_admin(&op.author(), *group) {
                Ok(())
            } else {
                Err(Rejected::NotGroupAdmin)
            }
        }
        // A graph-only node mutates nothing, so there is nothing to authorize.
        OpPayload::Noop => Ok(()),

        // ---- account plane ----
        OpPayload::DeviceLinked {
            genesis,
            chain,
            cert,
        } => {
            let verified = admit_device_link(
                &acl_at_cut.accounts,
                &acl_at_cut.devices,
                &acl_at_cut.revoked_devices,
                genesis,
                chain,
                cert,
            )?;
            // The op must be signed by the very key it enrolls. Without this,
            // anyone who observed a certificate could replay it and mint a
            // binding on the real device's behalf; requiring possession makes a
            // link an act of the device rather than an assertion about it.
            if op.authorship.device_key != verified.sign_pk
                || op.authorship.device != verified.device
            {
                return Err(Rejected::DeviceKeyStale {
                    device: verified.device,
                });
            }
            if op.author() != verified.account {
                return Err(Rejected::DeviceAccountMismatch {
                    device: verified.device,
                    bound: verified.account,
                    claimed: op.author(),
                });
            }
            // The one policy gate: a device may only link itself into a scope
            // its account already belongs to. This is what makes linking cheap
            // and safe at once — the account already holds every right the
            // device gains, so the link is no privilege escalation and needs no
            // admin action. It is also the only thing between a stranger and an
            // unbounded supply of link ops in this scope.
            if !acl_at_cut.is_scope_member(&verified.account) {
                return Err(Rejected::AccountNotMember);
            }
            Ok(())
        }
        OpPayload::DeviceRevoked { account, device } => {
            // Either the account withdraws its own device (the lost-laptop
            // case, which needs no admin), or a scope admin ejects it (the
            // compromised-member case, which the account may be unable or
            // unwilling to handle itself).
            //
            // Self-service requires a folded binding that *proves* the device
            // speaks for the author. Trusting the payload's own `account` field
            // when no binding exists made the claim unfalsifiable: any linked
            // member could name its own account beside an arbitrary unbound
            // device id and be authorized. Because a tombstone is terminal —
            // and because an early revocation deliberately beats the link it
            // withdraws — that spent the id for good, so an attacker could
            // permanently lock out a device it had no relationship to simply by
            // observing its link op and revoking at an earlier cut.
            match acl_at_cut.devices.get(device) {
                Some(binding) if binding.account == op.author() && binding.account == *account => {
                    Ok(())
                }
                // "No binding at this cut" is not a refusal, because an admin
                // must still be able to eject a device whose link this cut has
                // not folded. It only means the *self-service* claim cannot be
                // checked, so it does not authorize.
                _ if acl_at_cut.is_root_admin(&op.author()) => Ok(()),
                _ => Err(Rejected::NotRootAdmin),
            }
        }
        OpPayload::AccountKeysRotated { handoff } => {
            // Only the account may roll its own key. The handoff's signature is
            // checked by `admit_key_rotation`; this is the separate question of
            // whether the *op* was authored under that account's authority.
            if op.author() != handoff.account {
                return Err(Rejected::RotationNotByAccount {
                    account: handoff.account,
                    author: op.author(),
                });
            }
            admit_key_rotation(&acl_at_cut.accounts, handoff)
        }
    }
}

/// The device-binding precondition: is `op`'s signing key currently authorized
/// to act as `op.author()` at this cut?
///
/// Satisfied only by an explicit binding — a folded `DeviceLinked` naming this
/// device, this account, and this key. There is deliberately no implicit
/// fallback for an unlinked key: every author is an account, and every account
/// speaks through devices it has actually enrolled. A key nobody linked speaks
/// for nobody.
fn check_device_speaks_for_author(op: &Op, acl_at_cut: &AclView) -> Result<(), Rejected> {
    let device = op.device();

    // Checked ahead of the binding, not only in its absence. The fold does
    // maintain "revoked implies unbound" — a revocation removes the binding, and
    // `admit_device_link` refuses a link for a revoked device — so this is not a
    // reachable bypass today. It is here because `authorize` is the single
    // security boundary and takes an `AclView` with public fields from any
    // producer: resting a revocation check on an invariant maintained somewhere
    // else means a future fold that ever leaves a stale binding behind fails
    // open, silently.
    if acl_at_cut.revoked_devices.contains(&device) {
        return Err(Rejected::DeviceRevoked { device });
    }

    match acl_at_cut.devices.get(&device) {
        Some(binding) => {
            if binding.account != op.author() {
                return Err(Rejected::DeviceAccountMismatch {
                    device,
                    bound: binding.account,
                    claimed: op.author(),
                });
            }
            // Pinning the key, not merely the account, is what makes device key
            // rotation meaningful: after a re-link the retired key can no
            // longer author, even though the device is still bound.
            if binding.sign_pk != op.authorship.device_key {
                return Err(Rejected::DeviceKeyStale { device });
            }
            Ok(())
        }
        // The tombstone was already consulted above, so reaching here means the
        // device was never linked rather than withdrawn — worth keeping distinct
        // so whoever reads a rejection knows which happened.
        None => Err(Rejected::DeviceNotLinked { device }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_op::{Authorship, ScopeId};
    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
    use core::num::NonZeroU128;

    fn hlc0() -> HybridTimestamp {
        HybridTimestamp::new(Timestamp::new(
            NTP64(0),
            ID::from(NonZeroU128::new(1).unwrap()),
        ))
    }

    /// The device a test account authors from. Deterministic so
    /// [`bind_test_devices`] can register exactly the binding `op_with` uses.
    fn test_device(account: AccountId) -> DeviceId {
        DeviceId::from(*account.as_bytes())
    }

    /// The key that device signs with. Never verified — `authorize` assumes
    /// `Op::verify` already ran — so any deterministic value will do.
    fn test_device_key(account: AccountId) -> PublicKey {
        PublicKey::from(*account.as_bytes())
    }

    fn op_with(author: AccountId, payload: OpPayload) -> Op {
        Op::new(
            ScopeId::from([0u8; 32]),
            vec![],
            Authorship {
                account: author,
                device: test_device(author),
                device_key: test_device_key(author),
            },
            hlc0(),
            payload,
            [0u8; 32],
            [0u8; 64],
        )
    }

    /// Register a device binding for every account the view mentions, standing
    /// in for the `DeviceLinked` ops a real log would carry.
    ///
    /// These tests predate accounts and exercise the *policy* layer — who may
    /// write, who is an admin, how membership inherits. Binding every mentioned
    /// account keeps them aimed at that, instead of every one of them tripping
    /// the device precondition first. The precondition has its own dedicated
    /// tests below.
    /// Register a binding for one account the view does not otherwise mention.
    ///
    /// Needed wherever a test drives an *outsider* — a stranger, a non-admin —
    /// at the policy layer: without a binding they would be turned away by the
    /// device precondition, and the test would stop proving anything about the
    /// policy rule it names.
    #[test]
    fn a_rotation_authored_by_another_account_is_refused_with_the_roles_named() {
        // Only the account may roll its own key. The refusal used to reuse
        // `DeviceAccountMismatch`, whose fields mean "the account the device is bound
        // to" and "the account the op claimed" — and it passed the handoff's account
        // as `bound`. That is backwards: `check_device_speaks_for_author` has already
        // established the author's account, so the author is the settled identity and
        // the handoff's account is the claim. The message then named both accounts in
        // the wrong roles, on a rotation where no device binding is in question at
        // all.
        //
        // Asserting the payload, not just the rejection: getting refused was never
        // the bug.
        let owner = AccountId::from([0x11; 32]);
        let stranger = AccountId::from([0x22; 32]);
        let view = bind_account(bind_test_devices(AclView::default()), stranger);

        let handoff = RootKeyHandoff {
            account: owner,
            from_epoch: 0,
            new_root_sign_pk: calimero_primitives::identity::PrivateKey::from([0x33; 32])
                .public_key(),
            signature: [0u8; 64],
        };
        let op = op_with(stranger, OpPayload::AccountKeysRotated { handoff });

        match authorize(&op, &view) {
            Err(Rejected::RotationNotByAccount { account, author }) => {
                assert_eq!(
                    account, owner,
                    "`account` must name what the handoff rotates"
                );
                assert_eq!(author, stranger, "`author` must name who actually signed");
            }
            other => panic!("expected RotationNotByAccount, got {other:?}"),
        }
    }

    fn bind_account(mut view: AclView, account: AccountId) -> AclView {
        let _ = view.devices.insert(
            test_device(account),
            DeviceBinding {
                account,
                sign_pk: test_device_key(account),
                kem_pk: KemPublicKey::from([0u8; 32]),
                device_epoch: 0,
                key_epoch: 0,
            },
        );
        view
    }

    fn bind_test_devices(mut view: AclView) -> AclView {
        let mut accounts: Vec<AccountId> = Vec::new();
        accounts.extend(view.acl.values().flat_map(|w| w.keys().copied()));
        accounts.extend(view.groups.values().flat_map(|m| m.keys().copied()));
        accounts.extend(view.root_admin);
        accounts.extend(view.group_admin.values().copied());
        accounts.extend(view.member_caps.keys().map(|(_, m)| *m));
        for account in accounts {
            let _ = view.devices.insert(
                test_device(account),
                DeviceBinding {
                    account,
                    sign_pk: test_device_key(account),
                    kem_pk: KemPublicKey::from([0u8; 32]),
                    device_epoch: 0,
                    key_epoch: 0,
                },
            );
        }
        view
    }

    fn view_with_writer(entity: Id, who: AccountId, mask: OpMask) -> AclView {
        let mut acl = BTreeMap::new();
        acl.insert(entity, [(who, mask)].into_iter().collect());
        bind_test_devices(AclView {
            acl,
            ..Default::default()
        })
    }

    // Build a view: parent group with `member` (holding `caps`), an open
    // subgroup `child` nested under `parent`. Mirrors the inheritance scenario.
    fn inheritance_view(
        parent: ContextGroupId,
        child: ContextGroupId,
        member: AccountId,
        caps: u32,
        child_restricted: bool,
        parent_has_member: bool,
    ) -> AclView {
        let mut groups: BTreeMap<ContextGroupId, BTreeMap<AccountId, GroupMemberRole>> =
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

    #[test]
    fn put_requires_write_capability() {
        let author = AccountId::from([1u8; 32]);
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
            authorize(&op, &bind_account(AclView::default(), author)),
            Err(Rejected::NotPermitted {
                required: OpMask::WRITE
            })
        );
        // A different writer holding the cap doesn't authorize this author.
        let other = AccountId::from([9u8; 32]);
        assert!(authorize(&op, &view_with_writer(entity, other, OpMask::FULL)).is_err());
    }

    #[test]
    fn delete_requires_delete_capability() {
        let author = AccountId::from([1u8; 32]);
        let entity = Id::new([2u8; 32]);
        let op = op_with(author, OpPayload::Delete { entity });
        // WRITE alone is not enough for a delete.
        assert!(authorize(&op, &view_with_writer(entity, author, OpMask::WRITE)).is_err());
        assert!(authorize(&op, &view_with_writer(entity, author, OpMask::FULL)).is_ok());
    }

    #[test]
    fn set_writers_requires_owner_admin_bit() {
        let author = AccountId::from([1u8; 32]);
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
        let admin = AccountId::from([1u8; 32]);
        let stranger = AccountId::from([2u8; 32]);
        let group = ContextGroupId::from([3u8; 32]);
        let newcomer = AccountId::from([4u8; 32]);

        let mut groups = BTreeMap::new();
        groups.insert(
            group,
            [(admin, GroupMemberRole::Admin)].into_iter().collect(),
        );
        let view = bind_account(
            bind_test_devices(AclView {
                groups,
                ..Default::default()
            }),
            stranger,
        );

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
        let root = AccountId::from([1u8; 32]);
        let other = AccountId::from([2u8; 32]);
        let view = bind_account(
            bind_test_devices(AclView {
                root_admin: Some(root),
                ..Default::default()
            }),
            other,
        );
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

    fn membership_view(group: ContextGroupId, member: AccountId, role: GroupMemberRole) -> AclView {
        let mut groups = BTreeMap::new();
        groups.insert(group, [(member, role)].into_iter().collect());
        // Carol (the non-member these tests contrast against) is bound too, so
        // her rejection proves the membership rule rather than the device one.
        bind_account(
            bind_test_devices(AclView {
                groups,
                ..Default::default()
            }),
            AccountId::from([0xC0; 32]),
        )
    }

    #[test]
    fn default_write_lets_a_member_write_a_non_restricted_entity() {
        // kv-store context: Bob is a member, no per-key ACL. Bob may Put/Delete
        // any key; Carol (non-member) may not.
        let group = ContextGroupId::from([0x33; 32]);
        let bob = AccountId::from([0xB0; 32]);
        let carol = AccountId::from([0xC0; 32]);
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
        let bob = AccountId::from([0xB0; 32]);
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
        let alice = AccountId::from([0xA1; 32]);
        let bob = AccountId::from([0xB0; 32]);
        let secret = Id::new([0x5E; 32]);

        let mut view = membership_view(group, bob, GroupMemberRole::Member);
        // Both are members; only Alice is a writer of the restricted object.
        view.groups
            .get_mut(&group)
            .unwrap()
            .insert(alice, GroupMemberRole::Admin);
        view.acl
            .insert(secret, [(alice, OpMask::FULL)].into_iter().collect());
        // Alice joined the view after `membership_view` built it, so bind her
        // device explicitly.
        let view = bind_account(view, alice);

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
