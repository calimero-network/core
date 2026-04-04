use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{
    GroupAlias, GroupContextAlias, GroupContextIndex, GroupMemberAlias, GroupMetaValue,
    GROUP_CONTEXT_INDEX_PREFIX, GROUP_MEMBER_ALIAS_PREFIX,
};
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::{
    check_group_membership, collect_keys_with_prefix, count_group_members, count_keys_with_prefix,
    enumerate_group_contexts, get_parent_group, list_child_groups,
};

/// Stores a human-readable alias for a context within a group.
pub fn set_context_alias(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    alias: &str,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.put(
        &GroupContextAlias::new(group_id.to_bytes(), *context_id),
        &alias.to_owned(),
    )?;
    Ok(())
}

/// Returns the alias for a context within a group, if one was set.
pub fn get_context_alias(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<Option<String>> {
    let handle = store.handle();
    handle
        .get(&GroupContextAlias::new(group_id.to_bytes(), *context_id))
        .map_err(Into::into)
}

/// Returns context IDs together with their optional aliases.
pub fn enumerate_group_contexts_with_aliases(
    store: &Store,
    group_id: &ContextGroupId,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<(ContextId, Option<String>)>> {
    let ids = enumerate_group_contexts(store, group_id, offset, limit)?;
    ids.into_iter()
        .map(|ctx_id| {
            let alias = get_context_alias(store, group_id, &ctx_id)?;
            Ok((ctx_id, alias))
        })
        .collect()
}

/// Stores a human-readable alias for a group member within a group.
pub fn set_member_alias(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
    alias: &str,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.put(
        &GroupMemberAlias::new(group_id.to_bytes(), *member),
        &alias.to_owned(),
    )?;
    Ok(())
}

/// Returns the alias for a group member within a group, if one was set.
pub fn get_member_alias(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Option<String>> {
    let handle = store.handle();
    handle
        .get(&GroupMemberAlias::new(group_id.to_bytes(), *member))
        .map_err(Into::into)
}

/// Stores a human-readable alias for the group itself.
pub fn set_group_alias(store: &Store, group_id: &ContextGroupId, alias: &str) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.put(&GroupAlias::new(group_id.to_bytes()), &alias.to_owned())?;
    Ok(())
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

    let alias = get_group_alias(store, group_id).ok().flatten();
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
        alias,
        member_count,
        context_count,
        subgroup_count,
    }))
}

/// Returns the alias for a group, if one was set.
pub fn get_group_alias(store: &Store, group_id: &ContextGroupId) -> EyreResult<Option<String>> {
    let handle = store.handle();
    handle
        .get(&GroupAlias::new(group_id.to_bytes()))
        .map_err(Into::into)
}

/// Returns all member aliases stored for a group as `(PublicKey, alias_string)` pairs.
pub fn enumerate_member_aliases(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(PublicKey, String)>> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupMemberAlias::new(gid, PublicKey::from([0u8; 32])),
        GROUP_MEMBER_ALIAS_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let handle = store.handle();
    let mut results = Vec::new();
    for key in keys {
        let Some(alias) = handle.get(&key)? else {
            continue;
        };
        results.push((key.member(), alias));
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

pub fn delete_group_alias(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupAlias::new(group_id.to_bytes()))?;
    Ok(())
}

pub fn delete_all_member_aliases(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupMemberAlias::new(gid, PublicKey::from([0u8; 32])),
        GROUP_MEMBER_ALIAS_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let mut handle = store.handle();
    for key in keys {
        handle.delete(&key)?;
    }
    Ok(())
}
