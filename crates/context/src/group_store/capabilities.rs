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
use eyre::Result as EyreResult;

use super::collect_keys_with_prefix;

pub fn get_member_capability(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Option<u32>> {
    let handle = store.handle();
    let key = GroupMemberCapability::new(group_id.to_bytes(), *member);
    let value = handle.get(&key)?;
    Ok(value.map(|v| v.capabilities))
}

pub fn set_member_capability(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
    caps: u32,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMemberCapability::new(group_id.to_bytes(), *member);
    handle.put(&key, &GroupMemberCapabilityValue { capabilities: caps })?;
    Ok(())
}

pub fn enumerate_member_capabilities(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(PublicKey, u32)>> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupMemberCapability::new(gid, PublicKey::from([0u8; 32])),
        GROUP_MEMBER_CAPABILITY_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let handle = store.handle();
    let mut results = Vec::new();
    for key in keys {
        let Some(val) = handle.get(&key)? else {
            continue;
        };
        results.push((PublicKey::from(*key.identity()), val.capabilities));
    }
    Ok(results)
}

pub fn get_default_capabilities(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<u32>> {
    let handle = store.handle();
    let key = GroupDefaultCaps::new(group_id.to_bytes());
    let value = handle.get(&key)?;
    Ok(value.map(|v| v.capabilities))
}

pub fn set_default_capabilities(
    store: &Store,
    group_id: &ContextGroupId,
    caps: u32,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupDefaultCaps::new(group_id.to_bytes());
    handle.put(&key, &GroupDefaultCapsValue { capabilities: caps })?;
    Ok(())
}

/// Read the subgroup visibility setting for `group_id`.
///
/// An absent key is treated as [`VisibilityMode::Restricted`] — the safer
/// default. Membership inheritance via [`super::check_group_membership`]
/// only walks parents when the subgroup is `Open`.
pub fn get_subgroup_visibility(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<VisibilityMode> {
    let handle = store.handle();
    let key = GroupSubgroupVis::new(group_id.to_bytes());
    let value = handle.get(&key)?;
    Ok(match value.map(|v| v.mode) {
        Some(0) => VisibilityMode::Open,
        _ => VisibilityMode::Restricted,
    })
}

/// Returns `true` iff the chain `group_id → ... → namespace_id` consists
/// entirely of `Open` subgroups — i.e. there is no `Restricted` ancestor
/// between `group_id` and the namespace root.
///
/// This is the correct gate for **encryption-key selection** under the
/// Option-C alignment (issue #2256): a subgroup whose access boundary is
/// effectively namespace-wide must be `Open` *all the way up*. If any
/// ancestor is `Restricted`, the membership walk in
/// [`super::check_group_membership_path`] would terminate at that wall
/// and refuse inheritance — so encrypting with the namespace key would
/// open a confidentiality gap (all namespace members could decrypt
/// content for a subgroup nobody can actually join via inheritance).
///
/// Returns `false` if `group_id == namespace_id` (the namespace itself
/// has no parent and does not participate in subgroup inheritance), if
/// any ancestor is `Restricted`, if the parent chain doesn't reach
/// `namespace_id`, or if the walk exceeds [`super::namespace::MAX_NAMESPACE_DEPTH`].
pub fn is_open_chain_to_namespace(
    store: &Store,
    group_id: &ContextGroupId,
    namespace_id: &ContextGroupId,
) -> EyreResult<bool> {
    if group_id == namespace_id {
        return Ok(false);
    }
    let mut current = *group_id;
    for _ in 0..super::namespace::MAX_NAMESPACE_DEPTH {
        if get_subgroup_visibility(store, &current)? != VisibilityMode::Open {
            return Ok(false);
        }
        let Some(parent) = super::namespace::get_parent_group(store, &current)? else {
            return Ok(false);
        };
        if &parent == namespace_id {
            return Ok(true);
        }
        current = parent;
    }
    Ok(false)
}

pub fn set_subgroup_visibility(
    store: &Store,
    group_id: &ContextGroupId,
    mode: VisibilityMode,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupSubgroupVis::new(group_id.to_bytes());
    let mode_byte = match mode {
        VisibilityMode::Open => 0u8,
        VisibilityMode::Restricted => 1u8,
    };
    handle.put(&key, &GroupSubgroupVisValue { mode: mode_byte })?;
    Ok(())
}

pub fn delete_default_capabilities(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupDefaultCaps::new(group_id.to_bytes()))?;
    Ok(())
}

pub fn delete_subgroup_visibility(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupSubgroupVis::new(group_id.to_bytes()))?;
    Ok(())
}

pub fn delete_all_member_capabilities(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupMemberCapability::new(gid, PublicKey::from([0u8; 32])),
        GROUP_MEMBER_CAPABILITY_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let mut handle = store.handle();
    for key in keys {
        handle.delete(&key)?;
    }
    Ok(())
}

pub fn set_context_member_capability(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    member: &PublicKey,
    capabilities: u8,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupContextMemberCap::new(group_id.to_bytes(), *context_id, *member);
    handle.put(&key, &capabilities)?;
    Ok(())
}

pub fn get_context_member_capability(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    member: &PublicKey,
) -> EyreResult<Option<u8>> {
    let handle = store.handle();
    let key = GroupContextMemberCap::new(group_id.to_bytes(), *context_id, *member);
    Ok(handle.get(&key)?)
}
