use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{
    AsKeyParts, ContextGroupRef, GroupContextIndex, GroupMember, GroupMeta, GroupMetaValue,
    GroupUpgradeKey, GroupUpgradeStatus, GroupUpgradeValue, GROUP_CONTEXT_INDEX_PREFIX,
    GROUP_MEMBER_PREFIX, GROUP_UPGRADE_PREFIX,
};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

// ---------------------------------------------------------------------------
// Group meta helpers
// ---------------------------------------------------------------------------

pub fn load_group_meta(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<GroupMetaValue>> {
    let handle = store.handle();
    let key = GroupMeta::new(group_id.to_bytes());
    let value = handle.get(&key)?;
    Ok(value)
}

pub fn save_group_meta(
    store: &Store,
    group_id: &ContextGroupId,
    meta: &GroupMetaValue,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMeta::new(group_id.to_bytes());
    handle.put(&key, meta)?;
    Ok(())
}

pub fn delete_group_meta(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMeta::new(group_id.to_bytes());
    handle.delete(&key)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Group member helpers
// ---------------------------------------------------------------------------

pub fn add_group_member(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
    role: GroupMemberRole,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    handle.put(&key, &role)?;
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

pub fn get_group_member_role(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<Option<GroupMemberRole>> {
    let handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    let value = handle.get(&key)?;
    Ok(value)
}

pub fn check_group_membership(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    let handle = store.handle();
    let key = GroupMember::new(group_id.to_bytes(), *identity);
    let exists = handle.has(&key)?;
    Ok(exists)
}

pub fn is_group_admin(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    match get_group_member_role(store, group_id, identity)? {
        Some(GroupMemberRole::Admin) => Ok(true),
        _ => Ok(false),
    }
}

pub fn require_group_admin(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<()> {
    if !is_group_admin(store, group_id, identity)? {
        bail!("requester is not an admin of group '{group_id:?}'");
    }
    Ok(())
}

pub fn count_group_admins(store: &Store, group_id: &ContextGroupId) -> EyreResult<usize> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupMember::new(group_id_bytes, [0u8; 32].into());
    let mut iter = handle.iter::<GroupMember>()?;
    let first = iter.seek(start_key).transpose();
    let mut count = 0usize;

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.as_key().as_bytes()[0] != GROUP_MEMBER_PREFIX {
            break;
        }
        if key.group_id() != group_id_bytes {
            break;
        }
        let role = handle
            .get(&key)?
            .ok_or_else(|| eyre::eyre!("member key exists but value is missing"))?;
        if role == GroupMemberRole::Admin {
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
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupMember::new(group_id_bytes, [0u8; 32].into());
    let mut iter = handle.iter::<GroupMember>()?;
    let first_key = iter.seek(start_key).transpose();
    let mut results = Vec::new();
    let mut skipped = 0usize;

    // TODO: replace with iter.entries() for a single-pass scan once the
    // Iter::read() / Iter::next() borrow-conflict (read takes &'a self) is
    // resolved in the store API — currently each value requires a separate
    // handle.get() lookup after collecting the key.
    for key_result in first_key.into_iter().chain(iter.keys()) {
        let key = key_result?;

        if key.as_key().as_bytes()[0] != GROUP_MEMBER_PREFIX {
            break;
        }

        if key.group_id() != group_id_bytes {
            break;
        }

        if skipped < offset {
            skipped += 1;
            continue;
        }

        if results.len() >= limit {
            break;
        }

        let role = handle
            .get(&key)?
            .ok_or_else(|| eyre::eyre!("member key exists but value is missing"))?;
        results.push((key.identity(), role));
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Context-group index helpers
// ---------------------------------------------------------------------------

pub fn register_context_in_group(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();

    // If already registered in a different group, remove the stale index entry
    // to prevent orphaned counts and enumerations for the old group.
    let ref_key = ContextGroupRef::new(*context_id);
    if let Some(existing_group_bytes) = handle.get(&ref_key)? {
        if existing_group_bytes != group_id_bytes {
            let old_idx = GroupContextIndex::new(existing_group_bytes, *context_id);
            handle.delete(&old_idx)?;
        }
    }

    let idx_key = GroupContextIndex::new(group_id_bytes, *context_id);
    handle.put(&idx_key, &())?;
    handle.put(&ref_key, &group_id_bytes)?;

    Ok(())
}

pub fn unregister_context_from_group(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();

    let idx_key = GroupContextIndex::new(group_id_bytes, *context_id);
    handle.delete(&idx_key)?;

    let ref_key = ContextGroupRef::new(*context_id);
    handle.delete(&ref_key)?;

    Ok(())
}

pub fn get_group_for_context(
    store: &Store,
    context_id: &ContextId,
) -> EyreResult<Option<ContextGroupId>> {
    let handle = store.handle();
    let key = ContextGroupRef::new(*context_id);
    let value = handle.get(&key)?;
    Ok(value.map(ContextGroupId::from))
}

pub fn enumerate_group_contexts(
    store: &Store,
    group_id: &ContextGroupId,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<ContextId>> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupContextIndex::new(group_id_bytes, ContextId::from([0u8; 32]));
    let mut iter = handle.iter::<GroupContextIndex>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();
    let mut skipped = 0usize;

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;

        if key.as_key().as_bytes()[0] != GROUP_CONTEXT_INDEX_PREFIX {
            break;
        }

        if key.group_id() != group_id_bytes {
            break;
        }

        if skipped < offset {
            skipped += 1;
            continue;
        }

        if results.len() >= limit {
            break;
        }

        results.push(key.context_id());
    }

    Ok(results)
}

pub fn count_group_contexts(store: &Store, group_id: &ContextGroupId) -> EyreResult<usize> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupContextIndex::new(group_id_bytes, ContextId::from([0u8; 32]));
    let mut iter = handle.iter::<GroupContextIndex>()?;
    let first = iter.seek(start_key).transpose();
    let mut count = 0usize;

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;
        if key.as_key().as_bytes()[0] != GROUP_CONTEXT_INDEX_PREFIX {
            break;
        }
        if key.group_id() != group_id_bytes {
            break;
        }
        count += 1;
    }

    Ok(count)
}

// ---------------------------------------------------------------------------
// Group upgrade helpers
// ---------------------------------------------------------------------------

pub fn save_group_upgrade(
    store: &Store,
    group_id: &ContextGroupId,
    upgrade: &GroupUpgradeValue,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupUpgradeKey::new(group_id.to_bytes());
    handle.put(&key, upgrade)?;
    Ok(())
}

pub fn load_group_upgrade(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<GroupUpgradeValue>> {
    let handle = store.handle();
    let key = GroupUpgradeKey::new(group_id.to_bytes());
    let value = handle.get(&key)?;
    Ok(value)
}

pub fn delete_group_upgrade(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupUpgradeKey::new(group_id.to_bytes());
    handle.delete(&key)?;
    Ok(())
}

/// Scans all GroupUpgradeKey entries and returns (group_id, upgrade_value)
/// pairs where status is InProgress. Used for crash recovery on startup.
pub fn enumerate_in_progress_upgrades(
    store: &Store,
) -> EyreResult<Vec<(ContextGroupId, GroupUpgradeValue)>> {
    let handle = store.handle();
    let start_key = GroupUpgradeKey::new([0u8; 32]);

    let mut iter = handle.iter::<GroupUpgradeKey>()?;
    let first = iter.seek(start_key).transpose();

    let mut results = Vec::new();

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;

        if key.as_key().as_bytes()[0] != GROUP_UPGRADE_PREFIX {
            break;
        }

        let group_id = ContextGroupId::from(key.group_id());

        if let Some(upgrade) = load_group_upgrade(store, &group_id)? {
            if matches!(upgrade.status, GroupUpgradeStatus::InProgress { .. }) {
                results.push((group_id, upgrade));
            }
        }
    }

    Ok(results)
}
