use std::str::FromStr;

use calimero_primitives::alias::Alias;
use calimero_primitives::hash::Hash;
use calimero_store::key::{Alias as AliasKey, Aliasable, StoreScopeCompat};
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
        T: Aliasable<Scope: StoreScopeCompat> + Into<Hash>,
    {
        let mut handle = self.datastore.handle();

        let alias_key =
            AliasKey::new(scope, alias).ok_or_eyre("alias requires scope to be present")?;

        if handle.has(&alias_key)? {
            bail!("alias already exists");
        }

        handle.put(&alias_key, &value.into())?;

        Ok(())
    }

    pub fn delete_alias<T>(&self, alias: Alias<T>, scope: Option<T::Scope>) -> eyre::Result<()>
    where
        T: Aliasable<Scope: StoreScopeCompat>,
    {
        let mut handle = self.datastore.handle();

        let alias_key =
            AliasKey::new(scope, alias).ok_or_eyre("alias requires scope to be present")?;

        handle.delete(&alias_key)?;

        Ok(())
    }

    pub fn lookup_alias<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
    ) -> eyre::Result<Option<T>>
    where
        T: Aliasable<Scope: StoreScopeCompat> + From<Hash>,
    {
        let handle = self.datastore.handle();

        let alias_key =
            AliasKey::new(scope, alias).ok_or_eyre("alias requires scope to be present")?;

        let Some(value) = handle.get(&alias_key)? else {
            return Ok(None);
        };

        Ok(Some(value.into()))
    }

    pub fn resolve_alias<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
    ) -> eyre::Result<Option<T>>
    where
        T: Aliasable<Scope: StoreScopeCompat> + From<Hash> + FromStr<Err: Into<eyre::Report>>,
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
        T: Aliasable + From<Hash>,
        T::Scope: Copy + PartialEq + StoreScopeCompat,
    {
        let handle = self.datastore.handle();

        let mut iter = handle.iter::<AliasKey>()?;

        let first = scope
            .map(|scope| {
                iter.seek(AliasKey::new_unchecked::<T>(Some(scope), [0; 50]))
                    .transpose()
                    .map(|k| (k, iter.read()))
            })
            .flatten();

        let mut aliases = vec![];

        for (k, v) in first.into_iter().chain(iter.entries()) {
            let (k, v) = (k?, v?);

            if let Some(expected_scope) = &scope {
                let Some(found_scope) = k.scope::<T>() else {
                    bail!("scope mismatch: {:?}", k);
                };

                if found_scope != *expected_scope {
                    break;
                }
            }

            let Some(alias) = k.alias() else {
                continue;
            };

            aliases.push((alias, v.into(), k.scope::<T>()));
        }

        Ok(aliases)
    }
}
