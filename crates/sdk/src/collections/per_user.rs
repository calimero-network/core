//! `PerUser<T>`: one value per account at `(name, "u", account)` (v1's `UserStorage`).

use core::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::app::StateField;
use crate::commons::{collection_key, iter_entries, read_page, KeyPath};
use crate::encoding::key::KeyPart;
use crate::encoding::value;
use crate::env::{self, Caller};
use crate::error::{AppError, StorageError};
use crate::store::{DefaultStore, Store};
use crate::table::{FindOpts, Iter, Page};

use super::slots::{decode_account, UserValue};

/// A value per account. Writes always go to the caller's own value (`env::caller()`'s
/// account, or the account an API key acts for), so no method can name anyone else's; any
/// account's value can be read with [`PerUser::get_for`]. Callers that are not an account
/// (system, public, another app) get `LoginRequired`.
pub struct PerUser<T, S: Store = DefaultStore> {
    name: KeyPath,
    store: S,
    _type: PhantomData<fn() -> T>,
}

impl<T, S: Store> StateField for PerUser<T, S> {
    const KIND: &'static str = "perUser";
}

/// The account the caller acts for.
fn caller_account() -> Result<String, AppError> {
    match env::caller() {
        Caller::Account(account) | Caller::ApiKey { account, .. } => Ok(account),
        Caller::Public | Caller::System | Caller::App(_) => Err(AppError::login_required()),
    }
}

impl<T, S: Store + Default> PerUser<T, S> {
    /// A per-account store on the default store.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<T, S: Store> PerUser<T, S> {
    /// A per-account store on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: KeyPath::root(name),
            store,
            _type: PhantomData,
        }
    }

    fn storage_key(&self, account: &str) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "u", &[KeyPart::Str(account.into())])
    }
}

impl<T: UserValue<S>, S: Store> PerUser<T, S> {
    /// `account`'s value (`Option<T>`), or for collections `account`'s collection (a handle
    /// that reads nothing until used). Reading anyone's is allowed; writing goes through
    /// [`PerUser::get`], the caller's own.
    pub fn get_for(&self, account: &str) -> Result<T::Get, StorageError> {
        T::get(&self.store, &self.name, account)
    }

    /// Whether `account` has a value, or a collection with anything in it.
    pub fn contains_for(&self, account: &str) -> Result<bool, StorageError> {
        T::contains(&self.store, &self.name, account)
    }

    /// The caller's own value, or the caller's own collection to read and write.
    pub fn get(&self) -> Result<T::Get, AppError> {
        Ok(T::get(&self.store, &self.name, &caller_account()?)?)
    }

    /// Remove the caller's own value and return it; for collections, remove the caller's
    /// collection whole and answer how many stored entries it had.
    pub fn remove(&mut self) -> Result<T::Removed, AppError> {
        let account = caller_account()?;
        Ok(T::remove(&mut self.store, &self.name, &account)?)
    }

    /// Every account's value (or collection), in account order, through one cursor; for
    /// collections it skips over each account's rows, two host calls per account.
    pub fn iter(&self) -> Iter<'_, (String, T::Item)> {
        T::entries(&self.store, &self.name)
    }
}

impl<T: Serialize + DeserializeOwned, S: Store> PerUser<T, S> {
    /// Every account's value as `(account, value)`, in account order (last first with
    /// `reverse`), one page of at most 1,000; `from` and `to` take accounts as
    /// `KeyPart::Str`.
    pub fn page(&self, opts: FindOpts) -> Result<Page<(String, T)>, StorageError> {
        let base = collection_key(&self.name, "u", &[])?;
        let (entries, next) = read_page(&self.store, &base, &opts)?;
        let items = entries
            .into_iter()
            .map(|e| decode_account(&base, e))
            .collect::<Result<_, StorageError>>()?;
        Ok(Page { items, next })
    }

    /// [`PerUser::page`] over every page from `opts` on.
    pub fn iter_with(&self, opts: FindOpts) -> Iter<'_, (String, T)> {
        match collection_key(&self.name, "u", &[]) {
            Ok(base) => {
                let under = base.clone();
                iter_entries(&self.store, &base, &opts, move |e| {
                    decode_account(&under, e)
                })
            }
            Err(e) => Iter::failed(e),
        }
    }

    /// Set the caller's own value; returns the caller's account.
    pub fn set(&mut self, v: &T) -> Result<String, AppError> {
        let account = caller_account()?;
        let key = self.storage_key(&account)?;
        self.store.put(&key, &value::to_vec(v)?);
        Ok(account)
    }
}

impl<T, S: Store> crate::collections::Nestable<S> for PerUser<T, S> {
    fn at(path: KeyPath, store: S) -> Self {
        Self {
            name: path,
            store,
            _type: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::mock;
    use crate::store::MemStore;

    fn as_(caller: Caller) {
        mock::with_mut(|s| s.caller = caller);
    }

    #[test]
    fn each_account_writes_its_own_value() {
        mock::reset();
        let mut p: PerUser<String, MemStore> = PerUser::with_store("p", MemStore::new());
        as_(Caller::Account("alice".into()));
        assert_eq!(p.set(&"a".into()).unwrap(), "alice");
        as_(Caller::ApiKey {
            key_id: "k".into(),
            account: "bob".into(),
        });
        assert_eq!(p.set(&"b".into()).unwrap(), "bob");
        assert_eq!(p.get().unwrap().as_deref(), Some("b"));
        assert_eq!(p.get_for("alice").unwrap().as_deref(), Some("a"));
        assert_eq!(p.remove().unwrap().as_deref(), Some("b"));
        assert_eq!(p.remove().unwrap(), None);
        assert_eq!(p.get_for("bob").unwrap(), None);
        as_(Caller::Account("alice".into()));
        assert_eq!(p.get().unwrap().as_deref(), Some("a"));
        mock::reset();
    }

    #[test]
    fn non_accounts_are_refused() {
        mock::reset();
        let mut p: PerUser<String, MemStore> = PerUser::with_store("p", MemStore::new());
        for caller in [Caller::System, Caller::Public, Caller::App("x".into())] {
            as_(caller);
            assert_eq!(p.set(&"v".into()).unwrap_err(), AppError::login_required());
            assert_eq!(p.get().unwrap_err(), AppError::login_required());
        }
        mock::reset();
    }
}
