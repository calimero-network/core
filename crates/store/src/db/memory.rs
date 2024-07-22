use std::borrow::Borrow;
use std::collections::{btree_map, BTreeMap};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::config::StoreConfig;
use crate::db::{Column, Database};
use crate::iter::{DBIter, Iter};
use crate::slice::Slice;
use crate::tx::{Operation, Transaction};

pub trait RefBy<'a> {
    type Key;
    type Value;

    fn key_from_slice(key: Slice<'a>) -> Self::Key;
    fn value_from_slice(value: Slice<'a>) -> Self::Value;

    fn key_to_slice(key: &Self::Key) -> Slice;
    fn value_to_slice(key: &Self::Value) -> Slice;
}

pub struct Ref<'a> {
    _marker: std::marker::PhantomData<&'a ()>,
}

pub enum Owned {}

impl<'a> RefBy<'a> for Ref<'a> {
    type Key = Slice<'a>;
    type Value = Slice<'a>;

    fn key_from_slice(key: Slice<'a>) -> Self::Key {
        key
    }

    fn value_from_slice(value: Slice<'a>) -> Self::Value {
        value
    }

    fn key_to_slice(key: &Self::Key) -> Slice {
        key.into()
    }

    fn value_to_slice(value: &Self::Value) -> Slice {
        value.into()
    }
}

impl<'a> RefBy<'a> for Owned {
    type Key = Slice<'static>;
    type Value = Slice<'static>;

    fn key_from_slice(key: Slice<'a>) -> Self::Key {
        key.into_boxed().into()
    }

    fn value_from_slice(value: Slice<'a>) -> Self::Value {
        value.into_boxed().into()
    }

    fn key_to_slice(key: &Self::Key) -> Slice {
        key.into()
    }

    fn value_to_slice(value: &Self::Value) -> Slice {
        value.into()
    }
}

struct DBArena<V> {
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

impl<K: Clone, V> Clone for InMemoryDBInner<K, V> {
    fn clone(&self) -> Self {
        Self {
            arena: self.arena.clone(),
            links: self.links.clone(),
        }
    }
}

impl<K: Ord + Borrow<[u8]>, V> InMemoryDBInner<K, V> {
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
}

pub struct PinnedValue<'this, 'a, T: RefBy<'a>> {
    _ref: Arc<T::Value>,
    data: Slice<'this>,
}

impl<'this, 'a, T: RefBy<'a>> PinnedValue<'this, 'a, T> {
    fn new(_ref: Arc<T::Value>) -> Self {
        let data = T::value_to_slice(&_ref);

        // safety: data lives as long as _ref
        let data = unsafe { std::mem::transmute(data) };

        Self { _ref, data }
    }
}

impl<'a, T: RefBy<'a>> AsRef<[u8]> for PinnedValue<'_, 'a, T> {
    fn as_ref(&self) -> &[u8] {
        self.data.as_ref()
    }
}

pub struct InMemoryDB<'a, T: RefBy<'a>> {
    inner: Arc<RwLock<InMemoryDBInner<T::Key, T::Value>>>,
    _marker: std::marker::PhantomData<T>,
}

unsafe impl<'a, T: RefBy<'a>> Sync for InMemoryDB<'a, T> {}
unsafe impl<'a, T: RefBy<'a>> Send for InMemoryDB<'a, T> {}

impl<'a> InMemoryDB<'a, Owned> {
    pub fn referenced() -> InMemoryDB<'a, Ref<'a>> {
        InMemoryDB {
            inner: Arc::new(RwLock::new(InMemoryDBInner {
                arena: DBArena::default(),
                links: BTreeMap::new(),
            })),
            _marker: std::marker::PhantomData,
        }
    }

    pub fn owned() -> InMemoryDB<'a, Owned> {
        InMemoryDB {
            inner: Arc::new(RwLock::new(InMemoryDBInner {
                arena: DBArena::default(),
                links: BTreeMap::new(),
            })),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<'a, T: RefBy<'a>> InMemoryDB<'a, T> {
    fn db(&self) -> eyre::Result<RwLockReadGuard<InMemoryDBInner<T::Key, T::Value>>> {
        self.inner
            .read()
            .map_err(|_| eyre::eyre!("failed to acquire read lock on db"))
    }

    fn db_mut(&self) -> eyre::Result<RwLockWriteGuard<InMemoryDBInner<T::Key, T::Value>>> {
        self.inner
            .write()
            .map_err(|_| eyre::eyre!("failed to acquire write lock on db"))
    }
}

impl<'a, T: RefBy<'a>> Database<'a> for InMemoryDB<'a, T>
where
    T::Key: Ord + Clone + Borrow<[u8]> + Borrow<Slice<'a>> + 'a,
{
    fn open(_config: &StoreConfig) -> eyre::Result<Self> {
        Ok(Self {
            inner: Arc::new(RwLock::new(InMemoryDBInner {
                arena: DBArena::default(),
                links: BTreeMap::new(),
            })),
            _marker: std::marker::PhantomData,
        })
    }

    fn has(&self, col: Column, key: Slice) -> eyre::Result<bool> {
        self.get(col, key).map(|v| v.is_some())
    }

    fn get(&self, col: Column, key: Slice) -> eyre::Result<Option<Slice>> {
        let db = self.db()?;

        let Some(value) = db.get(col, &key)? else {
            return Ok(None);
        };

        Ok(Some(Slice::from_owned(PinnedValue::<T>::new(value))))
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

    fn iter(&self, col: Column, key: Slice<'a>) -> eyre::Result<Iter> {
        let db = self.db()?;

        Ok(Iter::new(InMemoryIter {
            arena: db.arena.clone(),
            column: db.links.get(&col).cloned(),
            state: State::Seek { key: Some(key) },
            _marker: std::marker::PhantomData::<T>,
        }))
    }

    fn apply(&self, tx: &Transaction<'a>) -> eyre::Result<()> {
        let mut db = self.db_mut()?;

        for (entry, op) in tx.iter() {
            // todo! move to Inner
            match op {
                Operation::Put { value } => {
                    db.insert(
                        entry.column(),
                        T::key_from_slice(entry.key().into()),
                        T::value_from_slice(value.clone()),
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

pub struct InMemoryIter<'a, 'b, T: RefBy<'a>> {
    arena: DBArena<T::Value>,
    column: Option<BTreeMap<T::Key, Arc<thunderdome::Index>>>,
    state: State<'a, 'b, T::Key, T::Value>,
    _marker: std::marker::PhantomData<T>,
}

enum State<'a, 'b, K, V> {
    Seek {
        key: Option<Slice<'b>>,
    },
    Iter {
        range: btree_map::Range<'a, K, Arc<thunderdome::Index>>,
        value: Option<Arc<V>>,
    },
}

impl<'a, 'b, T: RefBy<'a>> Drop for InMemoryIter<'a, 'b, T> {
    fn drop(&mut self) {
        let Some(column) = self.column.as_ref() else {
            return;
        };

        for idx in column.values() {
            if Arc::strong_count(idx) == 1 {
                if let Ok(mut value) = self.arena.write() {
                    value.remove(**idx);
                }
            }
        }
    }
}

impl<'a, 'b, T: RefBy<'a, Key: Ord>> DBIter for InMemoryIter<'a, 'b, T>
where
    T::Key: Ord + Borrow<Slice<'b>>,
{
    fn next(&mut self) -> eyre::Result<Option<Slice>> {
        loop {
            match &mut self.state {
                State::Seek { key } => {
                    let Some(column) = self.column.as_ref() else {
                        return Ok(None);
                    };

                    let key = key.take().expect("inconsistent state");

                    let range = column.range(key..);

                    // safety: range lives as long as self
                    let range = unsafe { std::mem::transmute(range) };

                    self.state = State::Iter { range, value: None };
                }
                State::Iter { range, value } => {
                    let Some((key, idx)) = range.next() else {
                        return Ok(None);
                    };

                    let arena = self.arena.read()?;

                    let Some(value_) = arena.get(**idx) else {
                        panic!("inconsistent state, index points to non-existent value");
                    };

                    *value = Some(value_.clone());

                    return Ok(Some(T::key_to_slice(&key)));
                }
            }
        }
    }

    fn read(&self) -> Option<Slice> {
        match &self.state {
            State::Iter { value, .. } => value.as_ref().map(|v| T::value_to_slice(v)),
            _ => None,
        }
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

                // db.put(Column::Identity, (&key).into(), (&value).into())
                //     .unwrap();
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

        db.get(Column::Identity, (&[]).into()).unwrap();
    }

    #[test]
    fn test_ref_memory() {
        let db = InMemoryDB::referenced();

        for b1 in 0..10 {
            for b2 in 0..10 {
                let bytes = [b1, b2];

                let key = Slice::from(&bytes[..]);
                let value = Slice::from(&bytes[..]);

                // db.put(Column::Identity, (&key).into(), (&value).into())
                //     .unwrap();
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

        db.get(Column::Identity, (&[]).into()).unwrap();
    }
}
