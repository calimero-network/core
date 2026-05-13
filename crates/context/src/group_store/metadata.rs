use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_primitives::metadata::MetadataRecord;
use calimero_store::key::{
    GroupContextIndex, GroupContextMetadata, GroupMemberMetadata, GroupMetaValue, GroupMetadata,
    GROUP_CONTEXT_INDEX_PREFIX, GROUP_MEMBER_METADATA_PREFIX,
};
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::{
    check_group_membership, collect_keys_with_prefix, count_group_members, count_keys_with_prefix,
    enumerate_group_contexts, get_parent_group, list_child_groups,
};

/// Store the full [`MetadataRecord`] for a context registered within a group.
pub fn set_context_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    record: &MetadataRecord,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.put(
        &GroupContextMetadata::new(group_id.to_bytes(), *context_id),
        record,
    )?;
    Ok(())
}

/// Returns the [`MetadataRecord`] for a context within a group, if one was set.
pub fn get_context_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<Option<MetadataRecord>> {
    let handle = store.handle();
    handle
        .get(&GroupContextMetadata::new(group_id.to_bytes(), *context_id))
        .map_err(Into::into)
}

/// Returns context IDs together with their optional display names
/// ([`MetadataRecord::name`]).
pub fn enumerate_group_contexts_with_names(
    store: &Store,
    group_id: &ContextGroupId,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<(ContextId, Option<String>)>> {
    let ids = enumerate_group_contexts(store, group_id, offset, limit)?;
    ids.into_iter()
        .map(|ctx_id| {
            let name = get_context_metadata(store, group_id, &ctx_id)?.and_then(|r| r.name);
            Ok((ctx_id, name))
        })
        .collect()
}

/// Store the full [`MetadataRecord`] for a group member.
pub fn set_member_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
    record: &MetadataRecord,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.put(
        &GroupMemberMetadata::new(group_id.to_bytes(), *member),
        record,
    )?;
    Ok(())
}

/// Returns the [`MetadataRecord`] for a group member, if one was set.
pub fn get_member_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Option<MetadataRecord>> {
    let handle = store.handle();
    handle
        .get(&GroupMemberMetadata::new(group_id.to_bytes(), *member))
        .map_err(Into::into)
}

/// Store the full [`MetadataRecord`] for the group itself (a namespace is a
/// root group, so this covers it).
pub fn set_group_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    record: &MetadataRecord,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.put(&GroupMetadata::new(group_id.to_bytes()), record)?;
    Ok(())
}

/// Returns the [`MetadataRecord`] for a group, if one was set.
pub fn get_group_metadata(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<MetadataRecord>> {
    let handle = store.handle();
    handle
        .get(&GroupMetadata::new(group_id.to_bytes()))
        .map_err(Into::into)
}

/// Build a `NamespaceSummary` for a root group, fetching counts from the store.
///
/// Returns `None` if the group has a parent (not a namespace root) or if
/// `node_identity` is not a member.
pub fn build_namespace_summary(
    store: &Store,
    group_id: &ContextGroupId,
    meta: &GroupMetaValue,
    node_identity: &PublicKey,
) -> EyreResult<Option<calimero_context_client::group::NamespaceSummary>> {
    if get_parent_group(store, group_id)?.is_some() {
        return Ok(None);
    }
    if !check_group_membership(store, group_id, node_identity)? {
        return Ok(None);
    }

    let name = get_group_metadata(store, group_id)
        .ok()
        .flatten()
        .and_then(|r| r.name);
    let member_count = count_group_members(store, group_id).unwrap_or(0);
    let context_count = enumerate_group_contexts(store, group_id, 0, usize::MAX)
        .unwrap_or_default()
        .len();
    let subgroup_count = list_child_groups(store, group_id).unwrap_or_default().len();

    Ok(Some(calimero_context_client::group::NamespaceSummary {
        namespace_id: *group_id,
        app_key: meta.app_key.into(),
        target_application_id: meta.target_application_id,
        upgrade_policy: meta.upgrade_policy.clone(),
        created_at: meta.created_at,
        name,
        member_count,
        context_count,
        subgroup_count,
    }))
}

/// Returns all member metadata stored for a group as `(PublicKey, MetadataRecord)` pairs.
pub fn enumerate_member_metadata(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(PublicKey, MetadataRecord)>> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupMemberMetadata::new(gid, PublicKey::from([0u8; 32])),
        GROUP_MEMBER_METADATA_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let handle = store.handle();
    let mut results = Vec::new();
    for key in keys {
        let Some(record) = handle.get(&key)? else {
            continue;
        };
        results.push((key.member(), record));
    }
    Ok(results)
}

pub fn count_group_contexts(store: &Store, group_id: &ContextGroupId) -> EyreResult<usize> {
    let gid = group_id.to_bytes();
    count_keys_with_prefix(
        store,
        GroupContextIndex::new(gid, ContextId::from([0u8; 32])),
        GROUP_CONTEXT_INDEX_PREFIX,
        |k| k.group_id() == gid,
    )
}

pub fn delete_group_metadata(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupMetadata::new(group_id.to_bytes()))?;
    Ok(())
}

pub fn delete_member_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupMemberMetadata::new(group_id.to_bytes(), *member))?;
    Ok(())
}

pub fn delete_context_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupContextMetadata::new(group_id.to_bytes(), *context_id))?;
    Ok(())
}

pub fn delete_all_member_metadata(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupMemberMetadata::new(gid, PublicKey::from([0u8; 32])),
        GROUP_MEMBER_METADATA_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let mut handle = store.handle();
    for key in keys {
        handle.delete(&key)?;
    }
    Ok(())
}
