use std::borrow::Borrow;
use std::collections::{btree_map, BTreeMap};
use std::ops::Bound;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::config::StoreConfig;
use crate::db::{Column, Database};
use crate::iter::{DBIter, Iter};
use crate::slice::Slice;
use crate::tx::{Operation, Transaction};

struct DBArena<V> {
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

struct InMemoryDBInner<K, V> {
    arena: DBArena<V>,
    links: BTreeMap<Column, BTreeMap<K, Arc<thunderdome::Index>>>,
}

impl<K: Ord + Clone + Borrow<[u8]>, V> InMemoryDBInner<K, V> {
    fn get(&self, col: Column, key: &[u8]) -> eyre::Result<Option<Arc<V>>> {
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

    fn insert(&mut self, col: Column, key: K, value: V) -> eyre::Result<()> {
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

    fn remove(&mut self, col: Column, key: &[u8]) -> eyre::Result<()> {
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

    fn iter<'a>(&self, col: Column) -> InMemoryIterInner<'a, K, V> {
        InMemoryIterInner {
            arena: self.arena.clone(),
            column: self.links.get(&col).cloned(),
            state: None,
        }
    }
}

mod private {
    /// Safety to ensure all casts are valid
    pub trait CastsTo<This> {}
}

use private::CastsTo;

impl<'a, 'b> CastsTo<Slice<'a>> for Slice<'b> {}

pub trait InMemoryDBImpl<'a> {
    type Key: AsRef<[u8]> + CastsTo<Slice<'a>>;
    type Value: CastsTo<Slice<'a>>;

    fn db(&self) -> &RwLock<InMemoryDBInner<Self::Key, Self::Value>>;

    fn key_from_slice(slice: Slice<'a>) -> Self::Key;
    fn value_from_slice(slice: Slice<'a>) -> Self::Value;
}

pub struct Ref<'a> {
    inner: Arc<RwLock<InMemoryDBInner<Slice<'a>, Slice<'a>>>>,
}

impl<'a> InMemoryDBImpl<'a> for Ref<'a> {
    type Key = Slice<'a>;
    type Value = Slice<'a>;

    fn db(&self) -> &RwLock<InMemoryDBInner<Self::Key, Self::Value>> {
        &self.inner
    }

    fn key_from_slice(slice: Slice<'a>) -> Self::Key {
        slice
    }

    fn value_from_slice(slice: Slice<'a>) -> Self::Value {
        slice
    }
}

pub struct Owned {
    inner: Arc<RwLock<InMemoryDBInner<Slice<'static>, Slice<'static>>>>,
}

impl<'a> InMemoryDBImpl<'a> for Owned {
    type Key = Slice<'static>;
    type Value = Slice<'static>;

    fn db(&self) -> &RwLock<InMemoryDBInner<Self::Key, Self::Value>> {
        &self.inner
    }

    fn key_from_slice(slice: Slice<'a>) -> Self::Key {
        slice.into_boxed().into()
    }

    fn value_from_slice(slice: Slice<'a>) -> Self::Value {
        slice.into_boxed().into()
    }
}

pub struct InMemoryDB<T> {
    inner: T,
}

// todo! vvvvv remove this once miraclx/slice/multi-thread-capable is merged in
unsafe impl<T> Sync for InMemoryDB<T> {}
unsafe impl<T> Send for InMemoryDB<T> {}
// todo! ^^^^^ remove this once miraclx/slice/multi-thread-capable is merged in

impl InMemoryDB<()> {
    pub fn referenced<'a>() -> InMemoryDB<Ref<'a>> {
        InMemoryDB {
            inner: Ref {
                inner: Arc::new(RwLock::new(InMemoryDBInner {
                    arena: DBArena::default(),
                    links: BTreeMap::new(),
                })),
            },
        }
    }

    pub fn owned() -> InMemoryDB<Owned> {
        InMemoryDB {
            inner: Owned {
                inner: Arc::new(RwLock::new(InMemoryDBInner {
                    arena: DBArena::default(),
                    links: BTreeMap::new(),
                })),
            },
        }
    }
}

impl<'a, T: InMemoryDBImpl<'a>> InMemoryDB<T> {
    fn db(&self) -> eyre::Result<RwLockReadGuard<InMemoryDBInner<T::Key, T::Value>>> {
        self.inner
            .db()
            .read()
            .map_err(|_| eyre::eyre!("failed to acquire read lock on db"))
    }

    fn db_mut(&self) -> eyre::Result<RwLockWriteGuard<InMemoryDBInner<T::Key, T::Value>>> {
        self.inner
            .db()
            .write()
            .map_err(|_| eyre::eyre!("failed to acquire write lock on db"))
    }
}

struct ArcSlice<'this> {
    inner: Arc<Slice<'this>>,
}

impl<'this> ArcSlice<'this> {
    fn new<'a, T: CastsTo<Slice<'a>>>(value: Arc<T>) -> Self {
        Self {
            inner: unsafe { std::mem::transmute(value) },
        }
    }
}

impl<'this> AsRef<[u8]> for ArcSlice<'this> {
    fn as_ref(&self) -> &[u8] {
        &self.inner
    }
}

impl<'a, T: InMemoryDBImpl<'a>> Database<'a> for InMemoryDB<T>
where
    T::Key: Ord
        + Clone
        + Borrow<[u8]>
        //vv\
        + 'a,
    //  ~~^^~ this piece is weird, we don't exactly "need" it for InMemoryIter
    //        but Rust forces it on us since it specifies a constraint of `CastsTo<Slice<'a>>`
    //        The same doesn't apply for T::Value, even though they're both used in the same way.
    //        Dropping that constraint fixes this requirement, but it exists as to guard against improper use
    //        but I'll like to understand why it's happening in the first place, I'll leave it in until there's
    //        an observable issue
{
    fn open(_config: &StoreConfig) -> eyre::Result<Self> {
        todo!("phase this out, please. it's not even worth writing an accomodation for")
    }

    fn has(&self, col: Column, key: Slice) -> eyre::Result<bool> {
        self.get(col, key).map(|v| v.is_some())
    }

    fn get(&self, col: Column, key: Slice) -> eyre::Result<Option<Slice>> {
        let db = self.db()?;

        let Some(value) = db.get(col, &key)? else {
            return Ok(None);
        };

        Ok(Some(Slice::from_owned(ArcSlice::new(value))))
    }

    fn put(&self, col: Column, key: Slice<'a>, value: Slice<'a>) -> eyre::Result<()> {
        let mut db = self.db_mut()?;

        db.insert(col, T::key_from_slice(key), T::value_from_slice(value))?;

        Ok(())
    }

    fn delete(&self, col: Column, key: Slice) -> eyre::Result<()> {
        let mut db = self.db_mut()?;

        db.remove(col, &key)?;

        Ok(())
    }

    fn iter(&self, col: Column) -> eyre::Result<Iter> {
        let db = self.db()?;

        Ok(Iter::new(InMemoryDBIter::new(db.iter(col))))
    }

    fn apply(&self, tx: &Transaction) -> eyre::Result<()> {
        let mut db = self.db_mut()?;

        for (entry, op) in tx.iter() {
            // todo! move to Inner
            match op {
                Operation::Put { value } => {
                    db.insert(
                        entry.column(),
                        T::key_from_slice(entry.key().to_owned().into()),
                        T::value_from_slice(value.as_ref().to_owned().into()),
                    )?;
                }
                Operation::Delete => {
                    db.remove(entry.column(), entry.key().into())?;
                }
            }
        }

        Ok(())
    }
}

struct InMemoryIterInner<'a, K: Ord, V> {
    arena: DBArena<V>,
    column: Option<BTreeMap<K, Arc<thunderdome::Index>>>,
    state: Option<State<'a, K, V>>,
}

struct State<'a, K, V> {
    range: btree_map::Range<'a, K, Arc<thunderdome::Index>>,
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
    fn seek(&mut self, key: &[u8]) -> eyre::Result<Option<&K>> {
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

    fn next(&mut self) -> eyre::Result<Option<&K>> {
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

    fn read(&self) -> eyre::Result<&V> {
        let Some(state) = &self.state else {
            eyre::bail!("attempted to read from unadvanced iterator");
        };

        let Some(value) = &state.value else {
            eyre::bail!("missing value in iterator state");
        };

        Ok(value)
    }
}

struct InMemoryDBIter<'this> {
    inner: InMemoryIterInner<'this, Slice<'this>, Slice<'this>>,
}

impl<'this> InMemoryDBIter<'this> {
    fn new<'a, K, V>(inner: InMemoryIterInner<'a, K, V>) -> Self
    where
        K: Ord + CastsTo<Slice<'a>>,
        V: CastsTo<Slice<'a>>,
    {
        unsafe { std::mem::transmute(inner) }
    }
}

impl<'this> DBIter for InMemoryDBIter<'this> {
    fn seek(&mut self, key: Slice) -> eyre::Result<Option<Slice>> {
        let Some(key) = self.inner.seek(&key)? else {
            return Ok(None);
        };

        Ok(Some(key.into()))
    }

    fn next(&mut self) -> eyre::Result<Option<Slice>> {
        let Some(key) = self.inner.next()? else {
            return Ok(None);
        };

        Ok(Some(key.into()))
    }

    fn read(&self) -> eyre::Result<Slice> {
        self.inner.read().map(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryDB;
    use crate::db::{Column, Database};
    use crate::slice::Slice;

    #[test]
    fn test_owned_memory() {
        let db = InMemoryDB::owned();

        for b1 in 0..10 {
            for b2 in 0..10 {
                let bytes = [b1, b2];

                let key = Slice::from(&bytes[..]);
                let value = Slice::from(&bytes[..]);

                db.put(Column::Identity, (&key).into(), (&value).into())
                    .unwrap();

                assert!(db.has(Column::Identity, (&key).into()).unwrap());
                assert_eq!(db.get(Column::Identity, key).unwrap().unwrap(), value);
            }
        }

        assert_eq!(None, db.get(Column::Identity, (&[]).into()).unwrap());

        let mut iter = db.iter(Column::Identity).unwrap();

        iter.seek((&[]).into()).unwrap();

        let mut entries = iter.entries();

        for b1 in 0..10 {
            for b2 in 0..10 {
                let bytes = [b1, b2];

                let key = Slice::from(&bytes[..]);
                let value = Slice::from(&bytes[..]);

                let (k, v) = entries.next().unwrap().unwrap();

                assert_eq!(k, key);
                assert_eq!(v, value);
            }
        }
    }

    #[test]
    fn test_ref_memory() {
        let db = InMemoryDB::referenced();

        for b1 in 0..10 {
            for b2 in 0..10 {
                let bytes = [b1, b2];

                let key = Slice::from(&bytes[..]);
                let value = Slice::from(&bytes[..]);

                db.put(
                    Column::Identity,
                    key.clone().into_boxed().into(),
                    value.clone().into_boxed().into(),
                )
                .unwrap();

                assert!(db.has(Column::Identity, (&key).into()).unwrap());
                assert_eq!(db.get(Column::Identity, key).unwrap().unwrap(), value);
            }
        }

        assert_eq!(None, db.get(Column::Identity, (&[]).into()).unwrap());

        let mut iter = db.iter(Column::Identity).unwrap();

        iter.seek((&[]).into()).unwrap();

        let mut entries = iter.entries();

        for b1 in 0..10 {
            for b2 in 0..10 {
                let bytes = [b1, b2];

                let key = Slice::from(&bytes[..]);
                let value = Slice::from(&bytes[..]);

                let (k, v) = entries.next().unwrap().unwrap();

                assert_eq!(k, key);
                assert_eq!(v, value);
            }
        }
    }
}
