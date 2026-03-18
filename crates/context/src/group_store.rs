use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use calimero_context_config::repr::ReprBytes;
use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::client::ContextClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{
    AsKeyParts, ContextGroupRef, ContextIdentity, GroupAlias, GroupContextAlias,
    GroupContextAllowlist, GroupContextIndex, GroupContextLastMigration,
    GroupContextLastMigrationValue, GroupContextVisibility, GroupContextVisibilityValue,
    GroupDefaultCaps, GroupDefaultCapsValue, GroupDefaultVis, GroupDefaultVisValue, GroupMember,
    GroupMemberAlias, GroupMemberCapability, GroupMemberCapabilityValue, GroupMeta, GroupMetaValue,
    GroupSigningKey, GroupSigningKeyValue, GroupUpgradeKey, GroupUpgradeStatus, GroupUpgradeValue,
    GROUP_CONTEXT_ALLOWLIST_PREFIX, GROUP_CONTEXT_INDEX_PREFIX, GROUP_CONTEXT_VISIBILITY_PREFIX,
    GROUP_MEMBER_ALIAS_PREFIX, GROUP_MEMBER_CAPABILITY_PREFIX, GROUP_MEMBER_PREFIX,
    GROUP_META_PREFIX, GROUP_SIGNING_KEY_PREFIX, GROUP_UPGRADE_PREFIX,
};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};
use tracing::{debug, warn};

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

pub fn enumerate_all_groups(
    store: &Store,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<([u8; 32], GroupMetaValue)>> {
    let handle = store.handle();
    let start_key = GroupMeta::new([0u8; 32]);
    let mut iter = handle.iter::<GroupMeta>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();
    let mut skipped = 0usize;

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;

        if key.as_key().as_bytes()[0] != GROUP_META_PREFIX {
            break;
        }

        if skipped < offset {
            skipped += 1;
            continue;
        }

        if results.len() >= limit {
            break;
        }

        let Some(meta) = handle.get(&key)? else {
            continue;
        };
        results.push((key.group_id(), meta));
    }

    Ok(results)
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
        bail!(
            "requester lacks permission to {operation} in group '{group_id:?}' \
             (not an admin and capability bit 0x{capability_bit:x} is not set)"
        );
    }
    Ok(())
}

// TODO: replace with iter.entries() for a single-pass scan once the
// Iter::read() / Iter::next() borrow-conflict (read takes &'a self) is
// resolved in the store API — currently each value requires a separate
// handle.get() lookup after collecting the key.
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

pub fn count_group_members(store: &Store, group_id: &ContextGroupId) -> EyreResult<usize> {
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
        count += 1;
    }

    Ok(count)
}

/// Scans the ContextIdentity column for the given context and returns the first
/// `PublicKey` for which the node holds a local private key. Used to find a
/// valid signer when performing group upgrades on behalf of a context that the
/// group admin may not be a member of.
pub fn find_local_signing_identity(
    store: &Store,
    context_id: &ContextId,
) -> EyreResult<Option<PublicKey>> {
    let handle = store.handle();
    let start_key = ContextIdentity::new(*context_id, [0u8; 32].into());
    let mut iter = handle.iter::<ContextIdentity>()?;
    let first = iter.seek(start_key).transpose();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.context_id() != *context_id {
            break;
        }
        let Some(value) = handle.get(&key)? else {
            continue;
        };
        if value.private_key.is_some() {
            return Ok(Some(key.public_key()));
        }
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// Group signing key helpers
// ---------------------------------------------------------------------------

pub fn store_group_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    public_key: &PublicKey,
    private_key: &[u8; 32],
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupSigningKey::new(group_id.to_bytes(), *public_key);
    handle.put(
        &key,
        &GroupSigningKeyValue {
            private_key: *private_key,
        },
    )?;
    Ok(())
}

pub fn get_group_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    public_key: &PublicKey,
) -> EyreResult<Option<[u8; 32]>> {
    let handle = store.handle();
    let key = GroupSigningKey::new(group_id.to_bytes(), *public_key);
    let value = handle.get(&key)?;
    Ok(value.map(|v| v.private_key))
}

pub fn delete_group_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    public_key: &PublicKey,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupSigningKey::new(group_id.to_bytes(), *public_key);
    handle.delete(&key)?;
    Ok(())
}

/// Verify that the node holds a signing key for the given requester in this group.
pub fn require_group_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    requester: &PublicKey,
) -> EyreResult<()> {
    if get_group_signing_key(store, group_id, requester)?.is_none() {
        bail!(
            "node does not hold a signing key for requester in group '{group_id:?}'; \
             register one via POST /admin-api/groups/<id>/signing-key"
        );
    }
    Ok(())
}

/// Delete all signing keys for a group (used during group deletion).
pub fn delete_all_group_signing_keys(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupSigningKey::new(group_id_bytes, [0u8; 32].into());
    let mut iter = handle.iter::<GroupSigningKey>()?;
    let first = iter.seek(start_key).transpose();

    let mut keys_to_delete = Vec::new();
    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;
        if key.as_key().as_bytes()[0] != GROUP_SIGNING_KEY_PREFIX {
            break;
        }
        if key.group_id() != group_id_bytes {
            break;
        }
        keys_to_delete.push(key);
    }
    drop(iter);

    let mut handle = store.handle();
    for key in keys_to_delete {
        handle.delete(&key)?;
    }

    Ok(())
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
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupMemberAlias::new(group_id_bytes, PublicKey::from([0u8; 32]));
    let mut iter = handle.iter::<GroupMemberAlias>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;
        if key.as_key().as_bytes()[0] != GROUP_MEMBER_ALIAS_PREFIX {
            break;
        }
        if key.group_id() != group_id_bytes {
            break;
        }
        let Some(alias) = handle.get(&key)? else {
            continue;
        };
        results.push((key.member(), alias));
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

// ---------------------------------------------------------------------------
// Cross-node sync helpers
// ---------------------------------------------------------------------------

/// Queries the on-chain contract for group state and updates local storage.
/// Returns the synced `GroupMetaValue` and the raw `GroupInfoQueryResponse`
/// (so callers can extract target application blob info for P2P sharing).
///
/// Syncs metadata (app_key, target_application), group contexts, and group
/// members from the on-chain contract. Prunes locally-stored entries that no
/// longer exist on-chain.
// TODO(test): add integration test with mock ContextClient — tracked in PR #2043 review
pub async fn sync_group_state_from_contract(
    datastore: &Store,
    context_client: &ContextClient,
    group_id: &ContextGroupId,
    protocol: &str,
    network_id: &str,
    contract_id: &str,
) -> EyreResult<(
    GroupMetaValue,
    calimero_context_config::client::env::config::requests::GroupInfoQueryResponse,
)> {
    let info = context_client
        .query_group_info(*group_id, protocol, network_id, contract_id)
        .await?
        .ok_or_else(|| eyre::eyre!("group '{group_id:?}' not found on-chain"))?;

    let app_key: [u8; 32] = info.app_key.to_bytes();
    let target_application_id = extract_application_id(&info.target_application)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| {
            warn!("system clock is before Unix epoch, using 0 as timestamp");
            std::time::Duration::ZERO
        })
        .as_secs();

    let existing = load_group_meta(datastore, group_id)?;
    let meta = GroupMetaValue {
        app_key,
        target_application_id,
        upgrade_policy: existing
            .as_ref()
            .map(|m| m.upgrade_policy.clone())
            .unwrap_or_default(),
        created_at: existing.as_ref().map(|m| m.created_at).unwrap_or(now),
        admin_identity: existing
            .as_ref()
            .map(|m| m.admin_identity)
            .unwrap_or_else(|| PublicKey::from([0u8; 32])),
        migration: info
            .migration_method
            .as_ref()
            .map(|m| m.as_bytes().to_vec()),
    };

    save_group_meta(datastore, group_id, &meta)?;

    // Sync group contexts from on-chain state.
    sync_group_contexts_from_contract(
        datastore,
        context_client,
        group_id,
        protocol,
        network_id,
        contract_id,
    )
    .await?;

    // Sync group members from on-chain state.
    sync_group_members_from_contract(
        datastore,
        context_client,
        group_id,
        protocol,
        network_id,
        contract_id,
    )
    .await?;

    // Sync group default capabilities and visibility.
    set_default_capabilities(datastore, group_id, info.default_member_capabilities)?;
    let vis_mode = match info.default_context_visibility {
        calimero_context_config::VisibilityMode::Open => 0u8,
        calimero_context_config::VisibilityMode::Restricted => 1u8,
    };
    set_default_visibility(datastore, group_id, vis_mode)?;

    Ok((meta, info))
}

/// Extracts the blob ID and source URL from the target application JSON
/// returned by the on-chain contract query.
pub fn extract_application_blob_info(
    app_json: &serde_json::Value,
) -> Option<(calimero_primitives::blobs::BlobId, String, u64)> {
    use calimero_context_config::repr::{Repr, ReprBytes};
    use calimero_context_config::types::BlobId as ConfigBlobId;

    let blob_val = app_json.get("blob")?;
    let repr: Repr<ConfigBlobId> = serde_json::from_value(blob_val.clone()).ok()?;
    let blob_id = calimero_primitives::blobs::BlobId::from(repr.as_bytes());

    let source = app_json.get("source")?.as_str()?.to_owned();
    let size = app_json.get("size")?.as_u64().unwrap_or(0);

    Some((blob_id, source, size))
}

/// Paginates through `query_group_contexts()` and reconciles the local
/// context-group index with the on-chain state (upsert + prune).
async fn sync_group_contexts_from_contract(
    datastore: &Store,
    context_client: &ContextClient,
    group_id: &ContextGroupId,
    protocol: &str,
    network_id: &str,
    contract_id: &str,
) -> EyreResult<()> {
    const PAGE_SIZE: usize = 100;

    let mut on_chain_contexts = HashSet::new();
    let mut offset = 0;

    loop {
        let page = context_client
            .query_group_contexts(
                *group_id,
                protocol,
                network_id,
                contract_id,
                offset,
                PAGE_SIZE,
            )
            .await?;

        let page_len = page.len();
        for context_id in page {
            on_chain_contexts.insert(context_id);
            register_context_in_group(datastore, group_id, &context_id)?;
        }

        if page_len < PAGE_SIZE {
            break;
        }
        offset += page_len;
    }

    // Prune locally-registered contexts that no longer exist on-chain.
    let local_contexts = enumerate_group_contexts(datastore, group_id, 0, usize::MAX)?;
    for local_ctx in local_contexts {
        if !on_chain_contexts.contains(&local_ctx) {
            unregister_context_from_group(datastore, group_id, &local_ctx)?;
            debug!(?group_id, ?local_ctx, "pruned stale context from group");
        }
    }

    // Sync visibility and allowlist for each on-chain context.
    for context_id in &on_chain_contexts {
        // Sync visibility
        match context_client
            .query_context_visibility(*group_id, *context_id, protocol, network_id, contract_id)
            .await
        {
            Ok(Some(vis)) => {
                let mode_u8 = match vis.mode {
                    calimero_context_config::VisibilityMode::Open => 0u8,
                    calimero_context_config::VisibilityMode::Restricted => 1u8,
                };
                let creator_bytes: [u8; 32] = vis.creator.as_bytes();
                if let Err(err) =
                    set_context_visibility(datastore, group_id, context_id, mode_u8, creator_bytes)
                {
                    warn!(
                        ?group_id, %context_id, %err,
                        "failed to store context visibility"
                    );
                }

                // Sync allowlist: clear existing entries then re-populate from chain
                let existing =
                    list_context_allowlist(datastore, group_id, context_id).unwrap_or_default();
                for member in &existing {
                    let _ = remove_from_context_allowlist(datastore, group_id, context_id, member);
                }

                let mut al_offset = 0;
                const AL_PAGE_SIZE: usize = 100;
                loop {
                    match context_client
                        .query_context_allowlist(
                            *group_id,
                            *context_id,
                            protocol,
                            network_id,
                            contract_id,
                            al_offset,
                            AL_PAGE_SIZE,
                        )
                        .await
                    {
                        Ok(page) => {
                            let page_len = page.len();
                            for signer in &page {
                                let bytes: [u8; 32] = signer.as_bytes();
                                let pk = PublicKey::from(bytes);
                                let _ =
                                    add_to_context_allowlist(datastore, group_id, context_id, &pk);
                            }
                            if page_len < AL_PAGE_SIZE {
                                break;
                            }
                            al_offset += page_len;
                        }
                        Err(err) => {
                            warn!(
                                ?group_id, %context_id, %err,
                                "failed to query context allowlist"
                            );
                            break;
                        }
                    }
                }
            }
            Ok(None) => {
                // No visibility data on-chain (context may not have visibility set yet)
            }
            Err(err) => {
                warn!(
                    ?group_id, %context_id, %err,
                    "failed to query context visibility"
                );
            }
        }
    }

    debug!(
        ?group_id,
        count = on_chain_contexts.len(),
        "synced group contexts from contract"
    );
    Ok(())
}

/// Paginates through `query_group_members()` and reconciles the local
/// member list with the on-chain state (upsert + prune).
async fn sync_group_members_from_contract(
    datastore: &Store,
    context_client: &ContextClient,
    group_id: &ContextGroupId,
    protocol: &str,
    network_id: &str,
    contract_id: &str,
) -> EyreResult<()> {
    const PAGE_SIZE: usize = 100;

    let mut on_chain_members: HashSet<[u8; 32]> = HashSet::new();
    let mut offset = 0;

    loop {
        let page = context_client
            .query_group_members(
                *group_id,
                protocol,
                network_id,
                contract_id,
                offset,
                PAGE_SIZE,
            )
            .await?;

        let page_len = page.len();
        for entry in page {
            let identity_bytes: [u8; 32] = entry.identity.as_bytes();
            let pk = PublicKey::from(identity_bytes);
            let role = match entry.role.as_str() {
                "Admin" => GroupMemberRole::Admin,
                _ => GroupMemberRole::Member,
            };
            on_chain_members.insert(identity_bytes);
            add_group_member(datastore, group_id, &pk, role)?;
            set_member_capability(datastore, group_id, &pk, entry.capabilities)?;
        }

        if page_len < PAGE_SIZE {
            break;
        }
        offset += page_len;
    }

    // Prune locally-stored members that no longer exist on-chain.
    let local_members = list_group_members(datastore, group_id, 0, usize::MAX)?;
    for (local_pk, _role) in local_members {
        if !on_chain_members.contains(AsRef::<[u8; 32]>::as_ref(&local_pk)) {
            remove_group_member(datastore, group_id, &local_pk)?;
            debug!(?group_id, ?local_pk, "pruned stale member from group");
        }
    }

    debug!(
        ?group_id,
        count = on_chain_members.len(),
        "synced group members from contract"
    );
    Ok(())
}

fn extract_application_id(app_json: &serde_json::Value) -> EyreResult<ApplicationId> {
    use calimero_context_config::repr::{Repr, ReprBytes};
    use calimero_context_config::types::ApplicationId as ConfigApplicationId;

    let id_val = app_json
        .get("id")
        .ok_or_else(|| eyre::eyre!("missing 'id' in target_application"))?;
    let repr: Repr<ConfigApplicationId> = serde_json::from_value(id_val.clone())
        .map_err(|e| eyre::eyre!("invalid application id encoding: {e}"))?;
    Ok(ApplicationId::from(repr.as_bytes()))
}

// ---------------------------------------------------------------------------
// Group upgrade helpers
// ---------------------------------------------------------------------------

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

        if let Some(upgrade) = handle.get(&key)? {
            if matches!(upgrade.status, GroupUpgradeStatus::InProgress { .. }) {
                let group_id = ContextGroupId::from(key.group_id());
                results.push((group_id, upgrade));
            }
        }
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

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

/// Returns (mode, creator_pk). mode: 0 = Open, 1 = Restricted.
pub fn get_context_visibility(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<Option<(u8, [u8; 32])>> {
    let handle = store.handle();
    let key = GroupContextVisibility::new(group_id.to_bytes(), *context_id);
    let value = handle.get(&key)?;
    Ok(value.map(|v| (v.mode, v.creator)))
}

pub fn set_context_visibility(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    mode: u8,
    creator: [u8; 32],
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupContextVisibility::new(group_id.to_bytes(), *context_id);
    handle.put(&key, &GroupContextVisibilityValue { mode, creator })?;
    Ok(())
}

pub fn check_context_allowlist(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    member: &PublicKey,
) -> EyreResult<bool> {
    let handle = store.handle();
    let key = GroupContextAllowlist::new(group_id.to_bytes(), *context_id, *member);
    // If the key exists (even with unit value), the member is on the allowlist
    let value: Option<()> = handle.get(&key)?;
    Ok(value.is_some())
}

pub fn add_to_context_allowlist(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    member: &PublicKey,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupContextAllowlist::new(group_id.to_bytes(), *context_id, *member);
    handle.put(&key, &())?;
    Ok(())
}

pub fn remove_from_context_allowlist(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    member: &PublicKey,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupContextAllowlist::new(group_id.to_bytes(), *context_id, *member);
    handle.delete(&key)?;
    Ok(())
}

pub fn list_context_allowlist(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<Vec<PublicKey>> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key =
        GroupContextAllowlist::new(group_id_bytes, *context_id, PublicKey::from([0u8; 32]));
    let mut iter = handle.iter::<GroupContextAllowlist>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();

    for entry in first.into_iter().chain(iter.keys()) {
        let key = entry?;

        if key.as_key().as_bytes()[0] != GROUP_CONTEXT_ALLOWLIST_PREFIX {
            break;
        }

        if key.group_id() != group_id_bytes {
            break;
        }

        if key.context_id() != *context_id {
            break;
        }

        results.push(PublicKey::from(*key.member()));
    }

    Ok(results)
}

pub fn delete_context_visibility(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupContextVisibility::new(group_id.to_bytes(), *context_id);
    handle.delete(&key)?;
    Ok(())
}

pub fn clear_context_allowlist(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    let members = list_context_allowlist(store, group_id, context_id)?;
    for member in &members {
        remove_from_context_allowlist(store, group_id, context_id, member)?;
    }
    Ok(())
}

pub fn enumerate_member_capabilities(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(PublicKey, u32)>> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupMemberCapability::new(group_id_bytes, PublicKey::from([0u8; 32]));
    let mut iter = handle.iter::<GroupMemberCapability>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;

        if key.as_key().as_bytes()[0] != GROUP_MEMBER_CAPABILITY_PREFIX {
            break;
        }

        if key.group_id() != group_id_bytes {
            break;
        }

        let Some(val) = handle.get(&key)? else {
            continue;
        };

        results.push((PublicKey::from(*key.identity()), val.capabilities));
    }

    Ok(results)
}

pub fn enumerate_context_visibilities(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(ContextId, u8, [u8; 32])>> {
    let handle = store.handle();
    let group_id_bytes: [u8; 32] = group_id.to_bytes();
    let start_key = GroupContextVisibility::new(group_id_bytes, ContextId::from([0u8; 32]));
    let mut iter = handle.iter::<GroupContextVisibility>()?;
    let first = iter.seek(start_key).transpose();
    let mut results = Vec::new();

    for key_result in first.into_iter().chain(iter.keys()) {
        let key = key_result?;

        if key.as_key().as_bytes()[0] != GROUP_CONTEXT_VISIBILITY_PREFIX {
            break;
        }

        if key.group_id() != group_id_bytes {
            break;
        }

        let Some(val) = handle.get(&key)? else {
            continue;
        };

        results.push((ContextId::from(*key.context_id()), val.mode, val.creator));
    }

    Ok(results)
}

pub fn enumerate_contexts_with_allowlists(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(ContextId, Vec<PublicKey>)>> {
    let context_ids = enumerate_group_contexts(store, group_id, 0, usize::MAX)?;
    let mut results = Vec::new();

    for context_id in context_ids {
        let members = list_context_allowlist(store, group_id, &context_id)?;
        if !members.is_empty() {
            results.push((context_id, members));
        }
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

// ---------------------------------------------------------------------------
// Per-context migration tracking
// ---------------------------------------------------------------------------

/// Returns the migration method name that was last successfully applied to
/// `context_id` within `group_id`, or `None` if no migration has been recorded.
pub fn get_context_last_migration(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<Option<String>> {
    let handle = store.handle();
    let key = GroupContextLastMigration::new(group_id.to_bytes(), (*context_id).into());
    Ok(handle
        .get(&key)?
        .map(|v: GroupContextLastMigrationValue| v.method))
}

/// Records that `method` was successfully applied to `context_id` within
/// `group_id`. Subsequent calls to `maybe_lazy_upgrade` will skip this
/// migration for this context unless a different method is configured.
pub fn set_context_last_migration(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    method: &str,
) -> EyreResult<()> {
    let mut handle = store.handle();
    let key = GroupContextLastMigration::new(group_id.to_bytes(), (*context_id).into());
    handle.put(
        &key,
        &GroupContextLastMigrationValue {
            method: method.to_owned(),
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupMetaValue, GroupUpgradeStatus, GroupUpgradeValue};
    use calimero_store::Store;

    use super::*;

    fn test_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn test_group_id() -> ContextGroupId {
        ContextGroupId::from([0xAA; 32])
    }

    fn test_meta() -> GroupMetaValue {
        GroupMetaValue {
            app_key: [0xBB; 32],
            target_application_id: ApplicationId::from([0xCC; 32]),
            upgrade_policy: UpgradePolicy::Automatic,
            created_at: 1_700_000_000,
            admin_identity: PublicKey::from([0x01; 32]),
            migration: None,
        }
    }

    // -----------------------------------------------------------------------
    // Group meta tests
    // -----------------------------------------------------------------------

    #[test]
    fn save_load_delete_group_meta() {
        let store = test_store();
        let gid = test_group_id();
        let meta = test_meta();

        assert!(load_group_meta(&store, &gid).unwrap().is_none());

        save_group_meta(&store, &gid, &meta).unwrap();
        let loaded = load_group_meta(&store, &gid).unwrap().unwrap();
        assert_eq!(loaded.app_key, meta.app_key);
        assert_eq!(loaded.target_application_id, meta.target_application_id);

        delete_group_meta(&store, &gid).unwrap();
        assert!(load_group_meta(&store, &gid).unwrap().is_none());
    }

    // -----------------------------------------------------------------------
    // Member tests
    // -----------------------------------------------------------------------

    #[test]
    fn add_and_check_membership() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x01; 32]);

        assert!(!check_group_membership(&store, &gid, &pk).unwrap());

        add_group_member(&store, &gid, &pk, GroupMemberRole::Admin).unwrap();
        assert!(check_group_membership(&store, &gid, &pk).unwrap());
        assert!(is_group_admin(&store, &gid, &pk).unwrap());
    }

    #[test]
    fn remove_member() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x02; 32]);

        add_group_member(&store, &gid, &pk, GroupMemberRole::Member).unwrap();
        assert!(check_group_membership(&store, &gid, &pk).unwrap());

        remove_group_member(&store, &gid, &pk).unwrap();
        assert!(!check_group_membership(&store, &gid, &pk).unwrap());
    }

    #[test]
    fn get_member_role() {
        let store = test_store();
        let gid = test_group_id();
        let admin = PublicKey::from([0x01; 32]);
        let member = PublicKey::from([0x02; 32]);

        add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
        add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();

        assert_eq!(
            get_group_member_role(&store, &gid, &admin).unwrap(),
            Some(GroupMemberRole::Admin)
        );
        assert_eq!(
            get_group_member_role(&store, &gid, &member).unwrap(),
            Some(GroupMemberRole::Member)
        );
        assert!(!is_group_admin(&store, &gid, &member).unwrap());
    }

    #[test]
    fn require_group_admin_rejects_non_admin() {
        let store = test_store();
        let gid = test_group_id();
        let member = PublicKey::from([0x03; 32]);

        add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();
        assert!(require_group_admin(&store, &gid, &member).is_err());
    }

    #[test]
    fn count_members_and_admins() {
        let store = test_store();
        let gid = test_group_id();

        assert_eq!(count_group_members(&store, &gid).unwrap(), 0);
        assert_eq!(count_group_admins(&store, &gid).unwrap(), 0);

        add_group_member(
            &store,
            &gid,
            &PublicKey::from([0x01; 32]),
            GroupMemberRole::Admin,
        )
        .unwrap();
        add_group_member(
            &store,
            &gid,
            &PublicKey::from([0x02; 32]),
            GroupMemberRole::Member,
        )
        .unwrap();
        add_group_member(
            &store,
            &gid,
            &PublicKey::from([0x03; 32]),
            GroupMemberRole::Admin,
        )
        .unwrap();

        assert_eq!(count_group_members(&store, &gid).unwrap(), 3);
        assert_eq!(count_group_admins(&store, &gid).unwrap(), 2);
    }

    #[test]
    fn list_members_with_offset_and_limit() {
        let store = test_store();
        let gid = test_group_id();

        for i in 0u8..5 {
            let mut pk_bytes = [0u8; 32];
            pk_bytes[0] = i;
            add_group_member(
                &store,
                &gid,
                &PublicKey::from(pk_bytes),
                GroupMemberRole::Member,
            )
            .unwrap();
        }

        let all = list_group_members(&store, &gid, 0, 100).unwrap();
        assert_eq!(all.len(), 5);

        let page = list_group_members(&store, &gid, 1, 2).unwrap();
        assert_eq!(page.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Signing key tests
    // -----------------------------------------------------------------------

    #[test]
    fn store_and_get_signing_key() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x10; 32]);
        let sk = [0xAA; 32];

        assert!(get_group_signing_key(&store, &gid, &pk).unwrap().is_none());

        store_group_signing_key(&store, &gid, &pk, &sk).unwrap();
        let loaded = get_group_signing_key(&store, &gid, &pk).unwrap().unwrap();
        assert_eq!(loaded, sk);
    }

    #[test]
    fn delete_signing_key() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x10; 32]);
        let sk = [0xAA; 32];

        store_group_signing_key(&store, &gid, &pk, &sk).unwrap();
        delete_group_signing_key(&store, &gid, &pk).unwrap();
        assert!(get_group_signing_key(&store, &gid, &pk).unwrap().is_none());
    }

    #[test]
    fn require_signing_key_fails_when_missing() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x10; 32]);

        assert!(require_group_signing_key(&store, &gid, &pk).is_err());
    }

    #[test]
    fn delete_all_group_signing_keys_removes_all() {
        let store = test_store();
        let gid = test_group_id();
        let pk1 = PublicKey::from([0x10; 32]);
        let pk2 = PublicKey::from([0x11; 32]);

        store_group_signing_key(&store, &gid, &pk1, &[0xAA; 32]).unwrap();
        store_group_signing_key(&store, &gid, &pk2, &[0xBB; 32]).unwrap();

        delete_all_group_signing_keys(&store, &gid).unwrap();

        assert!(get_group_signing_key(&store, &gid, &pk1).unwrap().is_none());
        assert!(get_group_signing_key(&store, &gid, &pk2).unwrap().is_none());
    }

    // -----------------------------------------------------------------------
    // Context-group index tests
    // -----------------------------------------------------------------------

    #[test]
    fn register_and_unregister_context() {
        let store = test_store();
        let gid = test_group_id();
        let cid = ContextId::from([0x11; 32]);

        assert!(get_group_for_context(&store, &cid).unwrap().is_none());

        register_context_in_group(&store, &gid, &cid).unwrap();
        assert_eq!(get_group_for_context(&store, &cid).unwrap().unwrap(), gid);

        unregister_context_from_group(&store, &gid, &cid).unwrap();
        assert!(get_group_for_context(&store, &cid).unwrap().is_none());
    }

    #[test]
    fn re_register_context_cleans_old_group() {
        let store = test_store();
        let gid1 = ContextGroupId::from([0x01; 32]);
        let gid2 = ContextGroupId::from([0x02; 32]);
        let cid = ContextId::from([0x11; 32]);

        register_context_in_group(&store, &gid1, &cid).unwrap();
        assert_eq!(count_group_contexts(&store, &gid1).unwrap(), 1);

        register_context_in_group(&store, &gid2, &cid).unwrap();
        assert_eq!(count_group_contexts(&store, &gid1).unwrap(), 0);
        assert_eq!(count_group_contexts(&store, &gid2).unwrap(), 1);
        assert_eq!(get_group_for_context(&store, &cid).unwrap().unwrap(), gid2);
    }

    #[test]
    fn enumerate_and_count_contexts() {
        let store = test_store();
        let gid = test_group_id();

        for i in 0u8..4 {
            let mut cid_bytes = [0u8; 32];
            cid_bytes[0] = i;
            register_context_in_group(&store, &gid, &ContextId::from(cid_bytes)).unwrap();
        }

        assert_eq!(count_group_contexts(&store, &gid).unwrap(), 4);

        let page = enumerate_group_contexts(&store, &gid, 1, 2).unwrap();
        assert_eq!(page.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Upgrade tests
    // -----------------------------------------------------------------------

    #[test]
    fn save_load_delete_upgrade() {
        let store = test_store();
        let gid = test_group_id();

        assert!(load_group_upgrade(&store, &gid).unwrap().is_none());

        let upgrade = GroupUpgradeValue {
            from_version: "1.0.0".to_owned(),
            to_version: "2.0.0".to_owned(),
            migration: None,
            initiated_at: 1_700_000_000,
            initiated_by: PublicKey::from([0x01; 32]),
            status: GroupUpgradeStatus::InProgress {
                total: 5,
                completed: 0,
                failed: 0,
            },
        };

        save_group_upgrade(&store, &gid, &upgrade).unwrap();
        let loaded = load_group_upgrade(&store, &gid).unwrap().unwrap();
        assert_eq!(loaded.from_version, "1.0.0");
        assert_eq!(loaded.to_version, "2.0.0");

        delete_group_upgrade(&store, &gid).unwrap();
        assert!(load_group_upgrade(&store, &gid).unwrap().is_none());
    }

    #[test]
    fn enumerate_in_progress_upgrades_filters_completed() {
        let store = test_store();
        let gid_in_progress = ContextGroupId::from([0x01; 32]);
        let gid_completed = ContextGroupId::from([0x02; 32]);

        save_group_upgrade(
            &store,
            &gid_in_progress,
            &GroupUpgradeValue {
                from_version: "1.0.0".to_owned(),
                to_version: "2.0.0".to_owned(),
                migration: None,
                initiated_at: 1_700_000_000,
                initiated_by: PublicKey::from([0x01; 32]),
                status: GroupUpgradeStatus::InProgress {
                    total: 5,
                    completed: 2,
                    failed: 0,
                },
            },
        )
        .unwrap();

        save_group_upgrade(
            &store,
            &gid_completed,
            &GroupUpgradeValue {
                from_version: "1.0.0".to_owned(),
                to_version: "2.0.0".to_owned(),
                migration: None,
                initiated_at: 1_700_000_000,
                initiated_by: PublicKey::from([0x01; 32]),
                status: GroupUpgradeStatus::Completed {
                    completed_at: Some(1_700_001_000),
                },
            },
        )
        .unwrap();

        let results = enumerate_in_progress_upgrades(&store).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, gid_in_progress);
    }

    // -----------------------------------------------------------------------
    // enumerate_all_groups — prefix guard regression test
    // -----------------------------------------------------------------------

    /// Regression test: `enumerate_all_groups` must stop at GroupMeta keys and
    /// not spill into adjacent GroupMember keys (prefix 0x21).  Before the fix,
    /// the function would attempt to deserialise a `GroupMemberRole` value as
    /// `GroupMetaValue`, panicking with "failed to fill whole buffer".
    #[test]
    fn enumerate_all_groups_stops_before_member_keys() {
        let store = test_store();
        let gid = test_group_id();
        let meta = test_meta();
        let member = PublicKey::from([0x10; 32]);

        save_group_meta(&store, &gid, &meta).unwrap();
        // Add a group member — this writes a GroupMember key (prefix 0x21)
        // into the same column, right after GroupMeta keys (prefix 0x20).
        add_group_member(&store, &gid, &member, GroupMemberRole::Admin).unwrap();

        // Must return exactly one group without panicking.
        let groups = enumerate_all_groups(&store, 0, 100).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, gid.to_bytes());
    }

    #[test]
    fn enumerate_all_groups_multiple_groups_with_members() {
        let store = test_store();
        let gid1 = ContextGroupId::from([0x01; 32]);
        let gid2 = ContextGroupId::from([0x02; 32]);
        let meta = test_meta();

        save_group_meta(&store, &gid1, &meta).unwrap();
        save_group_meta(&store, &gid2, &meta).unwrap();
        add_group_member(
            &store,
            &gid1,
            &PublicKey::from([0xAA; 32]),
            GroupMemberRole::Admin,
        )
        .unwrap();
        add_group_member(
            &store,
            &gid2,
            &PublicKey::from([0xBB; 32]),
            GroupMemberRole::Member,
        )
        .unwrap();

        let groups = enumerate_all_groups(&store, 0, 100).unwrap();
        assert_eq!(groups.len(), 2);

        // Pagination
        let page = enumerate_all_groups(&store, 1, 1).unwrap();
        assert_eq!(page.len(), 1);
    }

    // -----------------------------------------------------------------------
    // extract_application_id — base58 decoding regression test
    // -----------------------------------------------------------------------

    /// Regression test: `extract_application_id` must decode the `id` field
    /// using base58 (via `Repr<ApplicationId>`), not hex.  Before the fix,
    /// `hex::decode` was called on a base58 string, producing
    /// "Invalid character 'g' at position 1" errors at runtime.
    #[test]
    fn extract_application_id_decodes_base58() {
        // Repr<[u8; 32]> serialises as base58, matching what the NEAR contract
        // returns for Repr<ConfigApplicationId> with the same underlying bytes.
        use calimero_context_config::repr::Repr;

        let raw: [u8; 32] = [0xDE; 32];
        let encoded = Repr::new(raw).to_string(); // base58 string

        let json = serde_json::json!({ "id": encoded });
        let result = extract_application_id(&json).unwrap();
        assert_eq!(*result, raw);
    }

    #[test]
    fn extract_application_id_rejects_hex() {
        // A hex string decodes to ~46 bytes via base58, causing a length
        // mismatch against the required 32-byte ApplicationId.
        let hex_str = hex::encode([0xDE; 32]);
        let json = serde_json::json!({ "id": hex_str });
        assert!(extract_application_id(&json).is_err());
    }

    #[test]
    fn extract_application_id_missing_field_returns_error() {
        let json = serde_json::json!({});
        assert!(extract_application_id(&json).is_err());
    }

    // -----------------------------------------------------------------------
    // Member capability tests
    // -----------------------------------------------------------------------

    #[test]
    fn set_and_get_member_capability() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x10; 32]);

        // No capability stored yet
        assert!(get_member_capability(&store, &gid, &pk).unwrap().is_none());

        // Set capabilities
        set_member_capability(&store, &gid, &pk, 0b101).unwrap();
        let caps = get_member_capability(&store, &gid, &pk).unwrap().unwrap();
        assert_eq!(caps, 0b101);

        // Update capabilities
        set_member_capability(&store, &gid, &pk, 0b111).unwrap();
        let caps = get_member_capability(&store, &gid, &pk).unwrap().unwrap();
        assert_eq!(caps, 0b111);
    }

    #[test]
    fn capability_zero_means_no_permissions() {
        let store = test_store();
        let gid = test_group_id();
        let pk = PublicKey::from([0x11; 32]);

        set_member_capability(&store, &gid, &pk, 0).unwrap();
        let caps = get_member_capability(&store, &gid, &pk).unwrap().unwrap();
        assert_eq!(caps, 0);
        // All capability bits are off
        assert_eq!(caps & (1 << 0), 0); // CAN_CREATE_CONTEXT
        assert_eq!(caps & (1 << 1), 0); // CAN_INVITE_MEMBERS
        assert_eq!(caps & (1 << 2), 0); // CAN_JOIN_OPEN_CONTEXTS
    }

    #[test]
    fn capabilities_isolated_per_member() {
        let store = test_store();
        let gid = test_group_id();
        let alice = PublicKey::from([0x12; 32]);
        let bob = PublicKey::from([0x13; 32]);

        set_member_capability(&store, &gid, &alice, 0b001).unwrap();
        set_member_capability(&store, &gid, &bob, 0b110).unwrap();

        assert_eq!(
            get_member_capability(&store, &gid, &alice)
                .unwrap()
                .unwrap(),
            0b001
        );
        assert_eq!(
            get_member_capability(&store, &gid, &bob).unwrap().unwrap(),
            0b110
        );
    }

    // -----------------------------------------------------------------------
    // Context visibility tests
    // -----------------------------------------------------------------------

    #[test]
    fn set_and_get_context_visibility() {
        let store = test_store();
        let gid = test_group_id();
        let ctx = ContextId::from([0x20; 32]);
        let creator: [u8; 32] = [0x01; 32];

        // No visibility stored yet
        assert!(get_context_visibility(&store, &gid, &ctx)
            .unwrap()
            .is_none());

        // Set to Open (0)
        set_context_visibility(&store, &gid, &ctx, 0, creator).unwrap();
        let (mode, stored_creator) = get_context_visibility(&store, &gid, &ctx).unwrap().unwrap();
        assert_eq!(mode, 0);
        assert_eq!(stored_creator, creator);

        // Update to Restricted (1)
        set_context_visibility(&store, &gid, &ctx, 1, creator).unwrap();
        let (mode, _) = get_context_visibility(&store, &gid, &ctx).unwrap().unwrap();
        assert_eq!(mode, 1);
    }

    #[test]
    fn visibility_isolated_per_context() {
        let store = test_store();
        let gid = test_group_id();
        let ctx1 = ContextId::from([0x21; 32]);
        let ctx2 = ContextId::from([0x22; 32]);
        let creator: [u8; 32] = [0x01; 32];

        set_context_visibility(&store, &gid, &ctx1, 0, creator).unwrap();
        set_context_visibility(&store, &gid, &ctx2, 1, creator).unwrap();

        let (mode1, _) = get_context_visibility(&store, &gid, &ctx1)
            .unwrap()
            .unwrap();
        let (mode2, _) = get_context_visibility(&store, &gid, &ctx2)
            .unwrap()
            .unwrap();
        assert_eq!(mode1, 0);
        assert_eq!(mode2, 1);
    }

    // -----------------------------------------------------------------------
    // Context allowlist tests
    // -----------------------------------------------------------------------

    #[test]
    fn add_check_remove_context_allowlist() {
        let store = test_store();
        let gid = test_group_id();
        let ctx = ContextId::from([0x30; 32]);
        let member = PublicKey::from([0x31; 32]);

        // Not on allowlist initially
        assert!(!check_context_allowlist(&store, &gid, &ctx, &member).unwrap());

        // Add to allowlist
        add_to_context_allowlist(&store, &gid, &ctx, &member).unwrap();
        assert!(check_context_allowlist(&store, &gid, &ctx, &member).unwrap());

        // Remove from allowlist
        remove_from_context_allowlist(&store, &gid, &ctx, &member).unwrap();
        assert!(!check_context_allowlist(&store, &gid, &ctx, &member).unwrap());
    }

    #[test]
    fn list_context_allowlist_returns_all_members() {
        let store = test_store();
        let gid = test_group_id();
        let ctx = ContextId::from([0x32; 32]);
        let m1 = PublicKey::from([0x33; 32]);
        let m2 = PublicKey::from([0x34; 32]);
        let m3 = PublicKey::from([0x35; 32]);

        add_to_context_allowlist(&store, &gid, &ctx, &m1).unwrap();
        add_to_context_allowlist(&store, &gid, &ctx, &m2).unwrap();
        add_to_context_allowlist(&store, &gid, &ctx, &m3).unwrap();

        let members = list_context_allowlist(&store, &gid, &ctx).unwrap();
        assert_eq!(members.len(), 3);
        assert!(members.contains(&m1));
        assert!(members.contains(&m2));
        assert!(members.contains(&m3));
    }

    #[test]
    fn list_context_allowlist_isolated_per_context() {
        let store = test_store();
        let gid = test_group_id();
        let ctx1 = ContextId::from([0x36; 32]);
        let ctx2 = ContextId::from([0x37; 32]);
        let m1 = PublicKey::from([0x38; 32]);
        let m2 = PublicKey::from([0x39; 32]);

        add_to_context_allowlist(&store, &gid, &ctx1, &m1).unwrap();
        add_to_context_allowlist(&store, &gid, &ctx2, &m2).unwrap();

        let ctx1_members = list_context_allowlist(&store, &gid, &ctx1).unwrap();
        assert_eq!(ctx1_members.len(), 1);
        assert!(ctx1_members.contains(&m1));

        let ctx2_members = list_context_allowlist(&store, &gid, &ctx2).unwrap();
        assert_eq!(ctx2_members.len(), 1);
        assert!(ctx2_members.contains(&m2));
    }

    #[test]
    fn allowlist_add_idempotent() {
        let store = test_store();
        let gid = test_group_id();
        let ctx = ContextId::from([0x3A; 32]);
        let member = PublicKey::from([0x3B; 32]);

        add_to_context_allowlist(&store, &gid, &ctx, &member).unwrap();
        add_to_context_allowlist(&store, &gid, &ctx, &member).unwrap();

        let members = list_context_allowlist(&store, &gid, &ctx).unwrap();
        assert_eq!(members.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Default capabilities and visibility tests
    // -----------------------------------------------------------------------

    #[test]
    fn set_and_get_default_capabilities() {
        let store = test_store();
        let gid = test_group_id();

        assert!(get_default_capabilities(&store, &gid).unwrap().is_none());

        set_default_capabilities(&store, &gid, 0b100).unwrap();
        assert_eq!(
            get_default_capabilities(&store, &gid).unwrap().unwrap(),
            0b100
        );

        // Update
        set_default_capabilities(&store, &gid, 0b111).unwrap();
        assert_eq!(
            get_default_capabilities(&store, &gid).unwrap().unwrap(),
            0b111
        );
    }

    #[test]
    fn set_and_get_default_visibility() {
        let store = test_store();
        let gid = test_group_id();

        assert!(get_default_visibility(&store, &gid).unwrap().is_none());

        // Open = 0
        set_default_visibility(&store, &gid, 0).unwrap();
        assert_eq!(get_default_visibility(&store, &gid).unwrap().unwrap(), 0);

        // Restricted = 1
        set_default_visibility(&store, &gid, 1).unwrap();
        assert_eq!(get_default_visibility(&store, &gid).unwrap().unwrap(), 1);
    }

    #[test]
    fn defaults_isolated_per_group() {
        let store = test_store();
        let g1 = ContextGroupId::from([0x40; 32]);
        let g2 = ContextGroupId::from([0x41; 32]);

        set_default_capabilities(&store, &g1, 0b001).unwrap();
        set_default_capabilities(&store, &g2, 0b110).unwrap();
        set_default_visibility(&store, &g1, 0).unwrap();
        set_default_visibility(&store, &g2, 1).unwrap();

        assert_eq!(
            get_default_capabilities(&store, &g1).unwrap().unwrap(),
            0b001
        );
        assert_eq!(
            get_default_capabilities(&store, &g2).unwrap().unwrap(),
            0b110
        );
        assert_eq!(get_default_visibility(&store, &g1).unwrap().unwrap(), 0);
        assert_eq!(get_default_visibility(&store, &g2).unwrap().unwrap(), 1);
    }
}
