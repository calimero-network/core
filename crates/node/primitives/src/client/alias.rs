use core::str::FromStr;

use calimero_primitives::alias::Alias;
use calimero_store::key::{self as key, Aliasable, StoreScopeCompat};
use eyre::OptionExt;

use super::NodeClient;

/// A resolved alias entry: the alias, its target value, and optional scope.
type AliasEntry<T, S> = (Alias<T>, T, Option<S>);

/// Returned by [`NodeClient::create_alias`] when the alias already exists.
///
/// A distinct error type (rather than a string `bail!`) so the caller can map
/// it to `409 Conflict` by downcasting, without a separate pre-check that would
/// race the write.
#[derive(Debug)]
pub struct AliasExists;

impl core::fmt::Display for AliasExists {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("alias already exists")
    }
}

impl core::error::Error for AliasExists {}

impl NodeClient {
    pub fn create_alias<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
        value: T,
    ) -> eyre::Result<()>
    where
        T: Aliasable<Scope: StoreScopeCompat> + AsRef<[u8; 32]>,
    {
        let mut handle = self.datastore.handle();

        let key = key::Alias::new(scope, alias).ok_or_eyre("alias requires scope to be present")?;

        // Existence check + write on the same handle (no separate caller-side
        // pre-check that would race the write). Returns a typed `AliasExists` so
        // the server can map it to 409 without inspecting the message.
        if handle.has(&key)? {
            return Err(AliasExists.into());
        }

        let value = (*value.as_ref()).into();

        handle.put(&key, &value)?;

        Ok(())
    }

    pub fn delete_alias<T>(&self, alias: Alias<T>, scope: Option<T::Scope>) -> eyre::Result<()>
    where
        T: Aliasable<Scope: StoreScopeCompat>,
    {
        let mut handle = self.datastore.handle();

        let key = key::Alias::new(scope, alias).ok_or_eyre("alias requires scope to be present")?;

        handle.delete(&key)?;

        Ok(())
    }

    pub fn lookup_alias<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
    ) -> eyre::Result<Option<T>>
    where
        T: Aliasable<Scope: StoreScopeCompat> + From<[u8; 32]>,
    {
        let handle = self.datastore.handle();

        let key = key::Alias::new(scope, alias).ok_or_eyre("alias requires scope to be present")?;

        let Some(value) = handle.get(&key)? else {
            return Ok(None);
        };

        Ok(Some((*value).into()))
    }

    pub fn resolve_alias<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
    ) -> eyre::Result<Option<T>>
    where
        T: Aliasable<Scope: StoreScopeCompat> + From<[u8; 32]> + FromStr<Err: Into<eyre::Report>>,
    {
        if let Some(value) = self.lookup_alias(alias, scope)? {
            return Ok(Some(value));
        }

        Ok(alias.as_str().parse().ok())
    }

    pub fn list_aliases<T>(
        &self,
        scope: Option<T::Scope>,
    ) -> eyre::Result<Vec<AliasEntry<T, T::Scope>>>
    where
        T: Aliasable + From<[u8; 32]>,
        T::Scope: Copy + PartialEq + StoreScopeCompat,
    {
        let handle = self.datastore.handle();

        let mut iter = handle.iter::<key::Alias>()?;

        let first = scope.and_then(|scope| {
            iter.seek(key::Alias::new_unchecked::<T>(Some(scope), [0; 50]))
                .transpose()
                .map(|k| (k, iter.read()))
        });

        let mut aliases = vec![];

        for (k, v) in first.into_iter().chain(iter.entries()) {
            let (k, v) = (k?, v?);

            if let Some(expected_scope) = &scope {
                if let Some(found_scope) = k.scope::<T>() {
                    if found_scope != *expected_scope {
                        break;
                    }
                }
            }

            let Some(alias) = k.alias() else {
                continue;
            };

            aliases.push((alias, (*v).into(), k.scope::<T>()));
        }

        Ok(aliases)
    }
}
