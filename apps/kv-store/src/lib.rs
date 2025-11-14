#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

// Include the generated ABI code
//include!(env!("GENERATED_ABI_PATH"));

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::PublicKey;
use calimero_storage::collections::Mergeable;
use calimero_storage::collections::{FrozenStorage, LwwRegister, UnorderedMap, UserStorage};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct KvStore {
    // Public items, viewable by all
    items: UnorderedMap<String, LwwRegister<String>>,

    // Simple user-owned data (e.g., a user's profile name)
    // Stores: UnorderedMap<PublicKey, LwwRegister<String>>
    user_items_simple: UserStorage<LwwRegister<String>>,

    // Nested user-owned data (e.g., a user's private key-value store)
    // Stores: UnorderedMap<PublicKey, UnorderedMap<String, LwwRegister<String>>>
    user_items_nested: UserStorage<NestedMap>,

    // Content-addressable, immutable data
    // Stores: UnorderedMap<Hash, FrozenValue<String>>
    frozen_items: FrozenStorage<String>,
}

// Define the nested map type for user_items_nested
// It must implement Mergeable, Borsh, and Default
#[derive(Debug, BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct NestedMap {
    map: UnorderedMap<String, LwwRegister<String>>,
}

impl Mergeable for NestedMap {
    fn merge(
        &mut self,
        other: &Self,
    ) -> Result<(), calimero_storage::collections::crdt_meta::MergeError> {
        self.map.merge(&other.map)
    }
}

#[app::event]
pub enum Event<'a> {
    Inserted {
        key: &'a str,
        value: &'a str,
    },
    Updated {
        key: &'a str,
        value: &'a str,
    },
    Removed {
        key: &'a str,
    },
    Cleared,
    // Events for new storage types
    UserSimpleSet {
        executor_id: PublicKey,
        value: &'a str,
    },
    UserNestedSet {
        executor_id: PublicKey,
        key: &'a str,
        value: &'a str,
    },
    FrozenAdded {
        hash: [u8; 32],
        value: &'a str,
    },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    /// Error if key was not found
    #[error("key not found: {0}")]
    NotFound(&'a str),
    /// Error for user-specific data not found
    #[error("User data not found for key: {0}")]
    UserNotFound(PublicKey),
    /// Error for frozen data not found
    #[error("Frozen data not found for hash: {0}")]
    FrozenNotFound(&'a str),
}

#[app::logic]
impl KvStore {
    #[app::init]
    pub fn init() -> KvStore {
        KvStore {
            //
            items: UnorderedMap::new(),
            user_items_simple: UserStorage::new(),
            user_items_nested: UserStorage::new(),
            frozen_items: FrozenStorage::new(),
        }
    }

    // --- Public Storage Methods ---

    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("Setting key: {:?} to value: {:?}", key, value);
        if self.items.contains(&key)? {
            //
            app::emit!(Event::Updated {
                //
                key: &key,
                value: &value,
            });
        } else {
            app::emit!(Event::Inserted {
                //
                key: &key,
                value: &value,
            });
        }

        self.items.insert(key, value.into())?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        app::log!("Getting key: {:?}", key);

        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }

    // --- User Storage (Simple) Methods ---

    /// Sets a simple string value for the *current* user.
    pub fn set_user_simple(&mut self, value: String) -> app::Result<()> {
        let executor_id = calimero_sdk::env::executor_id();
        app::log!(
            "Setting simple value for user {:?}: {:?}",
            executor_id,
            value
        );

        app::emit!(Event::UserSimpleSet {
            executor_id: executor_id.into(),
            value: &value
        });

        self.user_items_simple.insert(value.into())?;
        Ok(())
    }

    /// Gets the simple string value for the *current* user.
    pub fn get_user_simple(&self) -> app::Result<Option<String>> {
        let executor_id = calimero_sdk::env::executor_id();
        app::log!("Getting simple value for user {:?}", executor_id);

        Ok(self.user_items_simple.get()?.map(|v| v.get().clone()))
    }

    /// Gets the simple string value for a *specific* user.
    pub fn get_user_simple_for(&self, user_key: PublicKey) -> app::Result<Option<String>> {
        app::log!("Getting simple value for specific user {:?}", user_key);
        Ok(self
            .user_items_simple
            .get_for_user(&user_key)?
            .map(|v| v.get().clone()))
    }

    // --- User Storage (Nested) Methods ---

    /// Sets a key-value pair in the *current* user's nested map.
    pub fn set_user_nested(&mut self, key: String, value: String) -> app::Result<()> {
        let executor_id = calimero_sdk::env::executor_id();
        app::log!(
            "Setting nested key {:?} for user {:?}: {:?}",
            key,
            executor_id,
            value
        );

        // This is a get-modify-put operation on the user's T value
        let mut nested_map = self.user_items_nested.get()?.unwrap_or_default();
        nested_map.map.insert(key.clone(), value.clone().into())?;
        self.user_items_nested.insert(nested_map)?;

        app::emit!(Event::UserNestedSet {
            executor_id: executor_id.into(),
            key: &key,
            value: &value
        });
        Ok(())
    }

    /// Gets a value from the *current* user's nested map.
    pub fn get_user_nested(&self, key: &str) -> app::Result<Option<String>> {
        let executor_id = calimero_sdk::env::executor_id();
        app::log!("Getting nested key {:?} for user {:?}", key, executor_id);

        let nested_map = self.user_items_nested.get()?;
        match nested_map {
            Some(map) => Ok(map.map.get(key)?.map(|v| v.get().clone())),
            None => Ok(None),
        }
    }

    // --- Frozen Storage Methods ---

    /// Adds an immutable value to frozen storage.
    /// Returns the hex-encoded SHA256 hash (key) of the value.
    pub fn add_frozen(&mut self, value: String) -> app::Result<String> {
        app::log!("Adding frozen value: {:?}", value);

        let hash = self.frozen_items.insert(value.clone().into())?;

        app::emit!(Event::FrozenAdded {
            hash,
            value: &value
        });

        let hash_hex = hex::encode(hash);
        Ok(hash_hex)
    }

    /// Gets an immutable value from frozen storage by its hash.
    pub fn get_frozen(&self, hash_hex: String) -> app::Result<String> {
        app::log!("Getting frozen value for hash {:?}", hash_hex);
        let mut hash = [0u8; 32];
        hex::decode_to_slice(hash_hex, &mut hash[..])
            .map_err(|_| Error::NotFound("dehex error"))?;

        Ok(self
            .frozen_items
            .get(&hash)?
            .map(|v| v.clone())
            .ok_or_else(|| Error::FrozenNotFound("Frozen value is not found"))?)
    }

    // --- Original KvStore Methods (Public) ---

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
