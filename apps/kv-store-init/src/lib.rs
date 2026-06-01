#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
pub struct KvStoreInit {
    items: UnorderedMap<String, LwwRegister<String>>,
}

#[app::event]
pub enum Event<'a> {
    Inserted { key: &'a str, value: &'a str },
    Updated { key: &'a str, value: &'a str },
    Removed { key: &'a str },
    Cleared,
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("key not found: {0}")]
    NotFound(&'a str),
}

#[app::logic]
impl KvStoreInit {
    #[app::init]
    pub fn init() -> KvStoreInit {
        app::log!("Initializing KvStoreInit with default items");

        let mut store = KvStoreInit {
            items: UnorderedMap::new(),
        };

        // Add some initial data during initialization
        store
            .items
            .insert("init_key_1".to_owned(), "Initial value 1".to_owned().into())
            .expect("Failed to insert init_key_1");
        store
            .items
            .insert("init_key_2".to_owned(), "Initial value 2".to_owned().into())
            .expect("Failed to insert init_key_2");
        store
            .items
            .insert(
                "welcome".to_owned(),
                "Welcome to KvStoreInit!".to_owned().into(),
            )
            .expect("Failed to insert welcome");

        app::log!("KvStoreInit initialized with 3 default items");

        store
    }

    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("Setting key: {:?} to value: {:?}", key, value);

        if self.items.contains(&key)? {
            app::emit!(Event::Updated {
                key: &key,
                value: &value,
            });
        } else {
            app::emit!(Event::Inserted {
                key: &key,
                value: &value,
            });
        }

        self.items.insert(key, value.into())?;

        Ok(())
    }

    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        app::log!("Getting all entries");

        Ok(self
            .items
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    pub fn len(&self) -> app::Result<usize> {
        app::log!("Getting the number of entries");

        Ok(self.items.len()?)
    }

    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        app::log!("Getting key: {:?}", key);

        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }

    pub fn get_unchecked(&self, key: &str) -> app::Result<String> {
        app::log!("Getting key without checking: {:?}", key);

        // this panics, which we do not recommend
        Ok(self.items.get(key)?.expect("key not found").get().clone())
    }

    pub fn get_result(&self, key: &str) -> app::Result<String> {
        app::log!("Getting key, possibly failing: {:?}", key);

        let Some(value) = self.get(key)? else {
            app::bail!(Error::NotFound(key));
        };

        Ok(value)
    }

    pub fn remove(&mut self, key: &str) -> app::Result<Option<String>> {
        app::log!("Removing key: {:?}", key);

        app::emit!(Event::Removed { key });

        Ok(self.items.remove(key)?.map(|v| v.get().clone()))
    }

    pub fn clear(&mut self) -> app::Result<()> {
        app::log!("Clearing all entries");

        app::emit!(Event::Cleared);

        self.items.clear().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    /// Number of entries `#[app::init]` seeds into the store.
    const SEEDED: usize = 3;

    #[test]
    fn init_seeds_default_items() {
        let app = TestHost::new(KvStoreInit::init);

        assert_eq!(app.view(|s| s.len()).unwrap(), SEEDED);
        assert_eq!(
            app.view(|s| s.get("welcome")).unwrap(),
            Some("Welcome to KvStoreInit!".to_owned())
        );
        // The init body logs through `app::log!`, captured by the harness.
        assert!(app.logs().iter().any(|line| line.contains("Initializing")));
    }

    #[test]
    fn set_get_remove_on_top_of_seed() {
        let mut app = TestHost::new(KvStoreInit::init);

        app.call(|s| s.set("k".into(), "v".into())).unwrap();
        assert_eq!(app.view(|s| s.get("k")).unwrap(), Some("v".to_owned()));
        assert_eq!(app.view(|s| s.len()).unwrap(), SEEDED + 1);

        assert_eq!(app.call(|s| s.remove("k")).unwrap(), Some("v".to_owned()));
        assert_eq!(app.view(|s| s.get("k")).unwrap(), None);
        assert_eq!(app.view(|s| s.len()).unwrap(), SEEDED);
    }

    #[test]
    fn clear_removes_everything() {
        let mut app = TestHost::new(KvStoreInit::init);

        app.call(|s| s.clear()).unwrap();
        assert_eq!(app.view(|s| s.len()).unwrap(), 0);
    }
}
