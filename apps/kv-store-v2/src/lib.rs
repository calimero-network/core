//! KV Store V2 - Upgraded version with migration support
//!
//! This module demonstrates state migration from KvStore (v1) to KvStoreV2.
//! The migration adds a `migration_version` field to track schema versions.

#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::unordered_map::Entry;
use calimero_storage::collections::{LwwRegister, UnorderedMap};
use thiserror::Error;

/// V2 state with migration version tracking
#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct KvStoreV2 {
    items: UnorderedMap<String, LwwRegister<String>>,
    /// Tracks the schema version after migration
    migration_version: LwwRegister<String>,
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
    Migrated {
        from_version: &'a str,
        to_version: &'a str,
    },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("key not found: {0}")]
    NotFound(&'a str),
}

// ============================================================================
// Old State Schema (for deserialization during migration)
// ============================================================================

/// V1 state structure (used only for deserialization during migration)
#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct KvStoreV1 {
    items: UnorderedMap<String, LwwRegister<String>>,
}

// ============================================================================
// Migration Function
// ============================================================================

/// Migrates state from V1 to V2 schema.
///
/// This function:
/// 1. Reads the raw V1 state bytes from storage
/// 2. Deserializes using the old schema
/// 3. Creates a new V2 state with the additional `migration_version` field
/// 4. Returns the new state to be written by the runtime
#[app::migrate]
pub fn migrate_v1_to_v2() -> KvStoreV2 {
    // Read raw bytes from storage (old V1 format)
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: No existing state found. Cannot migrate empty state - this migration requires existing V1 state.");
    });

    // Deserialize using old schema
    let old_state: KvStoreV1 = BorshDeserialize::try_from_slice(&old_bytes).unwrap_or_else(|e| {
        panic!(
            "Migration failed: Failed to deserialize old state - schema mismatch. \
                 Error: {:?}. \
                 Ensure the existing state matches the V1 schema expected by this migration.",
            e
        );
    });

    app::emit!(Event::Migrated {
        from_version: "1.0.0",
        to_version: "2.0.0",
    });

    // Transform to new schema with additional field
    KvStoreV2 {
        items: old_state.items,
        migration_version: LwwRegister::new("2.0.0".to_owned()),
    }
}

// ============================================================================
// Application Logic
// ============================================================================

#[app::logic]
impl KvStoreV2 {
    #[app::init]
    pub fn init() -> KvStoreV2 {
        KvStoreV2 {
            items: UnorderedMap::new(),
            migration_version: LwwRegister::new("2.0.0".to_owned()),
        }
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

    /// Updates a value only if the key already exists, using in-place mutation.
    pub fn update_if_exists(&mut self, key: String, value: String) -> app::Result<bool> {
        app::log!("Updating if exists: {:?} -> {:?}", key, value);

        if let Some(mut v) = self.items.get_mut(&key)? {
            v.set(value.clone());

            app::emit!(Event::Updated {
                key: &key,
                value: &value,
            });
            return Ok(true);
        }

        Ok(false)
    }

    /// Gets a value, inserting it if it doesn't exist.
    pub fn get_or_insert(&mut self, key: String, value: String) -> app::Result<String> {
        app::log!("Get or insert: {:?} -> {:?}", key, value);

        let entry = self.items.entry(key.clone())?;

        if let Entry::Vacant(_) = &entry {
            app::emit!(Event::Inserted {
                key: &key,
                value: &value,
            });
        }

        let val = entry.or_insert(LwwRegister::new(value))?;

        Ok(val.get().clone())
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

    /// Returns the migration version of the state schema.
    /// After migration from V1, this returns "2.0.0".
    pub fn get_migration_version(&self) -> app::Result<String> {
        app::log!("Getting migration version");

        Ok(self.migration_version.get().clone())
    }
}
