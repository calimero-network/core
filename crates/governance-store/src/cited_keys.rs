//! The key ids the stored ops of a group name in the clear, one row per `(group, key)`.
//!
//! A current member of a space may be served any epoch its ops cite; the index makes "does an
//! op of this group cite this key" a point lookup instead of a scan of the log.

use calimero_context_config::types::ContextGroupId;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::{Store, StoreBatch};
use eyre::Result as EyreResult;
use sha2::{Digest, Sha256};

/// 16-byte `Generic` scope of the rows.
const SCOPE: [u8; 16] = *b"calimero-opcited";

pub struct CitedKeyRepository<'a> {
    store: &'a Store,
}

impl<'a> CitedKeyRepository<'a> {
    #[must_use]
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Stage that an op of `group` cites `key_id`. A repeat writes the same row.
    pub(crate) fn stage(
        batch: &mut StoreBatch<'_>,
        group: &ContextGroupId,
        key_id: &[u8; 32],
    ) -> EyreResult<()> {
        let _ = batch.put(&Self::key(group, key_id), &Self::value())?;
        Ok(())
    }

    /// Whether a stored op of `group` cites `key_id`.
    pub fn cites(&self, group: &ContextGroupId, key_id: &[u8; 32]) -> EyreResult<bool> {
        Ok(self
            .store
            .handle()
            .get(&Self::key(group, key_id))?
            .is_some())
    }

    fn key(group: &ContextGroupId, key_id: &[u8; 32]) -> GenericKey {
        let mut hasher = Sha256::new();
        hasher.update(group.to_bytes());
        hasher.update(key_id);
        GenericKey::new(SCOPE, hasher.finalize().into())
    }

    fn value() -> GenericData<'static> {
        GenericData::from(Slice::from(vec![1u8]))
    }
}
