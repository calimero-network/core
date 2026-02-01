//! Comprehensive Sync Test Application
//!
//! This application tests ALL storage spaces and CRDT types for synchronization:
//! - Public storage (shared state)
//! - User storage (per-user state)
//! - Frozen storage (content-addressed immutable data)
//!
//! It also tests:
//! - LWW registers (last-write-wins)
//! - Counters (G-Counter/PN-Counter)
//! - UnorderedMap operations
//! - Deletions and tombstones
//! - Nested CRDT structures
//!
//! The state is designed to be DETERMINISTIC: given a sequence of operations,
//! the final state can be computed and verified.

#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_sdk::PublicKey;
use calimero_storage::collections::{
    Counter, FrozenStorage, LwwRegister, Mergeable, UnorderedMap, UserStorage,
};
use calimero_storage_macros::Mergeable;
use thiserror::Error;

// =============================================================================
// NESTED CRDT TYPES
// =============================================================================

/// Statistics with multiple counters - demonstrates nested CRDTs
#[derive(Debug, Mergeable, BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Stats {
    pub increments: Counter,
    pub decrements: Counter,
}

/// User's private key-value store
#[derive(Debug, BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct UserKvStore {
    pub data: UnorderedMap<String, LwwRegister<String>>,
}

impl Mergeable for UserKvStore {
    fn merge(
        &mut self,
        other: &Self,
    ) -> Result<(), calimero_storage::collections::crdt_meta::MergeError> {
        self.data.merge(&other.data)
    }
}

// =============================================================================
// STATE DEFINITION
// =============================================================================

#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct SyncTestApp {
    // -------------------------------------------------------------------------
    // PUBLIC STORAGE - Shared across all nodes
    // -------------------------------------------------------------------------
    /// Simple key-value pairs (LWW semantics)
    pub public_kv: UnorderedMap<String, LwwRegister<String>>,

    /// Counters for testing PN-Counter merge
    pub public_counters: UnorderedMap<String, Counter>,

    /// Stats per entity (nested CRDT)
    pub public_stats: UnorderedMap<String, Stats>,

    /// Track deleted keys (for verification)
    pub deleted_keys: UnorderedMap<String, LwwRegister<bool>>,

    // -------------------------------------------------------------------------
    // USER STORAGE - Per-user isolated state
    // -------------------------------------------------------------------------
    /// Simple per-user value
    pub user_simple: UserStorage<LwwRegister<String>>,

    /// Per-user key-value store (nested)
    pub user_kv: UserStorage<UserKvStore>,

    /// Per-user counter
    pub user_counter: UserStorage<Counter>,

    // -------------------------------------------------------------------------
    // FROZEN STORAGE - Content-addressed immutable data
    // -------------------------------------------------------------------------
    /// Immutable blobs
    pub frozen_data: FrozenStorage<String>,

    // -------------------------------------------------------------------------
    // VERIFICATION STATE
    // -------------------------------------------------------------------------
    /// Number of operations performed (for verification)
    pub operation_count: Counter,
}

// =============================================================================
// ERRORS
// =============================================================================

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum SyncTestError {
    #[error("Key not found: {0}")]
    KeyNotFound(String),

    #[error("Counter not found: {0}")]
    CounterNotFound(String),

    #[error("Frozen data not found: {0}")]
    FrozenNotFound(String),

    #[error("User data not found")]
    UserDataNotFound,

    #[error("Invalid hex: {0}")]
    InvalidHex(String),

    #[error("Verification failed: expected {expected}, got {actual}")]
    VerificationFailed { expected: String, actual: String },
}

// =============================================================================
// SNAPSHOT - For deterministic state verification
// =============================================================================

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(crate = "calimero_sdk::serde")]
pub struct StateSnapshot {
    pub public_kv_count: usize,
    pub public_kv_entries: BTreeMap<String, String>,
    pub public_counter_values: BTreeMap<String, i64>,
    pub deleted_keys_count: usize,
    pub frozen_count: usize,
    pub operation_count: u64,
}

// =============================================================================
// APPLICATION LOGIC
// =============================================================================

#[app::logic]
impl SyncTestApp {
    #[app::init]
    pub fn init() -> SyncTestApp {
        SyncTestApp {
            // Public storage
            public_kv: UnorderedMap::new_with_field_name("public_kv"),
            public_counters: UnorderedMap::new_with_field_name("public_counters"),
            public_stats: UnorderedMap::new_with_field_name("public_stats"),
            deleted_keys: UnorderedMap::new_with_field_name("deleted_keys"),

            // User storage
            user_simple: UserStorage::new(),
            user_kv: UserStorage::new(),
            user_counter: UserStorage::new(),

            // Frozen storage
            frozen_data: FrozenStorage::new(),

            // Verification
            operation_count: Counter::new(),
        }
    }

    // =========================================================================
    // PUBLIC KEY-VALUE OPERATIONS
    // =========================================================================

    /// Set a public key-value pair
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("SET public: {} = {}", key, value);
        self.public_kv.insert(key, LwwRegister::new(value))?;
        self.operation_count.increment()?;
        Ok(())
    }

    /// Get a public value
    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        app::log!("GET public: {}", key);
        Ok(self.public_kv.get(key)?.map(|v| v.get().clone()))
    }

    /// Delete a public key (creates tombstone)
    pub fn delete(&mut self, key: &str) -> app::Result<bool> {
        app::log!("DELETE public: {}", key);
        let existed = self.public_kv.remove(key)?.is_some();
        if existed {
            // Track deletion for verification
            self.deleted_keys
                .insert(key.to_string(), LwwRegister::new(true))?;
        }
        self.operation_count.increment()?;
        Ok(existed)
    }

    /// Batch set multiple key-value pairs
    pub fn batch_set(&mut self, pairs: Vec<(String, String)>) -> app::Result<usize> {
        app::log!("BATCH_SET: {} pairs", pairs.len());
        let count = pairs.len();
        for (key, value) in pairs {
            self.public_kv.insert(key, LwwRegister::new(value))?;
        }
        self.operation_count.increment()?;
        Ok(count)
    }

    /// Get all public entries
    pub fn entries(&self) -> app::Result<BTreeMap<String, String>> {
        app::log!("ENTRIES public");
        Ok(self
            .public_kv
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    /// Get count of public entries
    pub fn len(&self) -> app::Result<usize> {
        Ok(self.public_kv.len()?)
    }

    // =========================================================================
    // PUBLIC COUNTER OPERATIONS
    // =========================================================================

    /// Increment a named counter
    pub fn counter_inc(&mut self, name: String) -> app::Result<i64> {
        app::log!("COUNTER_INC: {}", name);
        let mut counter = self.public_counters.get(&name)?.unwrap_or_default();
        counter.increment()?;
        let value = counter.value()? as i64;
        self.public_counters.insert(name, counter)?;
        self.operation_count.increment()?;
        Ok(value)
    }

    /// Decrement a named counter (using two increments to simulate)
    /// Note: Counter only supports increment, so we track decrements separately
    pub fn counter_dec(&mut self, name: String) -> app::Result<i64> {
        app::log!("COUNTER_DEC: {}", name);
        // For now, just return current value - Counter is a G-Counter (grow-only)
        // Real decrement would need PN-Counter
        let counter = self.public_counters.get(&name)?.unwrap_or_default();
        let value = counter.value()? as i64;
        self.operation_count.increment()?;
        Ok(value)
    }

    /// Get counter value
    pub fn counter_get(&self, name: &str) -> app::Result<i64> {
        app::log!("COUNTER_GET: {}", name);
        Ok(self
            .public_counters
            .get(name)?
            .map(|c| c.value().unwrap_or(0) as i64)
            .unwrap_or(0))
    }

    // =========================================================================
    // PUBLIC STATS (NESTED CRDT) OPERATIONS
    // =========================================================================

    /// Record an increment for an entity's stats
    pub fn stats_inc(&mut self, entity: String) -> app::Result<u64> {
        app::log!("STATS_INC: {}", entity);
        let mut stats = self.public_stats.get(&entity)?.unwrap_or_default();
        stats.increments.increment()?;
        let value = stats.increments.value()?;
        self.public_stats.insert(entity, stats)?;
        self.operation_count.increment()?;
        Ok(value)
    }

    /// Record a decrement for an entity's stats
    pub fn stats_dec(&mut self, entity: String) -> app::Result<u64> {
        app::log!("STATS_DEC: {}", entity);
        let mut stats = self.public_stats.get(&entity)?.unwrap_or_default();
        stats.decrements.increment()?;
        let value = stats.decrements.value()?;
        self.public_stats.insert(entity, stats)?;
        self.operation_count.increment()?;
        Ok(value)
    }

    /// Get stats for an entity
    pub fn stats_get(&self, entity: &str) -> app::Result<(u64, u64)> {
        app::log!("STATS_GET: {}", entity);
        let stats = self.public_stats.get(entity)?.unwrap_or_default();
        Ok((
            stats.increments.value().unwrap_or(0),
            stats.decrements.value().unwrap_or(0),
        ))
    }

    // =========================================================================
    // USER STORAGE OPERATIONS
    // =========================================================================

    /// Set the current user's simple value
    pub fn user_set_simple(&mut self, value: String) -> app::Result<()> {
        let user = calimero_sdk::env::executor_id();
        app::log!("USER_SET_SIMPLE: {:?} = {}", user, value);
        self.user_simple.insert(LwwRegister::new(value))?;
        self.operation_count.increment()?;
        Ok(())
    }

    /// Get the current user's simple value
    pub fn user_get_simple(&self) -> app::Result<Option<String>> {
        let user = calimero_sdk::env::executor_id();
        app::log!("USER_GET_SIMPLE: {:?}", user);
        Ok(self.user_simple.get()?.map(|v| v.get().clone()))
    }

    /// Set a key-value in the current user's private store
    pub fn user_set_kv(&mut self, key: String, value: String) -> app::Result<()> {
        let user = calimero_sdk::env::executor_id();
        app::log!("USER_SET_KV: {:?} {} = {}", user, key, value);
        let mut store = self.user_kv.get()?.unwrap_or_default();
        store.data.insert(key, LwwRegister::new(value))?;
        self.user_kv.insert(store)?;
        self.operation_count.increment()?;
        Ok(())
    }

    /// Get a value from the current user's private store
    pub fn user_get_kv(&self, key: &str) -> app::Result<Option<String>> {
        let user = calimero_sdk::env::executor_id();
        app::log!("USER_GET_KV: {:?} {}", user, key);
        let store = self.user_kv.get()?;
        match store {
            Some(s) => Ok(s.data.get(key)?.map(|v| v.get().clone())),
            None => Ok(None),
        }
    }

    /// Delete from the current user's private store
    pub fn user_delete_kv(&mut self, key: &str) -> app::Result<bool> {
        let user = calimero_sdk::env::executor_id();
        app::log!("USER_DELETE_KV: {:?} {}", user, key);
        let mut store = self.user_kv.get()?.unwrap_or_default();
        let existed = store.data.remove(key)?.is_some();
        self.user_kv.insert(store)?;
        self.operation_count.increment()?;
        Ok(existed)
    }

    /// Increment the current user's counter
    pub fn user_counter_inc(&mut self) -> app::Result<u64> {
        let user = calimero_sdk::env::executor_id();
        app::log!("USER_COUNTER_INC: {:?}", user);
        let mut counter = self.user_counter.get()?.unwrap_or_default();
        counter.increment()?;
        let value = counter.value()?;
        self.user_counter.insert(counter)?;
        self.operation_count.increment()?;
        Ok(value)
    }

    /// Get the current user's counter value
    pub fn user_counter_get(&self) -> app::Result<u64> {
        let user = calimero_sdk::env::executor_id();
        app::log!("USER_COUNTER_GET: {:?}", user);
        Ok(self
            .user_counter
            .get()?
            .map(|c| c.value().unwrap_or(0))
            .unwrap_or(0))
    }

    /// Get another user's simple value (read-only cross-user access)
    pub fn user_get_simple_for(&self, user_key: PublicKey) -> app::Result<Option<String>> {
        app::log!("USER_GET_SIMPLE_FOR: {:?}", user_key);
        Ok(self
            .user_simple
            .get_for_user(&user_key)?
            .map(|v| v.get().clone()))
    }

    // =========================================================================
    // FROZEN STORAGE OPERATIONS
    // =========================================================================

    /// Add immutable data to frozen storage
    pub fn frozen_add(&mut self, data: String) -> app::Result<String> {
        app::log!("FROZEN_ADD: {} bytes", data.len());
        let hash = self.frozen_data.insert(data)?;
        self.operation_count.increment()?;
        Ok(hex::encode(hash))
    }

    /// Get immutable data from frozen storage
    pub fn frozen_get(&self, hash_hex: &str) -> app::Result<Option<String>> {
        app::log!("FROZEN_GET: {}", hash_hex);
        let hash =
            hex::decode(hash_hex).map_err(|_| SyncTestError::InvalidHex(hash_hex.to_string()))?;
        if hash.len() != 32 {
            app::bail!(SyncTestError::InvalidHex(hash_hex.to_string()));
        }
        let mut hash_arr = [0u8; 32];
        hash_arr.copy_from_slice(&hash);
        Ok(self.frozen_data.get(&hash_arr)?.map(|s| s.clone()))
    }

    // =========================================================================
    // VERIFICATION OPERATIONS
    // =========================================================================

    /// Get a snapshot of the current state for verification
    pub fn snapshot(&self) -> app::Result<StateSnapshot> {
        app::log!("SNAPSHOT");

        let public_kv_entries: BTreeMap<String, String> = self
            .public_kv
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect();

        let public_counter_values: BTreeMap<String, i64> = self
            .public_counters
            .entries()?
            .map(|(k, c)| (k, c.value().unwrap_or(0) as i64))
            .collect();

        Ok(StateSnapshot {
            public_kv_count: public_kv_entries.len(),
            public_kv_entries,
            public_counter_values,
            deleted_keys_count: self.deleted_keys.len()?,
            frozen_count: 0, // FrozenStorage doesn't expose len
            operation_count: self.operation_count.value()?,
        })
    }

    /// Verify the state matches expected values
    pub fn verify(&self, expected: StateSnapshot) -> app::Result<bool> {
        app::log!("VERIFY");
        let actual = self.snapshot()?;

        if actual.public_kv_count != expected.public_kv_count {
            app::bail!(SyncTestError::VerificationFailed {
                expected: format!("public_kv_count={}", expected.public_kv_count),
                actual: format!("public_kv_count={}", actual.public_kv_count),
            });
        }

        if actual.public_kv_entries != expected.public_kv_entries {
            app::bail!(SyncTestError::VerificationFailed {
                expected: format!("public_kv_entries={:?}", expected.public_kv_entries),
                actual: format!("public_kv_entries={:?}", actual.public_kv_entries),
            });
        }

        if actual.public_counter_values != expected.public_counter_values {
            app::bail!(SyncTestError::VerificationFailed {
                expected: format!("public_counter_values={:?}", expected.public_counter_values),
                actual: format!("public_counter_values={:?}", actual.public_counter_values),
            });
        }

        Ok(true)
    }

    /// Get the total operation count
    pub fn get_operation_count(&self) -> app::Result<u64> {
        Ok(self.operation_count.value()?)
    }

    /// Get count of deleted keys
    pub fn get_deleted_count(&self) -> app::Result<usize> {
        Ok(self.deleted_keys.len()?)
    }

    /// Check if a key was deleted
    pub fn was_deleted(&self, key: &str) -> app::Result<bool> {
        Ok(self.deleted_keys.get(key)?.is_some())
    }

    // =========================================================================
    // BULK OPERATIONS FOR BENCHMARKING
    // =========================================================================

    /// Write N keys with a prefix (for benchmarking)
    pub fn bulk_write(&mut self, prefix: String, count: u32, value_size: u32) -> app::Result<u32> {
        app::log!(
            "BULK_WRITE: prefix={}, count={}, value_size={}",
            prefix,
            count,
            value_size
        );
        let value_base: String = (0..value_size).map(|_| 'x').collect();

        for i in 0..count {
            let key = format!("{}_{}", prefix, i);
            let value = format!("{}_{}", value_base, i);
            self.public_kv.insert(key, LwwRegister::new(value))?;
        }

        self.operation_count.increment()?;
        Ok(count)
    }

    /// Delete N keys with a prefix (for benchmarking tombstones)
    pub fn bulk_delete(&mut self, prefix: String, count: u32) -> app::Result<u32> {
        app::log!("BULK_DELETE: prefix={}, count={}", prefix, count);
        let mut deleted = 0;

        for i in 0..count {
            let key = format!("{}_{}", prefix, i);
            if self.public_kv.remove(&key)?.is_some() {
                self.deleted_keys.insert(key, LwwRegister::new(true))?;
                deleted += 1;
            }
        }

        self.operation_count.increment()?;
        Ok(deleted)
    }

    /// Increment a counter N times (for CRDT merge testing)
    pub fn bulk_counter_inc(&mut self, name: String, count: u32) -> app::Result<i64> {
        app::log!("BULK_COUNTER_INC: name={}, count={}", name, count);
        let mut counter = self.public_counters.get(&name)?.unwrap_or_default();

        for _ in 0..count {
            counter.increment()?;
        }

        let value = counter.value()? as i64;
        self.public_counters.insert(name, counter)?;
        self.operation_count.increment()?;
        Ok(value)
    }
}
