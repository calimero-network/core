//! `Tally<K>`: signed counters keyed by `K`, one integer at `(name, "t", key..)` each.
//!
//! Key layout: `(name, "t", key..)` holds a CBOR integer changed only with `Store::add`, so
//! a tally never reads before it writes and concurrent adds never conflict (D47). The host
//! writes the same keys for the collections the state ABI lists.

use core::marker::PhantomData;

use crate::app::StateField;
use crate::encoding::key::{self, KeyPart, ToKey};
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{check_key, DefaultStore, Store};

/// A set of signed counters, one per key. `add` never reads, so adds never conflict.
pub struct Tally<K: ToKey, S: Store = DefaultStore> {
    name: String,
    store: S,
    key: PhantomData<fn(&K)>,
}

impl<K: ToKey, S: Store> StateField for Tally<K, S> {
    const KIND: &'static str = "tally";
}

impl<K: ToKey, S: Store + Default> Tally<K, S> {
    /// A tally on the default store.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<K: ToKey, S: Store> Tally<K, S> {
    /// A tally on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: name.into(),
            store,
            key: PhantomData,
        }
    }

    fn storage_key(&self, k: &K) -> Result<Vec<u8>, StorageError> {
        let mut parts = vec![KeyPart::Str(self.name.clone()), KeyPart::Str("t".into())];
        parts.extend(k.to_key());
        let encoded = key::encode(&parts);
        check_key(&encoded)?;
        Ok(encoded)
    }

    /// Add `delta` to the counter at `key` without reading it (`storage.add`).
    pub fn add(&mut self, key: &K, delta: i64) -> Result<(), StorageError> {
        via_host!(
            S,
            crate::host_collections::tally_add(&self.name, &key.to_key(), delta)
        );
        let k = self.storage_key(key)?;
        self.store.add(&k, delta);
        Ok(())
    }

    /// The counter at `key`; 0 when nothing was ever added.
    pub fn get(&self, key: &K) -> Result<i64, StorageError> {
        via_host!(
            S,
            crate::host_collections::tally_get(&self.name, &key.to_key())
        );
        match self.store.get(&self.storage_key(key)?) {
            Some(bytes) => {
                let n = value::int_from_slice(&bytes)?;
                i64::try_from(n).map_err(|_| StorageError::BadCounter(self.name.clone()))
            }
            None => Ok(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::key::{self, KeyPart};
    use crate::encoding::value;
    use crate::store::{MemStore, Store};

    fn tally() -> (Tally<String, MemStore>, MemStore) {
        let store = MemStore::new();
        (Tally::with_store("votes", store.clone()), store)
    }

    #[test]
    fn tally_adds_without_reading() {
        let (mut t, store) = tally();
        t.add(&"a".to_owned(), 2).unwrap();
        t.add(&"a".to_owned(), 3).unwrap();
        let k = key::encode(&[
            KeyPart::Str("votes".into()),
            KeyPart::Str("t".into()),
            KeyPart::Str("a".into()),
        ]);
        assert_eq!(store.len(), 1);
        assert_eq!(store.get(&k).unwrap(), value::int_to_vec(5).unwrap());
        // Two `add`s and no `put`: the tally never read-modify-wrote the key.
        assert_eq!(store.op_counts(&k), (0, 2));
    }

    #[test]
    fn tally_get_of_absent_is_zero() {
        let (t, _) = tally();
        assert_eq!(t.get(&"nobody".to_owned()).unwrap(), 0);
    }
}
