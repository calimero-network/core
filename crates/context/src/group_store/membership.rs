use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{AutoFollowFlags, GroupMember, GroupMemberValue, GROUP_MEMBER_PREFIX};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::{
    collect_keys_with_prefix, collect_keys_with_prefix_paginated, count_keys_with_prefix,
    get_member_capability, load_group_meta, set_member_capability, GroupStoreError,
};

pub fn add_group_member(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
    role: GroupMemberRole,
) -> EyreResult<()> {
    add_group_member_with_keys(store, group_id, identity, role, None, None)
}

/// Bulk-delete every `GroupMember` record for `group_id`.
/// Used by cascade-delete; mirrors `delete_all_group_signing_keys`.
pub fn delete_all_group_members(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<()> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupMember::new(gid, PublicKey::from([0u8; 32]).into()),
        GROUP_MEMBER_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let mut handle = store.handle();
    for key in keys {
        handle.delete(&key)?;
    }
    Ok(())
}

pub fn add_group_member_with_keys(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
    role: GroupMemberRole,
    private_key: Option<[u8; 32]>,
    sender_key: Option<[u8; 32]>,
) -> EyreResult<()> {
    let is_admin = role == GroupMemberRole::Admin;
    let mut handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    // Preserve auto_follow across updates — add_group_member is used for
    // upserts (e.g. MemberRoleSet), and users will have set their
    // auto-follow flags independently of role changes.
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
        if let Some(defaults) = get_member_default_capabilities(store, group_id)? {
            if defaults != 0 {
                set_member_capability(store, group_id, identity, defaults)?;
            }
        }
    }

    Ok(())
}

pub fn remove_group_member(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    handle.delete(&key)?;
    Ok(())
}

/// Update the auto-follow flags for an existing member. Caller must
/// have already verified the member exists (this function bails if
/// not) and that the signer is authorized to mutate them.
pub fn set_member_auto_follow(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
    auto_follow: AutoFollowFlags,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    let existing = handle
        .get(&key)?
        .ok_or_else(|| eyre::eyre!("member not found in group"))?;
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
pub fn get_group_member_role(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<Option<GroupMemberRole>> {
    get_direct_member_role(store, group_id, identity)
}

pub fn get_group_member_value(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<Option<GroupMemberValue>> {
    let handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    Ok(handle.get(&key)?)
}

pub fn check_group_membership(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    has_direct_member(store, group_id, identity)
}

/// Returns `true` if `identity` is a direct admin of this specific group
/// (no ancestor walk). Used for operations where inherited admin authority
/// should NOT apply (e.g., managing Restricted context allowlists).
pub fn is_direct_group_admin(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    match get_direct_member_role(store, group_id, identity)? {
        Some(GroupMemberRole::Admin) => Ok(true),
        _ => Ok(false),
    }
}

pub fn is_group_admin(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    if let Some(GroupMemberRole::Admin) = get_group_member_role(store, group_id, identity)? {
        return Ok(true);
    }
    if let Some(meta) = load_group_meta(store, group_id)? {
        if meta.admin_identity == *identity {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn require_group_admin(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<()> {
    if !is_group_admin(store, group_id, identity)? {
        bail!(GroupStoreError::NotAdmin {
            group_id: format!("{group_id:?}"),
            identity: format!("{identity:?}"),
        });
    }
    Ok(())
}

/// Returns `true` if `identity` is a group admin **or** holds the given capability bit.
/// Admins always pass regardless of capability bits.
pub fn is_group_admin_or_has_capability(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
    capability_bit: u32,
) -> EyreResult<bool> {
    if is_group_admin(store, group_id, identity)? {
        return Ok(true);
    }
    let caps = get_member_capability(store, group_id, identity)?.unwrap_or(0);
    Ok(caps & capability_bit != 0)
}

/// Enforces that `identity` is a group admin or holds the given capability bit.
pub fn require_group_admin_or_capability(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
    capability_bit: u32,
    operation: &str,
) -> EyreResult<()> {
    if !is_group_admin_or_has_capability(store, group_id, identity, capability_bit)? {
        bail!(GroupStoreError::Unauthorized {
            group_id: format!("{group_id:?}"),
            operation: operation.to_owned(),
        });
    }
    Ok(())
}

pub fn count_group_admins(store: &Store, group_id: &ContextGroupId) -> EyreResult<usize> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupMember::new(gid, [0u8; 32].into()),
        GROUP_MEMBER_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let handle = store.handle();
    let mut count = 0usize;
    for key in keys {
        let val: GroupMemberValue = handle
            .get(&key)?
            .ok_or_else(|| eyre::eyre!("member key exists but value is missing"))?;
        if val.role == GroupMemberRole::Admin {
            count += 1;
        }
    }
    Ok(count)
}

pub fn list_group_members(
    store: &Store,
    group_id: &ContextGroupId,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<(PublicKey, GroupMemberRole)>> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix_paginated(
        store,
        GroupMember::new(gid, [0u8; 32].into()),
        GROUP_MEMBER_PREFIX,
        |k| k.group_id() == gid,
        offset,
        limit,
    )?;
    let handle = store.handle();
    let mut results = Vec::new();
    for key in keys {
        let val: GroupMemberValue = handle
            .get(&key)?
            .ok_or_else(|| eyre::eyre!("member key exists but value is missing"))?;
        results.push((key.identity(), val.role));
    }
    Ok(results)
}

pub fn count_group_members(store: &Store, group_id: &ContextGroupId) -> EyreResult<usize> {
    let gid = group_id.to_bytes();
    count_keys_with_prefix(
        store,
        GroupMember::new(gid, [0u8; 32].into()),
        GROUP_MEMBER_PREFIX,
        |k| k.group_id() == gid,
    )
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

fn get_member_default_capabilities(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<u32>> {
    super::get_default_capabilities(store, group_id)
}
