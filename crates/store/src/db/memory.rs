#![allow(
    clippy::allow_attributes,
    reason = "Needed for lints that don't follow expect"
)]

#[cfg(test)]
#[path = "../tests/db/memory.rs"]
mod tests;

use core::borrow::Borrow;
use core::fmt::Debug;
use core::mem::transmute;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use eyre::{eyre, Result as EyreResult};
use raw::{CastsTo, InMemoryDBImpl, InMemoryDBInner, InMemoryIterInner};

use crate::config::StoreConfig;
use crate::db::{Column, Database};
use crate::iter::{DBIter, Iter};
use crate::slice::Slice;
use crate::tx::{Operation, Transaction};

mod raw;

#[derive(Debug)]
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

#[derive(Debug)]
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

#[derive(Debug)]
// TODO: Remove this lint exception once the is multi-thread-capable.
#[allow(clippy::non_send_fields_in_send_ty, reason = "TODO: This is temporary")]
pub struct InMemoryDB<T> {
    inner: T,
}

// todo! vvvvv remove this once miraclx/slice/multi-thread-capable is merged in
unsafe impl<T: Debug> Sync for InMemoryDB<T> {}
unsafe impl<T: Debug> Send for InMemoryDB<T> {}
// todo! ^^^^^ remove this once miraclx/slice/multi-thread-capable is merged in

impl InMemoryDB<()> {
    #[must_use]
    pub fn referenced<'a>() -> InMemoryDB<Ref<'a>> {
        InMemoryDB {
            inner: Ref {
                inner: Arc::default(),
            },
        }
    }

    #[must_use]
    pub fn owned() -> InMemoryDB<Owned> {
        InMemoryDB {
            inner: Owned {
                inner: Arc::default(),
            },
        }
    }
}

impl<'a, T: InMemoryDBImpl<'a> + Debug> InMemoryDB<T> {
    fn db(&self) -> EyreResult<RwLockReadGuard<'_, InMemoryDBInner<T::Key, T::Value>>> {
        self.inner
            .db()
            .read()
            .map_err(|_| eyre!("failed to acquire read lock on db"))
    }

    fn db_mut(&self) -> EyreResult<RwLockWriteGuard<'_, InMemoryDBInner<T::Key, T::Value>>> {
        self.inner
            .db()
            .write()
            .map_err(|_| eyre!("failed to acquire write lock on db"))
    }
}

struct ArcSlice<'this> {
    inner: Arc<Slice<'this>>,
}

impl ArcSlice<'_> {
    fn new<'a, T: CastsTo<Slice<'a>>>(value: Arc<T>) -> Self {
        Self {
            // safety: T: CastsTo<Slice>
            inner: unsafe { transmute::<Arc<T>, Arc<Slice<'_>>>(value) },
        }
    }
}

impl AsRef<[u8]> for ArcSlice<'_> {
    fn as_ref(&self) -> &[u8] {
        &self.inner
    }
}

impl<'a, T: InMemoryDBImpl<'a> + Debug + 'static> Database<'a> for InMemoryDB<T>
where
    T::Key: Ord + Clone + Borrow<[u8]>,
{
    fn open(_config: &StoreConfig) -> EyreResult<Self> {
        todo!("phase this out, please. it's not even worth writing an accomodation for")
    }

    fn has(&self, col: Column, key: Slice<'_>) -> EyreResult<bool> {
        self.get(col, key).map(|v| v.is_some())
    }

    fn get(&self, col: Column, key: Slice<'_>) -> EyreResult<Option<Slice<'_>>> {
        let Some(value) = self.db()?.get(col, &key)? else {
            return Ok(None);
        };

        Ok(Some(Slice::from_owned(ArcSlice::new(value))))
    }

    fn put(&self, col: Column, key: Slice<'a>, value: Slice<'a>) -> EyreResult<()> {
        self.db_mut()?
            .insert(col, T::key_from_slice(key), T::value_from_slice(value))?;

        Ok(())
    }

    fn delete(&self, col: Column, key: Slice<'_>) -> EyreResult<()> {
        self.db_mut()?.remove(col, &key)?;

        Ok(())
    }

    fn iter(&self, col: Column) -> EyreResult<Iter<'_>> {
        let db = self.db()?;

        Ok(Iter::new(InMemoryDBIter::new(db.iter(col))))
    }

    fn apply(&self, tx: &Transaction<'_>) -> EyreResult<()> {
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
                    db.remove(entry.column(), entry.key())?;
                }
            }
        }
        drop(db);

        Ok(())
    }
}

struct InMemoryDBIter<'this> {
    inner: InMemoryIterInner<'this, Slice<'this>, Slice<'this>>,
}

impl InMemoryDBIter<'_> {
    fn new<'a, K, V>(inner: InMemoryIterInner<'a, K, V>) -> Self
    where
        K: Ord + CastsTo<Slice<'a>>,
        V: CastsTo<Slice<'a>>,
    {
        InMemoryDBIter {
            // safety: {K, V}: CastsTo<Slice>
            inner: unsafe {
                transmute::<InMemoryIterInner<'_, K, V>, InMemoryIterInner<'_, Slice<'_>, Slice<'_>>>(
                    inner,
                )
            },
        }
    }
}

impl DBIter for InMemoryDBIter<'_> {
    fn seek(&mut self, key: Slice<'_>) -> EyreResult<Option<Slice<'_>>> {
        let Some(key) = self.inner.seek(&key)? else {
            return Ok(None);
        };

        Ok(Some(key.into()))
    }

    fn next(&mut self) -> EyreResult<Option<Slice<'_>>> {
        let Some(key) = self.inner.next()? else {
            return Ok(None);
        };

        Ok(Some(key.into()))
    }

    fn read(&self) -> EyreResult<Slice<'_>> {
        self.inner.read().map(Into::into)
    }
}
