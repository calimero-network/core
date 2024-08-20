use std::borrow::Borrow;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::config::StoreConfig;
use crate::db::{Column, Database};
use crate::iter::{DBIter, Iter};
use crate::slice::Slice;
use crate::tx::{Operation, Transaction};

mod raw;

use raw::{CastsTo, InMemoryDBImpl, InMemoryDBInner, InMemoryIterInner};

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
                inner: Default::default(),
            },
        }
    }

    pub fn owned() -> InMemoryDB<Owned> {
        InMemoryDB {
            inner: Owned {
                inner: Default::default(),
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
            // safety: T: CastsTo<Slice>
            inner: unsafe { std::mem::transmute(value) },
        }
    }
}

impl<'this> AsRef<[u8]> for ArcSlice<'this> {
    fn as_ref(&self) -> &[u8] {
        &self.inner
    }
}

impl<'a, T: InMemoryDBImpl<'a> + 'static> Database<'a> for InMemoryDB<T>
where
    T::Key: Ord + Clone + Borrow<[u8]>,
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

struct InMemoryDBIter<'this> {
    inner: InMemoryIterInner<'this, Slice<'this>, Slice<'this>>,
}

impl<'this> InMemoryDBIter<'this> {
    fn new<'a, K, V>(inner: InMemoryIterInner<'a, K, V>) -> Self
    where
        K: Ord + CastsTo<Slice<'a>>,
        V: CastsTo<Slice<'a>>,
    {
        InMemoryDBIter {
            // safety: {K, V}: CastsTo<Slice>
            inner: unsafe { std::mem::transmute(inner) },
        }
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

        let mut key = Some(iter.seek((&[]).into()).unwrap().unwrap().into_boxed());
        let mut value = Some(iter.read().unwrap().clone().into_boxed());

        let mut entries = iter.entries();

        for b1 in 0..10 {
            for b2 in 0..10 {
                let (k, v) = entries
                    .next()
                    .map(|(k, v)| eyre::Ok((k?, v?)))
                    .transpose()
                    .unwrap()
                    .map_or_else(Default::default, |(k, v)| {
                        (Some(k.into_boxed()), Some(v.into_boxed()))
                    });

                let last_key = std::mem::replace(&mut key, k).unwrap();
                let last_value = std::mem::replace(&mut value, v).unwrap();

                let bytes = [b1, b2];

                assert_eq!(bytes, &*last_key);
                assert_eq!(bytes, &*last_value);
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

        let mut key = Some(iter.seek((&[]).into()).unwrap().unwrap().into_boxed());
        let mut value = Some(iter.read().unwrap().clone().into_boxed());

        let mut entries = iter.entries();

        for b1 in 0..10 {
            for b2 in 0..10 {
                let (k, v) = entries
                    .next()
                    .map(|(k, v)| eyre::Ok((k?, v?)))
                    .transpose()
                    .unwrap()
                    .map_or_else(Default::default, |(k, v)| {
                        (Some(k.into_boxed()), Some(v.into_boxed()))
                    });

                let last_key = std::mem::replace(&mut key, k).unwrap();
                let last_value = std::mem::replace(&mut value, v).unwrap();

                let bytes = [b1, b2];

                assert_eq!(bytes, &*last_key);
                assert_eq!(bytes, &*last_value);
            }
        }
    }
}
