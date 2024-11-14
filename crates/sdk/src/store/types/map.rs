use core::fmt;
use std::fmt::Debug;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::{Deref, DerefMut};

use borsh::{BorshDeserialize, BorshSerialize};

use crate::store::base::Id;
use crate::store::entry::{Entry, EntryMut};

#[derive(BorshSerialize, BorshDeserialize)]
pub struct Map<K: Debug, V: Debug> {
    inner: Entry<(K, V)>,
}

impl<K, V> fmt::Debug for Map<K, V>
where
    K: fmt::Debug + BorshSerialize + BorshDeserialize,
    V: fmt::Debug + BorshSerialize + BorshDeserialize,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.entries()).finish()
    }
}

impl<K: Debug, V: Debug> Map<K, V>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
{
    pub fn new() -> Self {
        Self {
            inner: Entry::new_dangling(),
        }
    }

    pub fn insert(&mut self, key: K, value: V)
    where
        K: Hash,
    {
        let id = self.derive_id(&key);

        self.inner.insert(id, (key, value));
    }

    pub fn get(&self, key: &K) -> Option<&V>
    where
        K: Hash,
    {
        let id = self.derive_id(key);

        self.inner.get(&id).map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn entries(&self) -> impl Iterator<Item = (&K, &V)> {
        self.inner.entries().map(|(k, v)| (k, v))
    }

    pub fn get_mut(&mut self, key: &K) -> Option<ValueMut<'_, K, V>>
    where
        K: Hash,
    {
        let id = self.derive_id(key);

        let inner = self.inner.get_mut(&id)?;

        Some(ValueMut { inner })
    }

    pub fn remove(&mut self, key: &K) -> Option<V>
    where
        K: Hash,
    {
        let id = self.derive_id(key);

        let item = self.inner.get_mut(&id)?;
        let (_, value) = item.remove();
        Some(value)
    }
}

pub struct ValueMut<'a, K, V> {
    inner: EntryMut<'a, (K, V)>,
}

impl<K, V> Deref for ValueMut<'_, K, V>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
{
    type Target = V;

    fn deref(&self) -> &Self::Target {
        let (_, v) = self.inner.get();
        v
    }
}

impl<K, V> DerefMut for ValueMut<'_, K, V>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        let (_, v) = self.inner.get_mut();
        v
    }
}

impl<K: Hash + Debug, V: Debug> Map<K, V> {
    fn derive_id(&self, key: &K) -> Id {
        let mut bytes = [0; 32];

        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let key = hasher.finish().to_le_bytes();

        bytes[..8].copy_from_slice(&key);

        let mut hasher = DefaultHasher::new();
        self.inner.id().hash(&mut hasher);
        let key = hasher.finish().to_le_bytes();

        bytes[(32 - 8)..].copy_from_slice(&key);

        Id::from(bytes)
    }
}

#[cfg(test)]
#[path = "map_tests.rs"]
mod tests;
