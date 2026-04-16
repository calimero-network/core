use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{GroupSigningKey, GroupSigningKeyValue, GROUP_SIGNING_KEY_PREFIX};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::{collect_keys_with_prefix, GroupStoreError};

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
        bail!(GroupStoreError::NoSigningKey {
            group_id: format!("{group_id:?}"),
            identity: format!("{requester:?}"),
        });
    }
    Ok(())
}

/// Walk the parent chain looking for a stored signing key for `requester`.
///
/// This allows a namespace (root-group) admin to sign governance ops on any
/// descendant group without needing a per-group key copy.  The walk is bounded
/// by the same depth limit used for namespace resolution (16 levels).
pub fn find_ancestor_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    requester: &PublicKey,
) -> Option<[u8; 32]> {
    let mut current = *group_id;
    for _ in 0..super::namespace::MAX_NAMESPACE_DEPTH {
        match super::namespace::get_parent_group(store, &current) {
            Ok(Some(parent)) => {
                if let Ok(Some(sk)) = get_group_signing_key(store, &parent, requester) {
                    return Some(sk);
                }
                current = parent;
            }
            _ => break,
        }
    }
    None
}

/// Delete all signing keys for a group (used during group deletion).
pub fn delete_all_group_signing_keys(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupSigningKey::new(gid, [0u8; 32].into()),
        GROUP_SIGNING_KEY_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let mut handle = store.handle();
    for key in keys {
        handle.delete(&key)?;
    }
    Ok(())
}
