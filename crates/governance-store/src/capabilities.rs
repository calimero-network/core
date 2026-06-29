use crate::NamespaceRepository;
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::VisibilityMode;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{
    GroupContextMemberCap, GroupDefaultCaps, GroupDefaultCapsValue, GroupMemberCapability,
    GroupMemberCapabilityValue, GroupSubgroupVis, GroupSubgroupVisValue,
    GROUP_MEMBER_CAPABILITY_PREFIX,
};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::collect_keys_with_prefix;

/// Typed Repository for member capabilities, default capabilities,
/// per-context member caps, and subgroup visibility.
///
/// Combines three closely-related concerns under one Repository
/// because they're all "what's the policy at this group?" data and
/// they're cross-referenced by membership and capability checks
/// (e.g. `is_open_chain_to_namespace` walks parents reading
/// visibility on each).
///
/// Issue #2303 / epic #2300.
pub struct CapabilitiesRepository<'a> {
    store: &'a Store,
}

impl<'a> CapabilitiesRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn member_capability(
        &self,
        group_id: &ContextGroupId,
        member: &PublicKey,
    ) -> EyreResult<Option<u32>> {
        let handle = self.store.handle();
        let key = GroupMemberCapability::new(group_id.to_bytes(), *member);
        let value = handle.get(&key)?;
        Ok(value.map(|v| v.capabilities))
    }

    pub fn set_member_capability(
        &self,
        group_id: &ContextGroupId,
        member: &PublicKey,
        caps: u32,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = GroupMemberCapability::new(group_id.to_bytes(), *member);
        handle.put(&key, &GroupMemberCapabilityValue { capabilities: caps })?;
        Ok(())
    }

    pub fn enumerate_members(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<Vec<(PublicKey, u32)>> {
        let gid = group_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupMemberCapability::new(gid, PublicKey::from([0u8; 32])),
            GROUP_MEMBER_CAPABILITY_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let handle = self.store.handle();
        let mut results = Vec::new();
        for key in keys {
            let Some(val) = handle.get(&key)? else {
                continue;
            };
            results.push((PublicKey::from(*key.identity()), val.capabilities));
        }
        Ok(results)
    }

    pub fn default_capabilities(&self, group_id: &ContextGroupId) -> EyreResult<Option<u32>> {
        let handle = self.store.handle();
        let key = GroupDefaultCaps::new(group_id.to_bytes());
        let value = handle.get(&key)?;
        Ok(value.map(|v| v.capabilities))
    }

    pub fn set_default_capabilities(&self, group_id: &ContextGroupId, caps: u32) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = GroupDefaultCaps::new(group_id.to_bytes());
        handle.put(&key, &GroupDefaultCapsValue { capabilities: caps })?;
        Ok(())
    }

    /// Read the subgroup visibility setting for `group_id`.
    ///
    /// An absent key is treated as [`VisibilityMode::Restricted`] — the
    /// safer default. Membership inheritance via
    /// [`super::check_group_membership`] only walks parents when the
    /// subgroup is `Open`.
    pub fn subgroup_visibility(&self, group_id: &ContextGroupId) -> EyreResult<VisibilityMode> {
        let handle = self.store.handle();
        let key = GroupSubgroupVis::new(group_id.to_bytes());
        let value = handle.get(&key)?;
        Ok(match value.map(|v| v.mode) {
            Some(0) => VisibilityMode::Open,
            _ => VisibilityMode::Restricted,
        })
    }

    /// Whether an explicit subgroup-visibility key exists in the store for
    /// `group_id`. Distinct from [`Self::subgroup_visibility`], which collapses
    /// an absent key to [`VisibilityMode::Restricted`] and so cannot tell
    /// "never written" apart from "explicitly Restricted".
    ///
    /// Used by `GroupCreated` apply to gate the birth-visibility write to the
    /// genuine first create: the key is absent on the originator's first apply
    /// (the `create_group` handler pre-populates meta but NOT visibility) and
    /// present on any replay — so birth visibility is written once and never
    /// re-asserted over a later `SubgroupVisibilitySet` flip.
    pub fn has_subgroup_visibility(&self, group_id: &ContextGroupId) -> EyreResult<bool> {
        let handle = self.store.handle();
        let key = GroupSubgroupVis::new(group_id.to_bytes());
        Ok(handle.get(&key)?.is_some())
    }

    pub fn set_subgroup_visibility(
        &self,
        group_id: &ContextGroupId,
        mode: VisibilityMode,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = GroupSubgroupVis::new(group_id.to_bytes());
        let mode_byte = match mode {
            VisibilityMode::Open => 0u8,
            VisibilityMode::Restricted => 1u8,
        };
        handle.put(&key, &GroupSubgroupVisValue { mode: mode_byte })?;
        Ok(())
    }

    /// Returns `true` iff the chain `group_id → ... → namespace_id` consists
    /// entirely of `Open` subgroups — i.e. there is no `Restricted` ancestor
    /// between `group_id` and the namespace root.
    ///
    /// See the module-level rationale; same depth-bound contract as
    /// [`super::membership::check_group_membership_path`].
    pub fn is_open_chain_to_namespace(
        &self,
        group_id: &ContextGroupId,
        namespace_id: &ContextGroupId,
    ) -> EyreResult<bool> {
        if group_id == namespace_id {
            return Ok(false);
        }
        let mut current = *group_id;
        for _ in 0..super::namespace::MAX_NAMESPACE_DEPTH {
            if self.subgroup_visibility(&current)? != VisibilityMode::Open {
                return Ok(false);
            }
            let Some(parent) = NamespaceRepository::new(self.store).parent(&current)? else {
                return Ok(false);
            };
            if &parent == namespace_id {
                return Ok(true);
            }
            current = parent;
        }
        bail!(
            "is_open_chain_to_namespace exceeded MAX_NAMESPACE_DEPTH ({}); \
             possible cycle in store",
            super::namespace::MAX_NAMESPACE_DEPTH
        )
    }

    pub fn delete_default(&self, group_id: &ContextGroupId) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.delete(&GroupDefaultCaps::new(group_id.to_bytes()))?;
        Ok(())
    }

    pub fn delete_subgroup_visibility(&self, group_id: &ContextGroupId) -> EyreResult<()> {
        let mut handle = self.store.handle();
        handle.delete(&GroupSubgroupVis::new(group_id.to_bytes()))?;
        Ok(())
    }

    pub fn delete_all_member_caps(&self, group_id: &ContextGroupId) -> EyreResult<()> {
        let gid = group_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupMemberCapability::new(gid, PublicKey::from([0u8; 32])),
            GROUP_MEMBER_CAPABILITY_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let mut handle = self.store.handle();
        for key in keys {
            handle.delete(&key)?;
        }
        Ok(())
    }

    pub fn set_context_member(
        &self,
        group_id: &ContextGroupId,
        context_id: &ContextId,
        member: &PublicKey,
        capabilities: u8,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = GroupContextMemberCap::new(group_id.to_bytes(), *context_id, *member);
        handle.put(&key, &capabilities)?;
        Ok(())
    }

    pub fn context_member_capability(
        &self,
        group_id: &ContextGroupId,
        context_id: &ContextId,
        member: &PublicKey,
    ) -> EyreResult<Option<u8>> {
        let handle = self.store.handle();
        let key = GroupContextMemberCap::new(group_id.to_bytes(), *context_id, *member);
        Ok(handle.get(&key)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{test_group_id, test_store};

    #[test]
    fn member_capability_returns_none_when_unset() {
        let store = test_store();
        let repo = CapabilitiesRepository::new(&store);
        let pk = PublicKey::from([0x01; 32]);
        assert!(repo
            .member_capability(&test_group_id(), &pk)
            .unwrap()
            .is_none());
    }

    #[test]
    fn set_then_get_member_capability_round_trip() {
        let store = test_store();
        let repo = CapabilitiesRepository::new(&store);
        let gid = test_group_id();
        let pk = PublicKey::from([0x01; 32]);

        repo.set_member_capability(&gid, &pk, 0b1010_0101).unwrap();
        assert_eq!(
            repo.member_capability(&gid, &pk).unwrap(),
            Some(0b1010_0101)
        );
    }

    #[test]
    fn default_capabilities_round_trip() {
        let store = test_store();
        let repo = CapabilitiesRepository::new(&store);
        let gid = test_group_id();

        assert!(repo.default_capabilities(&gid).unwrap().is_none());
        repo.set_default_capabilities(&gid, 0xFF).unwrap();
        assert_eq!(repo.default_capabilities(&gid).unwrap(), Some(0xFF));
    }

    #[test]
    fn subgroup_visibility_defaults_to_restricted() {
        let store = test_store();
        let repo = CapabilitiesRepository::new(&store);
        // An absent visibility key MUST be treated as Restricted — that's the
        // safer default and the membership-walk's wall semantic depends on it.
        assert_eq!(
            repo.subgroup_visibility(&test_group_id()).unwrap(),
            VisibilityMode::Restricted,
        );
    }

    #[test]
    fn set_then_get_subgroup_visibility_round_trip() {
        let store = test_store();
        let repo = CapabilitiesRepository::new(&store);
        let gid = test_group_id();

        repo.set_subgroup_visibility(&gid, VisibilityMode::Open)
            .unwrap();
        assert_eq!(
            repo.subgroup_visibility(&gid).unwrap(),
            VisibilityMode::Open
        );
        repo.set_subgroup_visibility(&gid, VisibilityMode::Restricted)
            .unwrap();
        assert_eq!(
            repo.subgroup_visibility(&gid).unwrap(),
            VisibilityMode::Restricted
        );
    }

    #[test]
    fn enumerate_members_returns_set_caps() {
        let store = test_store();
        let repo = CapabilitiesRepository::new(&store);
        let gid = test_group_id();
        let pk_a = PublicKey::from([0x01; 32]);
        let pk_b = PublicKey::from([0x02; 32]);

        repo.set_member_capability(&gid, &pk_a, 1).unwrap();
        repo.set_member_capability(&gid, &pk_b, 2).unwrap();

        let members = repo.enumerate_members(&gid).unwrap();
        assert_eq!(members.len(), 2);
        assert!(members.iter().any(|(pk, c)| *pk == pk_a && *c == 1));
        assert!(members.iter().any(|(pk, c)| *pk == pk_b && *c == 2));
    }
}
