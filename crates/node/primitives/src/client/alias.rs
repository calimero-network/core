use std::str::FromStr;

use calimero_primitives::alias::Alias;
use calimero_store::key::{self as key, Aliasable, StoreScopeCompat};
use eyre::{bail, OptionExt};

use super::NodeClient;

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

        if handle.has(&key)? {
            bail!("alias already exists");
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
    ) -> eyre::Result<Vec<(Alias<T>, T, Option<T::Scope>)>>
    where
        T: Aliasable + From<[u8; 32]>,
        T::Scope: Copy + PartialEq + StoreScopeCompat,
    {
        let handle = self.datastore.handle();

        let mut iter = handle.iter::<key::Alias>()?;

        let first = scope
            .map(|scope| {
                iter.seek(key::Alias::new_unchecked::<T>(Some(scope), [0; 50]))
                    .transpose()
                    .map(|k| (k, iter.read()))
            })
            .flatten();

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
