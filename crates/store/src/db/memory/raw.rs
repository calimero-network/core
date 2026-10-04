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
    // The `for<'x>` bound makes the cast lifetime-agnostic (it holds for every
    // `'x` already, via `impl CastsTo<Slice<'_>> for Slice<'_>`). This lets the
    // database impl recover the value as a `Slice<'static>` when it owns it,
    // instead of being pinned to `'a`, which is what made `ArcSlice::new`'s
    // lifetime laundering necessary in the first place.
    type Key: AsRef<[u8]> + for<'x> CastsTo<Slice<'x>>;
    type Value: for<'x> CastsTo<Slice<'x>>;

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

/// One column's keys, shared copy-on-write with the iterators that read it.
///
/// An iterator reads a snapshot: what the column held when it was made, whatever
/// is written afterwards. Cloning the map for every iterator made that snapshot
/// cost O(rows in the column), so a prefix scan over a few keys paid for the
/// whole column. The `Arc` makes the snapshot a refcount bump instead, and a
/// write clones the map only while an iterator still holds it — at most once per
/// iterator, and only when something writes during the iteration.
type ColumnMap<K> = Arc<BTreeMap<K, Arc<Index>>>;

#[derive(Debug)]
pub struct InMemoryDBInner<K, V> {
    arena: DBArena<V>,
    links: BTreeMap<Column, ColumnMap<K>>,
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

        let column = Arc::make_mut(self.links.entry(col).or_default());

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
        if !column.contains_key(key) {
            // Nothing to remove, so no reason to copy a column an iterator holds.
            return Ok(());
        }

        if let Some(idx) = Arc::make_mut(column).remove(key) {
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

    /// Values the arena holds, live or kept alive by a snapshot. A test reads it
    /// to check that dropping the last snapshot reclaims what only it held.
    #[cfg(test)]
    pub fn arena_len(&self) -> usize {
        self.arena.read().map_or(0, |arena| arena.len())
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
    column: Option<ColumnMap<K>>,
    state: Option<State<'a, K, V>>,
}

#[derive(Debug)]
struct State<'a, K, V> {
    range: BTreeMapRange<'a, K, Arc<Index>>,
    value: Option<Arc<V>>,
}

impl<K: Ord, V> Drop for InMemoryIterInner<'_, K, V> {
    fn drop(&mut self) {
        // The range borrows the snapshot; end it before the snapshot goes.
        self.state = None;

        // Only the last holder of a snapshot can hold indices nothing else does.
        // While the live column or another iterator still shares it, every
        // index in it is reachable from there, so there is nothing to reclaim.
        let Some(mut column) = self.column.take().and_then(Arc::into_inner) else {
            return;
        };

        // Hold the arena write lock across the whole drain so each
        // `strong_count == 1` check and its matching `remove` are one atomic
        // step against other arena mutations. The previous per-entry
        // lock/unlock left a window where a concurrent operation could observe
        // or resurrect an index between the count check and the removal.
        let Ok(mut arena) = self.arena.write() else {
            return;
        };
        while let Some((_, idx)) = column.pop_first() {
            // This snapshot is the sole remaining holder of the index — the
            // live column already dropped its copy — so the arena slot is now
            // unreachable and safe to reclaim.
            if Arc::strong_count(&idx) == 1 {
                drop(arena.remove(*idx));
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
