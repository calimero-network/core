use crate::{CapabilitiesRepository, MetadataRepository};
use crate::{DenyListRepository, MetaRepository, NamespaceRepository};
use calimero_governance_types::NamespaceId;
use std::collections::BTreeSet;

use calimero_context_config::types::ContextGroupId;
use calimero_context_config::{MemberCapabilities, VisibilityMode};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{AutoFollowFlags, GroupMember, GroupMemberValue, GROUP_MEMBER_PREFIX};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::super::namespace::MAX_NAMESPACE_DEPTH;
use super::super::{
    collect_keys_with_prefix, collect_keys_with_prefix_paginated, count_keys_with_prefix,
    CapabilitiesError, MembershipError,
};

/// Typed Repository for the membership cluster — direct member rows,
/// role lookups, ancestor-walks for inherited / admin membership,
/// trusted-anchor enumeration, and capability checks.
///
/// The bulk of group_store's "who is a member, and via what path?"
/// logic lives here. Most methods preserve byte-for-byte semantics
/// from the previous free-function form; the rename log is in the
/// commit message for #2303 commit 5.
///
/// Issue #2303 / epic #2300.
pub struct MembershipRepository<'a> {
    store: &'a Store,
}

impl<'a> MembershipRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn add_member(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
        role: GroupMemberRole,
    ) -> EyreResult<()> {
        self.add_member_with_keys(group_id, identity, role, None, None)
    }

    pub fn add_member_with_keys(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
        role: GroupMemberRole,
        private_key: Option<[u8; 32]>,
        sender_key: Option<[u8; 32]>,
    ) -> EyreResult<()> {
        let is_admin = role == GroupMemberRole::Admin;
        let mut handle = self.store.handle();
        let key = GroupMember::new(group_id.to_bytes(), *identity);
        // Preserve auto_follow across updates — used for upserts (e.g. MemberRoleSet).
        let existing_auto_follow = handle
            .get::<GroupMember>(&key)?
            .map(|v| v.auto_follow)
            .unwrap_or_default();
        handle.put(
            &key,
            &GroupMemberValue {
                role,
                private_key,
                sender_key,
                auto_follow: existing_auto_follow,
            },
        )?;
        drop(handle);

        if !is_admin {
            let capabilities = CapabilitiesRepository::new(self.store);
            if let Some(defaults) = capabilities.default_capabilities(group_id)? {
                if defaults != 0 {
                    capabilities.set_member_capability(group_id, identity, defaults)?;
                }
            }
        }

        Ok(())
    }

    /// Change ONLY the role of an existing member, preserving every other
    /// field on the row (`private_key`, `sender_key`, `auto_follow`).
    ///
    /// This is the correct primitive for a role change (`MemberRoleSet`, admin
    /// promotion): unlike [`add_member`](Self::add_member) — which rewrites the
    /// whole `GroupMemberValue` and therefore zeroes `private_key`/`sender_key`
    /// (it preserves only `auto_follow`) — `set_role` touches the role and
    /// nothing else. Bails with `MemberNotFound` if the row does not already
    /// exist, so callers must have verified membership first.
    ///
    /// For a non-admin role, baseline capabilities are seeded ONLY when the
    /// member has no capability row yet. A role change — unlike a fresh add —
    /// must not overwrite an existing grant: `set_role` can demote an admin who
    /// holds custom capabilities to a non-admin role, and unconditionally
    /// re-seeding the group defaults would silently wipe them.
    pub fn set_role(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
        role: GroupMemberRole,
    ) -> EyreResult<()> {
        let is_admin = role == GroupMemberRole::Admin;
        let mut handle = self.store.handle();
        let key = GroupMember::new(group_id.to_bytes(), *identity);
        let existing =
            handle
                .get::<GroupMember>(&key)?
                .ok_or_else(|| MembershipError::MemberNotFound {
                    group_id: format!("{group_id:?}"),
                    member: format!("{identity:?}"),
                })?;
        handle.put(
            &key,
            &GroupMemberValue {
                role,
                private_key: existing.private_key,
                sender_key: existing.sender_key,
                auto_follow: existing.auto_follow,
            },
        )?;
        drop(handle);

        // Two-phase write (member row, then the default-caps row on its own
        // handle), identical to `add_member_with_keys`. It is NOT a transaction,
        // and deliberately so: the store is unbuffered/write-through, so a single
        // shared handle would not make the two puts atomic across a crash anyway.
        // Safety rests on the apply model, not a lock: group-op apply is
        // serialized per group_id by the single-threaded ContextManager actor, so
        // no concurrent reader observes the in-between state; and apply is
        // replay-safe/idempotent, so a crash landing between the two writes is
        // healed when the op re-applies (set_role re-runs and re-seeds the caps).
        //
        // Seed baseline caps ONLY when no capability row exists yet — never
        // clobber an existing grant on a role change (e.g. demoting an admin who
        // held custom caps). The `is_none` gate also keeps re-apply idempotent.
        if !is_admin {
            let capabilities = CapabilitiesRepository::new(self.store);
            if capabilities
                .member_capability(group_id, identity)?
                .is_none()
            {
                if let Some(defaults) = capabilities.default_capabilities(group_id)? {
                    if defaults != 0 {
                        capabilities.set_member_capability(group_id, identity, defaults)?;
                    }
                }
            }
        }

        Ok(())
    }

    pub fn remove_member(&self, group_id: &ContextGroupId, identity: &PublicKey) -> EyreResult<()> {
        {
            let mut handle = self.store.handle();
            handle.delete(&GroupMember::new(group_id.to_bytes(), *identity))?;
        }
        MetadataRepository::new(self.store).delete_member(group_id, identity)?;
        Ok(())
    }

    /// Update the auto-follow flags for an existing member. Caller must
    /// have already verified the member exists (this function bails if
    /// not) and that the signer is authorized to mutate them.
    pub fn set_auto_follow(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
        auto_follow: AutoFollowFlags,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = GroupMember::new(group_id.to_bytes(), *identity);
        let existing = handle
            .get(&key)?
            .ok_or_else(|| MembershipError::MemberNotFound {
                group_id: format!("{group_id:?}"),
                member: format!("{identity:?}"),
            })?;
        handle.put(
            &key,
            &GroupMemberValue {
                role: existing.role,
                private_key: existing.private_key,
                sender_key: existing.sender_key,
                auto_follow,
            },
        )?;
        Ok(())
    }

    /// Returns the member's direct role in this group, if present.
    pub fn role_of(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
    ) -> EyreResult<Option<GroupMemberRole>> {
        get_direct_member_role(self.store, group_id, identity)
    }

    pub fn member_value(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
    ) -> EyreResult<Option<GroupMemberValue>> {
        let handle = self.store.handle();
        let key = GroupMember::new(group_id.to_bytes(), *identity);
        Ok(handle.get(&key)?)
    }

    /// Returns the [`MembershipPath`] by which `identity` is a member of
    /// `group_id`, or `None` if they are not a member.
    ///
    /// Walk semantics and architectural caveats are documented at length
    /// on [`MembershipPath`] — same wording as the pre-#2303 free
    /// function this method replaces.
    pub fn check_path(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
    ) -> EyreResult<MembershipPath> {
        if has_direct_member(self.store, group_id, identity)? {
            return Ok(MembershipPath::Direct);
        }

        let mut anchor_decision: Option<MembershipPath> = None;
        let mut current = *group_id;
        for _ in 0..=MAX_NAMESPACE_DEPTH {
            if CapabilitiesRepository::new(self.store).subgroup_visibility(&current)?
                != VisibilityMode::Open
            {
                return Ok(anchor_decision.unwrap_or(MembershipPath::None));
            }
            let Some(parent) = NamespaceRepository::new(self.store).parent(&current)? else {
                return Ok(anchor_decision.unwrap_or(MembershipPath::None));
            };
            if self.is_admin(&parent, identity)? {
                return Ok(MembershipPath::Inherited {
                    anchor: parent,
                    via_admin: true,
                });
            }
            if has_direct_member(self.store, &parent, identity)? && anchor_decision.is_none() {
                let caps = CapabilitiesRepository::new(self.store)
                    .member_capability(&parent, identity)?
                    .unwrap_or(0);
                anchor_decision = Some(
                    if caps & MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits() != 0 {
                        MembershipPath::Inherited {
                            anchor: parent,
                            via_admin: false,
                        }
                    } else {
                        MembershipPath::None
                    },
                );
            }
            current = parent;
        }
        bail!(MembershipError::DepthExceeded(MAX_NAMESPACE_DEPTH))
    }

    /// Returns `true` if `identity` is a member of `group_id` either
    /// directly or by inheritance. Thin wrapper over [`Self::check_path`].
    pub fn is_member(&self, group_id: &ContextGroupId, identity: &PublicKey) -> EyreResult<bool> {
        Ok(!matches!(
            self.check_path(group_id, identity)?,
            MembershipPath::None
        ))
    }

    /// Returns the capability bitmask `identity` holds as an *effective*
    /// member of `group_id` — direct or inherited — or `None` when they
    /// are not a member at all. See the original `get_effective_member_capabilities`
    /// doc for the deny-list-asymmetry rationale.
    pub fn effective_capabilities(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
    ) -> EyreResult<Option<u32>> {
        match self.check_path(group_id, identity)? {
            MembershipPath::None => Ok(None),
            MembershipPath::Inherited { .. }
                if DenyListRepository::new(self.store).is_denied(group_id, identity)? =>
            {
                Ok(None)
            }
            MembershipPath::Direct | MembershipPath::Inherited { .. } => Ok(Some(
                CapabilitiesRepository::new(self.store)
                    .member_capability(group_id, identity)?
                    .unwrap_or(0),
            )),
        }
    }

    /// Enumerate the identities that are members of `group_id` purely by
    /// inheritance. See `enumerate_inherited_members` doc for full
    /// semantics — preserved verbatim from the pre-#2303 free function.
    pub fn enumerate_inherited(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<Vec<(PublicKey, GroupMemberRole)>> {
        let mut seen: BTreeSet<PublicKey> = self
            .list(group_id, 0, usize::MAX)?
            .into_iter()
            .map(|(pk, _)| pk)
            .collect();
        let mut result = Vec::new();

        let mut current = *group_id;
        let mut terminated = false;
        for _ in 0..=MAX_NAMESPACE_DEPTH {
            if CapabilitiesRepository::new(self.store).subgroup_visibility(&current)?
                != VisibilityMode::Open
            {
                terminated = true;
                break;
            }
            let Some(parent) = NamespaceRepository::new(self.store).parent(&current)? else {
                terminated = true;
                break;
            };

            let mut candidates: Vec<PublicKey> = self
                .list(&parent, 0, usize::MAX)?
                .into_iter()
                .map(|(pk, _)| pk)
                .collect();
            if let Some(meta) = MetaRepository::new(self.store).load(&parent)? {
                candidates.push(meta.admin_identity);
            }

            for candidate in candidates {
                if !seen.insert(candidate) {
                    continue;
                }
                if DenyListRepository::new(self.store).is_denied(group_id, &candidate)? {
                    continue;
                }
                if let MembershipPath::Inherited { anchor, via_admin } =
                    self.check_path(group_id, &candidate)?
                {
                    // For a non-admin inheritor, carry the member's REAL role
                    // from the anchor (the ancestor where they hold the direct
                    // row) instead of defaulting to `Member` — otherwise an
                    // inherited `ReadOnlyTee` would be reported as a plain
                    // `Member`. Fall back to `Member` only if the anchor row is
                    // unexpectedly absent.
                    let role = if via_admin {
                        GroupMemberRole::Admin
                    } else {
                        self.role_of(&anchor, &candidate)?
                            .unwrap_or(GroupMemberRole::Member)
                    };
                    result.push((candidate, role));
                }
            }

            current = parent;
        }
        if !terminated {
            bail!(MembershipError::DepthExceeded(MAX_NAMESPACE_DEPTH));
        }
        Ok(result)
    }

    /// Returns `true` if `identity` is a direct admin of this specific group
    /// (no ancestor walk).
    pub fn is_direct_admin(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
    ) -> EyreResult<bool> {
        match get_direct_member_role(self.store, group_id, identity)? {
            Some(GroupMemberRole::Admin) => Ok(true),
            _ => Ok(false),
        }
    }

    /// Returns `true` iff `identity` holds direct admin authority at *any*
    /// ancestor in the Open chain rooted at `group_id` (or at `group_id`
    /// itself). See the original `is_inherited_admin` doc for full walk
    /// semantics.
    pub fn is_inherited_admin(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
    ) -> EyreResult<bool> {
        if self.is_admin(group_id, identity)? {
            return Ok(true);
        }
        let mut current = *group_id;
        for _ in 0..=MAX_NAMESPACE_DEPTH {
            if CapabilitiesRepository::new(self.store).subgroup_visibility(&current)?
                != VisibilityMode::Open
            {
                return Ok(false);
            }
            let Some(parent) = NamespaceRepository::new(self.store).parent(&current)? else {
                return Ok(false);
            };
            if self.is_admin(&parent, identity)? {
                return Ok(true);
            }
            current = parent;
        }
        bail!(MembershipError::DepthExceeded(MAX_NAMESPACE_DEPTH))
    }

    pub fn is_admin(&self, group_id: &ContextGroupId, identity: &PublicKey) -> EyreResult<bool> {
        if let Some(GroupMemberRole::Admin) = self.role_of(group_id, identity)? {
            return Ok(true);
        }
        if let Some(meta) = MetaRepository::new(self.store).load(group_id)? {
            if meta.admin_identity == *identity {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn require_admin(&self, group_id: &ContextGroupId, identity: &PublicKey) -> EyreResult<()> {
        if !self.is_admin(group_id, identity)? {
            bail!(MembershipError::NotAdmin {
                group_id: format!("{group_id:?}"),
                identity: format!("{identity:?}"),
            });
        }
        Ok(())
    }

    /// Decide whether `child_group_id` should appear in a subgroup
    /// listing for `caller`. See original `subgroup_visible_to` doc.
    pub fn subgroup_visible_to(
        &self,
        parent_group_id: &ContextGroupId,
        child_group_id: &ContextGroupId,
        caller: Option<&PublicKey>,
    ) -> EyreResult<bool> {
        if CapabilitiesRepository::new(self.store).subgroup_visibility(child_group_id)?
            == VisibilityMode::Open
        {
            return Ok(true);
        }
        let Some(caller_pk) = caller else {
            return Ok(false);
        };
        if self.is_inherited_admin(parent_group_id, caller_pk)? {
            return Ok(true);
        }
        self.is_member(child_group_id, caller_pk)
    }

    /// Returns `true` if `identity` is a group admin **or** holds the
    /// given capability bit. Admins always pass regardless of caps.
    pub fn is_admin_or_has_capability(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
        capability_bit: u32,
    ) -> EyreResult<bool> {
        if self.is_admin(group_id, identity)? {
            return Ok(true);
        }
        let caps = CapabilitiesRepository::new(self.store)
            .member_capability(group_id, identity)?
            .unwrap_or(0);
        Ok(caps & capability_bit != 0)
    }

    pub fn require_admin_or_capability(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
        capability_bit: u32,
        operation: &str,
    ) -> EyreResult<()> {
        if !self.is_admin_or_has_capability(group_id, identity, capability_bit)? {
            bail!(CapabilitiesError::Unauthorized {
                group_id: format!("{group_id:?}"),
                operation: operation.to_owned(),
            });
        }
        Ok(())
    }

    pub fn count_admins(&self, group_id: &ContextGroupId) -> EyreResult<usize> {
        let gid = group_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupMember::new(gid, [0u8; 32].into()),
            GROUP_MEMBER_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let handle = self.store.handle();
        let mut count = 0usize;
        for key in keys {
            let val: GroupMemberValue =
                handle
                    .get(&key)?
                    .ok_or_else(|| MembershipError::MissingMemberValue {
                        group_id: format!("{group_id:?}"),
                        identity: format!("{:?}", key.identity()),
                    })?;
            if val.role == GroupMemberRole::Admin {
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn list(
        &self,
        group_id: &ContextGroupId,
        offset: usize,
        limit: usize,
    ) -> EyreResult<Vec<(PublicKey, GroupMemberRole)>> {
        let gid = group_id.to_bytes();
        let keys = collect_keys_with_prefix_paginated(
            self.store,
            GroupMember::new(gid, [0u8; 32].into()),
            GROUP_MEMBER_PREFIX,
            |k| k.group_id() == gid,
            offset,
            limit,
        )?;
        let handle = self.store.handle();
        let mut results = Vec::new();
        for key in keys {
            let val: GroupMemberValue =
                handle
                    .get(&key)?
                    .ok_or_else(|| MembershipError::MissingMemberValue {
                        group_id: format!("{group_id:?}"),
                        identity: format!("{:?}", key.identity()),
                    })?;
            results.push((key.identity(), val.role));
        }
        Ok(results)
    }

    /// Public-key-only view of the current member set for a namespace.
    /// Includes the meta `admin_identity` if not already in the member
    /// rows — see the original `namespace_member_pubkeys` doc.
    pub fn namespace_pubkeys(&self, namespace_id: NamespaceId) -> EyreResult<Vec<PublicKey>> {
        let group_id = ContextGroupId::from(namespace_id.to_bytes());
        let members = self.list(&group_id, 0, usize::MAX)?;
        let mut pubkeys: Vec<PublicKey> = members.into_iter().map(|(pk, _role)| pk).collect();
        if let Some(meta) = MetaRepository::new(self.store).load(&group_id)? {
            if !pubkeys.contains(&meta.admin_identity) {
                pubkeys.push(meta.admin_identity);
            }
        }
        Ok(pubkeys)
    }

    /// Enumerate the trusted-anchor set: `{Owner} ∪ {Admins} ∪ {ReadOnlyTee}`.
    /// See original `trusted_anchors_for_group` doc.
    pub fn trusted_anchors(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<std::collections::BTreeSet<PublicKey>> {
        let mut anchors = std::collections::BTreeSet::new();
        if let Some(meta) = MetaRepository::new(self.store).load(group_id)? {
            let _ = anchors.insert(meta.owner_identity);
            let _ = anchors.insert(meta.admin_identity);
        }
        for (pk, role) in self.list(group_id, 0, usize::MAX)? {
            match role {
                GroupMemberRole::Admin | GroupMemberRole::ReadOnlyTee => {
                    let _ = anchors.insert(pk);
                }
                GroupMemberRole::Member | GroupMemberRole::ReadOnly => {}
            }
        }
        Ok(anchors)
    }

    /// True if `identity` is the namespace owner, an admin, or an
    /// admitted TEE node. See original `is_authoritative_namespace_identity`.
    pub fn is_authoritative_namespace_identity(
        &self,
        namespace_id: NamespaceId,
        identity: &PublicKey,
    ) -> EyreResult<bool> {
        let gid = ContextGroupId::from(namespace_id.to_bytes());

        if let Some(meta) = MetaRepository::new(self.store).load(&gid)? {
            if *identity == meta.owner_identity {
                return Ok(true);
            }
        }

        if self.is_admin(&gid, identity)? {
            return Ok(true);
        }

        super::super::tee::is_tee_admitted_identity(self.store, &gid, identity)
    }

    pub fn count(&self, group_id: &ContextGroupId) -> EyreResult<usize> {
        let gid = group_id.to_bytes();
        count_keys_with_prefix(
            self.store,
            GroupMember::new(gid, [0u8; 32].into()),
            GROUP_MEMBER_PREFIX,
            |k| k.group_id() == gid,
        )
    }

    /// Returns `true` iff `identity` has a **direct** membership row in
    /// `group_id` — never walks the parent chain. Use this when the
    /// caller's intent is "would I be creating a duplicate direct row?".
    pub fn has_direct_member(
        &self,
        group_id: &ContextGroupId,
        identity: &PublicKey,
    ) -> EyreResult<bool> {
        has_direct_member(self.store, group_id, identity)
    }
}

/// How a positive membership decision was reached. See
/// [`MembershipRepository::check_path`] for full walk semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MembershipPath {
    /// Identity is not a member of the subgroup, directly or by inheritance.
    None,
    /// Identity has a direct membership row in the subgroup.
    ///
    /// **Caution:** `Direct` does *not* imply the identity lacks
    /// inherited admin authority. A parent admin who is also added as a
    /// regular subgroup member will appear as `Direct` here while still
    /// holding inherited admin authority — callers needing that must
    /// call [`MembershipRepository::is_inherited_admin`] separately.
    Direct,
    /// Identity inherits membership from the closest ancestor where they
    /// hold a direct row (`anchor`). `via_admin` is `true` when the
    /// inheritance came from an admin grant; `false` when it came from
    /// `CAN_JOIN_OPEN_SUBGROUPS`.
    Inherited {
        anchor: ContextGroupId,
        via_admin: bool,
    },
}

fn has_direct_member(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    let handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    Ok(handle.has(&key)?)
}

fn get_direct_member_role(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<Option<GroupMemberRole>> {
    let handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    Ok(handle.get(&key)?.map(|v: GroupMemberValue| v.role))
}

/// Repository-API smoke tests. Membership-feature coverage (path walks,
/// inheritance, deny-list interaction, anchor caps, namespace pubkeys)
/// lives in the cluster-level `membership/tests.rs`; this module is
/// just a thin set of "the Repository surface dispatches correctly"
/// checks so the API contract is self-documented next to the
/// implementation.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{test_group_id, test_meta, test_store};
    use crate::MetaRepository;

    #[test]
    fn role_of_returns_none_when_not_a_member() {
        let store = test_store();
        let repo = MembershipRepository::new(&store);
        let pk = PublicKey::from([0x01; 32]);
        assert!(repo.role_of(&test_group_id(), &pk).unwrap().is_none());
    }

    #[test]
    fn add_then_role_of_round_trip() {
        let store = test_store();
        let repo = MembershipRepository::new(&store);
        let gid = test_group_id();
        let pk = PublicKey::from([0x01; 32]);

        repo.add_member(&gid, &pk, GroupMemberRole::Admin).unwrap();
        assert_eq!(
            repo.role_of(&gid, &pk).unwrap(),
            Some(GroupMemberRole::Admin)
        );
    }

    #[test]
    fn remove_member_clears_the_row() {
        let store = test_store();
        let repo = MembershipRepository::new(&store);
        let gid = test_group_id();
        let pk = PublicKey::from([0x01; 32]);

        repo.add_member(&gid, &pk, GroupMemberRole::Member).unwrap();
        repo.remove_member(&gid, &pk).unwrap();
        assert!(repo.role_of(&gid, &pk).unwrap().is_none());
    }

    #[test]
    fn is_admin_recognises_meta_admin_identity() {
        let store = test_store();
        let mut meta = test_meta();
        meta.admin_identity = PublicKey::from([0xAA; 32]);
        MetaRepository::new(&store)
            .save(&test_group_id(), &meta)
            .unwrap();

        let repo = MembershipRepository::new(&store);
        // The meta `admin_identity` is admin even without a stored member row.
        assert!(repo
            .is_admin(&test_group_id(), &meta.admin_identity)
            .unwrap());
    }

    #[test]
    fn count_admins_counts_admin_rows_only() {
        let store = test_store();
        let repo = MembershipRepository::new(&store);
        let gid = test_group_id();
        let admin_a = PublicKey::from([0x01; 32]);
        let admin_b = PublicKey::from([0x02; 32]);
        let member = PublicKey::from([0x03; 32]);

        repo.add_member(&gid, &admin_a, GroupMemberRole::Admin)
            .unwrap();
        repo.add_member(&gid, &admin_b, GroupMemberRole::Admin)
            .unwrap();
        repo.add_member(&gid, &member, GroupMemberRole::Member)
            .unwrap();

        assert_eq!(repo.count_admins(&gid).unwrap(), 2);
        assert_eq!(repo.count(&gid).unwrap(), 3);
    }

    #[test]
    fn list_paginates() {
        let store = test_store();
        let repo = MembershipRepository::new(&store);
        let gid = test_group_id();
        for i in 0..5 {
            let pk = PublicKey::from([i as u8; 32]);
            repo.add_member(&gid, &pk, GroupMemberRole::Member).unwrap();
        }
        let page_1 = repo.list(&gid, 0, 2).unwrap();
        let page_2 = repo.list(&gid, 2, 2).unwrap();
        assert_eq!(page_1.len(), 2);
        assert_eq!(page_2.len(), 2);
        assert_eq!(repo.list(&gid, 0, usize::MAX).unwrap().len(), 5);
    }
}
