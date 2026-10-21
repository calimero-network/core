//! Namespaces to purge once the account's own leave is applied here, node-local and never
//! synced, also after a restart. Every purging leave folded on this node's own account
//! namespace arms one; a rejoin or plain leave withdraws it.
//! The decision is the last purge, rejoin or leave folded on this node.

use calimero_context_config::types::ContextGroupId;
use calimero_store::iter::DBIter;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use eyre::Result as EyreResult;
use std::sync::Mutex;

/// 16-byte `Generic` scope of the rows, keyed by namespace id.
const SCOPE: [u8; 16] = *b"calimero-nspurge";

/// Serialises the read-then-write of `disarm_if` against `arm` and `disarm`.
static ROWS: Mutex<()> = Mutex::new(());

/// Typed repository for the waiting purges.
pub struct PendingNamespacePurges<'a> {
    store: &'a Store,
}

impl<'a> PendingNamespacePurges<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Arm a purge of `namespace`, under a fresh token. Re-arming replaces the token.
    pub fn arm(&self, namespace: &ContextGroupId) -> EyreResult<()> {
        let _rows = ROWS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let token = rand::random::<u64>().to_be_bytes();
        let marker = GenericData::from(Slice::from(&token[..]));
        self.store.handle().put(&key(namespace), &marker)?;
        Ok(())
    }

    /// Whether a purge of `namespace` is waiting.
    pub fn is_armed(&self, namespace: &ContextGroupId) -> EyreResult<bool> {
        Ok(self.store.handle().has(&key(namespace))?)
    }

    /// The token of the waiting purge of `namespace`, if any.
    pub fn token(&self, namespace: &ContextGroupId) -> EyreResult<Option<Vec<u8>>> {
        let handle = self.store.handle();
        let token = handle
            .get(&key(namespace))?
            .map(|row| row.as_ref().to_vec());
        Ok(token)
    }

    /// Drop the waiting purge, because it no longer applies. Idempotent.
    pub fn disarm(&self, namespace: &ContextGroupId) -> EyreResult<()> {
        let _rows = ROWS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.store.handle().delete(&key(namespace))?;
        Ok(())
    }

    /// Drop the waiting purge only if it still holds `token`: a purge that ran must not
    /// clear one armed after it started.
    pub fn disarm_if(&self, namespace: &ContextGroupId, token: &[u8]) -> EyreResult<()> {
        let _rows = ROWS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.token(namespace)?.as_deref() == Some(token) {
            self.store.handle().delete(&key(namespace))?;
        }
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

    #[test]
    fn a_finished_purge_clears_only_the_arm_it_started_from() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let repo = PendingNamespacePurges::new(&store);
        let namespace = ContextGroupId::from([3; 32]);

        repo.arm(&namespace).unwrap();
        let started = repo.token(&namespace).unwrap().unwrap();
        repo.disarm(&namespace).unwrap();
        repo.arm(&namespace).unwrap();
        repo.disarm_if(&namespace, &started).unwrap();
        assert!(repo.is_armed(&namespace).unwrap());

        let current = repo.token(&namespace).unwrap().unwrap();
        repo.disarm_if(&namespace, &current).unwrap();
        assert!(!repo.is_armed(&namespace).unwrap());
    }
}
