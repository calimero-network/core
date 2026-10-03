//! Key rotations that rode an op this node could not open on arrival, kept with
//! the DAG sequence the op took so the key-arrival replay can apply them.

use calimero_governance_types::NamespaceId;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use eyre::Result as EyreResult;
use sha2::{Digest, Sha256};

/// 16-byte `Generic` scope of the rows.
const SCOPE: [u8; 16] = *b"calimero-rotlate";

pub(crate) struct DeferredRotations<'a> {
    store: &'a Store,
    namespace: NamespaceId,
}

impl<'a> DeferredRotations<'a> {
    pub(crate) fn new(store: &'a Store, namespace: NamespaceId) -> Self {
        Self { store, namespace }
    }

    /// Record that the rotation riding `op` was not applied, at `sequence`.
    pub(crate) fn defer(&self, op: [u8; 32], sequence: u64) -> EyreResult<()> {
        let key = self.key(op);
        let data = GenericData::from(Slice::from(sequence.to_le_bytes().to_vec()));
        self.store.handle().put(&key, &data)?;
        Ok(())
    }

    /// The sequence `op` took when its rotation was deferred, or `None` when none is owed.
    pub(crate) fn sequence(&self, op: [u8; 32]) -> EyreResult<Option<u64>> {
        let key = self.key(op);
        let handle = self.store.handle();
        let Some(data) = handle.get(&key)? else {
            return Ok(None);
        };
        let bytes: &[u8] = data.as_ref();
        let sequence = bytes
            .try_into()
            .map(u64::from_le_bytes)
            .map_err(|_| eyre::eyre!("deferred rotation row is not a u64"))?;
        Ok(Some(sequence))
    }

    pub(crate) fn forget(&self, op: [u8; 32]) -> EyreResult<()> {
        let key = self.key(op);
        let mut handle = self.store.handle();
        handle.delete(&key)?;
        Ok(())
    }

    fn key(&self, op: [u8; 32]) -> GenericKey {
        let mut hasher = Sha256::new();
        hasher.update(self.namespace.as_bytes());
        hasher.update(op);
        GenericKey::new(SCOPE, hasher.finalize().into())
    }
}
