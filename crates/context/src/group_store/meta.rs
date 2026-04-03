use calimero_context_config::types::ContextGroupId;
use calimero_store::key::{GroupMeta, GroupMetaValue, GROUP_META_PREFIX};
use calimero_store::Store;
use eyre::Result as EyreResult;
use sha2::{Digest, Sha256};

use super::{collect_keys_with_prefix, list_group_members};

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
    let keys = collect_keys_with_prefix(store, GroupMeta::new([0u8; 32]), GROUP_META_PREFIX, |_| {
        true
    })?;
    let handle = store.handle();
    let mut results = Vec::new();

    for key in keys.into_iter().skip(offset).take(limit) {
        let Some(meta) = handle.get(&key)? else {
            continue;
        };
        results.push((key.group_id(), meta));
    }

    Ok(results)
}

/// Compute a deterministic SHA-256 hash of the group's authorization-relevant state.
///
/// Covers members (sorted by public key) + roles + admin identity + target application.
/// This hash is embedded in each SignedGroupOp to ensure ops can only apply against
/// the exact state they were signed against, preventing divergence from concurrent ops.
pub fn compute_group_state_hash(store: &Store, group_id: &ContextGroupId) -> EyreResult<[u8; 32]> {
    let meta = load_group_meta(store, group_id)?
        .ok_or_else(|| eyre::eyre!("group not found for state hash computation"))?;

    let mut members = list_group_members(store, group_id, 0, usize::MAX)?;
    members.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    hasher.update(group_id.to_bytes());
    hasher.update(AsRef::<[u8]>::as_ref(&meta.admin_identity));
    hasher.update(meta.target_application_id.as_ref());
    for (pk, role) in &members {
        hasher.update(AsRef::<[u8]>::as_ref(pk));
        hasher.update(&borsh::to_vec(role).unwrap_or_default());
    }
    Ok(hasher.finalize().into())
}
