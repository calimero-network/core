use crate::group_store::NamespaceRepository;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{GroupSigningKey, GroupSigningKeyValue, GROUP_SIGNING_KEY_PREFIX};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::{collect_keys_with_prefix, namespace::MAX_NAMESPACE_DEPTH, SigningKeysError};

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
            bail!(SigningKeysError::NotFound {
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
            match NamespaceRepository::new(self.store).parent(&current)? {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group_store::test_fixtures::{test_group_id, test_store};

    #[test]
    fn get_key_returns_none_when_unset() {
        let store = test_store();
        let repo = SigningKeysRepository::new(&store);
        let pk = PublicKey::from([0x01; 32]);
        assert!(repo.get_key(&test_group_id(), &pk).unwrap().is_none());
    }

    #[test]
    fn store_then_get_key_round_trip() {
        let store = test_store();
        let repo = SigningKeysRepository::new(&store);
        let gid = test_group_id();
        let pk = PublicKey::from([0x01; 32]);
        let sk = [0xAB; 32];

        repo.store_key(&gid, &pk, &sk).unwrap();
        assert_eq!(repo.get_key(&gid, &pk).unwrap(), Some(sk));
    }

    #[test]
    fn require_key_bails_when_absent() {
        let store = test_store();
        let repo = SigningKeysRepository::new(&store);
        let pk = PublicKey::from([0x01; 32]);
        let err = repo.require_key(&test_group_id(), &pk).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<SigningKeysError>(),
            Some(SigningKeysError::NotFound { .. })
        ));
    }

    #[test]
    fn delete_key_is_idempotent() {
        let store = test_store();
        let repo = SigningKeysRepository::new(&store);
        let gid = test_group_id();
        let pk = PublicKey::from([0x01; 32]);
        // Deleting an absent key is a no-op, not an error.
        repo.delete_key(&gid, &pk).unwrap();
        repo.store_key(&gid, &pk, &[0xAB; 32]).unwrap();
        repo.delete_key(&gid, &pk).unwrap();
        assert!(repo.get_key(&gid, &pk).unwrap().is_none());
    }

    #[test]
    fn delete_all_for_group_clears_only_that_group() {
        let store = test_store();
        let repo = SigningKeysRepository::new(&store);
        let gid_a = test_group_id();
        let gid_b = ContextGroupId::from([0xBB; 32]);
        let pk = PublicKey::from([0x01; 32]);

        repo.store_key(&gid_a, &pk, &[0xAA; 32]).unwrap();
        repo.store_key(&gid_b, &pk, &[0xBB; 32]).unwrap();

        repo.delete_all_for_group(&gid_a).unwrap();

        assert!(repo.get_key(&gid_a, &pk).unwrap().is_none());
        assert_eq!(repo.get_key(&gid_b, &pk).unwrap(), Some([0xBB; 32]));
    }
}
