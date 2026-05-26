use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{GroupSigningKey, GroupSigningKeyValue, GROUP_SIGNING_KEY_PREFIX};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::{collect_keys_with_prefix, namespace::MAX_NAMESPACE_DEPTH, GroupStoreError};

/// Typed Repository for per-group signing keys (used by local-identity
/// signing during governance op publication).
///
/// Each row is a `(group_id, public_key) -> private_key` mapping
/// stored under [`GroupSigningKey`]. The Repository borrows the
/// store once and exposes the lookup/insert/walk operations on
/// `&self`. Walk operations use [`MAX_NAMESPACE_DEPTH`] to bound
/// ancestor traversal.
///
/// Issue #2303 / epic #2300.
pub struct SigningKeysRepository<'a> {
    store: &'a Store,
}

impl<'a> SigningKeysRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn store_key(
        &self,
        group_id: &ContextGroupId,
        public_key: &PublicKey,
        private_key: &[u8; 32],
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = GroupSigningKey::new(group_id.to_bytes(), *public_key);
        handle.put(
            &key,
            &GroupSigningKeyValue {
                private_key: *private_key,
            },
        )?;
        Ok(())
    }

    pub fn get_key(
        &self,
        group_id: &ContextGroupId,
        public_key: &PublicKey,
    ) -> EyreResult<Option<[u8; 32]>> {
        let handle = self.store.handle();
        let key = GroupSigningKey::new(group_id.to_bytes(), *public_key);
        let value = handle.get(&key)?;
        Ok(value.map(|v| v.private_key))
    }

    pub fn delete_key(&self, group_id: &ContextGroupId, public_key: &PublicKey) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = GroupSigningKey::new(group_id.to_bytes(), *public_key);
        handle.delete(&key)?;
        Ok(())
    }

    /// Verify that the node holds a signing key for `requester` in this group.
    pub fn require_key(&self, group_id: &ContextGroupId, requester: &PublicKey) -> EyreResult<()> {
        if self.get_key(group_id, requester)?.is_none() {
            bail!(GroupStoreError::NoSigningKey {
                group_id: format!("{group_id:?}"),
                identity: format!("{requester:?}"),
            });
        }
        Ok(())
    }

    /// Walk the ancestor chain from `group_id` upward looking for a
    /// signing key for `public_key`. Returns the first match found
    /// (closest ancestor), or `None` if no ancestor holds a key for
    /// this identity. Bounded by [`MAX_NAMESPACE_DEPTH`].
    pub fn resolve(
        &self,
        group_id: &ContextGroupId,
        public_key: &PublicKey,
    ) -> EyreResult<Option<[u8; 32]>> {
        let mut current = *group_id;
        for _ in 0..MAX_NAMESPACE_DEPTH {
            if let Some(sk) = self.get_key(&current, public_key)? {
                return Ok(Some(sk));
            }
            match super::get_parent_group(self.store, &current)? {
                Some(parent) => current = parent,
                None => return Ok(None),
            }
        }
        self.get_key(&current, public_key)
    }

    /// Delete all signing keys for a group (used during group deletion).
    pub fn delete_all_for_group(&self, group_id: &ContextGroupId) -> EyreResult<()> {
        let gid = group_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupSigningKey::new(gid, [0u8; 32].into()),
            GROUP_SIGNING_KEY_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let mut handle = self.store.handle();
        for key in keys {
            handle.delete(&key)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Deprecated free-function wrappers.
// ---------------------------------------------------------------------------

#[deprecated(note = "use SigningKeysRepository::new(store).store_key(...)")]
pub fn store_group_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    public_key: &PublicKey,
    private_key: &[u8; 32],
) -> EyreResult<()> {
    SigningKeysRepository::new(store).store_key(group_id, public_key, private_key)
}

#[deprecated(note = "use SigningKeysRepository::new(store).get_key(...)")]
pub fn get_group_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    public_key: &PublicKey,
) -> EyreResult<Option<[u8; 32]>> {
    SigningKeysRepository::new(store).get_key(group_id, public_key)
}

#[deprecated(note = "use SigningKeysRepository::new(store).delete_key(...)")]
pub fn delete_group_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    public_key: &PublicKey,
) -> EyreResult<()> {
    SigningKeysRepository::new(store).delete_key(group_id, public_key)
}

#[deprecated(note = "use SigningKeysRepository::new(store).require_key(...)")]
pub fn require_group_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    requester: &PublicKey,
) -> EyreResult<()> {
    SigningKeysRepository::new(store).require_key(group_id, requester)
}

#[deprecated(note = "use SigningKeysRepository::new(store).resolve(...)")]
pub fn resolve_group_signing_key(
    store: &Store,
    group_id: &ContextGroupId,
    public_key: &PublicKey,
) -> EyreResult<Option<[u8; 32]>> {
    SigningKeysRepository::new(store).resolve(group_id, public_key)
}

#[deprecated(note = "use SigningKeysRepository::new(store).delete_all_for_group(...)")]
pub fn delete_all_group_signing_keys(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    SigningKeysRepository::new(store).delete_all_for_group(group_id)
}
