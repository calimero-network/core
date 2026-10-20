//! Namespaces whose purge reached this node before the account's own leave did, node-local
//! and never synced: the purge is done once the leave is applied here, also after a restart.

use calimero_context_config::types::ContextGroupId;
use calimero_store::iter::DBIter;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use eyre::Result as EyreResult;

/// 16-byte `Generic` scope of the rows, keyed by namespace id.
const SCOPE: [u8; 16] = *b"calimero-nspurge";

/// Typed repository for the waiting purges.
pub struct PendingNamespacePurges<'a> {
    store: &'a Store,
}

impl<'a> PendingNamespacePurges<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Remember that `namespace` is to be purged once the account's leave is applied.
    /// Idempotent.
    pub fn arm(&self, namespace: &ContextGroupId) -> EyreResult<()> {
        let marker = GenericData::from(Slice::from(&[1u8][..]));
        self.store.handle().put(&key(namespace), &marker)?;
        Ok(())
    }

    /// Whether a purge of `namespace` is waiting.
    pub fn is_armed(&self, namespace: &ContextGroupId) -> EyreResult<bool> {
        Ok(self.store.handle().has(&key(namespace))?)
    }

    /// Drop the waiting purge, because it ran or no longer applies. Idempotent.
    pub fn disarm(&self, namespace: &ContextGroupId) -> EyreResult<()> {
        self.store.handle().delete(&key(namespace))?;
        Ok(())
    }

    /// Every namespace with a purge waiting.
    pub fn armed(&self) -> EyreResult<Vec<ContextGroupId>> {
        let handle = self.store.handle();
        let mut iter = handle.iter::<GenericKey>()?;
        let mut start = [0; 48];
        start[..16].copy_from_slice(&SCOPE);
        let mut found = Vec::new();
        let mut next = DBIter::seek(&mut iter, Slice::from(&start[..]))?.and_then(namespace_of);
        while let Some(namespace) = next {
            found.push(namespace);
            next = DBIter::next(&mut iter)?.and_then(namespace_of);
        }
        Ok(found)
    }
}

fn key(namespace: &ContextGroupId) -> GenericKey {
    GenericKey::new(SCOPE, namespace.to_bytes())
}

fn namespace_of(key: Slice<'_>) -> Option<ContextGroupId> {
    let fragment: [u8; 32] = key.as_ref().strip_prefix(&SCOPE[..])?.try_into().ok()?;
    Some(ContextGroupId::from(fragment))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_store::db::InMemoryDB;

    use super::*;

    #[test]
    fn a_waiting_purge_is_listed_until_it_is_disarmed() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let repo = PendingNamespacePurges::new(&store);
        let (one, two) = (ContextGroupId::from([1; 32]), ContextGroupId::from([2; 32]));
        assert_eq!(repo.armed().unwrap(), vec![]);

        repo.arm(&one).unwrap();
        repo.arm(&two).unwrap();
        repo.arm(&two).unwrap();
        assert!(repo.is_armed(&one).unwrap());
        assert_eq!(repo.armed().unwrap(), vec![one, two]);

        repo.disarm(&one).unwrap();
        assert!(!repo.is_armed(&one).unwrap());
        assert_eq!(repo.armed().unwrap(), vec![two]);
    }
}
