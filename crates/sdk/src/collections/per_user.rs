//! `PerUser<T>`: one value per account at `(name, "u", account)` (v1's `UserStorage`).

use core::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::app::StateField;
use crate::commons::collection_key;
use crate::encoding::key::KeyPart;
use crate::encoding::value;
use crate::env::{self, Caller};
use crate::error::{AppError, StorageError};
use crate::store::{DefaultStore, Store};

/// A value per account. Writes always go to the caller's own value (`env::caller()`'s
/// account, or the account an API key acts for), so no method can name anyone else's; any
/// account's value can be read with [`PerUser::get_for`]. Callers that are not an account
/// (system, public, another app) get `LoginRequired`.
pub struct PerUser<T, S: Store = DefaultStore> {
    name: String,
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

impl<T: Serialize + DeserializeOwned, S: Store + Default> PerUser<T, S> {
    /// A per-account store on the default store.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_store(name, S::default())
    }
}

impl<T: Serialize + DeserializeOwned, S: Store> PerUser<T, S> {
    /// A per-account store on a given store.
    pub fn with_store(name: impl Into<String>, store: S) -> Self {
        Self {
            name: name.into(),
            store,
            _type: PhantomData,
        }
    }

    fn storage_key(&self, account: &str) -> Result<Vec<u8>, StorageError> {
        collection_key(&self.name, "u", &[KeyPart::Str(account.into())])
    }

    /// `account`'s value.
    pub fn get_for(&self, account: &str) -> Result<Option<T>, StorageError> {
        self.store
            .get(&self.storage_key(account)?)
            .map(|bytes| value::from_slice(&bytes))
            .transpose()
    }

    /// The caller's own value.
    pub fn get(&self) -> Result<Option<T>, AppError> {
        Ok(self.get_for(&caller_account()?)?)
    }

    /// Set the caller's own value; returns the caller's account.
    pub fn set(&mut self, v: &T) -> Result<String, AppError> {
        let account = caller_account()?;
        let key = self.storage_key(&account)?;
        self.store.put(&key, &value::to_vec(v)?);
        Ok(account)
    }

    /// Remove the caller's own value, returning it.
    pub fn remove(&mut self) -> Result<Option<T>, AppError> {
        let account = caller_account()?;
        let old = self.get_for(&account)?;
        if old.is_some() {
            let key = self.storage_key(&account)?;
            self.store.delete(&key);
        }
        Ok(old)
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
