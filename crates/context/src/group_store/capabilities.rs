use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{
    GroupContextMemberCap, GroupDefaultCaps, GroupDefaultCapsValue, GroupDefaultVis,
    GroupDefaultVisValue, GroupMemberCapability, GroupMemberCapabilityValue,
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

pub fn get_default_visibility(store: &Store, group_id: &ContextGroupId) -> EyreResult<Option<u8>> {
    let handle = store.handle();
    let key = GroupDefaultVis::new(group_id.to_bytes());
    let value = handle.get(&key)?;
    Ok(value.map(|v| v.mode))
}

pub fn set_default_visibility(
    store: &Store,
    group_id: &ContextGroupId,
    mode: u8,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupDefaultVis::new(group_id.to_bytes());
    handle.put(&key, &GroupDefaultVisValue { mode })?;
    Ok(())
}

pub fn delete_default_capabilities(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupDefaultCaps::new(group_id.to_bytes()))?;
    Ok(())
}

pub fn delete_default_visibility(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupDefaultVis::new(group_id.to_bytes()))?;
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
