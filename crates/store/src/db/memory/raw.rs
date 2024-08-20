use std::borrow::Borrow;
use std::collections::btree_map::{BTreeMap, Range as BTreeMapRange};
use std::ops::Bound;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::db::Column;
use crate::slice::Slice;

/// Safety to ensure all casts are valid
pub trait CastsTo<This> {}

impl<'a, 'b> CastsTo<Slice<'a>> for Slice<'b> {}

pub trait InMemoryDBImpl<'a> {
    type Key: AsRef<[u8]> + CastsTo<Slice<'a>>;
    type Value: CastsTo<Slice<'a>>;

    fn db(&self) -> &RwLock<InMemoryDBInner<Self::Key, Self::Value>>;

    fn key_from_slice(slice: Slice<'a>) -> Self::Key;
    fn value_from_slice(slice: Slice<'a>) -> Self::Value;
}

pub struct DBArena<V> {
    // todo! Slice::clone points to the same object, can save one allocation here
    inner: Arc<RwLock<thunderdome::Arena<Arc<V>>>>,
}

impl<V> DBArena<V> {
    fn read(&self) -> eyre::Result<RwLockReadGuard<thunderdome::Arena<Arc<V>>>> {
        self.inner
            .read()
            .map_err(|_| eyre::eyre!("failed to acquire read lock on arena"))
    }

    fn write(&self) -> eyre::Result<RwLockWriteGuard<thunderdome::Arena<Arc<V>>>> {
        self.inner
            .write()
            .map_err(|_| eyre::eyre!("failed to acquire write lock on arena"))
    }
}

impl<V> Clone for DBArena<V> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<V> Default for DBArena<V> {
    fn default() -> Self {
        Self {
            inner: Default::default(),
        }
    }
}

pub struct InMemoryDBInner<K, V> {
    arena: DBArena<V>,
    links: BTreeMap<Column, BTreeMap<K, Arc<thunderdome::Index>>>,
}

impl<K, V> Default for InMemoryDBInner<K, V> {
    fn default() -> Self {
        Self {
            arena: Default::default(),
            links: Default::default(),
        }
    }
}

impl<K: Ord + Clone + Borrow<[u8]>, V> InMemoryDBInner<K, V> {
    pub fn get(&self, col: Column, key: &[u8]) -> eyre::Result<Option<Arc<V>>> {
        let Some(column) = self.links.get(&col) else {
            return Ok(None);
        };

        let Some(idx) = column.get(key) else {
            return Ok(None);
        };

        let arena = self.arena.read()?;

        let Some(value) = arena.get(**idx) else {
            panic!("inconsistent state, index points to non-existent value");
        };

        Ok(Some(value.clone()))
    }

    pub fn insert(&mut self, col: Column, key: K, value: V) -> eyre::Result<()> {
        let idx = self.arena.write()?.insert(Arc::new(value));

        let column = self.links.entry(col).or_default();

        if let Some(idx) = column.insert(key, Arc::new(idx)) {
            if let Ok(idx) = Arc::try_unwrap(idx) {
                if self.arena.write()?.remove(idx).is_none() {
                    panic!("inconsistent state, index points to non-existent value");
                };
            }
        }

        Ok(())
    }

    pub fn remove(&mut self, col: Column, key: &[u8]) -> eyre::Result<()> {
        let Some(column) = self.links.get_mut(&col) else {
            return Ok(());
        };

        if let Some(idx) = column.remove(&key) {
            if let Ok(idx) = Arc::try_unwrap(idx) {
                let Some(_value) = self.arena.write()?.remove(idx) else {
                    panic!("inconsistent state, index points to non-existent value")
                };
            }
        }

        Ok(())
    }

    pub fn iter<'a>(&self, col: Column) -> InMemoryIterInner<'a, K, V> {
        InMemoryIterInner {
            arena: self.arena.clone(),
            column: self.links.get(&col).cloned(),
            state: None,
        }
    }
}

pub struct InMemoryIterInner<'a, K: Ord, V> {
    arena: DBArena<V>,
    column: Option<BTreeMap<K, Arc<thunderdome::Index>>>,
    state: Option<State<'a, K, V>>,
}

struct State<'a, K, V> {
    range: BTreeMapRange<'a, K, Arc<thunderdome::Index>>,
    value: Option<Arc<V>>,
}

impl<'a, K: Ord, V> Drop for InMemoryIterInner<'a, K, V> {
    fn drop(&mut self) {
        let Some(column) = self.column.as_mut() else {
            return;
        };

        while let Some((_, idx)) = column.pop_first() {
            if Arc::strong_count(&idx) == 1 {
                if let Ok(mut value) = self.arena.write() {
                    value.remove(*idx);
                }
            }
        }
    }
}

impl<'a, K, V> InMemoryIterInner<'a, K, V>
where
    K: Ord + Borrow<[u8]>,
{
    pub fn seek(&mut self, key: &[u8]) -> eyre::Result<Option<&K>> {
        let Some(column) = self.column.as_ref() else {
            return Ok(None);
        };

        let range = column.range((Bound::Included(key.as_ref()), Bound::Unbounded));

        self.state = Some(State {
            // safety: range lives as long as self
            range: unsafe { std::mem::transmute(range) },
            value: None,
        });

        self.next()
    }

    pub fn next(&mut self) -> eyre::Result<Option<&K>> {
        let Some(column) = self.column.as_ref() else {
            return Ok(None);
        };

        let state = self.state.get_or_insert_with(|| State {
            // safety: range lives as long as self
            range: unsafe { std::mem::transmute(column.range(..)) },
            value: None,
        });

        let Some((key, idx)) = state.range.next() else {
            return Ok(None);
        };

        let arena = self.arena.read()?;

        let value = arena
            .get(**idx)
            .expect("inconsistent state, index points to non-existent value");

        state.value = Some(value.clone());

        Ok(Some(key))
    }

    pub fn read(&self) -> eyre::Result<&V> {
        let Some(state) = &self.state else {
            eyre::bail!("attempted to read from unadvanced iterator");
        };

        let Some(value) = &state.value else {
            eyre::bail!("missing value in iterator state");
        };

        Ok(value)
    }
}
