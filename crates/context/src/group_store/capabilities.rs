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

    pub fn set_default_capabilities(
        &self,
        group_id: &ContextGroupId,
        caps: u32,
    ) -> EyreResult<()> {
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
    pub fn subgroup_visibility(
        &self,
        group_id: &ContextGroupId,
    ) -> EyreResult<VisibilityMode> {
        let handle = self.store.handle();
        let key = GroupSubgroupVis::new(group_id.to_bytes());
        let value = handle.get(&key)?;
        Ok(match value.map(|v| v.mode) {
            Some(0) => VisibilityMode::Open,
            _ => VisibilityMode::Restricted,
        })
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
            let Some(parent) = super::namespace::get_parent_group(self.store, &current)? else {
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

// ---------------------------------------------------------------------------
// Deprecated free-function wrappers.
// ---------------------------------------------------------------------------

#[deprecated(note = "use CapabilitiesRepository::new(store).member_capability(...)")]
pub fn get_member_capability(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Option<u32>> {
    CapabilitiesRepository::new(store).member_capability(group_id, member)
}

#[deprecated(note = "use CapabilitiesRepository::new(store).set_member_capability(...)")]
pub fn set_member_capability(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
    caps: u32,
) -> EyreResult<()> {
    CapabilitiesRepository::new(store).set_member_capability(group_id, member, caps)
}

#[deprecated(note = "use CapabilitiesRepository::new(store).enumerate_members(...)")]
pub fn enumerate_member_capabilities(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(PublicKey, u32)>> {
    CapabilitiesRepository::new(store).enumerate_members(group_id)
}

#[deprecated(note = "use CapabilitiesRepository::new(store).default_capabilities(...)")]
pub fn get_default_capabilities(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<u32>> {
    CapabilitiesRepository::new(store).default_capabilities(group_id)
}

#[deprecated(note = "use CapabilitiesRepository::new(store).set_default_capabilities(...)")]
pub fn set_default_capabilities(
    store: &Store,
    group_id: &ContextGroupId,
    caps: u32,
) -> EyreResult<()> {
    CapabilitiesRepository::new(store).set_default_capabilities(group_id, caps)
}

#[deprecated(note = "use CapabilitiesRepository::new(store).subgroup_visibility(...)")]
pub fn get_subgroup_visibility(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<VisibilityMode> {
    CapabilitiesRepository::new(store).subgroup_visibility(group_id)
}

#[deprecated(note = "use CapabilitiesRepository::new(store).is_open_chain_to_namespace(...)")]
pub fn is_open_chain_to_namespace(
    store: &Store,
    group_id: &ContextGroupId,
    namespace_id: &ContextGroupId,
) -> EyreResult<bool> {
    CapabilitiesRepository::new(store).is_open_chain_to_namespace(group_id, namespace_id)
}

#[deprecated(note = "use CapabilitiesRepository::new(store).set_subgroup_visibility(...)")]
pub fn set_subgroup_visibility(
    store: &Store,
    group_id: &ContextGroupId,
    mode: VisibilityMode,
) -> EyreResult<()> {
    CapabilitiesRepository::new(store).set_subgroup_visibility(group_id, mode)
}

#[deprecated(note = "use CapabilitiesRepository::new(store).delete_default(...)")]
pub fn delete_default_capabilities(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    CapabilitiesRepository::new(store).delete_default(group_id)
}

#[deprecated(note = "use CapabilitiesRepository::new(store).delete_subgroup_visibility(...)")]
pub fn delete_subgroup_visibility(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    CapabilitiesRepository::new(store).delete_subgroup_visibility(group_id)
}

#[deprecated(note = "use CapabilitiesRepository::new(store).delete_all_member_caps(...)")]
pub fn delete_all_member_capabilities(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    CapabilitiesRepository::new(store).delete_all_member_caps(group_id)
}

#[deprecated(note = "use CapabilitiesRepository::new(store).set_context_member(...)")]
pub fn set_context_member_capability(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    member: &PublicKey,
    capabilities: u8,
) -> EyreResult<()> {
    CapabilitiesRepository::new(store).set_context_member(group_id, context_id, member, capabilities)
}

#[deprecated(note = "use CapabilitiesRepository::new(store).context_member_capability(...)")]
pub fn get_context_member_capability(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    member: &PublicKey,
) -> EyreResult<Option<u8>> {
    CapabilitiesRepository::new(store).context_member_capability(group_id, context_id, member)
}
