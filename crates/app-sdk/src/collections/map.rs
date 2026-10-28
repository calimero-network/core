//! `UnorderedMap<K, V>`: keyed values at `(name, "m", key..)`.

use core::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::encoding::key::{self, KeyPart, ToKey};
use crate::encoding::value;
use crate::error::StorageError;
use crate::store::{check_key, DefaultStore, Store};

/// A map from tuple keys to CBOR values.
pub struct UnorderedMap<K: ToKey, V, S: Store = DefaultStore> {
    name: String,
    store: S,
    _types: PhantomData<fn() -> (K, V)>,
}

impl<K: ToKey, V: Serialize + DeserializeOwned, S: Store + Default> UnorderedMap<K, V, S> {
    /// A map on the default store; the name is part of every key.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<K: ToKey, V: Serialize + DeserializeOwned, S: Store> UnorderedMap<K, V, S> {
    /// A map on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: name.into(),
            store,
            _types: PhantomData,
        }
    }

    fn storage_key(&self, k: &K) -> Result<Vec<u8>, StorageError> {
        let mut parts = vec![KeyPart::Str(self.name.clone()), KeyPart::Str("m".into())];
        parts.extend(k.to_key());
        let encoded = key::encode(&parts);
        check_key(&encoded)?;
        Ok(encoded)
    }

    /// The value at `k`.
    pub fn get(&self, k: &K) -> Result<Option<V>, StorageError> {
        self.store
            .get(&self.storage_key(k)?)
            .map(|bytes| value::from_slice(&bytes))
            .transpose()
    }

    /// Whether `k` has a value.
    pub fn contains(&self, k: &K) -> Result<bool, StorageError> {
        Ok(self.store.get(&self.storage_key(k)?).is_some())
    }

    /// Set the value at `k`.
    pub fn insert(&mut self, k: &K, v: &V) -> Result<(), StorageError> {
        let key = self.storage_key(k)?;
        let bytes = value::to_vec(v)?;
        self.store.put(&key, &bytes);
        Ok(())
    }

    /// Remove `k`.
    pub fn remove(&mut self, k: &K) -> Result<(), StorageError> {
        let key = self.storage_key(k)?;
        self.store.delete(&key);
        Ok(())
    }
}
