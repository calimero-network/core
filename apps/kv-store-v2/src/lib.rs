//! KV Store V2 - Migration test app (CIP-0001 migration v0)
//!
//! Demonstrates state migration from KvStore (v1) to KvStoreV2 and what the
//! migration flow covers.
//!
//! ## What migration covers
//!
//! - **Unchanged methods (same name and params)**: `set`, `get`, `get_result`,
//!   `get_or_insert`, `update_if_exists`, `entries`, `len`, `remove`, `clear`,
//!   `get_unchecked`. These keep the same ABI; post-migration they operate on
//!   the migrated state (existing keys/values preserved, plus new V2 fields).
//! - **New state fields**: V2 adds `migration_version`; migration initializes it
//!   from existing V1 state.
//! - **New methods (V2-only)**: `get_migration_version`, `get_with_default`,
//!   `set_if_absent`, `schema_info`. Callable only after upgrade; they validate
//!   that the context is running V2 code and migrated state.
//! - **Changed semantics via new params (backward compatible)**: `get_with_default(key, default)`
//!   provides a variant of `get` with an extra argument; existing `get(key)` is unchanged.
//!
//! Migration does **not** change existing method signatures; it only adds state
//! and new methods so that existing callers continue to work.

#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::state::read_raw;
use calimero_storage::collections::unordered_map::Entry;
use calimero_storage::collections::{LwwRegister, UnorderedMap};
use thiserror::Error;

/// Schema version strings used by migration and `get_migration_version`.
const SCHEMA_VERSION_V1: &str = "1.0.0";
const SCHEMA_VERSION_V2: &str = "2.0.0";

/// V2 state with migration version tracking
#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct KvStoreV2 {
    items: UnorderedMap<String, LwwRegister<String>>,
    /// Tracks the schema version after migration (set by migration or init).
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

/// Schema and migration info (V2-only); returned by `schema_info()`.
#[derive(Debug, Clone, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub migration_version: String,
}

// ============================================================================
// Old State Schema (for deserialization during migration)
// ============================================================================

/// V1 state structure (must match `apps/kv-store` KvStore layout).
/// Used only for deserialization during migration; field order and types must
/// match V1 Borsh encoding.
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
/// - Reads raw root state via `read_raw()` (V1 Borsh bytes).
/// - Deserializes with V1 schema, then builds V2 state with `migration_version` set.
/// - Panics if no state (empty context) or deserialization fails; migration is
///   only valid when the context already has V1 state.
#[app::migrate]
pub fn migrate_v1_to_v2() -> KvStoreV2 {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!(
            "Migration failed: no existing state. This migration requires existing V1 state (e.g. context created with kv-store 1.0.0)."
        );
    });

    // Use `deserialize` (reader-based) instead of `try_from_slice` because the raw
    // bytes from storage may contain trailing storage-layer metadata beyond the V1
    // struct fields.  `try_from_slice` rejects any leftover bytes ("Not all bytes
    // read") while `deserialize` reads only what the struct needs.
    let old_state: KvStoreV1 =
        BorshDeserialize::deserialize(&mut &old_bytes[..]).unwrap_or_else(|e| {
            panic!(
                "Migration failed: V1 state deserialization error {:?} (raw len={}). \
                 Ensure the context was created with kv-store (v1).",
                e,
                old_bytes.len()
            );
        });

    app::emit!(Event::Migrated {
        from_version: SCHEMA_VERSION_V1,
        to_version: SCHEMA_VERSION_V2,
    });

    KvStoreV2 {
        items: old_state.items,
        migration_version: LwwRegister::new(SCHEMA_VERSION_V2.to_owned()),
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
            migration_version: LwwRegister::new(SCHEMA_VERSION_V2.to_owned()),
        }
    }

    // -------------------------------------------------------------------------
    // New V2-only methods (not present in v1; validate post-migration behavior)
    // -------------------------------------------------------------------------

    /// Returns the schema version of the state (e.g. "2.0.0" after migration).
    pub fn get_migration_version(&self) -> app::Result<String> {
        app::log!("Getting migration version");
        Ok(self.migration_version.get().clone())
    }

    /// Like `get` but with a default when the key is missing (new param: default).
    /// Tests that V2 can add methods with extra parameters without breaking callers
    /// that still use `get(key)`.
    pub fn get_with_default(&self, key: &str, default: String) -> app::Result<String> {
        app::log!("Getting key with default: {:?}", key);
        Ok(self
            .items
            .get(key)?
            .map(|v| v.get().clone())
            .unwrap_or(default))
    }

    /// Sets the value only if the key is absent; returns whether the key was absent.
    /// V2-only; complements `update_if_exists` (which only updates when key exists).
    pub fn set_if_absent(&mut self, key: String, value: String) -> app::Result<bool> {
        app::log!("Set if absent: {:?} -> {:?}", key, value);
        let entry = self.items.entry(key.clone())?;
        if let Entry::Vacant(v) = entry {
            v.insert(LwwRegister::new(value.clone()))?;
            app::emit!(Event::Inserted {
                key: &key,
                value: &value,
            });
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Returns schema and migration info (V2-only). Used to verify migration and version.
    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        app::log!("Getting schema info");
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V2.to_owned(),
            migration_version: self.migration_version.get().clone(),
        })
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
}
