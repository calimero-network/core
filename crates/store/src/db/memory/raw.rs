use core::borrow::Borrow;
use core::mem::transmute;
use core::ops::Bound;
use std::collections::btree_map::{BTreeMap, Range as BTreeMapRange, Range};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use eyre::{bail, eyre, Result as EyreResult};
use thunderdome::{Arena, Index};

use crate::db::Column;
use crate::slice::Slice;

/// Safety to ensure all casts are valid
pub trait CastsTo<This> {}

impl CastsTo<Slice<'_>> for Slice<'_> {}

pub trait InMemoryDBImpl<'a> {
    type Key: AsRef<[u8]> + CastsTo<Slice<'a>>;
    type Value: CastsTo<Slice<'a>>;

    fn db(&self) -> &RwLock<InMemoryDBInner<Self::Key, Self::Value>>;

    fn key_from_slice(slice: Slice<'a>) -> Self::Key;
    fn value_from_slice(slice: Slice<'a>) -> Self::Value;
}

#[derive(Debug)]
pub struct DBArena<V> {
    // todo! Slice::clone points to the same object, can save one allocation here
    inner: Arc<RwLock<Arena<Arc<V>>>>,
}

impl<V> DBArena<V> {
    fn read(&self) -> EyreResult<RwLockReadGuard<'_, Arena<Arc<V>>>> {
        self.inner
            .read()
            .map_err(|_| eyre!("failed to acquire read lock on arena"))
    }

    fn write(&self) -> EyreResult<RwLockWriteGuard<'_, Arena<Arc<V>>>> {
        self.inner
            .write()
            .map_err(|_| eyre!("failed to acquire write lock on arena"))
    }
}

impl<V> Clone for DBArena<V> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<V> Default for DBArena<V> {
    fn default() -> Self {
        Self {
            inner: Arc::default(),
        }
    }
}

#[derive(Debug)]
pub struct InMemoryDBInner<K, V> {
    arena: DBArena<V>,
    links: BTreeMap<Column, BTreeMap<K, Arc<Index>>>,
}

impl<K, V> Default for InMemoryDBInner<K, V> {
    fn default() -> Self {
        Self {
            arena: DBArena::default(),
            links: BTreeMap::default(),
        }
    }
}

impl<K: Ord + Clone + Borrow<[u8]>, V> InMemoryDBInner<K, V> {
    pub fn get(&self, col: Column, key: &[u8]) -> EyreResult<Option<Arc<V>>> {
        let Some(column) = self.links.get(&col) else {
            return Ok(None);
        };

        let Some(idx) = column.get(key) else {
            return Ok(None);
        };

        let Some(value) = self.arena.read()?.get(**idx).cloned() else {
            return Err(eyre!(
                "inconsistent state, index points to non-existent value"
            ));
        };

        Ok(Some(value))
    }

    pub fn insert(&mut self, col: Column, key: K, value: V) -> EyreResult<()> {
        let idx = self.arena.write()?.insert(Arc::new(value));

        let column = self.links.entry(col).or_default();

        if let Some(idx) = column.insert(key, Arc::new(idx)) {
            if let Ok(idx) = Arc::try_unwrap(idx) {
                if self.arena.write()?.remove(idx).is_none() {
                    return Err(eyre!(
                        "inconsistent state, index points to non-existent value"
                    ));
                }
            }
        }

        Ok(())
    }

    pub fn remove(&mut self, col: Column, key: &[u8]) -> EyreResult<()> {
        let Some(column) = self.links.get_mut(&col) else {
            return Ok(());
        };

        if let Some(idx) = column.remove(key) {
            if let Ok(idx) = Arc::try_unwrap(idx) {
                let Some(_value) = self.arena.write()?.remove(idx) else {
                    return Err(eyre!(
                        "inconsistent state, index points to non-existent value"
                    ));
                };
            }
        }

        Ok(())
    }

    // TODO: We should consider returning Iterator here.
    #[expect(
        clippy::iter_not_returning_iterator,
        reason = "TODO: This should be implemented"
    )]
    pub fn iter<'a>(&self, col: Column) -> InMemoryIterInner<'a, K, V> {
        InMemoryIterInner {
            arena: self.arena.clone(),
            column: self.links.get(&col).cloned(),
            state: None,
        }
    }
}

#[derive(Debug)]
pub struct InMemoryIterInner<'a, K: Ord, V> {
    arena: DBArena<V>,
    column: Option<BTreeMap<K, Arc<Index>>>,
    state: Option<State<'a, K, V>>,
}

#[derive(Debug)]
struct State<'a, K, V> {
    range: BTreeMapRange<'a, K, Arc<Index>>,
    value: Option<Arc<V>>,
}

impl<K: Ord, V> Drop for InMemoryIterInner<'_, K, V> {
    fn drop(&mut self) {
        let Some(column) = self.column.as_mut() else {
            return;
        };

        while let Some((_, idx)) = column.pop_first() {
            if Arc::strong_count(&idx) == 1 {
                if let Ok(mut value) = self.arena.write() {
                    drop(value.remove(*idx));
                }
            }
        }
    }
}

impl<K, V> InMemoryIterInner<'_, K, V>
where
    K: Ord + Borrow<[u8]>,
{
    pub fn seek(&mut self, key: &[u8]) -> EyreResult<Option<&K>> {
        let Some(column) = self.column.as_ref() else {
            return Ok(None);
        };

        let range = column.range((Bound::Included(key), Bound::Unbounded));

        self.state = Some(State {
            // safety: range lives as long as self
            range: unsafe {
                transmute::<Range<'_, K, Arc<Index>>, Range<'_, K, Arc<Index>>>(range)
            },
            value: None,
        });

        self.next()
    }

    pub fn next(&mut self) -> EyreResult<Option<&K>> {
        let Some(column) = self.column.as_ref() else {
            return Ok(None);
        };

        let state = self.state.get_or_insert_with(|| State {
            // safety: range lives as long as self
            range: unsafe {
                transmute::<Range<'_, K, Arc<Index>>, Range<'_, K, Arc<Index>>>(column.range(..))
            },
            value: None,
        });

        let Some((key, idx)) = state.range.next() else {
            return Ok(None);
        };

        let Some(value) = self.arena.read()?.get(**idx).cloned() else {
            return Err(eyre!(
                "inconsistent state, index points to non-existent value"
            ));
        };

        state.value = Some(value);

        Ok(Some(key))
    }

    pub fn read(&self) -> EyreResult<&V> {
        let Some(state) = &self.state else {
            bail!("attempted to read from unadvanced iterator");
        };

        let Some(value) = &state.value else {
            bail!("missing value in iterator state");
        };

        Ok(value)
    }
}
